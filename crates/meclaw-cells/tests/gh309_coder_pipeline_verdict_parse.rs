//! GH #309 — the `coder-pipeline` reviewer's verdict survives the way models
//! actually write it.
//!
//! `revout` is the M4 cell behind the reviewer llm: it turns the reviewer's
//! first line into `hop.outcome`, and that header is the only thing the graph
//! routes on (`refine` → `./coder`, `approve`/`fail` → `./taskarchive`). The
//! shipped parse was an **exact whole-first-line match**
//! (`verdict.splitlines()[0].strip().upper()` against
//! `{APPROVE, REFINE, FAIL}`) with a `fail` fallback, so every spelling that
//! carried anything besides the bare word — `APPROVE - looks good`,
//! `REFINE: add the marker`, `APPROVE.`, `**APPROVE**` — fell through to
//! `fail`, was archived as the verdict, and ended the loop. A false rejection
//! is indistinguishable from an honest one downstream, which is why this is
//! pinned rather than left to the reviewer's phrasing.
//!
//! The matrix below is the issue's measured table plus the spellings the
//! `16-refine-loop-live` corpus item hit live (`REFINE:` from gpt-4o-mini,
//! `**APPROVE**`, `FAIL —`, a numbered `1. APPROVE`). Six of the fifteen were
//! red when this file was written — the issue's four plus `**REFINE**` and
//! `1. APPROVE`.
//!
//! **R2b guard.** Every read is guarded by [`shipped_pipeline`]: where the
//! template does not ship, these tests skip rather than fail on a dead
//! reference.

use meclaw_core::serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn templates_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// The files these tests read. The list is the guard AND the inventory.
const PIPELINE_FILES: &[&str] = &[
    "config.json",
    "template.json",
    "README.md",
    "revout/config.json",
];

