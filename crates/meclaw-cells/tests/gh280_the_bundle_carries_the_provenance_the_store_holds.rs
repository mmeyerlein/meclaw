//! GH #280 — the bundle carries the provenance the store already holds.
//!
//! Every field in this file exists in the `facts` table, is selected by the
//! hydration, travels through the projection — and was then dropped on the last
//! step, when the candidate was built. A reader of the bundle could therefore
//! not tell a hedged statement from a certain one, could not tell whether the
//! list in front of it was the whole answer or a truncated one, and could not
//! tell an open fact from a closed one whose successor is known by name.
//!
//! None of this is new retrieval. It is the store's own answer, stopped one
//! line short of the reader.

use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";

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

fn recall_script() -> String {
    let raw = std::fs::read_to_string(RECALL_CONFIG).expect("recall config");
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

/// Run the shipped script against a real stdin document; return the emissions.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(
        &recall_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "recall exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// A fact row as the hydration select returns it — every column the axis select
/// names, so a test can overwrite the one it is about and leave the rest honest.
fn fact(
    id: &str,
    session: &str,
    subject: &str,
    claim: &str,
    valid_from: &str,
) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": session, "subject": subject,
                       "predicate": "prefers", "claim": claim,
                       "canonical_subject": subject, "canonical_predicate": "prefers",
                       "canonical_claim": claim, "valid_from": valid_from,
                       "valid_until": serde_json::Value::Null,
                       "recorded_at": valid_from,
                       "expired_at": serde_json::Value::Null,
                       "superseded_by": serde_json::Value::Null,
                       "episode_id": format!("ep-of-{id}"),
                       "fact_kind": "state", "confidence": 90})
}

fn episode(id: &str, session: &str, content: &str, happened_at: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": session, "sender": "user",
                       "content": content, "happened_at": happened_at,
                       "recorded_at": happened_at})
}

