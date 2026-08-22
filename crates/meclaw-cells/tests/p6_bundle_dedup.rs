//! 0.2.0 P6 -- the bundle collapses verbatim repeated episodes (GitHub #15).
//!
//! The composition cut (ruling O-7) gives episodes a bounded share of the bundle;
//! inside that share every verbatim copy of the same question still occupied a
//! slot, so a question asked five times could fill the whole episode budget with
//! one string. The collapse is PRESENTATION: the stored rows are untouched
//! (no-delete, level 0 stays append-only), the bundle shows the newest copy once
//! and says how often it was seen.
//!
//! Two probes in one file, because the package has two halves that have to agree:
//! the normal form (a pure function, compared against the store's own
//! `normalize`) and the emit phase (the real script against a real stdin
//! document). Both run the shipped `params.script_inline`, never a copy.

use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";

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

fn recall_script() -> String {
    let raw = std::fs::read_to_string(RECALL_CONFIG).expect("recall config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
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

/// Call a pure function of the script: the module body runs against a stub stdin
/// and its `park()` exit is swallowed, then the probe runs in the same globals.
fn run_probe(probe: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'recall', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(&recall_script()).unwrap(),
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

/// Run the real script against a real stdin document and return the emitted messages.
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

/// One episode row as the hydration select returns it.
fn episode(id: &str, content: &str, happened_at: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": "s1", "sender": "user",
                       "content": content, "happened_at": happened_at,
                       "recorded_at": "2026-08-11T20:00:00.000000Z"})
}

/// The `t1-emit` document: the scratch select that carries the fused ranking and
/// the hydration rows, delivered with the request context of a tier-1 recall.
fn emit_doc(
    candidates: serde_json::Value,
    eps: serde_json::Value,
    facts: serde_json::Value,
) -> serde_json::Value {
    let fused = serde_json::json!({
        "candidates": candidates,
        "legs_present": ["keyword"],
        "leg_sizes": {"keyword": 5, "semantic": 0, "graph": 0, "temporal": 0},
        "semantic_degraded": true
    });
    let rows = serde_json::json!([
        {"request_id": "r1", "leg": "fused", "payload": fused.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-ep", "payload": eps.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-fact", "payload": facts.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-axis", "payload": "[]", "fired": 1}
    ]);
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "which editor did I prefer?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": ""},
            "hop": {"operation": "select"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

fn bundle_of(msgs: &[serde_json::Value]) -> serde_json::Value {
    let text = msgs[0]["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("bundle text");
    serde_json::from_str(text).expect("bundle json")
}

/// The internal candidate records of the emission (#296).
///
/// The collapse is a statement about the RETRIEVAL — which slot a copy owns,
/// which rank it holds, which legs nominated it — and since #296 that half
/// travels in `recall_diagnostic` rather than in the payload a model reads.
/// The payload carries the collapsed line and its count; everything this file
/// asserts about ids, ranks, scores and legs is asserted about the record.
fn record_of(msgs: &[serde_json::Value]) -> serde_json::Value {
    msgs[0]["recall_diagnostic"]["candidates"].clone()
}

fn rendered_of(msgs: &[serde_json::Value]) -> String {
    msgs[0]["messages"][0]["text"]
        .as_str()
        .expect("rendered text")
        .to_string()
}

/// The diagnostic rendering of the same emission (#279) — the flat ranked form
/// with the leg tags, which is what explains a collapse afterwards.
fn diagnostic_text(msgs: &[serde_json::Value]) -> String {
    msgs[0]["recall_diagnostic"]["text"]
        .as_str()
        .expect("diagnostic text")
        .to_string()
}

/// Five copies of one question, oldest first and best ranked first: the fusion
/// order is descending, so the OLDEST copy owns the slot and the NEWEST owns the
/// wording. The middle copy arrives on a second leg.
fn five_copies() -> (serde_json::Value, serde_json::Value) {
    let candidates = serde_json::json!([
        {"kind": "episode", "id": "e1", "score": 0.05, "legs": ["keyword"]},
        {"kind": "episode", "id": "e2", "score": 0.04, "legs": ["keyword"]},
        {"kind": "episode", "id": "e3", "score": 0.03, "legs": ["graph"]},
        {"kind": "episode", "id": "e4", "score": 0.02, "legs": ["keyword"]},
        {"kind": "episode", "id": "e5", "score": 0.01, "legs": ["keyword"]}
    ]);
    let eps = serde_json::json!([
        episode(
            "e1",
            "Which editor did I prefer?",
            "2026-08-09T09:00:00.000000Z"
        ),
        episode(
            "e2",
            "which editor did i prefer?",
            "2026-08-09T10:00:00.000000Z"
        ),
        episode(
            "e3",
            "WHICH EDITOR DID I PREFER?",
            "2026-08-09T11:00:00.000000Z"
        ),
        episode(
            "e4",
            "Which editor did I prefer?",
            "2026-08-09T12:00:00.000000Z"
        ),
        episode(
            "e5",
            "which  editor  did I prefer?",
            "2026-08-09T13:00:00.000000Z"
        )
    ]);
    (candidates, eps)
}

/// The collapse key is the store's own normal form (0.2.0 P4, ruling Q5), not a
/// second definition of "the same string". The script cannot import the Rust
/// function, so the twin is pinned here: same input, same output, on every effect
/// the normal form has.
#[test]
fn the_scripts_normal_form_is_the_stores_normal_form() {
    let samples = [
        "Which editor did I prefer?",
        "which  editor   did i prefer?",
        "  padded  ",
        "\tmixed \n whitespace ",
        "Helix",
        "user:u1",
        // NFD (e + combining acute) has to reach the NFC spelling
        "elve\u{0301}se",
        "ELVE\u{0301}SE",
        "elv\u{00e9}se",
        "",
        "   ",
    ];
    let probe = format!(
        "import json\nprint(json.dumps([normalize_text(s) for s in {}]))\n",
        serde_json::to_string(&samples).unwrap()
    );
    let twin: Vec<String> = serde_json::from_str(&run_probe(&probe)).expect("twin output");
    let want: Vec<String> = samples
        .iter()
        .map(|s| meclaw_cells::store::query::normalize::normalize(s))
        .collect();
    assert_eq!(twin, want);
}

/// The done-when of #15: the same question five times is ONE line in the bundle.
#[test]
fn five_verbatim_copies_leave_one_episode_line() {
    let (candidates, eps) = five_copies();
    let msgs = emit(emit_doc(candidates, eps, serde_json::json!([])));
    let record = record_of(&msgs);
    let items = record.as_array().expect("candidates");
    assert_eq!(items.len(), 1, "record: {record}");
    assert_eq!(items[0]["seen"], 5);
    assert_eq!(items[0]["rank"], 1);
    // And the payload says the same thing in the shape a reader gets it in:
    // one candidate, standing for five copies.
    let bundle = bundle_of(&msgs);
    assert_eq!(
        bundle["candidates"].as_array().expect("candidates").len(),
        1,
        "bundle: {bundle}"
    );
    assert_eq!(bundle["candidates"][0]["seen"], 5, "bundle: {bundle}");
    // #281: the run's header, the section the row belongs to, and the one row.
    assert_eq!(rendered_of(&msgs).lines().count(), 3);
}

/// The slot belongs to the best-ranked copy, the wording to the newest one: a
/// repetition is interesting for when it LAST happened. Score and rank stay where
/// the fusion put them, and the legs of the swallowed copies are kept (O-4b).
#[test]
fn the_newest_copy_owns_the_surviving_line() {
    let (candidates, eps) = five_copies();
    let msgs = emit(emit_doc(candidates, eps, serde_json::json!([])));
    let record = record_of(&msgs);
    let item = &record[0];
    assert_eq!(item["id"], "e5");
    assert_eq!(item["text"], "which  editor  did I prefer?");
    assert_eq!(item["happened_at"], "2026-08-09T13:00:00.000000Z");
    assert_eq!(item["score"], 0.05);
    assert_eq!(item["legs"], serde_json::json!(["keyword", "graph"]));
    // The wording is the half a reader gets — the newest copy's, on the day it
    // last happened (#296: the id, the score and the legs are the record's).
    let bundle = bundle_of(&msgs);
    assert_eq!(
        bundle["candidates"][0]["text"], "which  editor  did I prefer?",
        "bundle: {bundle}"
    );
    assert_eq!(
        bundle["candidates"][0]["when"], "2026-08-09",
        "bundle: {bundle}"
    );
    // #281: the readable half is the PAYLOAD form — an utterance stands under
    // its own header, opening with who said it and when, and the count is the
    // same annotation in the same place.
    assert!(
        rendered_of(&msgs)
            .contains("  user on 2026-08-09: \"which  editor  did I prefer?\" (seen: 5)"),
        "rendered: {}",
        rendered_of(&msgs)
    );
    // #279: and the byte-identical diagnostic line is back, one slot over —
    // the flat ranked form with the legs that nominated the surviving copy,
    // which is the document whoever explains this collapse afterwards reads.
    assert!(
        diagnostic_text(&msgs)
            .contains("- [episode keyword/graph] which  editor  did I prefer? (seen: 5)"),
        "diagnostic: {}",
        diagnostic_text(&msgs)
    );
}

/// The protection direction: the collapse merges what the normal form calls
/// equal, and nothing else. Two questions that merely look alike stay two lines.
#[test]
fn two_different_episodes_stay_two_lines() {
    let candidates = serde_json::json!([
        {"kind": "episode", "id": "e1", "score": 0.05, "legs": ["keyword"]},
        {"kind": "episode", "id": "e2", "score": 0.04, "legs": ["keyword"]}
    ]);
    let eps = serde_json::json!([
        episode(
            "e1",
            "Which editor did I prefer?",
            "2026-08-09T09:00:00.000000Z"
        ),
        episode(
            "e2",
            "Which editors did I prefer?",
            "2026-08-09T10:00:00.000000Z"
        )
    ]);
    let msgs = emit(emit_doc(candidates, eps, serde_json::json!([])));
    let record = record_of(&msgs);
    assert_eq!(record.as_array().expect("candidates").len(), 2);
    assert_eq!(record[0]["seen"], 1);
    // #296: a count of one is absent from the payload, exactly as it is absent
    // from the rendered line — the two agree about what "nothing repeated"
    // looks like.
    let bundle = bundle_of(&msgs);
    assert_eq!(
        bundle["candidates"].as_array().expect("candidates").len(),
        2
    );
    assert!(
        bundle["candidates"][0].get("seen").is_none(),
        "bundle: {bundle}"
    );
    assert!(
        !rendered_of(&msgs).contains("seen:"),
        "{}",
        rendered_of(&msgs)
    );
}

/// A fact line is untouched by the package: no count, no new field. Only episodes
/// repeat verbatim. The axis in front of the claim is the R3 ruling, not this
/// package -- it is pinned in `r3_axis_rendering.rs` and carried here so the
/// end-to-end line stays fixed.
#[test]
fn a_fact_line_renders_exactly_as_before() {
    let candidates = serde_json::json!([
        {"kind": "fact", "id": "f1", "score": 0.05, "legs": ["keyword"]}
    ]);
    let facts = serde_json::json!([
        {"id": "f1", "episode_id": "e1", "subject": "user:u1", "canonical_subject": "user:u1",
         "predicate": "favorite_editor", "canonical_predicate": "favorite_editor",
         "claim": "the preferred editor is vscode", "fact_kind": "world",
         "valid_from": "2026-08-02T09:00:00Z", "valid_until": null, "confidence": 100}
    ]);
    let msgs = emit(emit_doc(candidates, serde_json::json!([]), facts));
    let record = record_of(&msgs);
    assert!(record[0].get("seen").is_none(), "{record}");
    // #281: same line in the payload form — the axis, the claim and the day it
    // started, under the facts header.
    assert!(
        rendered_of(&msgs).contains(
            "  user:u1 favorite_editor = the preferred editor is vscode   since 2026-08-02"
        ),
        "rendered: {}",
        rendered_of(&msgs)
    );
    // #279: and the diagnostic form, byte for byte as it always rendered.
    assert!(
        diagnostic_text(&msgs)
            .contains("- [fact keyword] user:u1 favorite_editor: the preferred editor is vscode"),
        "diagnostic: {}",
        diagnostic_text(&msgs)
    );
    assert!(
        !rendered_of(&msgs).contains("seen"),
        "{}",
        rendered_of(&msgs)
    );
}
