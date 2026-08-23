//! Statement identity W4 -- the extractor closes, in the turn, what it can SEE
//! (GitHub #13, ruling Q2 option C with its own guard rail).
//!
//! W3 gave the store its producer: the nightly judge reads the axes of a memory
//! with the whole store in front of it and closes what was replaced. That is the
//! load-bearing mechanism and it runs at 03:00Z. W4 is the other half of the same
//! ruling -- the north star of this memory is "resolve the conflict IN THE TURN",
//! and the extractor is the only party that is present in the turn.
//!
//! So the extraction lane gained three things, and exactly three:
//!
//! 1. its vocabulary window (P1: the axes this memory already carries) also
//!    carried the OPEN STATEMENTS of those axes, hard budgeted, because an
//!    extractor that cannot see a value cannot replace it;
//! 2. a fact may carry `replaces`, naming a statement FROM THAT WINDOW;
//! 3. a valid `replaces` becomes exactly the W3 write form -- `expired_at`,
//!    `superseded_by`, `closure_source` -- attributed to the batch.
//!
//! The guard rail is ruling Q2 rail 3 and it is mechanical, not advisory: a
//! `replaces` naming anything the window did not contain is discarded and logged.
//! The extractor may only close what it was provably shown. The night validates
//! the rest -- it sees recently extract-closed statements on its axis pages and
//! may contradict them, which is a revert in the W3 shape.
//!
//! **Point 1 is retired (wave 5, GitHub #298).** Per-turn extraction has no batch
//! prompt and no vocabulary leg, so nothing builds a window page any more and the
//! cases that pinned that leg -- the recency page, the whole-axis read, the axis
//! hint, the parked window and its caps -- are gone with the mechanism. Points 2
//! and 3 stand and are what this file still pins:
//!
//! * the PRODUCER of `replaces` is now the front model's annotation block, which
//!   reaches `validate()` through the inline ingress and nowhere else;
//! * the CONSUMER is unchanged -- the apply phase reads the `window` row back and
//!   applies guard rail 3 against it, so a batch that meets no window closes
//!   nothing at all rather than closing something unguarded. The direction guard
//!   on top of it (GH #71) lives in `f7_closure_direction.rs`.
//!
//! Everything here runs the REAL `params.script_inline` of `extract-glue` and
//! `dream-glue` against injected store replies, so no model is called and nothing
//! costs anything. Whether an extractor USES `replaces` well is a model property
//! and belongs to the paid scenario C15.

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

/// Run a real script with a real stdin document; returns (messages, stderr).
fn run(script: &str, doc: serde_json::Value) -> (Vec<serde_json::Value>, String) {
    let out = run_script_on_stdin(script, &meclaw_testing::code_stdin(&doc).to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "script exited non-zero: {err}");
    let msgs = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (msgs, err)
}

fn emit_x(doc: serde_json::Value) -> Vec<serde_json::Value> {
    run(&script_of(EXTRACT_CONFIG), doc).0
}

const BATCH: &str = "b1";

/// One store reply of the extraction lane as the edge delivers it.
fn x_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": phase, "batch_id": BATCH},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// The op the lane parks under one scratch kind, if it emitted one.
fn parked(msgs: &[serde_json::Value], kind: &str) -> Option<serde_json::Value> {
    msgs.iter()
        .map(args_of)
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == kind)
        .map(|a| {
            serde_json::from_str(a["row"]["payload"].as_str().expect("payload"))
                .expect("parked payload")
        })
}

/// The room and the cast an annotation block has to arrive with (#244), and the
/// turn it speaks for. Per-turn extraction (#298) made the INLINE ingress the
/// only producer that reaches `validate()`, so the three `replaces` cases below
/// enter through it instead of through the retired `parse` phase.
const CHANNEL: &str = "c-w4";
const AUDIENCE: &str = r#"["member:user","agent:assistant"]"#;

/// One annotation block as the `inline-extraction` port edge delivers it.
fn annotation(text: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "inline", "mem_phase": "inline",
                        "session_id": "s2", "channel": CHANNEL, "audience_set": AUDIENCE},
            "hop": {}
        },
        "messages": [{"origin": "user", "type": "text", "text": text}]
    })
}

const ONE_REPLACEMENT: &str = r#"{"episode_id":"e9","facts":[
  {"episode_id":"e9","subject":"user","predicate":"favorite_editor",
   "claim":"favorite editor is zed","fact_kind":"world","replaces":"f1","confidence":90}]}"#;

