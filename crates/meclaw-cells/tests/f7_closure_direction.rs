//! 0.3.x follow-up F7 -- a replacement points forwards in time, never backwards
//! (GitHub #71).
//!
//! The wave-final measurement found the regression this file closes. The
//! extractor's `replaces` signal (W4) closes a sibling statement it provably saw
//! in its window, and nothing checked WHICH of the two statements is newer:
//!
//! ```text
//! rachel | lives_in | Chicago      | 2023-05-24 | OPEN
//! rachel | lives_in | the suburbs  | 2023-05-27 | CLOSED by extract:403a7dc8
//!                                                superseded_by = the Chicago row
//! ```
//!
//! The gold answer was written into the store correctly and then ended by a fact
//! three days its senior. The read path then does exactly what it should with an
//! inverted chain (the older value ranks first and carries the newer one as its
//! history), so the answer came back wrong -- the only wrong answer of the run.
//! Corpus-wide: four of four judge closures correctly directed, one of three
//! extractor closures inverted. The nightly review answered `retract: 0`, because
//! it asks whether a closure contradicts later evidence and never whether it was
//! directed correctly at all.
//!
//! Two guards, one on each producer:
//!
//! 1. the extraction lane applies a `replaces` only when the statement being
//!    closed is older than the fact replacing it. A refusal is not an error: the
//!    new fact is minted exactly as it always was, only the closure is dropped,
//!    and the refused pair is receipted under the batch key so the night can
//!    still judge it.
//! 2. the nightly review checks the same direction on the extractor closures it
//!    audits, and takes an inverted one back with the revert W4 already owns.
//!
//! Everything here runs the REAL `params.script_inline` of `extract-glue` and
//! `dream-glue` against injected store replies, so no model is called and nothing
//! costs anything. The free scenario C14 proves the extraction half end to end on
//! a real colony.

use std::io::Write;
use std::process::{Command, Stdio};

const EXTRACT_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";
const DREAM_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn script_of(path: &str) -> String {
    let raw = std::fs::read_to_string(path).expect("template config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `python3 -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `python3 -c` ran it: same `__main__` globals, same
/// stdout, same exit status.
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run a real script with a real stdin document; returns the emitted messages
/// and whatever the script had to say on stderr.
fn run(script: &str, doc: serde_json::Value) -> (Vec<serde_json::Value>, String) {
    let out = run_script_on_stdin(script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        serde_json::from_slice(&out.stdout).expect("message array"),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// The payload of the scratch row of one kind, if the phase parked one.
fn parked(msgs: &[serde_json::Value], kind: &str) -> Option<serde_json::Value> {
    msgs.iter().map(args_of).find_map(|a| {
        if a["table"] == "scratch" && a["row"]["kind"] == kind {
            Some(
                serde_json::from_str(a["row"]["payload"].as_str().expect("payload")).expect("json"),
            )
        } else {
            None
        }
    })
}

// ================================================================ extract-glue

const BATCH: &str = "b1";

fn x_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": phase, "batch_id": BATCH},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

/// One open fact row in the shape the window page projects it.
fn fact_row(id: &str, claim: &str, from: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "subject": "rachel", "canonical_subject": "rachel",
        "predicate": "lives_in", "canonical_predicate": "lives_in",
        "claim": claim, "canonical_claim": claim,
        "valid_from": from, "recorded_at": from, "expired_at": null
    })
}

/// Every `facts` op of one operation the apply phase emitted, in order.
fn fact_ops(msgs: &[serde_json::Value], operation: &str) -> Vec<serde_json::Value> {
    msgs.iter()
        .map(args_of)
        .filter(|a| a["table"] == "facts" && a["operation"] == operation)
        .collect()
}

/// One window statement as the vocabulary phase parks it.
fn statement(id: &str, claim: &str, since: &str, last: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "claim": claim, "since": since, "last_asserted": last})
}

