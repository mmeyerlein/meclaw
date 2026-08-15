//! Statement identity W3 -- the nightly judge PRODUCES the closures
//! (GitHub #13, rulings Q2/Q4/Q5).
//!
//! W2 removed the axis arithmetic as a source of supersession: since then only a
//! re-assertion of the same statement and a `valid_until` of its own end a span,
//! which measured 17 closures on the eight benchmark stores instead of 928.
//! Everything else waits for a WRITER, and this package is that writer.
//!
//! The round it extends is the P5 canonicalisation round: one judge call a
//! night, one payload, sections that give each other context. W3 adds the third
//! section -- for every axis carrying more than one OPEN statement, which of
//! them are still true and which was replaced by which -- and turns each verdict
//! into one attributed `update` on `facts`.
//!
//! Everything here runs the REAL `params.script_inline` of `dream-glue` against
//! injected store replies and injected judgements, so no model is called and
//! nothing costs anything. Whether the judge answers WELL is a model property
//! and belongs to the paid scenario; the free scenario C12 proves the same chain
//! end to end on a real colony with the judgement handed in.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";

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

fn glue_script() -> String {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("dream-glue config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run the real script with a real stdin document and return the emitted messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(glue_script())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&meclaw_testing::code_stdin_bytes(&doc))
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "dream-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

const RUN: &str = "r1";
const TO: &str = "2026-08-12T03:00:00Z";

/// One store reply as the edge delivers it.
fn store_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": phase,
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

/// The judge's answer as the return edge delivers it.
fn judgement(text: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": "canon-judged",
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"finish_reason": "stop"}
        },
        "messages": [{"origin": "assistant", "type": "text", "text": text}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// Every `facts` update a judgement produced, in order.
fn fact_updates(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    msgs.iter()
        .map(args_of)
        .filter(|a| a["operation"] == "update" && a["table"] == "facts")
        .collect()
}

/// One open fact row in the shape the canonicalisation scan projects.
fn row(id: &str, predicate: &str, claim: &str, from: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "subject": "user", "canonical_subject": "user",
        "predicate": predicate, "canonical_predicate": predicate,
        "claim": claim, "canonical_claim": claim,
        "valid_from": from, "recorded_at": from
    })
}

/// A store with all four shapes at once: a replacement, an enumeration, a
/// single-statement axis and a bucket bigger than the per-axis page.
fn scan_rows() -> serde_json::Value {
    let mut rows = vec![
        row(
            "e1",
            "favorite_editor",
            "favorite editor is helix",
            "2026-03-05T00:00:00Z",
        ),
        row(
            "e2",
            "favorite_editor",
            "favorite editor is vscode",
            "2026-06-05T00:00:00Z",
        ),
        row(
            "c1",
            "has_child",
            "has a child named ada",
            "2026-02-01T00:00:00Z",
        ),
        row(
            "c2",
            "has_child",
            "has a child named ben",
            "2026-03-01T00:00:00Z",
        ),
        row("h1", "lives_in", "lives in zone-a", "2026-01-06T00:00:00Z"),
    ];
    for i in 0..7 {
        rows.push(row(
            &format!("p{i}"),
            "planned_activity",
            &format!("plan number {i}"),
            &format!("2026-04-0{}T00:00:00Z", i + 1),
        ));
    }
    serde_json::Value::Array(rows)
}

/// The inventory the scan parks for the meeting point.
fn parked_scan(rows: serde_json::Value) -> serde_json::Value {
    let msgs = emit(store_reply("canon-scan", "select", rows));
    let args = args_of(&msgs[0]);
    assert_eq!(args["row"]["kind"], "canon-scan");
    serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("scan payload")
}