#[test]
fn the_replaces_signal_survives_the_validator_into_the_staged_payload() {
    // The lane is a state machine over a stateless cell: what the ingress does not
    // carry into `scratch` is gone by the time the facts are written. The signal
    // travels with its fact, not next to it -- one fact replaces one statement,
    // and a second list would have to be re-joined to the first.
    let msgs = emit_x(annotation(ONE_REPLACEMENT));
    let staged = parked(&msgs, "payload").expect("the payload is staged");
    assert_eq!(staged["facts"][0]["replaces"], "f1");
    assert_eq!(
        staged["facts"][0]["claim"], "favorite editor is zed",
        "and the fact itself is untouched by the new field"
    );
}

#[test]
fn a_fact_without_the_field_stages_an_empty_signal() {
    // `replaces` is OPTIONAL, and the ordinary answer shape has to keep working
    // unchanged: the overwhelming majority of extracted facts replace nothing.
    let msgs = emit_x(annotation(
        r#"{"episode_id":"e9","facts":[{"episode_id":"e9","subject":"user",
                      "predicate":"has_child",
                      "claim":"has a child named ben","fact_kind":"world"}]}"#,
    ));
    let staged = parked(&msgs, "payload").expect("payload");
    assert_eq!(staged["facts"][0]["replaces"], "");
}

#[test]
fn a_replaces_that_is_not_a_plain_reference_is_flattened_not_trusted() {
    // A model that answers with an object, a list or a number must not put a
    // structure into the `where` of a store op. The validator drops everything
    // it cannot read as ONE reference -- the same discipline every other field of
    // this validator follows, and the reason bad items never abort a block.
    let msgs = emit_x(annotation(
        r#"{"episode_id":"e9","facts":[
                     {"episode_id":"e9","subject":"user","predicate":"favorite_editor",
                      "claim":"favorite editor is zed","fact_kind":"world",
                      "replaces":{"id":"f1"}},
                     {"episode_id":"e9","subject":"user","predicate":"lives_in",
                      "claim":"lives in zone-b","fact_kind":"world",
                      "replaces":["f1","f2"]}]}"#,
    ));
    let staged = parked(&msgs, "payload").expect("payload");
    assert_eq!(staged["facts"][0]["replaces"], "");
    assert_eq!(staged["facts"][1]["replaces"], "");
}

/// The window as the vocabulary phase parks it: one axis, one open statement.
fn parked_window() -> String {
    serde_json::json!([{
        "subject": "user", "predicate": "favorite_editor",
        "statements": [
            {"id": "f1", "claim": "favorite editor is helix", "since": "2026-03-05T00:00:00Z"}
        ]
    }])
    .to_string()
}

/// One staged fact in the shape `parse` leaves it in.
fn staged_fact(predicate: &str, claim: &str, replaces: &str) -> serde_json::Value {
    serde_json::json!({
        "episode_id": "e9", "subject": "user", "predicate": predicate, "claim": claim,
        "fact_kind": "world", "valid_from": "", "valid_until": null, "confidence": 90,
        "replaces": replaces, "claim_hash": claim.replace(' ', "_")
    })
}

/// Run the apply phase over a staged payload, with or without a parked window.
fn apply(facts: serde_json::Value, window: Option<String>) -> (Vec<serde_json::Value>, String) {
    let payload = serde_json::json!({"facts": facts, "entities": [], "edges": []});
    let episodes = serde_json::json!({"e9": {"happened_at": "2026-06-05T00:00:00Z",
                                             "session_id": "s2"}});
    let mut rows = vec![
        serde_json::json!({"key": BATCH, "kind": "payload", "payload": payload.to_string()}),
        serde_json::json!({"key": BATCH, "kind": "known", "payload": "[]"}),
        serde_json::json!({"key": BATCH, "kind": "episodes",
                           "payload": episodes.to_string()}),
    ];
    if let Some(w) = window {
        rows.push(serde_json::json!({"key": BATCH, "kind": "window", "payload": w}));
    }
    run(
        &script_of(EXTRACT_CONFIG),
        x_reply("apply", "select", serde_json::Value::Array(rows)),
    )
}

/// Every `facts` op the apply phase emitted, in order.
fn fact_ops(msgs: &[serde_json::Value], operation: &str) -> Vec<serde_json::Value> {
    msgs.iter()
        .map(args_of)
        .filter(|a| a["table"] == "facts" && a["operation"] == operation)
        .collect()
}