/// The window of one axis, in the shape the apply phase reads back.
fn window_of(statements: serde_json::Value) -> String {
    serde_json::json!([{"subject": "rachel", "predicate": "lives_in",
                        "statements": statements}])
    .to_string()
}

/// One staged fact in the shape `parse` leaves it in.
fn staged_fact(claim: &str, from: &str, replaces: &str) -> serde_json::Value {
    serde_json::json!({
        "episode_id": "e9", "subject": "rachel", "predicate": "lives_in", "claim": claim,
        "fact_kind": "world", "valid_from": from, "valid_until": null, "confidence": 90,
        "replaces": replaces, "claim_hash": claim.replace(' ', "_")
    })
}

/// Run the apply phase over one staged fact and one parked window.
fn apply(fact: serde_json::Value, window: &str) -> (Vec<serde_json::Value>, String) {
    let payload = serde_json::json!({"facts": [fact], "entities": [], "edges": []});
    let episodes = serde_json::json!({"e9": {"happened_at": "2023-05-27T09:00:00Z",
                                             "session_id": "s2"}});
    let rows = serde_json::json!([
        {"key": BATCH, "kind": "payload", "payload": payload.to_string()},
        {"key": BATCH, "kind": "known", "payload": "[]"},
        {"key": BATCH, "kind": "episodes", "payload": episodes.to_string()},
        {"key": BATCH, "kind": "window", "payload": window}
    ]);
    run(&script_of(EXTRACT_CONFIG), x_reply("apply", "select", rows))
}

/// The measured case, as the store held it: the gold answer is the NEWER
/// statement, and the fact that asked to replace it is three days older.
const SUBURBS: &str = "2023-05-27T00:00:00Z";
const CHICAGO: &str = "2023-05-24T00:00:00Z";

#[test]
fn the_window_says_when_each_statement_was_last_asserted() {
    // The instant the guard reads. `since` is when the statement STARTED and it
    // is what the prompt shows; the apply phase compares the assertion still
    // standing, which is the row a closure names -- so the window carries both,
    // the same pair the night's own axis page has carried since W3.
    let rows = serde_json::json!([
        fact_row("s1", "the suburbs", "2023-05-20T00:00:00Z"),
        fact_row("s2", "the suburbs", SUBURBS)
    ]);
    let (msgs, _) = run(
        &script_of(EXTRACT_CONFIG),
        x_reply("window-page", "select", rows),
    );
    let pool = parked(&msgs, "window-pool").expect("the pool");
    assert_eq!(
        pool[0]["statements"],
        serde_json::json!([statement(
            "s2",
            "the suburbs",
            "2023-05-20T00:00:00Z",
            SUBURBS
        )]),
        "one statement, named by its live assertion, dated by its first and by its last"
    );
}

#[test]
fn the_prompt_shows_the_instant_it_always_showed_and_not_the_new_one() {
    // The guard is arithmetic in the apply phase and costs the prompt nothing.
    // A second date on every line would be paid for by every batch of every
    // colony, for a comparison no model is asked to make.
    let rows = serde_json::json!([fact_row("s2", "the suburbs", SUBURBS)]);
    let (msgs, _) = run(
        &script_of(EXTRACT_CONFIG),
        x_reply("window-page", "select", rows),
    );
    let pool = parked(&msgs, "window-pool").expect("the pool");
    let batch = serde_json::json!([{"episode_id": "e9", "content": "I moved again."}]);
    let rows = serde_json::json!([
        {"key": BATCH, "kind": "batch", "payload": batch.to_string()},
        {"key": BATCH, "kind": "vocab", "payload": "{}"},
        {"key": BATCH, "kind": "window-pool", "payload": pool.to_string()}
    ]);
    let (msgs, _) = run(
        &script_of(EXTRACT_CONFIG),
        x_reply("prompt", "select", rows),
    );
    let prompt = msgs
        .iter()
        .find(|m| m["header"]["route"] == "extract")
        .expect("the extractor call");
    let text = prompt["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");
    assert!(
        text.contains("since 2023-05-27T00:00:00Z"),
        "the rendered window lost the instant it has always shown: {text}"
    );
    assert!(
        !text.contains("last_asserted"),
        "the new field reached the prompt: {text}"
    );
}