#[test]
fn the_scan_reads_the_columns_the_closure_question_needs() {
    // The third section asks about VALUES and TIMES, so the scan that feeds it
    // has to project them. Without `canonical_claim` the lane cannot tell a
    // re-assertion from a change, and without `valid_from` the judge cannot say
    // which of two statements is the later one.
    //
    // The phase the scan hangs on moved with W5 (#13, ruling Q3): the round now
    // reads the judged cardinality first and the fact scan follows its park. It
    // moved once more with GH #73, which puts the identity questions' own read of
    // the CLOSED rows in front of it -- so the fact page is emitted from
    // `canon-closed`, which reports no closed row here and emits nothing else.
    // The claim below is untouched by either.
    let msgs = emit(store_reply("canon-closed", "select", serde_json::json!([])));
    let args = args_of(&msgs[0]);
    let columns = args["columns"].as_array().expect("columns").clone();
    for needed in ["id", "canonical_subject", "canonical_predicate", "claim"] {
        assert!(
            columns.iter().any(|c| c == needed),
            "the scan lost {needed}: {}",
            args["columns"]
        );
    }
    for needed in ["canonical_claim", "valid_from"] {
        assert!(
            columns.iter().any(|c| c == needed),
            "the closure question needs {needed}, got {}",
            args["columns"]
        );
    }
    // The page is bounded on `expired_at` and on nothing else -- a statement
    // closed long ago has its answer and is not a question. Since W4 the bound is
    // an interval instead of a NULL check, because the extractor now closes at
    // write time and the round has to be able to review that (ruling Q2 option C
    // guard rail 3, second half); which of the two forms it is, is pinned in
    // `w4_extract_replaces.rs`.
    let bound = &args["where"]["expired_at"];
    assert!(
        bound["is_null"] == serde_json::json!(true) || bound["or_null"]["gte"].is_string(),
        "the closure question is asked about a bounded page of statements, got {bound}"
    );
    assert_eq!(
        args["where"].as_object().expect("where").len(),
        1,
        "one filter, one column: the scan of the night is not a second query layer"
    );
}

#[test]
fn the_scan_offers_the_axes_that_carry_more_than_one_open_statement() {
    let parked = parked_scan(scan_rows());
    let axes = parked["axes"].as_array().expect("axes section").clone();
    let names: Vec<&str> = axes
        .iter()
        .map(|a| a["predicate"].as_str().unwrap_or_default())
        .collect();
    assert!(
        names.contains(&"favorite_editor"),
        "the replacement shape is the whole question, got {names:?}"
    );
    assert!(
        names.contains(&"has_child"),
        "an ENUMERATION is offered too: refusing it is the judge's answer, not the \
         lane's -- a lane that pre-filters could never measure the refusal ({names:?})"
    );
    assert!(
        !names.contains(&"lives_in"),
        "an axis with ONE open statement has nothing to decide ({names:?})"
    );
    assert!(
        !names.contains(&"planned_activity"),
        "an axis longer than the per-axis page is never offered as a whole axis: a \
         judge that sees half a bucket cannot tell it is a bucket. Since GH #66 it \
         is not dropped either -- it leaves the scan as a triage candidate in the \
         `paged` section, which is pinned in f5_bucket_axes.rs ({names:?})"
    );
}

#[test]
fn every_offered_statement_carries_its_value_and_its_times() {
    let parked = parked_scan(scan_rows());
    let axis = parked["axes"]
        .as_array()
        .expect("axes")
        .iter()
        .find(|a| a["predicate"] == "favorite_editor")
        .cloned()
        .expect("the editor axis");
    assert_eq!(axis["subject"], "user");
    assert_eq!(
        axis["statements"],
        serde_json::json!([
            {"id": "e1", "claim": "favorite editor is helix",
             "since": "2026-03-05T00:00:00Z", "last_asserted": "2026-03-05T00:00:00Z",
             "assertions": 1},
            {"id": "e2", "claim": "favorite editor is vscode",
             "since": "2026-06-05T00:00:00Z", "last_asserted": "2026-06-05T00:00:00Z",
             "assertions": 1}
        ]),
        "oldest first, one entry per STATEMENT, with the value and both instants"
    );
}