#[test]
fn a_valid_replaces_becomes_exactly_the_write_form_the_judge_uses() {
    // The whole point of the handover: a closure is a closure. Three columns on
    // the closed statement, one `update` on `facts`, and the read path
    // (span_end / span_successor / chain_target) cannot tell the two producers
    // apart -- which is what keeps the rendering, the currency marker and the
    // withdrawal identical for both.
    let (msgs, _) = apply(
        serde_json::json!([staged_fact(
            "favorite_editor",
            "favorite editor is zed",
            "f1"
        )]),
        Some(parked_window()),
    );
    let inserted = fact_ops(&msgs, "insert");
    assert_eq!(inserted.len(), 1, "the new fact is written");
    let new_id = inserted[0]["row"]["id"].as_str().expect("id").to_string();
    let closures = fact_ops(&msgs, "update");
    assert_eq!(closures.len(), 1, "one replaces, one closure: {closures:?}");
    assert_eq!(
        closures[0]["set"],
        serde_json::json!({"expired_at": "2026-06-05T00:00:00Z",
                           "superseded_by": new_id,
                           "closure_source": "extract:b1"}),
        "the instant is the START of the replacing statement (its event time, not \
         the ingest), the successor is the fact just written, and the attribution \
         names the author AND the batch whose scratch row carries the reason"
    );
    assert_eq!(
        closures[0]["where"],
        serde_json::json!({"id": "f1", "canonical_subject": "user",
                           "canonical_predicate": "favorite_editor",
                           "expired_at": {"is_null": true}}),
        "the axis echo comes from the WINDOW rather than from the model, so a row \
         that moved axis since the prompt is not closed by a stale reference -- \
         and `expired_at is_null` means an extractor closure never overwrites a \
         judged one"
    );
}

#[test]
fn a_replaces_outside_the_window_is_discarded_and_says_so() {
    // Ruling Q2 guard rail 3, and it is the only thing this package really owns:
    // the extractor writes WITHOUT the gate, so what it may close is bounded by
    // what it was provably shown. A well-formed id of a real statement is
    // discarded here for one reason only -- it was not in the window.
    let (msgs, err) = apply(
        serde_json::json!([staged_fact(
            "favorite_editor",
            "favorite editor is zed",
            "somewhere-else"
        )]),
        Some(parked_window()),
    );
    assert_eq!(
        fact_ops(&msgs, "insert").len(),
        1,
        "the FACT is still written"
    );
    assert!(
        fact_ops(&msgs, "update").is_empty(),
        "a closure on an unseen statement was written: {msgs:?}"
    );
    assert!(
        err.contains("window"),
        "a discarded closure has to be visible somewhere, got stderr {err:?}"
    );
}

#[test]
fn a_batch_that_was_shown_no_window_closes_nothing_at_all() {
    // The inline ingress and every batch from before this package. There is no
    // window row, so there is no proof, so there is no closure -- and the facts
    // are written exactly as they always were.
    let (msgs, _) = apply(
        serde_json::json!([staged_fact(
            "favorite_editor",
            "favorite editor is zed",
            "f1"
        )]),
        None,
    );
    assert_eq!(fact_ops(&msgs, "insert").len(), 1);
    assert!(fact_ops(&msgs, "update").is_empty());
}

#[test]
fn the_enumeration_case_closes_nothing_because_nothing_asked_it_to() {
    // The second son, which is the case the whole track exists for. Whether the
    // MODEL refuses to replace here is a prompt property and belongs to the paid
    // run; what is mechanical -- and what this pins -- is that no closure exists
    // without an explicit `replaces`. The lane never infers one from the data
    // again (that arithmetic is exactly what W2 removed).
    let (msgs, _) = apply(
        serde_json::json!([
            staged_fact("has_child", "has a child named ben", ""),
            staged_fact("favorite_editor", "favorite editor is helix", "")
        ]),
        Some(parked_window()),
    );
    assert_eq!(fact_ops(&msgs, "insert").len(), 2);
    assert!(
        fact_ops(&msgs, "update").is_empty(),
        "the lane closed a statement nobody named: {msgs:?}"
    );
}

#[test]
fn the_extraction_lane_may_only_close_never_delete_and_never_rewrite() {
    // Ruling Q2 guard rail 1, as a property of every op the apply phase emits --
    // the same assertion W3 makes about the judge. `claim`, `valid_from` and
    // `valid_until` of the closed statement are not this lane's to touch: that is
    // what keeps the whole closure revertible.
    let (msgs, _) = apply(
        serde_json::json!([staged_fact(
            "favorite_editor",
            "favorite editor is zed",
            "f1"
        )]),
        Some(parked_window()),
    );
    for op in msgs.iter().map(args_of) {
        assert_ne!(op["operation"], "delete", "the lane deleted: {op}");
        if op["operation"] == "update" && op["table"] == "facts" {
            for k in op["set"].as_object().expect("set").keys() {
                assert!(
                    ["expired_at", "superseded_by", "closure_source"].contains(&k.as_str()),
                    "the extractor wrote {k} -- the closure columns are its whole \
                     write surface"
                );
            }
        }
    }
}

