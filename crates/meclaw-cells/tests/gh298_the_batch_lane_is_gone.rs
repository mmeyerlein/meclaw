//! Wave 5 -- extraction is per turn, so `extract-glue` loses the batch lane
//! (GitHub #298).
//!
//! The hive used to carry two extraction ingresses. The front model annotated
//! the turn it had just answered (inline), and everything it did not annotate
//! accumulated in `pending_extraction` until a gate opened, a batch was claimed
//! and a second model was paid to read the same turns again. Wave 5 keeps the
//! first and deletes the second: the queue is an EXCEPTION LIST now -- a row
//! left in `pending` is a turn whose annotation never arrived, not a work item
//! waiting to be scheduled.
//!
//! What this file locks is the ABSENCE. A deletion is only done when the phases
//! it removed emit nothing at all, because a phase that still answers is a lane
//! that still runs: the gate would still claim rows, the claim would still park
//! a batch, and the prompt would still send `route: extract` to a cell nobody
//! pays for any more. So every deleted phase is fed the echo it used to answer
//! and has to stay silent, and no phase anywhere may still address the
//! extractor.
//!
//! Two things this file also guards on the KEEP side, because both are easy to
//! delete by accident together with the lane around them:
//!
//! * the apply phase's guard rail 3 -- the `window` row read back as
//!   `window_row` before any `replaces` is honoured. The phases that WROTE that
//!   row die here; the read stays load-bearing, because the close pass writes it
//!   later in this wave. Deleting it because "nothing writes it any more" would
//!   silently unguard every future closure.
//! * the inline ingress itself, which is now the lane's only producer.
//!
//! Everything here runs the REAL `params.script_inline` of `extract-glue`
//! against injected replies, so nothing costs anything.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";
const HIVE_CONFIG: &str = "../../templates/memory-hive/config.json";
const DREAM_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";
const EXTRACTOR_DIR: &str = "../../templates/memory-hive/extractor";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string -- the same substitution the colony performs when it instantiates the
/// template. After this package the script carries none at all; the resolver
/// stays because the harness is shared with the other glue tests and a script
/// that grows one back must not silently run with `${...}` in its source.
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