fn shipped_pipeline() -> Option<PathBuf> {
    let root = templates_root().join("coder-pipeline");
    for rel in PIPELINE_FILES {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

fn read_json(p: &Path) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The `revout` cell's shipped script with `${…}` resolved the way bootstrap
/// resolves it: `substitute_env_only` over an empty environment.
fn revout_script(root: &Path) -> String {
    let cfg = read_json(&root.join("revout/config.json"));
    let env: HashMap<String, String> = HashMap::new();
    let params = meclaw_colony::mutation::substitute::substitute_env_only(&cfg["params"], &env)
        .expect("revout params must substitute");
    params["script_inline"]
        .as_str()
        .expect("script_inline must be a string")
        .to_string()
}

/// Run a shipped script over a real stdin document, handing the script to the
/// runner **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `<runner> -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `-c` ran it: same `__main__` globals, same stdout, same
/// exit status.
fn run_script_on_stdin(runner: &str, script: &str, stdin_doc: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).unwrap(),
        meclaw_core::serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new(runner)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the shipped runner");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run the shipped script exactly as the `code` cell runs it: `params.runner`
/// with the shipped script, the stdin document in the three-key wire shape.
fn run_revout(root: &Path, stdin_doc: Value) -> Value {
    let cfg = read_json(&root.join("revout/config.json"));
    let runner = cfg["params"]["runner"].as_str().unwrap();
    let script = revout_script(root);

    let out = run_script_on_stdin(runner, &script, &stdin_doc.to_string());
    assert!(
        out.status.success(),
        "revout exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "revout stdout is not JSON: {e}\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The document the reviewer's `stop` turn hands to `revout`.
fn reviewer_says(verdict: &str) -> Value {
    json!({
        "envelope": {
            "header": {
                "context": { "turn_id": "t-1", "iter": "0" },
                "hop": { "finish_reason": "stop" }
            },
            "target": "/coder-pipeline/revout",
            "trace_id": "00000000-0000-0000-0000-000000000000",
            "ttl": 64
        },
        "body": {
            "messages": [{ "origin": "assistant", "type": "text", "id": "r1", "text": verdict }]
        },
        "params": {}
    })
}

/// `revout` is `multi_send_capable`: the refine path emits a LIST (store insert
/// plus the instruction back to the coder), approve/fail a single object. The
/// header that routes is the one carrying `outcome` either way.
fn outcome_of(out: &Value) -> String {
    let header = match out {
        Value::Array(items) => items
            .iter()
            .find(|m| m["header"].get("outcome").is_some())
            .map(|m| m["header"].clone())
            .unwrap_or_else(|| panic!("no emitted message carries hop.outcome: {out}")),
        _ => out["header"].clone(),
    };
    header["outcome"]
        .as_str()
        .unwrap_or_else(|| panic!("hop.outcome must be a string, got {header}"))
        .to_string()
}

/// The matrix: what a reviewer writes, and the verdict the graph must route on.
/// Fifteen spellings — the issue's measured table plus the live ones from the
/// `16-refine-loop-live` receipt.
const MATRIX: &[(&str, &str)] = &[
    // The bare word, in the casings a model actually emits.
    ("APPROVE", "approve"),
    ("Approve", "approve"),
    ("approve", "approve"),
    // A verdict with its justification on the SAME line — the four that were
    // measured as false `fail`s.
    ("APPROVE - looks good", "approve"),
    ("APPROVE.", "approve"),
    ("**APPROVE**", "approve"),
    ("REFINE: add the marker", "refine"),
    // The word on its own line, justification below — this already worked and
    // must keep working.
    ("APPROVE\nlooks good", "approve"),
    ("REFINE\nfix the loop bound", "refine"),
    // Refine in the other spellings.
    ("REFINE", "refine"),
    ("**REFINE**", "refine"),
    ("1. APPROVE", "approve"),
    // An honest failure stays a failure.
    ("FAIL", "fail"),
    ("FAIL — the tests do not run", "fail"),
    // An unknown leading token is NOT a verdict, and the loud end stands.
    ("LGTM, ship it", "fail"),
];

/// 1 — the matrix. Every spelling maps to the verdict the reviewer meant.
#[test]
fn the_reviewers_verdict_survives_its_spelling() {
    let Some(root) = shipped_pipeline() else {
        return;
    };
    let mut wrong: Vec<String> = Vec::new();
    for (verdict, expected) in MATRIX {
        let got = outcome_of(&run_revout(&root, reviewer_says(verdict)));
        if got != *expected {
            wrong.push(format!("{verdict:?} -> {got:?} (want {expected:?})"));
        }
    }
    assert!(
        wrong.is_empty(),
        "the reviewer's verdict must survive its spelling; {} of {} were misread:\n  {}",
        wrong.len(),
        MATRIX.len(),
        wrong.join("\n  ")
    );
}

/// 2 — an empty reviewer turn is a failure, not a crash. The old parse guarded
/// the empty case explicitly; the new one must too.
#[test]
fn an_empty_verdict_is_a_loud_end() {
    let Some(root) = shipped_pipeline() else {
        return;
    };
    assert_eq!(outcome_of(&run_revout(&root, reviewer_says(""))), "fail");
    assert_eq!(
        outcome_of(&run_revout(&root, reviewer_says("   \n\n"))),
        "fail"
    );
    assert_eq!(outcome_of(&run_revout(&root, reviewer_says("..."))), "fail");
}

/// 3 — a refine still carries the whole feedback into the thread, not just the
/// verdict word. Trimming the token must not trim the payload the memoryless
/// coder needs on re-entry.
#[test]
fn a_refine_carries_the_full_feedback_back_to_the_coder() {
    let Some(root) = shipped_pipeline() else {
        return;
    };
    let out = run_revout(
        &root,
        reviewer_says("REFINE: add the marker to line 3 of fizz.py"),
    );
    let items = out.as_array().expect("refine is a multi-send");
    assert_eq!(
        items.len(),
        2,
        "refine emits the store insert AND the instruction: {out}"
    );
    assert_eq!(items[0]["header"]["route"], "rstore");
    let instruction = items[1]["messages"][0]["text"].as_str().unwrap_or_default();
    assert!(
        instruction.contains("add the marker to line 3 of fizz.py"),
        "the refine instruction must restate the reviewer's full text, got {instruction:?}"
    );
    assert_eq!(items[1]["messages"][0]["origin"], "user");
}

/// 4 — an error or op reply passes through untouched (the guard at the top of
/// the script). Hardening the parse must not swallow it.
#[test]
fn an_error_reply_still_emits_nothing() {
    let Some(root) = shipped_pipeline() else {
        return;
    };
    let mut doc = reviewer_says("APPROVE");
    doc["envelope"]["header"]["hop"] = json!({ "error_code": "provider_error" });
    assert_eq!(run_revout(&root, doc), json!([]));
}

/// 5 — the README describes the topology the graph actually has. The v1
/// `coderloop` cell has not existed since v2; the loop runs
/// `dispatch → collector → state → collector → coder` over the `thread` table.
#[test]
fn the_readme_documents_the_shipped_topology() {
    let Some(root) = shipped_pipeline() else {
        return;
    };
    let hive = read_json(&root.join("config.json"));
    let edges = hive["params"]["graph"]["edges"].as_array().unwrap();
    let endpoints: Vec<String> = edges
        .iter()
        .flat_map(|e| [e["from"].clone(), e["to"].clone()])
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    assert!(
        !endpoints.iter().any(|e| e.contains("coderloop")),
        "sanity: no `coderloop` endpoint exists in the shipped graph"
    );

    for rel in ["README.md", "coder/config.json"] {
        let text = std::fs::read_to_string(root.join(rel)).unwrap();
        assert!(
            !text.contains("coderloop"),
            "{rel} still names the retired v1 `coderloop` cell"
        );
    }

    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    for cell in ["collector", "state", "drain"] {
        assert!(
            readme.contains(cell),
            "the README must name `{cell}` — it is part of the v2 loop"
        );
    }
}