/// The parked halves, as the meeting-point select hands them back.
fn meeting_point(predicates: serde_json::Value, axes: serde_json::Value) -> serde_json::Value {
    let scan = serde_json::json!({"predicates": predicates, "context": {}, "axes": axes});
    serde_json::json!([
        {"key": RUN, "kind": "canon-scan", "payload": scan.to_string()},
        {"key": RUN, "kind": "canon-pairs", "payload": "[]"}
    ])
}

/// One offered axis in the shape the scan parks it.
fn offered_axis() -> serde_json::Value {
    serde_json::json!([{
        "subject": "user", "predicate": "favorite_editor",
        "statements": [
            {"id": "e1", "claim": "favorite editor is helix", "since": "2026-03-05T00:00:00Z",
             "last_asserted": "2026-03-05T00:00:00Z", "assertions": 1},
            {"id": "e2", "claim": "favorite editor is vscode", "since": "2026-06-05T00:00:00Z",
             "last_asserted": "2026-06-05T00:00:00Z", "assertions": 1}
        ]
    }])
}

#[test]
fn the_third_question_travels_in_the_same_single_call() {
    // The P5 ruling stands: ONE payload a night, sections that give each other
    // context. The currency question needs exactly what the identity questions
    // need -- which keys name one relation, who is one person -- so splitting it
    // into a call of its own would buy the same context twice.
    let msgs = emit(store_reply(
        "canon-ask",
        "select",
        meeting_point(
            serde_json::json!({"favorite_editor": ["user"], "Lieblingseditor": ["user"]}),
            offered_axis(),
        ),
    ));
    assert_eq!(msgs.len(), 1, "one call a night, not one per question");
    assert_eq!(msgs[0]["header"]["route"], "judge");
    let payload: serde_json::Value =
        serde_json::from_str(msgs[0]["messages"][0]["text"].as_str().expect("payload"))
            .expect("payload json");
    assert_eq!(
        payload["axes"],
        offered_axis(),
        "the axes reach the judge exactly as the scan parked them"
    );
    let instructions = msgs[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");
    assert!(
        instructions.contains("closures"),
        "the answer shape has to name the third section or nothing can come back"
    );
    assert!(
        instructions.contains("superseded_by") && instructions.contains("reason"),
        "ruling Q2 guard rail 2: every closure names its successor AND its reason"
    );
    assert!(
        instructions.contains("has_child"),
        "the enumeration example is the refusal half of the question"
    );
}

#[test]
fn an_axis_question_alone_is_worth_the_call() {
    // A store nobody drifted still has statements that may have been replaced.
    // Before W3 this shape (one relation, no candidate pair) was the "no
    // question" case and the round ended without asking anything.
    let msgs = emit(store_reply(
        "canon-ask",
        "select",
        meeting_point(
            serde_json::json!({"favorite_editor": ["user"]}),
            offered_axis(),
        ),
    ));
    assert_eq!(msgs[0]["header"]["route"], "judge");
}

#[test]
fn no_question_of_any_of_the_three_kinds_still_costs_nothing() {
    // And the counter-direction, unchanged: one relation, no pair, no axis with
    // two open statements -- the most expensive model of the hive is not called
    // to confirm that.
    let msgs = emit(store_reply(
        "canon-ask",
        "select",
        meeting_point(
            serde_json::json!({"favorite_editor": ["user"]}),
            serde_json::json!([]),
        ),
    ));
    assert_ne!(msgs[0]["header"]["route"], "judge");
    assert_eq!(msgs[0]["header"]["phase"], "sup-scope");
}

#[test]
fn a_re_asserted_statement_is_one_entry_and_names_its_live_assertion() {
    // Two assertions of one statement are ONE statement (W2): the judge is asked
    // about values, not about rows, and the id it may close is the assertion
    // still standing. Closing the older one instead would collide with the
    // arithmetic that already ends it.
    let rows = serde_json::json!([
        row(
            "y1",
            "practices",
            "yoga twice a week",
            "2026-01-05T00:00:00Z"
        ),
        row(
            "y2",
            "practices",
            "yoga twice a week",
            "2026-05-05T00:00:00Z"
        ),
        row(
            "s1",
            "practices",
            "sourdough on weekends",
            "2026-06-05T00:00:00Z"
        )
    ]);
    let parked = parked_scan(rows);
    let axis = &parked["axes"][0];
    assert_eq!(
        axis["statements"],
        serde_json::json!([
            {"id": "y2", "claim": "yoga twice a week", "since": "2026-01-05T00:00:00Z",
             "last_asserted": "2026-05-05T00:00:00Z", "assertions": 2},
            {"id": "s1", "claim": "sourdough on weekends", "since": "2026-06-05T00:00:00Z",
             "last_asserted": "2026-06-05T00:00:00Z", "assertions": 1}
        ])
    );
}

/// One well-formed closure verdict of the third section.
const ONE_CLOSURE: &str = r#"{"closures":[
  {"subject":"user","predicate":"favorite_editor","closed":"e1","superseded_by":"e2",
   "ended_at":"2026-06-05T00:00:00Z",
   "reason":"the user named vscode in a later session, so the helix statement is not current"}]}"#;