#[test]
fn a_replaces_that_points_backwards_in_time_is_refused() {
    // The replay of haystack 830ce83f. The older claim arrives second and asks to
    // end the newer statement; the mint happens, the closure does not, and both
    // statements stay open for the night to judge.
    let (msgs, err) = apply(
        staged_fact("lives in Chicago", CHICAGO, "s2"),
        &window_of(serde_json::json!([statement(
            "s2",
            "the suburbs",
            SUBURBS,
            SUBURBS
        )])),
    );
    assert_eq!(
        fact_ops(&msgs, "insert").len(),
        1,
        "the FACT is still written: a refused closure is not a refused extraction"
    );
    assert!(
        fact_ops(&msgs, "update").is_empty(),
        "an older claim ended a newer statement: {msgs:?}"
    );
    assert!(
        err.contains("direction"),
        "a refused closure has to be visible somewhere, got stderr {err:?}"
    );
}

#[test]
fn the_refused_pair_is_receipted_under_the_batch_key() {
    // Same currency as the closures the lane DOES write (ruling Q2 rail 2): the
    // reasons live in `scratch` under the batch, so a refusal can be read back to
    // the two values and the two instants that produced it. Without the row the
    // guard would be a silent drop, and a silent drop is what made the defect
    // invisible for a whole track.
    let (msgs, _) = apply(
        staged_fact("lives in Chicago", CHICAGO, "s2"),
        &window_of(serde_json::json!([statement(
            "s2",
            "the suburbs",
            SUBURBS,
            SUBURBS
        )])),
    );
    let receipt = parked(&msgs, "extract-refusals").expect("the refusal row");
    let entry = &receipt[0];
    assert_eq!(entry["id"], "s2");
    assert_eq!(entry["subject"], "rachel");
    assert_eq!(entry["predicate"], "lives_in");
    assert_eq!(entry["replaced_claim"], "the suburbs");
    assert_eq!(entry["claim"], "lives in Chicago");
    assert_eq!(entry["episode_id"], "e9");
    assert_eq!(
        entry["held_until"], SUBURBS,
        "the instant of the statement that was NOT ended"
    );
    assert_eq!(
        entry["valid_from"], CHICAGO,
        "and the instant of the fact that wanted to end it"
    );
    assert!(
        entry["reason"].as_str().expect("a reason").len() > 10,
        "a refusal nobody can read back is a silent drop: {entry}"
    );
}

#[test]
fn the_mirror_case_is_applied_exactly_as_it_always_was() {
    // The same pair, the right way round. This is the whole of W4 and the guard
    // must not cost a single closure of it: the newer fact ends the older
    // statement, in the write form the judge uses, down to the column.
    let (msgs, _) = apply(
        staged_fact("the suburbs", SUBURBS, "c1"),
        &window_of(serde_json::json!([statement(
            "c1",
            "lives in Chicago",
            CHICAGO,
            CHICAGO
        )])),
    );
    let inserted = fact_ops(&msgs, "insert");
    let new_id = inserted[0]["row"]["id"].as_str().expect("id").to_string();
    let closures = fact_ops(&msgs, "update");
    assert_eq!(closures.len(), 1, "the legal closure was refused: {msgs:?}");
    assert_eq!(
        closures[0]["set"],
        serde_json::json!({"expired_at": SUBURBS, "superseded_by": new_id,
                           "closure_source": "extract:b1"})
    );
    assert_eq!(closures[0]["where"]["id"], "c1");
    assert!(
        parked(&msgs, "extract-refusals").is_none(),
        "a closure that was APPLIED was receipted as refused too"
    );
}