fn glue_source() -> String {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("extract-glue config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    v["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

fn glue_script() -> String {
    resolve_vars(&glue_source())
}

/// A shipped `config.json` of this hive, parsed.
fn config(path: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path} is not json: {e}"))
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv (GH #279): a single argv string is
/// capped at 128 KiB and the shipped scripts live within a few KB of that line.
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

/// Run the real script with a real stdin document and return the emitted
/// messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(
        &glue_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "extract-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// One store echo as the lane's own edge delivers it: the phase in the context,
/// the answered operation in the hop, the rows as the first message's text.
fn echo(phase: &str, operation: Option<&str>, rows: serde_json::Value) -> serde_json::Value {
    let mut hop = serde_json::json!({"rows_affected": 3});
    if let Some(op) = operation {
        hop["operation"] = serde_json::Value::String(op.to_string());
    }
    serde_json::json!({
        "header": {
            "context": {"mem_phase": phase, "batch_id": "b-298",
                        "session_id": "s-298", "channel": "c-298",
                        "audience_set": r#"["member:user","agent:assistant"]"#},
            "hop": hop
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                      "text": rows.to_string()}]
    })
}

/// A queue row as the gate, the flush and the claim used to read it -- enough
/// token estimate and enough age to open every threshold there was, so a lane
/// that still ran would answer rather than park for lack of material.
fn queue_rows() -> serde_json::Value {
    serde_json::json!([{
        "id": "q1", "episode_id": "e1", "token_est": 9000,
        "enqueued_at": "2024-01-01T00:00:00.000000Z", "status": "pending",
        "sender": "user", "content": "I moved to Lisbon", "attempts": 0,
        "not_before": ""
    }])
}

/// Every phase this package deletes, with the echo it used to answer. The
/// `flush` entry appears twice because the drain had two forms and the second
/// one (`flush_reclaim`) reached a different op.
fn deleted_phases() -> Vec<(&'static str, Option<&'static str>, serde_json::Value)> {
    vec![
        ("flush", None, serde_json::json!([])),
        ("reclaimed", Some("update"), serde_json::json!([])),
        ("flush-eval", Some("select"), queue_rows()),
        ("closed", Some("update"), serde_json::json!([])),
        ("gate", Some("insert"), serde_json::json!([])),
        ("gate-eval", Some("select"), queue_rows()),
        ("claim", Some("update"), serde_json::json!([])),
        (
            "batch",
            Some("select"),
            serde_json::json!([{"id": "q1", "episode_id": "e1", "sender": "user",
                                "content": "I moved to Lisbon", "status": "claimed"}]),
        ),
        ("vocab-fetch", Some("insert"), serde_json::json!([])),
        (
            "window-scan",
            Some("select"),
            serde_json::json!([{"subject": "user", "canonical_subject": "user",
                                "predicate": "lives_in", "canonical_predicate": "lives_in",
                                "valid_from": "2024-01-01T00:00:00.000000Z"}]),
        ),
        (
            "window-page",
            Some("select"),
            serde_json::json!([{"id": "f1", "subject": "user", "canonical_subject": "user",
                                "predicate": "lives_in", "canonical_predicate": "lives_in",
                                "claim": "Berlin", "canonical_claim": "Berlin",
                                "valid_from": "2024-01-01T00:00:00.000000Z",
                                "recorded_at": "2024-01-01T00:00:00.000000Z",
                                "expired_at": null}]),
        ),
        (
            "vocab",
            Some("select"),
            serde_json::json!([{"subject": "user", "canonical_subject": "user",
                                "predicate": "lives_in", "canonical_predicate": "lives_in"}]),
        ),
        ("prompt-fetch", Some("insert"), serde_json::json!([])),
        (
            "prompt",
            Some("select"),
            serde_json::json!([{"key": "b-298", "kind": "batch",
                                "payload": r#"[{"episode_id":"e1","sender":"user","content":"I moved to Lisbon"}]"#}]),
        ),
        (
            "error-eval",
            Some("select"),
            serde_json::json!([{"id": "q1", "attempts": 1}]),
        ),
    ]
}

#[test]
fn a_flush_drains_nothing_because_there_is_no_queue_to_drain() {
    // The explicit drain was the operator's "extract now". There is nothing to
    // extract now: an annotation arrives with the turn or it never arrives, and
    // a drain that claimed rows would start a lane this wave removed.
    let msgs = emit(echo("flush", None, serde_json::json!([])));
    assert!(
        msgs.is_empty(),
        "a flush emits nothing at all, got {msgs:?}"
    );
    let mut reclaim = echo("flush", None, serde_json::json!([]));
    reclaim["header"]["context"]["flush_reclaim"] = serde_json::json!("1");
    let msgs = emit(reclaim);
    assert!(
        msgs.is_empty(),
        "and neither does the recovery sweep, got {msgs:?}"
    );
}

#[test]
fn the_echo_of_an_enqueued_turn_opens_no_gate() {
    // Task 4 unhooked the enqueue from the gate by giving the insert its own
    // dead-end phase; this deletes the gate it used to reach. Both halves have
    // to hold, or the queue is a work list again the moment somebody stamps the
    // old phase on an echo.
    let msgs = emit(echo("gate", Some("insert"), serde_json::json!([])));
    assert!(
        msgs.is_empty(),
        "the gate is gone and evaluates nothing, got {msgs:?}"
    );
}

#[test]
fn every_deleted_phase_stays_silent() {
    for (phase, operation, rows) in deleted_phases() {
        let msgs = emit(echo(phase, operation, rows));
        assert!(
            msgs.is_empty(),
            "phase '{phase}' still answers -- the batch lane is not gone: {msgs:?}"
        );
    }
}

#[test]
fn nothing_addresses_the_extractor_any_more() {
    // The lane that fed the extractor cell. Task 6 removes the cell itself; if
    // any phase still emitted this route, that removal would turn a running
    // colony's extraction into an unroutable message instead of into silence.
    for (phase, operation, rows) in deleted_phases() {
        for msg in emit(echo(phase, operation, rows)) {
            assert_ne!(
                msg["header"]["route"], "extract",
                "phase '{phase}' still addresses the extractor: {msg:?}"
            );
        }
    }
    // The parse phase is the extractor's ANSWER coming back, and it never
    // carried an operation of its own.
    for msg in emit(echo(
        "parse",
        None,
        serde_json::json!({"facts": [{"subject": "user", "predicate": "lives_in",
                                      "claim": "Lisbon", "fact_kind": "world"}]}),
    )) {
        assert_ne!(
            msg["header"]["route"], "extract",
            "the extractor answer path still runs: {msg:?}"
        );
    }
}

#[test]
fn the_prompt_cannot_be_reintroduced_unnoticed() {
    // The cheap drift lock: the extraction contract this lane rendered lives in
    // the front model's persona now (the hive ships it as `inline-contract.md`),
    // and a second copy growing back here is how one contract becomes two.
    let src = glue_source();
    for symbol in [
        "build_instructions",
        "parse_llm_json",
        "pick_batch",
        "claim_op",
        "backoff_until",
        "known_axes",
        "open_statements",
        "candidate_axes",
        "window_read",
        "vocab_read",
        "render_axes",
        "render_window",
        "matter_tokens",
        "subject_matter_overlap",
        "select_window",
        "BATCH_TOKENS",
        "BATCH_MAX_AGE_MIN",
        "BATCH_MAX_ITEMS",
        "CLAIM_LEASE_MIN",
        "ERROR_BUDGET",
        "ERROR_BACKOFF_SEC",
        "WINDOW_MAX_AXES",
        "WINDOW_POOL_AXES",
        "WINDOW_MAX_STATEMENTS_PER_AXIS",
        "WINDOW_CANDIDATE_AXES",
        "VOCAB_MAX_SUBJECTS",
        "VOCAB_MAX_PER_SUBJECT",
        "VOCAB_MAX_ROWS",
        "WINDOW_SCAN_ROWS",
        "WINDOW_MAX_ROWS",
        "MATTER_MIN_TOKEN",
        "MATTER_STOP",
    ] {
        assert!(
            !src.contains(symbol),
            "the batch lane's `{symbol}` is still in the script"
        );
    }
}

/// The apply phase's meeting point, with the four scratch rows it reads back.
/// `window` is the one under test; the other three are what the phase refuses to
/// run without.
fn apply_echo(with_window: bool) -> serde_json::Value {
    let fact = serde_json::json!({
        "episode_id": "e1", "subject": "user", "predicate": "lives_in",
        "claim": "Lisbon", "fact_kind": "world", "confidence": 90,
        "valid_from": "2025-01-01T00:00:00.000000Z", "valid_until": null,
        "replaces": "f-old", "claim_hash": "h1"
    });
    let mut rows = vec![
        serde_json::json!({"key": "b-298", "kind": "payload",
            "payload": serde_json::json!({"facts": [fact], "entities": [], "edges": []})
                .to_string()}),
        serde_json::json!({"key": "b-298", "kind": "known", "payload": "[]"}),
        serde_json::json!({"key": "b-298", "kind": "episodes",
            "payload": serde_json::json!({"e1": {"happened_at": "2025-01-01T00:00:00.000000Z",
                                                 "session_id": "s-298", "channel": "c-298",
                                                 "audience_set": "[\"member:user\"]"}})
                .to_string()}),
    ];
    if with_window {
        rows.push(serde_json::json!({"key": "b-298", "kind": "window",
            "payload": serde_json::json!([{"subject": "user", "predicate": "lives_in",
                "statements": [{"id": "f-old", "claim": "Berlin",
                                "since": "2024-01-01T00:00:00.000000Z",
                                "last_asserted": "2024-01-01T00:00:00.000000Z"}]}])
                .to_string()}));
    }
    echo("apply", Some("select"), serde_json::Value::Array(rows))
}

/// The closure this batch wrote, if it wrote one.
fn closure(msgs: &[serde_json::Value]) -> Option<serde_json::Value> {
    msgs.iter()
        .filter(|m| m["header"]["route"] == "xstore")
        .map(|m| {
            let text = m["messages"][0]["text"].as_str().expect("op text");
            serde_json::from_str::<serde_json::Value>(text).expect("op args")
        })
        .find(|a| a["table"] == "facts" && a["operation"] == "update")
}

#[test]
fn the_apply_phase_still_reads_the_window_row_back() {
    // Guard rail 3 of the replacement ruling, and the assertion this task exists
    // to keep honest. Every phase that WROTE this row is deleted above -- which
    // makes "nothing writes it any more, so the read is dead" the obvious wrong
    // conclusion. The close pass writes it later in this wave, and a `replaces`
    // honoured without this proof would end a statement the model was never
    // shown.
    let msgs = emit(apply_echo(true));
    let op = closure(&msgs).expect("a `replaces` inside the window is honoured");
    assert_eq!(op["where"]["id"], "f-old");
    assert_eq!(
        op["set"]["expired_at"], "2025-01-01T00:00:00.000000Z",
        "the closure ends the old statement at the new fact's own instant"
    );

    // ... and the read is what decides it: the same payload without the window
    // row closes nothing at all. Without this half the assertion above would
    // still pass over a script that honoured every `replaces` blindly.
    let msgs = emit(apply_echo(false));
    assert!(
        closure(&msgs).is_none(),
        "a `replaces` with no window to prove it must not close anything: {msgs:?}"
    );
}

#[test]
fn the_hive_no_longer_carries_the_extractor_or_its_lane() {
    // The script has been silent since Task 5; what is left is the topology that
    // still names the cell. As long as an edge addresses `./extractor`, the cell
    // is part of the shipped hive whether or not anything ever reaches it -- and
    // as long as `in_flush` is an accepted lane, a caller may still wire a drain
    // that arrives at a phase which now answers nothing. Both are promises, and
    // a promise nothing keeps is worse than no promise.
    let hive = config(HIVE_CONFIG);
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("params.graph.edges");
    for edge in edges {
        assert_ne!(
            edge["to"], "./extractor",
            "an edge still addresses the extractor: {edge}"
        );
        assert_ne!(
            edge["from"], "./extractor",
            "the extractor still answers into the lane: {edge}"
        );
        let names_flush = edge["condition"]
            .as_str()
            .is_some_and(|c| c.contains("in_flush"));
        assert!(!names_flush, "an edge still carries the flush lane: {edge}");
    }
    let accepts = hive["params"]["contract"]["accepts"]
        .as_array()
        .expect("params.contract.accepts");
    for entry in accepts {
        assert_ne!(
            entry["route"], "in_flush",
            "the hive still accepts the drain lane: {entry}"
        );
    }
    // ... and the cell directory itself, because a template directory nothing
    // instantiates is still a cell the next reader will believe in.
    assert!(
        !std::path::Path::new(EXTRACTOR_DIR).exists(),
        "templates/memory-hive/extractor/ still exists"
    );
    // The surviving ingress, asserted in the positive: removing three edges must
    // not take the inline lane with it.
    let inline = edges
        .iter()
        .find(|e| {
            e["from"] == "."
                && e["to"] == "./extract-glue"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("in_remember"))
        })
        .expect("the inline ingress survives");
    assert_eq!(
        inline["modifier"]["set_context"]["store_origin"],
        "'inline'"
    );
    assert_eq!(inline["modifier"]["set_context"]["mem_phase"], "'inline'");
}

#[test]
fn the_night_reclaims_nothing_because_nothing_claims() {
    // The dream run used to hand claimed rows back to the queue on the theory
    // that a batch had died mid-flight. Nothing claims a row any more, so the op
    // resurrects a state that cannot occur -- it would silently rewrite the one
    // column the queue now uses to mean "this turn was never annotated".
    let dream = config(DREAM_CONFIG);
    let src = dream["params"]["script_inline"]
        .as_str()
        .expect("dream-glue script_inline");
    assert!(
        !src.contains("pending_extraction"),
        "the night still writes the extraction queue"
    );
    assert!(
        !src.contains("\"backfill\""),
        "the reclaim's scratch phase is still in the script"
    );
}

#[test]
fn the_glue_declares_no_knob_for_a_lane_that_is_gone() {
    // `contract.settings` is public surface: it is what a caller reads to learn
    // what this cell can be told. Every one of these tuned the batch lane -- its
    // gate, its lease, its error budget, the prompt window it built. No script
    // has read them since Task 5, and a declared knob that changes nothing is a
    // lie a caller cannot detect from the outside.
    let settings = config(GLUE_CONFIG)["contract"]["settings"].clone();
    for knob in [
        "batch_tokens",
        "batch_max_age_min",
        "batch_claim_lease_min",
        "extract_error_budget",
        "extract_backoff_sec",
        "extract_window_axes",
        "extract_vocab_rows",
    ] {
        assert!(
            settings[knob].is_null(),
            "the batch lane's `{knob}` is still declared"
        );
    }
}