/// The `t1-emit` document of a **tier 1** request — the tier is what keeps the
/// run on the emit path instead of routing it to the dialectic.
fn emit_doc_full(
    fused: serde_json::Value,
    eps: serde_json::Value,
    facts: serde_json::Value,
    axis: serde_json::Value,
) -> serde_json::Value {
    let rows = serde_json::json!([
        {"request_id": "r1", "leg": "fused", "payload": fused.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-ep", "payload": eps.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-fact", "payload": facts.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-axis", "payload": axis.to_string(), "fired": 1}
    ]);
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what do I prefer?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": ""},
            "hop": {"operation": "select"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

/// The common case: one leg, well below every cap.
fn emit_doc(
    candidates: serde_json::Value,
    eps: serde_json::Value,
    facts: serde_json::Value,
    axis: serde_json::Value,
) -> serde_json::Value {
    emit_doc_full(
        fused(
            candidates,
            serde_json::json!({"keyword": 5}),
            serde_json::json!({}),
        ),
        eps,
        facts,
        axis,
    )
}

/// A `fused` scratch payload. `sizes` is what each leg contributed AFTER its
/// gate — a report, never a cap verdict — and `caps` is what the leg's own PAGE
/// reported: leg name → the limit it came back full on, absent when it came back
/// short. The two are deliberately independent here, because in production they
/// are, and the whole of GH #280 item 3's second round is that the second one is
/// the only one allowed to answer "was this capped?".
fn fused(
    candidates: serde_json::Value,
    sizes: serde_json::Value,
    caps: serde_json::Value,
) -> serde_json::Value {
    let mut leg_sizes = serde_json::json!({"keyword": 0, "semantic": 0, "graph": 0, "temporal": 0});
    for (k, v) in sizes.as_object().expect("sizes object") {
        leg_sizes[k] = v.clone();
    }
    serde_json::json!({
        "candidates": candidates,
        "legs_present": ["keyword"],
        "leg_sizes": leg_sizes,
        "leg_capped": caps,
        "semantic_degraded": true
    })
}

/// The JSON bundle the tier-1 emission carries in `system.memory.bundle`.
fn bundle(msgs: &[serde_json::Value]) -> serde_json::Value {
    let m = msgs
        .iter()
        .find(|m| m["header"]["route"] == "bundle")
        .expect("a tier-1 request emits a bundle message");
    let text = m["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("bundle text");
    serde_json::from_str(text).expect("bundle json")
}

/// The internal candidate records of the same emission (#296).
///
/// Two audiences, two slots: `system.memory.bundle` is the payload a model
/// reads and carries no row ids and no raw instants, `recall_diagnostic` is the
/// record. The store's own provenance — the id a closure names, the instant it
/// happened at — is a statement about rows, so it is asserted on the record;
/// the reader's half of the same answer (a day, a claim) is asserted on the
/// payload next to it. The two lists are built from one another and stay
/// parallel, which is what lets a test index into both.
fn record(msgs: &[serde_json::Value]) -> serde_json::Value {
    msgs.iter()
        .find(|m| m["header"]["route"] == "bundle")
        .expect("a tier-1 request emits a bundle message")["recall_diagnostic"]["candidates"]
        .clone()
}

/// The slot the candidate with this id occupies in the record — and therefore
/// in the payload beside it.
fn slot_of(records: &serde_json::Value, id: &str) -> usize {
    records
        .as_array()
        .expect("candidates")
        .iter()
        .position(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no candidate {id} in {records}"))
}

/// One fact hit, hydrated, with no axis page — the fallback path, which is the
/// smallest fixture that reaches the fact branch of `t1-emit`.
fn one_fact(row: serde_json::Value) -> serde_json::Value {
    let candidates = serde_json::json!([
        {"kind": "fact", "id": row["id"], "score": 0.09, "legs": ["keyword"]}
    ]);
    bundle(&emit(emit_doc(
        candidates,
        serde_json::json!([]),
        serde_json::json!([row]),
        serde_json::json!([]),
    )))
}

#[test]
fn a_fact_candidate_carries_the_stored_confidence() {
    // The column is written by the extractor, selected by the hydration and
    // selected again by the axis page — and then dropped when the candidate was
    // assembled. A reader that cannot see it has to treat a guess and a
    // certainty as the same statement.
    let mut row = fact(
        "f-1",
        "s-1",
        "person:example",
        "helix",
        "2026-01-05T09:00:00.000000Z",
    );
    row["confidence"] = serde_json::json!(95);

    let b = one_fact(row);
    assert_eq!(
        b["candidates"][0]["confidence"], 95,
        "the confidence the store holds reaches the candidate: {}",
        b["candidates"][0]
    );
}

#[test]
fn a_fact_without_a_confidence_column_carries_no_key() {
    // Absent-unless-known, the discipline `intent`, `superseded` and
    // `supersession_unknown` already follow: a candidate that knows nothing pays
    // no byte of the token budget for saying so.
    let mut row = fact(
        "f-1",
        "s-1",
        "person:example",
        "helix",
        "2026-01-05T09:00:00.000000Z",
    );
    row["confidence"] = serde_json::Value::Null;

    let b = one_fact(row);
    assert!(
        b["candidates"][0].get("confidence").is_none(),
        "an unknown confidence is absent, not null: {}",
        b["candidates"][0]
    );
}

// ---------------------------------------------------------------- item 3
// Whether the list in front of the reader is the whole answer is something only
// the producer knows. Twenty candidates look the same whether twenty is all
// there was or all that fit, and a model that cannot tell the two apart will
// happily answer "three times" off a list that was cut at twenty.

/// Twenty episodes, ranked, each with the hydration row it needs.
fn twenty() -> (serde_json::Value, serde_json::Value) {
    let mut cands = Vec::new();
    let mut eps = Vec::new();
    for i in 0..20 {
        let id = format!("e-{i:02}");
        cands.push(serde_json::json!(
            {"kind": "episode", "id": id, "score": 0.09, "legs": ["keyword"]}
        ));
        eps.push(episode(
            &id,
            "s-1",
            &format!("turn number {i}"),
            &format!("2026-01-{:02}T09:00:00.000000Z", i + 1),
        ));
    }
    (serde_json::json!(cands), serde_json::json!(eps))
}

#[test]
fn a_capped_result_set_says_so_and_says_why() {
    let (cands, eps) = twenty();
    let doc = emit_doc_full(
        fused(
            cands,
            serde_json::json!({"keyword": 20, "semantic": 20, "graph": 0, "temporal": 20}),
            serde_json::json!({"keyword": 20, "semantic": 20, "temporal": 20}),
        ),
        eps,
        serde_json::json!([]),
        serde_json::json!([]),
    );
    let b = bundle(&emit(doc));

    assert_eq!(
        b["complete"], false,
        "three legs came back at their cap — this list is a prefix, not the answer"
    );
    let reason = b["complete_reason"]
        .as_str()
        .expect("a capped bundle names what cut it");
    assert!(
        reason.contains("20") && reason.contains("cap"),
        "the reason names the cap and its size: {reason:?}"
    );
    assert!(
        reason.contains("possibly"),
        "an exactly-full page is ambiguous, and the sentence says so: {reason:?}"
    );
}

#[test]
fn an_uncapped_result_set_is_complete_with_no_reason() {
    // Four candidates out of legs that returned four: nothing was cut, and
    // saying so is what makes the capped case worth believing.
    let cands = serde_json::json!([
        {"kind": "episode", "id": "e-0", "score": 0.09, "legs": ["keyword"]},
        {"kind": "episode", "id": "e-1", "score": 0.08, "legs": ["keyword"]},
        {"kind": "episode", "id": "e-2", "score": 0.07, "legs": ["keyword"]},
        {"kind": "episode", "id": "e-3", "score": 0.06, "legs": ["semantic"]}
    ]);
    let eps = serde_json::json!([
        episode("e-0", "s-1", "one", "2026-01-01T09:00:00.000000Z"),
        episode("e-1", "s-1", "two", "2026-01-02T09:00:00.000000Z"),
        episode("e-2", "s-1", "three", "2026-01-03T09:00:00.000000Z"),
        episode("e-3", "s-1", "four", "2026-01-04T09:00:00.000000Z"),
    ]);
    let doc = emit_doc_full(
        fused(
            cands,
            serde_json::json!({"keyword": 3, "semantic": 1, "graph": 0, "temporal": 0}),
            serde_json::json!({}),
        ),
        eps,
        serde_json::json!([]),
        serde_json::json!([]),
    );
    let b = bundle(&emit(doc));

    assert_eq!(b["complete"], true, "nothing was cut");
    assert!(
        b.get("complete_reason").is_none(),
        "a complete bundle has no cut to name: {}",
        b["complete_reason"]
    );
}

// ---------------------------------------------------------------- item 4
// Validity travelled in one direction only: the bundle carried a DERIVED
// currency marker ("this was replaced by …") and threw away the two columns the
// store answers with. So a reader could not say when a statement stopped being
// true, and could not name its successor — the loop was open on the store's own
// side.

/// The successor: open, nothing closed it.
fn f_new() -> serde_json::Value {
    fact(
        "f-new",
        "s-2",
        "person:example",
        "helix",
        "2026-06-01T00:00:00Z",
    )
}

/// `f-old`, closed by a MATERIALISED closure — `expired_at` plus the name of
/// the fact that replaced it. This is the shape the projection follows: an
/// explicit `valid_until` deliberately names nobody (`span_successor`), so a
/// self-closed row stays its own answer and does not collapse.
fn f_old_materialised() -> serde_json::Value {
    let mut r = fact(
        "f-old",
        "s-1",
        "person:example",
        "vim",
        "2026-01-01T00:00:00Z",
    );
    r["expired_at"] = serde_json::json!("2026-06-01T00:00:00Z");
    r["superseded_by"] = serde_json::json!("f-new");
    r
}

/// The same row, plus the end it declares about itself — which is what a window
/// query renders, and what `valid_until` on a candidate has to reproduce.
fn f_old_self_closed() -> serde_json::Value {
    let mut r = f_old_materialised();
    r["valid_until"] = serde_json::json!("2026-06-01T00:00:00Z");
    r
}

/// The fused list nominates `f-old` alone; the axis page carries both versions,
/// which is what lets the projection walk the closure forward.
fn two_versions(old: serde_json::Value) -> serde_json::Value {
    let candidates = serde_json::json!([
        {"kind": "fact", "id": "f-old", "score": 0.09, "legs": ["keyword"]}
    ]);
    emit_doc(
        candidates,
        serde_json::json!([]),
        serde_json::json!([old]),
        serde_json::json!([old, f_new()]),
    )
}

fn with_window(mut doc: serde_json::Value, from: &str, to: &str) -> serde_json::Value {
    doc["header"]["context"]["recall_window_from"] = serde_json::json!(from);
    doc["header"]["context"]["recall_window_to"] = serde_json::json!(to);
    doc
}

#[test]
fn a_fact_says_what_closed_it_and_when() {
    // Point mode: the hit collapses onto the statement still standing, and what
    // travels is that statement's validity — open, with nobody named. `null` is
    // the honest answer here and an absent key would be a different claim.
    let msgs = emit(two_versions(f_old_materialised()));
    let cand = record(&msgs)[0].clone();

    assert_eq!(cand["id"], "f-new", "the hit collapsed onto its successor");
    assert!(
        cand.as_object()
            .expect("candidate object")
            .contains_key("valid_until"),
        "an open fact says it is open rather than staying silent: {cand}"
    );
    assert_eq!(
        cand["valid_until"],
        serde_json::Value::Null,
        "nothing closed `f-new`: {cand}"
    );
    assert!(
        cand.get("superseded_by").is_none(),
        "no successor, no key: {cand}"
    );
    // The reader's half of the same answer (#296): an open fact carries no end
    // at all, which is what "still true" looks like in a payload whose keys are
    // absent unless known.
    let b = bundle(&msgs);
    assert!(
        b["candidates"][0].get("until").is_none(),
        "an open fact names no end: {}",
        b["candidates"][0]
    );
}

#[test]
fn a_window_query_keeps_the_closed_version_and_names_its_successor() {
    // A window query does not collapse — the versions themselves are the answer
    // — so the closed row is a candidate of its own and carries both halves the
    // store holds about its end.
    let doc = with_window(
        two_versions(f_old_self_closed()),
        "2025-12-01T00:00:00Z",
        "2026-08-01T00:00:00Z",
    );
    let msgs = emit(doc);
    let records = record(&msgs);
    let slot = slot_of(&records, "f-old");
    let cand = &records[slot];

    assert_eq!(
        cand["valid_until"], "2026-06-01T00:00:00Z",
        "the end the store recorded: {cand}"
    );
    assert_eq!(
        cand["superseded_by"], "f-new",
        "and the name of what replaced it: {cand}"
    );
    // #296: the payload says the same end at the resolution a reader asks it
    // at. The successor's ID is a row name and stays in the record — what
    // replaced this claim is readable off the version standing next to it.
    let b = bundle(&msgs);
    assert_eq!(
        b["candidates"][slot]["until"], "2026-06-01",
        "the closed version names its end as a day: {}",
        b["candidates"][slot]
    );
}

// ------------------------------------------------ fix round 1, finding 1
// Reading the RAW `valid_until` column made a candidate contradict itself: the
// same fact carried `superseded_by: "f-new"`, a `span` that ended in June and a
// currency marker naming its replacement — next to `valid_until: null`, which
// says "still true". Three of those four are the store's answer; the fourth was
// a column that a materialised closure simply does not fill.

#[test]
fn a_materialised_closure_travels_as_the_end_it_is() {
    // Same shape as the point-mode test above, asked over a window so the closed
    // version survives as its own candidate and can be looked at directly.
    let doc = with_window(
        two_versions(f_old_materialised()),
        "2025-12-01T00:00:00Z",
        "2026-08-01T00:00:00Z",
    );
    let msgs = emit(doc);
    let records = record(&msgs);
    let slot = slot_of(&records, "f-old");
    let cand = &records[slot];

    assert_eq!(
        cand["valid_until"], "2026-06-01T00:00:00Z",
        "the closure instant, exactly as `span_end` reads it: {cand}"
    );
    assert_eq!(
        cand["superseded_by"], "f-new",
        "and the successor it was closed in favour of: {cand}"
    );
    // The point of the finding: no field of this candidate may say "open" while
    // its neighbours say "closed".
    assert_eq!(
        cand["valid_until"], cand["span"]["until"],
        "the two ends of one candidate agree: {cand}"
    );
    // And they still agree after #296 took the instants out of the payload —
    // the finding was about a candidate contradicting itself, and a day-grained
    // copy of the same contradiction would be the same bug.
    let b = bundle(&msgs);
    let payload_cand = &b["candidates"][slot];
    assert_eq!(
        payload_cand["until"], "2026-06-01",
        "the payload names the end as a day: {payload_cand}"
    );
    assert_eq!(
        payload_cand["until"], payload_cand["span"]["until"],
        "the two ends of one payload candidate agree too: {payload_cand}"
    );
}

// ------------------------------------------------ fix round 2, finding I2
// The same contradiction, on the branch the first round did not cover. When the
// axis page does not carry the hit — a small `MEMORY_TIER1_AXIS_LIMIT`, or a
// page that simply came back without the older version — window mode falls back
// to a candidate WITHOUT history rather than losing it, and that fallback built
// its span out of the RAW `valid_until` column while the candidate's own
// `valid_until` a few lines further down already read the closure. So the two
// halves of one candidate disagreed about the same closure again, in exactly the
// case where nothing else on the axis is around to contradict them.

/// Window mode, materialised closure, and an axis page that came back WITHOUT
/// the hit. This is the fallback branch: `fact_span` finds no chain containing
/// `f-old`, so the span is built from the hydration row alone.
#[test]
fn a_truncated_axis_page_still_spans_to_the_closure() {
    let candidates = serde_json::json!([
        {"kind": "fact", "id": "f-old", "score": 0.09, "legs": ["keyword"]}
    ]);
    let doc = with_window(
        emit_doc(
            candidates,
            serde_json::json!([]),
            serde_json::json!([f_old_materialised()]),
            // The page the axis select came back with: the successor only. The
            // hit itself fell off it, which is what a small AXIS_LIMIT does.
            serde_json::json!([f_new()]),
        ),
        "2025-12-01T00:00:00Z",
        "2026-08-01T00:00:00Z",
    );
    let msgs = emit(doc);
    let records = record(&msgs);
    let slot = slot_of(&records, "f-old");
    let cand = &records[slot];

    assert!(
        cand["history"].as_array().is_none_or(|h| h.is_empty()),
        "the fallback keeps the candidate WITHOUT history — that is the branch \
         under test, and a non-empty history means the axis page was not \
         truncated after all: {cand}"
    );
    assert_eq!(
        cand["valid_until"], "2026-06-01T00:00:00Z",
        "the closure the store holds, read the same way `span_end` reads it: {cand}"
    );
    assert_eq!(
        cand["span"]["until"], "2026-06-01T00:00:00Z",
        "and the fallback span ends at the same instant instead of at the empty \
         raw column: {cand}"
    );
    assert_eq!(
        cand["valid_until"], cand["span"]["until"],
        "the two ends of one candidate agree on the fallback branch too: {cand}"
    );

    // The reader's half, at the resolution the payload answers at. `[… -> open]`
    // beside a closed statement is the wrong answer this finding is about.
    let b = bundle(&msgs);
    let payload_cand = &b["candidates"][slot];
    assert_eq!(
        payload_cand["until"], "2026-06-01",
        "the payload names the end as a day: {payload_cand}"
    );
    assert_eq!(
        payload_cand["span"]["until"], "2026-06-01",
        "the payload span ends there too, rather than rendering `open`: {payload_cand}"
    );
}

// ------------------------------------------------ fix round 1, finding 2
// `leg_sizes` is a SUM taken after the audience gate, and it answered the cap
// question wrong in both directions. The verdict now comes off the pages
// themselves, and the two tests below are the two ways the sum lied.

#[test]
fn a_keyword_leg_of_two_short_pages_is_not_a_cap() {
    // The keyword leg is TWO searches with a limit each. 12 episodes and 8 facts
    // sum to exactly the limit and neither page was full — the store had nothing
    // more to give, and the old derivation called that a truncation.
    let cands = serde_json::json!([
        {"kind": "episode", "id": "e-0", "score": 0.09, "legs": ["keyword"]},
        {"kind": "episode", "id": "e-1", "score": 0.08, "legs": ["keyword"]}
    ]);
    let eps = serde_json::json!([
        episode("e-0", "s-1", "one", "2026-01-01T09:00:00.000000Z"),
        episode("e-1", "s-1", "two", "2026-01-02T09:00:00.000000Z"),
    ]);
    let doc = emit_doc_full(
        fused(
            cands,
            serde_json::json!({"keyword": 20}),
            serde_json::json!({}),
        ),
        eps,
        serde_json::json!([]),
        serde_json::json!([]),
    );
    let b = bundle(&emit(doc));

    assert_eq!(
        b["complete"], true,
        "12 + 8 is not a cap: {}",
        b["complete_reason"]
    );
}

#[test]
fn a_semantic_page_thinned_by_the_gate_is_still_a_cap() {
    // The other direction: the semantic leg came back full and then lost three
    // rows to the audience gate. Counting what survived says 17 and reports a
    // complete answer for a query whose evidence was demonstrably truncated.
    let cands = serde_json::json!([
        {"kind": "episode", "id": "e-0", "score": 0.09, "legs": ["semantic"]}
    ]);
    let eps = serde_json::json!([episode("e-0", "s-1", "one", "2026-01-01T09:00:00.000000Z")]);
    let doc = emit_doc_full(
        fused(
            cands,
            serde_json::json!({"semantic": 17}),
            serde_json::json!({"semantic": 20}),
        ),
        eps,
        serde_json::json!([]),
        serde_json::json!([]),
    );
    let b = bundle(&emit(doc));

    assert_eq!(b["complete"], false, "a full page stays a full page");
    let reason = b["complete_reason"].as_str().expect("a reason");
    assert!(
        reason.contains("semantic") && reason.contains("20"),
        "the reason names the leg and the limit its page came back on: {reason:?}"
    );
}

// The two above pin the DERIVATION. The two below pin the MEASUREMENT, because a
// flag nobody sets is a flag that reports `complete: true` forever.

/// The document a leg phase receives: a store result set, nothing else.
fn leg_doc(cid: &str, rows: serde_json::Value) -> serde_json::Value {
    // #418: the legs that need no query vector are ONE message and one reply,
    // so a leg is addressed by its `tool_call_id` in that bundle rather than by
    // a phase of its own. Each leg body is unchanged.
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-fan", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what do I prefer?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": "",
                        "audience_now": ["u1"], "channel": "chat"},
            "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": cid,
                      "text": rows.to_string()}],
        "results": [{"tool_call_id": cid, "operation": "search",
                     "rows_affected": 1, "duration_ms": 0}]
    })
}