#[test]
fn two_values_of_one_instant_still_replace_one_another() {
    // The tie, and it is decided by the second component of the lane's own
    // recency tuple rather than by a rule of its own: the statement being closed
    // was read out of the store by THIS batch, so it was recorded before this
    // batch's clock. Two turns of one session updating a value is the ordinary
    // case and it must not be refused.
    let (msgs, _) = apply(
        staged_fact("the suburbs", SUBURBS, "c1"),
        &window_of(serde_json::json!([statement(
            "c1",
            "lives in Chicago",
            SUBURBS,
            SUBURBS
        )])),
    );
    assert_eq!(
        fact_ops(&msgs, "update").len(),
        1,
        "an update inside one session was refused: {msgs:?}"
    );
}

#[test]
fn a_statement_re_asserted_after_the_new_fact_is_refused() {
    // Why the guard reads the LAST assertion and not the first. The statement
    // started before the new fact and was still being said after it, so the new
    // fact does not end it: whoever said it again later is the better witness.
    let (msgs, _) = apply(
        staged_fact("lives in Chicago", CHICAGO, "s2"),
        &window_of(serde_json::json!([statement(
            "s2",
            "the suburbs",
            "2023-05-01T00:00:00Z",
            SUBURBS
        )])),
    );
    assert!(
        fact_ops(&msgs, "update").is_empty(),
        "a statement re-asserted after the replacement was closed anyway: {msgs:?}"
    );
}

#[test]
fn a_window_parked_before_this_package_falls_back_to_the_instant_it_carries() {
    // A batch whose window row was written by the previous version has no
    // `last_asserted` at all. The guard reads `since` there rather than treating
    // the direction as unknown -- the two are the same value on every statement
    // asserted once, which is the shape of nearly every window entry.
    let old = serde_json::json!([{"subject": "rachel", "predicate": "lives_in",
                                  "statements": [{"id": "s2", "claim": "the suburbs",
                                                  "since": SUBURBS}]}])
    .to_string();
    let (msgs, _) = apply(staged_fact("lives in Chicago", CHICAGO, "s2"), &old);
    assert!(
        fact_ops(&msgs, "update").is_empty(),
        "an old window row disabled the guard: {msgs:?}"
    );
}

#[test]
fn a_batch_that_refused_nothing_writes_no_refusal_row() {
    // The receipt grows a row only when there is something to receipt, the same
    // way the closure receipt does.
    let (msgs, err) = apply(
        staged_fact("the suburbs", SUBURBS, ""),
        &window_of(serde_json::json!([statement(
            "c1",
            "lives in Chicago",
            CHICAGO,
            CHICAGO
        )])),
    );
    assert!(parked(&msgs, "extract-refusals").is_none());
    assert!(!err.contains("direction"), "stderr said {err:?}");
}

// ================================================================== dream-glue

const RUN: &str = "r1";
const TO: &str = "2026-08-12T03:00:00Z";

fn d_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": phase,
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

fn emit_d(doc: serde_json::Value) -> Vec<serde_json::Value> {
    run(&script_of(DREAM_CONFIG), doc).0
}

/// One open fact row as the canonicalisation scan projects it.
fn scan_row(id: &str, claim: &str, from: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "subject": "rachel", "canonical_subject": "rachel",
        "predicate": "lives_in", "canonical_predicate": "lives_in",
        "claim": claim, "canonical_claim": claim,
        "valid_from": from, "recorded_at": from,
        "expired_at": null, "closure_source": null
    })
}

/// The same row, closed AT `ended` by `source`.
fn closed_row(id: &str, claim: &str, from: &str, ended: &str, source: &str) -> serde_json::Value {
    let mut row = scan_row(id, claim, from);
    row["expired_at"] = serde_json::json!(ended);
    row["closure_source"] = serde_json::json!(source);
    row
}

