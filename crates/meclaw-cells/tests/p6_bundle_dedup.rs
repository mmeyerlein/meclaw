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

/// Run the real script against a real stdin document and return the emitted messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(recall_script())
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

fn rendered_of(msgs: &[serde_json::Value]) -> String {
    msgs[0]["messages"][0]["text"]
        .as_str()
        .expect("rendered text")
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
    let bundle = bundle_of(&msgs);
    let items = bundle["candidates"].as_array().expect("candidates");
    assert_eq!(items.len(), 1, "bundle: {bundle}");
    assert_eq!(items[0]["seen"], 5);
    assert_eq!(items[0]["rank"], 1);
    assert_eq!(rendered_of(&msgs).lines().count(), 2);
}

/// The slot belongs to the best-ranked copy, the wording to the newest one: a
/// repetition is interesting for when it LAST happened. Score and rank stay where
/// the fusion put them, and the legs of the swallowed copies are kept (O-4b).
#[test]
fn the_newest_copy_owns_the_surviving_line() {
    let (candidates, eps) = five_copies();
    let msgs = emit(emit_doc(candidates, eps, serde_json::json!([])));
    let bundle = bundle_of(&msgs);
    let item = &bundle["candidates"][0];
    assert_eq!(item["id"], "e5");
    assert_eq!(item["text"], "which  editor  did I prefer?");
    assert_eq!(item["happened_at"], "2026-08-09T13:00:00.000000Z");
    assert_eq!(item["score"], 0.05);
    assert_eq!(item["legs"], serde_json::json!(["keyword", "graph"]));
    assert!(
        rendered_of(&msgs)
            .contains("- [episode keyword/graph] which  editor  did I prefer? (seen: 5)"),
        "rendered: {}",
        rendered_of(&msgs)
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
    let bundle = bundle_of(&msgs);
    assert_eq!(
        bundle["candidates"].as_array().expect("candidates").len(),
        2
    );
    assert_eq!(bundle["candidates"][0]["seen"], 1);
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
    let bundle = bundle_of(&msgs);
    assert!(bundle["candidates"][0].get("seen").is_none(), "{bundle}");
    assert!(
        rendered_of(&msgs)
            .contains("- [fact keyword] user:u1 favorite_editor: the preferred editor is vscode"),
        "rendered: {}",
        rendered_of(&msgs)
    );
    assert!(
        !rendered_of(&msgs).contains("seen"),
        "{}",
        rendered_of(&msgs)
    );
}