#[test]
fn a_closure_verdict_becomes_exactly_one_attributed_update() {
    // The write surface ruling Q2 fixed and the W2 receipt handed over: three
    // columns on the closed statement, nothing else. `closure_source` is the
    // load-bearing one -- without it the very next `sup-axes` re-derive clears
    // the whole verdict back to NULL (ruling Q4), silently.
    let msgs = emit(judgement(ONE_CLOSURE));
    let updates = fact_updates(&msgs);
    assert_eq!(updates.len(), 1, "one verdict, one update: {updates:?}");
    assert_eq!(
        updates[0]["set"],
        serde_json::json!({"expired_at": "2026-06-05T00:00:00Z", "superseded_by": "e2",
                           "closure_source": "judge:r1"}),
        "the attribution names the author AND the night: `judge:<run id>` is the key \
         of the consolidation_log row whose payload carries the reason"
    );
    assert_eq!(
        updates[0]["where"],
        serde_json::json!({"id": "e1", "canonical_subject": "user",
                           "canonical_predicate": "favorite_editor",
                           "expired_at": {"is_null": true}}),
        "pinned to the axis it was judged on AND to a statement that is still open: \
         a verdict that names an axis it was not shown closes nothing, and a closure \
         is never written over an existing one"
    );
}

#[test]
fn the_judge_may_only_close_never_delete_and_never_rewrite() {
    // Ruling Q2 guard rail 1, as a property of every op the round emits. `claim`,
    // `valid_from` and `valid_until` are not the judge's to touch -- they are what
    // makes the whole round revertible.
    let msgs = emit(judgement(
        r#"{"predicates":[{"alias":"Lieblingseditor","canonical":"favorite_editor"}],
            "closures":[{"subject":"user","predicate":"favorite_editor","closed":"e1",
                         "superseded_by":"e2","ended_at":"2026-06-05T00:00:00Z",
                         "reason":"vscode replaced helix"},
                        {"subject":"user","predicate":"practices","closed":"y1",
                         "superseded_by":"y2","ended_at":"2026-05-05T00:00:00Z",
                         "reason":"three times a week replaced twice a week"}]}"#,
    ));
    for op in msgs.iter().map(args_of) {
        assert_ne!(op["operation"], "delete", "the round deleted: {op}");
        if op["operation"] == "update" {
            let keys: Vec<String> = op["set"]
                .as_object()
                .expect("set")
                .keys()
                .cloned()
                .collect();
            for k in &keys {
                assert!(
                    ["expired_at", "superseded_by", "closure_source"].contains(&k.as_str()),
                    "the judge wrote {k} -- the closure columns are its whole write surface"
                );
            }
        }
    }
    assert_eq!(fact_updates(&msgs).len(), 2, "both verdicts were written");
}