/// The scan of a night, split into the payload it parks and the ops it emits
/// next to it.
fn scan(rows: serde_json::Value) -> (serde_json::Value, Vec<serde_json::Value>) {
    let msgs = emit_d(d_reply("canon-scan", "select", rows));
    let args = args_of(&msgs[0]);
    assert_eq!(
        args["row"]["kind"], "canon-scan",
        "the chain still hangs on the scan row"
    );
    let payload =
        serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("scan");
    (payload, msgs.iter().skip(1).map(args_of).collect())
}

/// The inverted closure as the store holds it after an extraction batch got it
/// wrong: the statement began on the 27th and was ended on the 24th.
fn inverted_night() -> serde_json::Value {
    serde_json::json!([
        scan_row("c1", "lives in Chicago", CHICAGO),
        closed_row("s2", "the suburbs", SUBURBS, CHICAGO, "extract:b1")
    ])
}

#[test]
fn an_inverted_extract_closure_is_taken_back_by_the_night_itself() {
    // The second half of the fix, and it needs no model: a closure whose instant
    // lies BEFORE the statement it ended is wrong about time, and time is the one
    // thing this lane can check on its own. The revert is W4's, unchanged -- one
    // column, the axis echo and the exact attribution the row carries, so the
    // re-derive of this same night withdraws the two columns (ruling Q4).
    let (_, ops) = scan(inverted_night());
    let reverts: Vec<&serde_json::Value> = ops
        .iter()
        .filter(|a| a["operation"] == "update" && a["table"] == "facts")
        .collect();
    assert_eq!(reverts.len(), 1, "the inverted closure stands: {ops:?}");
    assert_eq!(
        reverts[0]["set"],
        serde_json::json!({"closure_source": ""}),
        "the revert is the attribution and nothing else"
    );
    assert_eq!(
        reverts[0]["where"],
        serde_json::json!({"id": "s2", "canonical_subject": "rachel",
                           "canonical_predicate": "lives_in",
                           "closure_source": "extract:b1"})
    );
}

#[test]
fn the_night_receipts_why_it_took_the_closure_back() {
    // Ruling Q2 rail 2 again: a retraction carries its reason, and the reason of
    // this one is the two instants that contradict each other.
    let (_, ops) = scan(inverted_night());
    let receipt = ops
        .iter()
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "canon-inverted")
        .expect("the retraction receipt");
    let payload: serde_json::Value =
        serde_json::from_str(receipt["row"]["payload"].as_str().expect("payload")).expect("json");
    assert_eq!(payload[0]["id"], "s2");
    assert_eq!(payload[0]["closed_by"], "extract:b1");
    assert_eq!(payload[0]["subject"], "rachel");
    assert_eq!(payload[0]["predicate"], "lives_in");
    let reason = payload[0]["reason"].as_str().expect("a reason");
    assert!(
        reason.contains(SUBURBS) && reason.contains(CHICAGO),
        "the reason names neither instant: {reason}"
    );
}

#[test]
fn a_closure_it_just_took_back_is_not_put_in_front_of_the_judge() {
    // The night does not pay for an opinion it no longer needs, and it never
    // shows the judge a closure that is already gone: the payload would ask for a
    // review of a row whose attribution this same night cleared.
    let (payload, _) = scan(inverted_night());
    let axes = payload["axes"].as_array().expect("axes");
    assert!(
        axes.is_empty(),
        "one open statement and a retracted closure leave no question at all: {axes:?}"
    );
}

#[test]
fn a_correctly_directed_closure_is_still_the_judges_to_review() {
    // The W4 path, untouched. The direction check is a filter in front of the
    // review, never a replacement for it: whether a correctly dated closure is a
    // replacement or an enumeration is still a judgement.
    let rows = serde_json::json!([
        scan_row("s2", "the suburbs", SUBURBS),
        closed_row("c1", "lives in Chicago", CHICAGO, SUBURBS, "extract:b1")
    ]);
    let (payload, ops) = scan(rows);
    let axes = payload["axes"].as_array().expect("axes");
    assert_eq!(axes.len(), 1, "the axis was dropped: {axes:?}");
    assert_eq!(
        axes[0]["closed"][0]["id"], "c1",
        "the closed side still reaches the judge: {}",
        axes[0]
    );
    assert!(
        !ops.iter()
            .any(|a| a["operation"] == "update" && a["table"] == "facts"),
        "a correctly directed closure was reverted: {ops:?}"
    );
}