#[test]
fn the_batch_receipt_carries_the_reason_of_every_closure() {
    // Ruling Q2 guard rail 2, in this lane's own currency. The night folds its
    // reasons into `consolidation_log` because that is where a run quits; the
    // extraction lane quits a batch in `scratch` under the batch key -- which is
    // exactly the key `closure_source` names, so a closed fact reads back to the
    // turn that replaced it, and one batch reverts with one `where`.
    let (msgs, _) = apply(
        serde_json::json!([staged_fact(
            "favorite_editor",
            "favorite editor is zed",
            "f1"
        )]),
        Some(parked_window()),
    );
    let receipt = parked(&msgs, "extract-closures").expect("the batch receipt row");
    let entry = &receipt[0];
    assert_eq!(entry["id"], "f1");
    assert_eq!(entry["subject"], "user");
    assert_eq!(entry["predicate"], "favorite_editor");
    assert_eq!(
        entry["replaced_claim"], "favorite editor is helix",
        "the value that stopped being current, as the window showed it"
    );
    assert_eq!(
        entry["claim"], "favorite editor is zed",
        "and the value that replaced it -- the two together ARE the reason, which \
         is why this lane needs no prose one: the extractor's justification is the \
         turn it just read, and the episode below names it"
    );
    assert_eq!(entry["episode_id"], "e9");
    let closure = fact_ops(&msgs, "update")[0].clone();
    assert_eq!(entry["superseded_by"], closure["set"]["superseded_by"]);
    assert_eq!(entry["expired_at"], closure["set"]["expired_at"]);
}

#[test]
fn a_batch_that_closed_nothing_writes_no_receipt_row() {
    // The receipt grows a row only when there is something to receipt, the same
    // way the night's payload grows a key only then.
    let (msgs, _) = apply(
        serde_json::json!([staged_fact("has_child", "has a child named ben", "")]),
        Some(parked_window()),
    );
    assert!(parked(&msgs, "extract-closures").is_none());
}

#[test]
fn a_fact_the_dedup_drops_closes_nothing() {
    // The re-delivery case: the same turn extracted twice writes no second fact,
    // so there is no successor -- and a closure whose `superseded_by` names a row
    // that was never written would point into nothing. No new statement, no
    // replacement.
    let payload = serde_json::json!({
        "facts": [staged_fact("favorite_editor", "favorite editor is zed", "f1")],
        "entities": [], "edges": []
    });
    let episodes = serde_json::json!({"e9": {"happened_at": "2026-06-05T00:00:00Z",
                                             "session_id": "s2"}});
    let rows = serde_json::json!([
        {"key": BATCH, "kind": "payload", "payload": payload.to_string()},
        {"key": BATCH, "kind": "known", "payload": "[\"e9:favorite_editor_is_zed\"]"},
        {"key": BATCH, "kind": "episodes", "payload": episodes.to_string()},
        {"key": BATCH, "kind": "window", "payload": parked_window()}
    ]);
    let (msgs, _) = run(&script_of(EXTRACT_CONFIG), x_reply("apply", "select", rows));
    assert!(fact_ops(&msgs, "insert").is_empty());
    assert!(fact_ops(&msgs, "update").is_empty());
}

// --------------------------------------------------------- the night validates
//
// Ruling Q2 guard rail 3 has a second half: the extractor writes WITHOUT the
// gate, so the nightly round has to be able to look at what it closed and
// contradict it. The axis page W3 built already carries everything needed for
// that judgement -- it only has to include the statements a RECENT extract
// closure ended, and the contradiction is a revert in the W3 shape (clear the
// attribution, let the same night's re-derive withdraw the columns).

const RUN: &str = "r1";
const TO: &str = "2026-08-12T03:00:00Z";

/// One store reply of the dream lane as the edge delivers it.
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
fn scan_row(id: &str, predicate: &str, claim: &str, from: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "subject": "user", "canonical_subject": "user",
        "predicate": predicate, "canonical_predicate": predicate,
        "claim": claim, "canonical_claim": claim,
        "valid_from": from, "recorded_at": from,
        "expired_at": null, "closure_source": null
    })
}