#[test]
fn the_closures_are_written_before_any_alias_moves_an_identity() {
    // The judgement is one set and the order inside it is not decorative: a
    // closure names the axis the judge was SHOWN, and `canonicalize` is what
    // moves an identity. Written after the re-derive, the same `where` would
    // match nothing.
    let msgs = emit(judgement(
        r#"{"entities":[{"alias":"user:u1","canonical":"user"}],
            "closures":[{"subject":"user","predicate":"favorite_editor","closed":"e1",
                         "superseded_by":"e2","ended_at":"2026-06-05T00:00:00Z",
                         "reason":"vscode replaced helix"}]}"#,
    ));
    let ops: Vec<String> = msgs
        .iter()
        .map(|m| {
            args_of(m)["operation"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let closure_at = ops.iter().position(|o| o == "update").expect("the closure");
    let alias_at = ops
        .iter()
        .position(|o| o == "set_alias")
        .expect("the alias");
    let derive_at = ops
        .iter()
        .position(|o| o == "canonicalize")
        .expect("the re-derive");
    assert!(
        closure_at < alias_at && alias_at < derive_at,
        "closures, then aliases, then exactly one re-derive: {ops:?}"
    );
}

#[test]
fn a_closure_that_cannot_be_read_back_is_dropped() {
    // Ruling Q2 guard rail 2 -- "every closure carries its reason" -- enforced
    // rather than requested. Plus the shapes that name nothing: no successor, a
    // statement closing itself, an axis the verdict does not say.
    let msgs = emit(judgement(
        r#"{"closures":[{"subject":"user","predicate":"favorite_editor","closed":"e1",
                         "superseded_by":"e2","ended_at":"2026-06-05T00:00:00Z","reason":"  "},
                        {"subject":"user","predicate":"favorite_editor","closed":"e1",
                         "superseded_by":"","ended_at":"2026-06-05T00:00:00Z","reason":"gone"},
                        {"subject":"user","predicate":"favorite_editor","closed":"e1",
                         "superseded_by":"e1","ended_at":"2026-06-05T00:00:00Z","reason":"self"},
                        {"subject":"","predicate":"","closed":"e1","superseded_by":"e2",
                         "ended_at":"2026-06-05T00:00:00Z","reason":"no axis"},
                        "not an object"]}"#,
    ));
    assert!(
        fact_updates(&msgs).is_empty(),
        "a closure the store cannot justify was written anyway: {msgs:?}"
    );
    assert_eq!(
        args_of(&msgs[0])["operation"],
        "canonicalize",
        "and the round still ends in its single re-derive"
    );
}

#[test]
fn a_closure_without_a_usable_instant_falls_back_to_the_night() {
    // `ended_at` is copied from the payload, and a model that garbles the copy
    // must not cost the verdict: the night's own clock is the honest fallback --
    // "this statement stopped being current at the latest tonight". `delta_to` is
    // the ONLY clock this lane has, so a re-run writes the same value again.
    let msgs = emit(judgement(
        r#"{"closures":[{"subject":"user","predicate":"favorite_editor","closed":"e1",
                         "superseded_by":"e2","ended_at":"last summer",
                         "reason":"vscode replaced helix"}]}"#,
    ));
    assert_eq!(fact_updates(&msgs)[0]["set"]["expired_at"], TO);
}

#[test]
fn a_judgement_without_closures_writes_no_update_at_all() {
    // The P5 shape, unchanged: an identity judgement is aliases, refusals and one
    // re-derive. W3 adds a section, it does not change what the other two do.
    let msgs = emit(judgement(
        r#"{"predicates":[{"alias":"Lieblingseditor","canonical":"favorite_editor"}],
            "different":[{"dimension":"subject","left":"leon","right":"leona"}]}"#,
    ));
    assert!(fact_updates(&msgs).is_empty());
}