/// One leg out of the single `legs` row the fan parks.
fn fan_leg(msgs: &[serde_json::Value], leg: &str) -> serde_json::Value {
    scratch_payload(msgs, "legs")[leg].clone()
}

/// Every store op an emission carries, in call order.
///
/// Since GH #418 a recall hop puts N ops into ONE message (#295), so an op is a
/// CALL of a message rather than a message of its own.
fn ops_of(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for m in msgs {
        let Some(turns) = m["messages"].as_array() else {
            continue;
        };
        for t in turns {
            let Some(text) = t["text"].as_str() else {
                continue;
            };
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                out.push(v);
            }
        }
    }
    out
}

/// The payload a leg wrote into `recall_scratch` under `leg`.
fn scratch_payload(msgs: &[serde_json::Value], leg: &str) -> serde_json::Value {
    for args in ops_of(msgs) {
        if args["operation"] == "insert" && args["row"]["leg"] == leg {
            let payload = args["row"]["payload"].as_str().expect("payload string");
            return serde_json::from_str(payload).expect("payload json");
        }
    }
    panic!("no scratch insert for leg {leg}");
}

#[test]
fn a_keyword_page_records_the_cap_it_came_back_on() {
    // 20 rows arrive, one of them invisible to this round. The leg's rank list
    // is 19 long and the page was still full — which is exactly the pair the
    // summed size could not express.
    let mut rows = Vec::new();
    for i in 0..20 {
        rows.push(serde_json::json!({
            "id": format!("e-{i:02}"), "session_id": "s-1", "content": "x",
            "recorded_at": "2026-01-01T09:00:00.000000Z", "channel": "chat",
            "audience_set": if i == 0 { serde_json::json!(["someone-else"]) }
                            else { serde_json::json!(["u1"]) }
        }));
    }
    let leg = fan_leg(
        &emit(leg_doc("r-fan-kw-ep", serde_json::json!(rows))),
        "kw-ep",
    );

    assert_eq!(
        leg["capped_at"], 20,
        "the cap is measured on the page, not on what survived the gate: {leg}"
    );
    assert_eq!(
        leg["hits"].as_array().map(Vec::len),
        Some(19),
        "and the gate still removed its row: {leg}"
    );
}