/// The same row, closed by somebody.
fn closed_row(
    id: &str,
    predicate: &str,
    claim: &str,
    from: &str,
    source: &str,
) -> serde_json::Value {
    let mut row = scan_row(id, predicate, claim, from);
    row["expired_at"] = serde_json::json!("2026-08-11T09:00:00Z");
    row["closure_source"] = serde_json::json!(source);
    row
}

/// The inventory the scan parks for the meeting point.
fn parked_scan(rows: serde_json::Value) -> serde_json::Value {
    let msgs = emit_d(d_reply("canon-scan", "select", rows));
    let args = args_of(&msgs[0]);
    assert_eq!(args["row"]["kind"], "canon-scan");
    serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("scan payload")
}

#[test]
fn the_nightly_scan_reaches_back_over_the_recent_extract_closures() {
    // One select, one column, one filter: `or_null` is "expired_at >= cutoff OR
    // expired_at is null", i.e. the open statements PLUS everything closed
    // recently. Widening the scan is what lets the round see a producer that
    // wrote without it -- and the lookback is what keeps the page bounded on a
    // memory that has been closing statements for years.
    // The scan hangs on the park of the judged cardinality since W5 (#13, ruling
    // Q3) and on the identity questions' own closed read since GH #73; the
    // widening this pin is about is unaffected by which door opens it. What GH
    // #73 does NOT do is widen this page further: the review lane keeps its event
    // clock, and the identity questions got a read of their own precisely because
    // that clock is the wrong bound for their question (`f10`).
    let msgs = emit_d(d_reply("canon-closed", "select", serde_json::json!([])));
    let args = args_of(&msgs[0]);
    assert_eq!(args["phase"], serde_json::Value::Null);
    assert_eq!(
        args["where"],
        serde_json::json!({"expired_at": {"or_null": {"gte": "2026-08-05T03:00:00Z"}}}),
        "the cutoff is derived from `delta_to`, the only clock this lane has, so a \
         re-run reads the same window back"
    );
    let cols = args["columns"].as_array().expect("columns").clone();
    assert!(
        cols.iter().any(|c| c == "closure_source"),
        "without the attribution the round cannot tell WHOSE closure it is looking \
         at, and it may only contradict the extractor: {}",
        args["columns"]
    );
    assert!(cols.iter().any(|c| c == "expired_at"));
}

#[test]
fn an_axis_the_extractor_closed_is_put_back_in_front_of_the_judge() {
    // The case the whole second half exists for: the extractor decided in the
    // turn, one statement is closed, one is open -- and W3's rule ("more than one
    // OPEN statement") would never look at this axis again. The closure would
    // stand forever without anyone with the whole store in front of them ever
    // having seen it.
    let rows = serde_json::json!([
        scan_row(
            "e2",
            "favorite_editor",
            "favorite editor is zed",
            "2026-08-10T00:00:00Z"
        ),
        closed_row(
            "e1",
            "favorite_editor",
            "favorite editor is helix",
            "2026-03-05T00:00:00Z",
            "extract:b1"
        )
    ]);
    let axes = parked_scan(rows)["axes"].as_array().expect("axes").clone();
    assert_eq!(axes.len(), 1, "the axis is offered: {axes:?}");
    assert_eq!(axes[0]["predicate"], "favorite_editor");
    assert_eq!(
        axes[0]["statements"],
        serde_json::json!([
            {"id": "e2", "claim": "favorite editor is zed", "since": "2026-08-10T00:00:00Z",
             "last_asserted": "2026-08-10T00:00:00Z", "assertions": 1}
        ]),
        "the open side is unchanged -- W3's shape, not a second one"
    );
    assert_eq!(
        axes[0]["closed"],
        serde_json::json!([
            {"id": "e1", "claim": "favorite editor is helix", "since": "2026-03-05T00:00:00Z",
             "ended_at": "2026-08-11T09:00:00Z", "closed_by": "extract:b1"}
        ]),
        "and the closed side names its author, because the revert has to echo it"
    );
}

#[test]
fn a_closure_the_night_itself_wrote_is_not_offered_for_review() {
    // The round may contradict the EXTRACTOR, never a judgement (its own or an
    // earlier night's): that would make "only close, never delete" (ruling Q2
    // rail 1) revocable by the same party that wrote it. The arithmetic residue
    // is out for the same reason -- ruling Q4 already withdraws that one, and it
    // needs no opinion.
    let rows = serde_json::json!([
        scan_row(
            "e2",
            "favorite_editor",
            "favorite editor is zed",
            "2026-08-10T00:00:00Z"
        ),
        closed_row(
            "e1",
            "favorite_editor",
            "favorite editor is helix",
            "2026-03-05T00:00:00Z",
            "judge:r0"
        ),
        scan_row("h2", "lives_in", "lives in zone-b", "2026-08-10T00:00:00Z"),
        closed_row(
            "h1",
            "lives_in",
            "lives in zone-a",
            "2026-01-06T00:00:00Z",
            ""
        )
    ]);
    let axes = parked_scan(rows)["axes"].as_array().expect("axes").clone();
    assert!(
        axes.is_empty(),
        "a closure that is not the extractor's was put up for review: {axes:?}"
    );
}