/// The scratch rows of a run, as the apply-run select hands them back.
fn run_scratch(closures: Option<&str>) -> serde_json::Value {
    let mut rows = vec![
        serde_json::json!({"key": RUN, "kind": "verdicts", "payload": "{\"beliefs\": []}"}),
        serde_json::json!({"key": RUN, "kind": "beliefs", "payload": "[]"}),
    ];
    if let Some(payload) = closures {
        rows.push(serde_json::json!({"key": RUN, "kind": "canon-closures", "payload": payload}));
    }
    serde_json::Value::Array(rows)
}

/// The `consolidation_log` update that closes a run.
fn close_op(msgs: &[serde_json::Value]) -> serde_json::Value {
    msgs.iter()
        .map(args_of)
        .find(|a| a["table"] == "consolidation_log")
        .expect("the run is closed")
}

#[test]
fn the_run_receipt_carries_the_reason_of_every_closure() {
    // Ruling Q2 guard rail 2 lands where this lane receipts every other artefact
    // of a night: the verdict payload of `consolidation_log`. That is what makes
    // the attribution readable in both directions -- the fact row names the run,
    // the run names the sentence.
    let msgs = emit(store_reply(
        "apply-run",
        "select",
        run_scratch(Some(
            r#"[{"id":"e1","superseded_by":"e2","expired_at":"2026-06-05T00:00:00Z",
                 "subject":"user","predicate":"favorite_editor",
                 "reason":"the user named vscode in a later session"}]"#,
        )),
    ));
    let closing = close_op(&msgs);
    assert_eq!(closing["set"]["status"], "done");
    let verdicts: serde_json::Value =
        serde_json::from_str(closing["set"]["verdicts"].as_str().expect("verdicts"))
            .expect("verdict payload");
    assert_eq!(
        verdicts["closures"][0]["reason"],
        "the user named vscode in a later session"
    );
    assert_eq!(verdicts["closures"][0]["id"], "e1");
    assert_eq!(verdicts["closures"][0]["superseded_by"], "e2");
}

#[test]
fn a_night_that_closed_nothing_receipts_what_it_always_did() {
    // The receipt grows a key only when there is something to receipt. A run
    // without closures writes the same payload it wrote before this package.
    let msgs = emit(store_reply("apply-run", "select", run_scratch(None)));
    let verdicts: serde_json::Value = serde_json::from_str(
        close_op(&msgs)["set"]["verdicts"]
            .as_str()
            .expect("verdicts"),
    )
    .expect("verdict payload");
    assert_eq!(verdicts, serde_json::json!({"beliefs": []}));
}

/// Run a probe against the module body of the real script. The `park()` exit at
/// its end is swallowed so the probe can call the helpers the script defines --
/// the same construction `w2_statement_chain.rs` uses for the chain rule.
fn run_probe(probe: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'dream-glue', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(&glue_script()).unwrap(),
        probe
    );
    let out = Command::new("python3")
        .arg("-c")
        .arg(src)
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Two open statements on one axis plus the materialisation loop of a night: the
/// re-derive writes its own result back, exactly as `sup-axes` does.
const AXIS_AND_ROUND: &str = r#"
rows = [
 {"id":"a","subject":"user","canonical_subject":"user","predicate":"favorite_editor",
  "canonical_predicate":"favorite_editor","claim":"helix","canonical_claim":"helix",
  "valid_from":"2026-03-05T00:00:00Z","valid_until":None,"recorded_at":"2026-03-05T00:00:00Z",
  "expired_at":None,"superseded_by":None,"closure_source":None,"session_id":"s1"},
 {"id":"b","subject":"user","canonical_subject":"user","predicate":"favorite_editor",
  "canonical_predicate":"favorite_editor","claim":"vscode","canonical_claim":"vscode",
  "valid_from":"2026-06-05T00:00:00Z","valid_until":None,"recorded_at":"2026-06-05T00:00:00Z",
  "expired_at":None,"superseded_by":None,"closure_source":None,"session_id":"s2"}
]