#[test]
fn a_short_page_writes_the_bare_rank_list_it_always_wrote() {
    // One row out of a page of twenty is the whole answer, and a leg with
    // nothing more to say says nothing more: the payload stays the plain rank
    // list. That is not cosmetic — GH #244 pins these payloads as rank lists,
    // and the wrapper below exists only for the case that has news.
    let rows = serde_json::json!([
        {"id": "e-0", "session_id": "s-1", "content": "x",
         "recorded_at": "2026-01-01T09:00:00.000000Z", "channel": "chat",
         "audience_set": ["u1"]}
    ]);
    let leg = fan_leg(&emit(leg_doc("r-fan-kw-ep", rows)), "kw-ep");

    assert_eq!(
        leg,
        serde_json::json!([{"kind": "episode", "id": "e-0"}]),
        "an uncapped leg is byte for byte what it was before #280: {leg}"
    );
    assert_eq!(
        leg.get("capped_at"),
        None,
        "no wrapper, so no cap key either: {leg}"
    );
}

#[test]
fn the_fused_payload_carries_the_caps_the_pages_reported() {
    // The seam between the two halves above: the fusion is where the pages'
    // verdicts are folded onto the four legs the bundle speaks of, and this is
    // the fixture the old derivation got wrong in BOTH directions at once —
    // keyword contributes one surviving hit off a full page, temporal
    // contributes none off a full page, and the gated sizes say 1 and 0.
    let leg = |leg: &str, payload: serde_json::Value| {
        serde_json::json!({"request_id": "r1", "leg": leg,
                           "payload": payload.to_string(), "fired": 0})
    };
    let rows = serde_json::json!([
        // #418: the five legs of the fan are ONE parked row, because the fan
        // that produced them is one message. The uncapped ones arrive in the
        // shape they have always had — a bare rank list — which is also what the
        // fusion has to keep reading.
        leg(
            "legs",
            serde_json::json!({
                "kw-ep": {"hits": [{"kind": "episode", "id": "e1"}], "capped_at": 20},
                "kw-fact": [],
                "temporal": {"hits": [], "capped_at": 200},
                "beliefs": [], "anchors": [], "axis": {},
                "model": {"model_id": "m", "dim": 1024}
            })
        ),
        // The semantic leg is parked on its own: it is computed one hop later,
        // out of the `similar` the rendezvous fired.
        leg("sem", serde_json::json!([])),
    ]);
    // The fusion runs inside the reply of `t1-legs`, out of the parked row plus
    // the walk and the companion that arrived with it.
    let doc = serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-legs", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what do I prefer?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": "",
                        "audience_now": ["u1"], "channel": "chat"},
            "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}
        },
        "messages": [{"origin": "tool", "type": "tool_result",
                      "id": "r-legs-read", "text": rows.to_string()}],
        "results": [{"tool_call_id": "r-legs-read", "operation": "select",
                     "rows_affected": 2, "duration_ms": 0}]
    });
    let f = scratch_payload(&emit(doc), "fused");

    assert_eq!(
        f["leg_capped"],
        serde_json::json!({"keyword": 20, "temporal": 200}),
        "either keyword page being full makes the leg capped, the temporal leg \
         reports the AXIS_LIMIT it asked with, and a short leg is absent: {f}"
    );
    assert_eq!(
        f["leg_sizes"],
        serde_json::json!({"keyword": 1, "semantic": 0, "graph": 0, "temporal": 0}),
        "while the sizes — the number this used to be derived from — say 1 and 0: {f}"
    );
}