#[test]
fn a_night_without_extract_closures_offers_exactly_what_w3_offered() {
    // The additive proof. The `closed` key exists only when there is something
    // closed to show, so every night on a store the extractor never closed on
    // builds byte for byte the payload it built before this package -- and the
    // W3 refusals (one open statement, a bucket bigger than the page) are
    // untouched.
    let mut rows = vec![
        scan_row(
            "e1",
            "favorite_editor",
            "favorite editor is helix",
            "2026-03-05T00:00:00Z",
        ),
        scan_row(
            "e2",
            "favorite_editor",
            "favorite editor is vscode",
            "2026-06-05T00:00:00Z",
        ),
        scan_row("h1", "lives_in", "lives in zone-a", "2026-01-06T00:00:00Z"),
    ];
    for i in 0..7 {
        rows.push(scan_row(
            &format!("p{i}"),
            "planned_activity",
            &format!("plan number {i}"),
            &format!("2026-04-0{}T00:00:00Z", i + 1),
        ));
    }
    let axes = parked_scan(serde_json::Value::Array(rows))["axes"]
        .as_array()
        .expect("axes")
        .clone();
    assert_eq!(
        axes,
        vec![serde_json::json!({
            "subject": "user", "predicate": "favorite_editor",
            "statements": [
                {"id": "e1", "claim": "favorite editor is helix",
                 "since": "2026-03-05T00:00:00Z", "last_asserted": "2026-03-05T00:00:00Z",
                 "assertions": 1},
                {"id": "e2", "claim": "favorite editor is vscode",
                 "since": "2026-06-05T00:00:00Z", "last_asserted": "2026-06-05T00:00:00Z",
                 "assertions": 1}
            ]
        })],
        "the W3 payload, unchanged: no `closed` key, one open axis, the single \
         statement and the oversized bucket both refused"
    );
}

#[test]
fn a_closed_statement_is_no_longer_part_of_the_identity_questions() {
    // The widened scan feeds THREE sections, and only the third one asked for it.
    // A predicate whose every value was closed must not reappear in the alias
    // inventory, or the round would spend its budget re-judging the names of
    // statements nobody holds any more.
    let rows = serde_json::json!([
        scan_row(
            "e2",
            "favorite_editor",
            "favorite editor is zed",
            "2026-08-10T00:00:00Z"
        ),
        closed_row(
            "g1",
            "Lieblingseditor",
            "favorite editor is helix",
            "2026-03-05T00:00:00Z",
            "extract:b1"
        )
    ]);
    let parked = parked_scan(rows);
    assert!(
        parked["predicates"].get("Lieblingseditor").is_none(),
        "a closed statement was offered as an identity question: {}",
        parked["predicates"]
    );
    assert!(parked["predicates"].get("favorite_editor").is_some());
}

/// The parked halves, as the meeting-point select hands them back.
fn meeting_point(axes: serde_json::Value) -> serde_json::Value {
    let scan = serde_json::json!({"predicates": {"favorite_editor": ["user"]},
                                  "context": {}, "axes": axes});
    serde_json::json!([
        {"key": RUN, "kind": "canon-scan", "payload": scan.to_string()},
        {"key": RUN, "kind": "canon-pairs", "payload": "[]"}
    ])
}