def a_round(rs):
    """One nightly round: derive, then write the differences back."""
    by = {str(r["id"]): r for r in rs}
    out = derive_supersessions(rs)
    for fid, end, succ in out:
        by[str(fid)]["expired_at"] = end
        by[str(fid)]["superseded_by"] = succ
    return out

def close_it(source):
    rows[0]["expired_at"] = "2026-06-05T00:00:00Z"
    rows[0]["superseded_by"] = "b"
    rows[0]["closure_source"] = source
"#;

#[test]
fn a_closure_without_an_author_is_cleared_by_the_next_round() {
    // Ruling Q4 as an OPERATING property, and the reason `closure_source` had to
    // be mandatory in the handover: the shape the pre-W2 axis arithmetic left
    // behind carries no author, the rule no longer finds it, and the very next
    // round clears it back to NULL. A judge that forgets the column writes a
    // verdict with the lifetime of one night.
    let probe = AXIS_AND_ROUND.to_string()
        + r#"
close_it(None)
print(a_round(rows)[0], rows[0]["expired_at"])
"#;
    assert_eq!(
        run_probe(&probe),
        "('a', None, None) None",
        "an unattributed closure survived the re-derive"
    );
}

#[test]
fn a_judge_closure_is_stable_round_after_round() {
    // The counter-direction, which is what makes the withdrawal safe to run every
    // night: an ATTRIBUTED closure is not re-derivable from the chain, so a round
    // that cleared it would silently delete a judgement. Three rounds, because a
    // rule that survives one may still drift on the second.
    let probe = AXIS_AND_ROUND.to_string()
        + r#"
close_it("judge:r1")
seen = [a_round(rows) for _ in range(3)]
print(seen[0] == seen[1] == seen[2], seen[2][0], rows[0]["closure_source"])
"#;
    assert_eq!(
        run_probe(&probe),
        "True ('a', '2026-06-05T00:00:00Z', 'b') judge:r1",
        "a judged closure did not survive repeated rounds unchanged"
    );
}

#[test]
fn clearing_the_attribution_is_the_revert_of_a_judgement() {
    // The revert path, and it is the alias one a dimension over: an alias is
    // reverted by a `delete` on the alias table plus a `canonicalize`, a closure
    // by clearing its attribution plus the next re-derive. There is no closure
    // TABLE to delete from -- the attribution is a column on the closed fact, so
    // the equivalent op is
    //   {"operation": "update", "table": "facts", "set": {"closure_source": ""},
    //    "where": {"closure_source": "judge:<run id>"}}
    // which is also why the source carries the run: one night, one revert, and
    // the columns fall back on their own in the round that follows.
    let probe = AXIS_AND_ROUND.to_string()
        + r#"
close_it("judge:r1")
a_round(rows)
rows[0]["closure_source"] = ""
print(a_round(rows)[0], rows[0]["expired_at"], rows[0]["superseded_by"])
"#;
    assert_eq!(
        run_probe(&probe),
        "('a', None, None) None None",
        "a reverted judgement left its closure standing"
    );
}

#[test]
fn the_round_that_wrote_the_closure_does_not_eat_it_again() {
    // The ordering clause of the handover, as a pin: the judge writes BEFORE
    // `sup-axes`, and the re-derive of that same night has to carry the fresh
    // verdict through instead of withdrawing it. Written after the re-derive the
    // closure would survive too -- but only until the next night, which is the
    // silent failure this pin stands in front of.
    let probe = AXIS_AND_ROUND.to_string()
        + r#"
close_it("judge:r1")
print([p for p in a_round(rows) if p[1] or p[2]])
"#;
    assert_eq!(run_probe(&probe), "[('a', '2026-06-05T00:00:00Z', 'b')]");
}