#[test]
fn a_judged_closure_is_never_taken_back_by_the_direction_check() {
    // Rail 1, and this is where it bites hardest: an inverted JUDGE closure is
    // left exactly where it is. A round that could revoke a judgement -- its own
    // or an earlier night's -- would make "only close, never delete" revocable by
    // the party that wrote it. The arithmetic residue is out for the same reason;
    // ruling Q4 withdraws that one without an opinion.
    let rows = serde_json::json!([
        scan_row("c1", "lives in Chicago", CHICAGO),
        closed_row("s2", "the suburbs", SUBURBS, CHICAGO, "judge:r0"),
        closed_row("s3", "the old flat", SUBURBS, CHICAGO, "")
    ]);
    let (_, ops) = scan(rows);
    assert!(
        !ops.iter()
            .any(|a| a["operation"] == "update" && a["table"] == "facts"),
        "the night reverted a closure that was not the extractor's: {ops:?}"
    );
}

#[test]
fn the_retraction_is_written_before_the_night_asks_anything() {
    // The ordering the revert depends on: it is emitted by the SCAN, which is the
    // first phase of the round, so it lands before `canonicalize` moves an
    // identity (the `where` echoes the axis as the scan read it) and before
    // `sup-axes` re-derives (which is what withdraws the two columns in this same
    // night rather than in the next one).
    let msgs = emit_d(d_reply("canon-scan", "select", inverted_night()));
    let ops: Vec<String> = msgs
        .iter()
        .map(|m| {
            args_of(m)["operation"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(
        !ops.iter().any(|o| o == "canonicalize"),
        "the scan phase ran a re-derive of its own: {ops:?}"
    );
    assert!(
        ops.iter().any(|o| o == "update"),
        "the revert was not emitted at all: {ops:?}"
    );
}

#[test]
fn the_run_receipt_carries_both_kinds_of_retraction_under_one_key() {
    // A night takes a closure back for two reasons now -- the judge contradicted
    // it, or its direction was wrong -- and both are the same verdict. One key,
    // so the books of a night still answer "what did this round take back" with
    // one list, and the reason of each entry says who asked for it.
    let rows = serde_json::json!([
        {"key": RUN, "kind": "verdicts", "payload": "{\"beliefs\": []}"},
        {"key": RUN, "kind": "beliefs", "payload": "[]"},
        {"key": RUN, "kind": "canon-inverted",
         "payload": "[{\"id\":\"s2\",\"subject\":\"rachel\",\"predicate\":\"lives_in\",\
                       \"closed_by\":\"extract:b1\",\"reason\":\"direction\"}]"},
        {"key": RUN, "kind": "canon-reopenings",
         "payload": "[{\"id\":\"e1\",\"subject\":\"user\",\"predicate\":\"favorite_editor\",\
                       \"closed_by\":\"extract:b2\",\"reason\":\"the two editors coexist\"}]"}
    ]);
    let msgs = emit_d(d_reply("apply-run", "select", rows));
    let closing = msgs
        .iter()
        .map(args_of)
        .find(|a| a["table"] == "consolidation_log")
        .expect("the run is closed");
    let verdicts: serde_json::Value =
        serde_json::from_str(closing["set"]["verdicts"].as_str().expect("verdicts"))
            .expect("verdict payload");
    let taken_back = verdicts["reopenings"].as_array().expect("reopenings");
    assert_eq!(
        taken_back.len(),
        2,
        "one of the two was lost: {taken_back:?}"
    );
    assert_eq!(taken_back[0]["id"], "s2", "the night's own one first");
    assert_eq!(taken_back[1]["id"], "e1");
}