/// One axis carrying an open statement and the one the extractor closed.
fn reviewed_axis() -> serde_json::Value {
    serde_json::json!([{
        "subject": "user", "predicate": "favorite_editor",
        "statements": [
            {"id": "e2", "claim": "favorite editor is zed", "since": "2026-08-10T00:00:00Z",
             "last_asserted": "2026-08-10T00:00:00Z", "assertions": 1}
        ],
        "closed": [
            {"id": "e1", "claim": "favorite editor is helix", "since": "2026-03-05T00:00:00Z",
             "ended_at": "2026-08-11T09:00:00Z", "closed_by": "extract:b1"}
        ]
    }])
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

/// Every `facts` update a judgement produced, in order.
fn judged_updates(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    msgs.iter()
        .map(args_of)
        .filter(|a| a["operation"] == "update" && a["table"] == "facts")
        .collect()
}

#[test]
fn the_review_travels_in_the_same_single_call() {
    // The P5 ruling stands and W4 does not spend a second call on it: the closure
    // the extractor wrote is reviewed inside the axis section that already
    // exists, by a judge that is already reading that axis.
    let msgs = emit_d(d_reply(
        "canon-ask",
        "select",
        meeting_point(reviewed_axis()),
    ));
    assert_eq!(msgs.len(), 1, "one call a night, not one per question");
    assert_eq!(msgs[0]["header"]["route"], "judge");
    let payload: serde_json::Value =
        serde_json::from_str(msgs[0]["messages"][0]["text"].as_str().expect("payload"))
            .expect("payload json");
    assert_eq!(
        payload["axes"],
        reviewed_axis(),
        "the closed side reaches the judge exactly as the scan parked it"
    );
    let text = msgs[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");
    assert!(
        text.contains("reopenings"),
        "the answer shape has to name the contradiction or nothing can come back"
    );
    assert!(
        text.contains("closed_by"),
        "the attribution is copied back by the judge, so it has to be asked for"
    );
}

#[test]
fn a_contradiction_clears_exactly_the_attribution_and_nothing_else() {
    // The revert the W3 receipt describes, applied to the other producer: there
    // is no closure table, the attribution is a column, so clearing it plus the
    // re-derive of this very night withdraws `expired_at` and `superseded_by` on
    // their own. The judge does not write those two back -- doing that here would
    // be a second source of truth for a value the chain derives.
    let msgs = emit_d(judgement(
        r#"{"reopenings":[{"subject":"user","predicate":"favorite_editor","statement":"e1",
                           "closed_by":"extract:b1",
                           "reason":"helix and zed are two editors the user uses side by side"}]}"#,
    ));
    let updates = judged_updates(&msgs);
    assert_eq!(
        updates.len(),
        1,
        "one contradiction, one update: {updates:?}"
    );
    assert_eq!(
        updates[0]["set"],
        serde_json::json!({"closure_source": ""}),
        "the revert is the attribution and nothing else"
    );
    assert_eq!(
        updates[0]["where"],
        serde_json::json!({"id": "e1", "canonical_subject": "user",
                           "canonical_predicate": "favorite_editor",
                           "closure_source": "extract:b1"}),
        "the axis echo AND the exact attribution the payload showed: a garbled \
         copy matches no row, and a judgement can never be reverted this way"
    );
}

#[test]
fn a_contradiction_that_cannot_be_read_back_or_aims_at_a_judgement_is_dropped() {
    // The same refusal discipline the closures already carry. The interesting one
    // is the last: `judge:` is a well-formed attribution of a well-formed row,
    // and it is refused because rail 1 says a judgement may only CLOSE -- a round
    // that could revoke a judgement would be able to delete after all.
    let msgs = emit_d(judgement(
        r#"{"reopenings":[{"subject":"user","predicate":"favorite_editor","statement":"e1",
                           "closed_by":"extract:b1","reason":"  "},
                          {"subject":"","predicate":"","statement":"e1",
                           "closed_by":"extract:b1","reason":"no axis"},
                          {"subject":"user","predicate":"favorite_editor","statement":"",
                           "closed_by":"extract:b1","reason":"no statement"},
                          {"subject":"user","predicate":"favorite_editor","statement":"e1",
                           "closed_by":"judge:r0","reason":"an earlier night was wrong"},
                          {"subject":"user","predicate":"favorite_editor","statement":"e1",
                           "closed_by":"","reason":"the arithmetic residue"},
                          "not an object"]}"#,
    ));
    assert!(
        judged_updates(&msgs).is_empty(),
        "a contradiction the store cannot justify was written anyway: {msgs:?}"
    );
    assert_eq!(
        args_of(&msgs[0])["operation"],
        "canonicalize",
        "and the round still ends in its single re-derive"
    );
}

#[test]
fn the_contradiction_is_written_before_any_alias_moves_an_identity() {
    // Same argument as the closures: the `where` echoes the axis the judge was
    // shown, and `canonicalize` is what moves an identity. Written after the
    // re-derive the same `where` would match nothing.
    let msgs = emit_d(judgement(
        r#"{"entities":[{"alias":"user:u1","canonical":"user"}],
            "reopenings":[{"subject":"user","predicate":"favorite_editor","statement":"e1",
                           "closed_by":"extract:b1","reason":"the two coexist"}]}"#,
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
    let revert_at = ops.iter().position(|o| o == "update").expect("the revert");
    let derive_at = ops
        .iter()
        .position(|o| o == "canonicalize")
        .expect("the re-derive");
    assert!(revert_at < derive_at, "the revert came too late: {ops:?}");
}

#[test]
fn the_run_receipt_carries_the_reason_of_every_contradiction() {
    // Ruling Q2 guard rail 2 in both directions: the closure names its reason and
    // so does the verdict that takes one back. Same place, same payload, and
    // absent when nothing was contradicted.
    let rows = serde_json::json!([
        {"key": RUN, "kind": "verdicts", "payload": "{\"beliefs\": []}"},
        {"key": RUN, "kind": "beliefs", "payload": "[]"},
        {"key": RUN, "kind": "canon-reopenings",
         "payload": "[{\"id\":\"e1\",\"subject\":\"user\",\"predicate\":\"favorite_editor\",\
                       \"closed_by\":\"extract:b1\",\"reason\":\"the two editors coexist\"}]"}
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
    assert_eq!(verdicts["reopenings"][0]["id"], "e1");
    assert_eq!(
        verdicts["reopenings"][0]["reason"],
        "the two editors coexist"
    );
    assert_eq!(verdicts["reopenings"][0]["closed_by"], "extract:b1");
}

/// Hand a probe program to python3 **on stdin**, never in argv.
///
/// A probe embeds the whole shipped script as a literal, and a single argv
/// string is capped at 128 KiB (`MAX_ARG_STRLEN`). The recall script crossed
/// that line in W2, and the failure mode is an opaque `ArgumentListTooLong`
/// that looks like a broken test rather than like a size limit. `python3 -`
/// reads and compiles the whole program from stdin before it runs a line of
/// it, so the probe's own `sys.stdin` replacement below is unaffected.
fn run_python(src: &str) -> std::process::Output {
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
    run_python(&src)
}

/// Run a probe against the module body of the real dream script. The `park()`
/// exit at its end is swallowed so the probe can call the helpers the script
/// defines -- the construction `w2_statement_chain.rs` introduced.
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
        serde_json::to_string(&script_of(DREAM_CONFIG)).unwrap(),
        probe
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// One axis closed by the EXTRACTOR plus the materialisation loop of a night.
const EXTRACT_CLOSED_AXIS: &str = r#"
rows = [
 {"id":"e1","subject":"user","canonical_subject":"user","predicate":"favorite_editor",
  "canonical_predicate":"favorite_editor","claim":"helix","canonical_claim":"helix",
  "valid_from":"2026-03-05T00:00:00Z","valid_until":None,"recorded_at":"2026-03-05T00:00:00Z",
  "expired_at":"2026-08-10T00:00:00Z","superseded_by":"e2","closure_source":"extract:b1",
  "session_id":"s1"},
 {"id":"e2","subject":"user","canonical_subject":"user","predicate":"favorite_editor",
  "canonical_predicate":"favorite_editor","claim":"zed","canonical_claim":"zed",
  "valid_from":"2026-08-10T00:00:00Z","valid_until":None,"recorded_at":"2026-08-10T00:00:00Z",
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
"#;

#[test]
fn an_extract_closure_is_stable_round_after_round() {
    // The pin the W3 handover asked W4 to copy: a closure is a closure, so the
    // nightly re-derive has to leave an ATTRIBUTED extractor closure exactly as
    // alone as it leaves a judged one. Three rounds, because a rule that survives
    // one may still drift on the second.
    let probe = EXTRACT_CLOSED_AXIS.to_string()
        + r#"
seen = [a_round(rows) for _ in range(3)]
print(seen[0] == seen[1] == seen[2], seen[2][0], rows[0]["closure_source"])
"#;
    assert_eq!(
        run_probe(&probe),
        "True ('e1', '2026-08-10T00:00:00Z', 'e2') extract:b1",
        "an extractor closure did not survive repeated rounds unchanged"
    );
}

#[test]
fn the_contradiction_takes_the_closure_back_in_the_same_night() {
    // The other half of the same mechanism, and the reason the revert needs to
    // write only ONE column: the round clears the attribution BEFORE `sup-axes`,
    // so the re-derive of that same night finds a closure the chain cannot
    // reproduce and withdraws it to NULL (ruling Q4). Without the ordering the
    // reopened statement would stay closed until the following night.
    let probe = EXTRACT_CLOSED_AXIS.to_string()
        + r#"
rows[0]["closure_source"] = ""
print(a_round(rows)[0], rows[0]["expired_at"], rows[0]["superseded_by"])
"#;
    assert_eq!(
        run_probe(&probe),
        "('e1', None, None) None None",
        "a contradicted closure was left standing"
    );
}
