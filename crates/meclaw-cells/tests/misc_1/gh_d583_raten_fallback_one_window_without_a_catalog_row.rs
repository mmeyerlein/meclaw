//! GH #1532 (wave d58-3, strand raten) -- one fallback window, never silent.
//!
//! A producer asked before any window travels falls back to the param
//! `input_soft_fallback`. Without a catalogue row an overflow at the system
//! gate is worse than a window too small (OR-IG-9), so the fallback hangs on
//! no catalogue row any more: it is ONE documented value, 17500 tokens, in
//! every core cell -- the param, its declared default and the literal the
//! script reads it with. The param that named a row, `input_soft_fallback_row`,
//! is gone everywhere. Every use says so on stderr, `no catalog row for
//! <model>` (the model the request names, else `?`).
//!
//! The embedding window (`embed_input_soft_fallback`, `file-space/derive`) is
//! another window: it stays the `input_soft` of its own row, 2048.
//!
//! Frozen versions (`templates/<name>@<version>`) are shipped history and are
//! not read.

use meclaw_core::serde_json::{self as sj, Value, json};
use std::path::{Path, PathBuf};

/// The one fallback window of every core cell (GH #1532).
const FALLBACK: u64 = 17500;
/// The cells that carry it as shipped.
const CARRIERS: &[&str] = &[
    "coder-pipeline/dispatch",
    "research-assistant/dispatch",
    "file-space/derive",
    "summarizer/prep",
    "curator/handover",
    "curator/policy",
    "curator/push",
    "memory-hive/recall",
];
/// The words every use of the fallback writes on stderr.
const SAID: &str = "no catalog row for ";

/// R2b / GH #49: whether the template of `cell` ships in this tree. The
/// public subset carries no `coder-pipeline` and no `research-assistant`; a
/// carrier whose template is not here is not judged, every carrier of the
/// full tree is.
fn ships(cell: &str) -> bool {
    let template = cell.split('/').next().unwrap_or_default();
    repo("templates").join(template).is_dir()
}

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every file under `templates/` outside frozen versions.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            if !p.file_name().unwrap().to_string_lossy().contains('@') {
                files(&p, out);
            }
        } else {
            out.push(p);
        }
    }
}

/// Every number a script reads the fallback with (`_int("input_soft_fallback",
/// N)`, `knob_int("input_soft_fallback", N)`).
fn literals(script: &str) -> Vec<String> {
    let needle = "\"input_soft_fallback\", ";
    script
        .match_indices(needle)
        .map(|(i, _)| {
            script[i + needle.len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect()
        })
        .collect()
}

/// Whether a text names `input_soft_fallback_row` -- not the embedding's own
/// `embed_input_soft_fallback_row`, which stays.
fn names_the_row(text: &str) -> bool {
    text.match_indices("input_soft_fallback_row")
        .any(|(i, _)| !text[..i].ends_with('_'))
}

/// The findings of one config: every place the fallback window is stated,
/// held to the one value; and whether its script reads the fallback.
fn check(at: &str, doc: &Value, wrong: &mut Vec<String>) -> (bool, bool) {
    let mut carries = false;
    if let Some(v) = doc["params"].get("input_soft_fallback") {
        carries = true;
        if *v != json!(FALLBACK) {
            wrong.push(format!(
                "{at}: params.input_soft_fallback = {v}, not {FALLBACK}"
            ));
        }
    }
    if let Some(d) = doc["contract"]["settings"]["input_soft_fallback"].get("default")
        && *d != json!(FALLBACK)
    {
        wrong.push(format!(
            "{at}: settings.input_soft_fallback.default = {d}, not {FALLBACK}"
        ));
    }
    let script = doc["params"]["script_inline"].as_str().unwrap_or_default();
    for n in literals(script) {
        if n != FALLBACK.to_string() {
            wrong.push(format!(
                "{at}: the script reads input_soft_fallback with {n:?}, not {FALLBACK}"
            ));
        }
    }
    let reads = script.contains("\"input_soft_fallback\"");
    if reads && !script.contains(SAID) {
        wrong.push(format!(
            "{at}: the script reads input_soft_fallback but never says `{SAID}<model>`"
        ));
    }
    (carries, reads)
}

#[test]
fn every_fallback_window_is_one_value_and_names_no_row() {
    let root = repo("templates");
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut wrong = Vec::new();
    let mut carriers = Vec::new();
    for f in &all {
        let rel = f.strip_prefix(&root).unwrap_or(f).display().to_string();
        let Ok(raw) = std::fs::read_to_string(f) else {
            continue;
        };
        // The corpus of the librarian is generated from these files; the
        // param is gone from the living ones, the corpus follows them.
        if names_the_row(&raw) {
            wrong.push(format!("{rel}: names `input_soft_fallback_row`"));
        }
        if !rel.ends_with("config.json") {
            continue;
        }
        let Ok(doc) = sj::from_str::<Value>(&raw) else {
            continue;
        };
        let cell = rel.trim_end_matches("/config.json").to_string();
        let (carries, reads) = check(&cell, &doc, &mut wrong);
        if carries || reads {
            carriers.push(cell);
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    let expected: Vec<&str> = CARRIERS.iter().copied().filter(|c| ships(c)).collect();
    assert!(
        expected.len() >= 6,
        "check of the setup: the core carriers ship ({expected:?})"
    );
    for c in &expected {
        assert!(
            carriers.iter().any(|s| s == c),
            "{c}: listed, but no fallback window there"
        );
    }
    assert_eq!(
        carriers.len(),
        expected.len(),
        "a carrier the list does not know: {carriers:?}"
    );
}

#[test]
fn the_embedding_window_is_its_own_row() {
    let doc: Value = sj::from_str(
        &std::fs::read_to_string(repo("templates/file-space/derive/config.json")).unwrap(),
    )
    .unwrap();
    let p = &doc["params"];
    assert_eq!(p["embed_input_soft_fallback"], json!(2048));
    let row = p["embed_input_soft_fallback_row"].as_str().expect("a row");
    assert_eq!(row, "google/gemini-embedding-2");
    let soft = std::fs::read_to_string(repo("templates/llm-registry/store/seed/models.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|l| sj::from_str::<Value>(l).ok())
        .find(|r| r["model_id"] == row)
        .map(|r| r["input_soft"].clone());
    assert_eq!(soft, Some(json!(2048)), "the row {row} states the window");
    let script = p["script_inline"].as_str().unwrap();
    assert!(
        script.contains("knob_int(\"embed_input_soft_fallback\", 2048)"),
        "the script reads the embedding window with its row's number"
    );
}

/// A drifted value, a row named again, a script that falls back in silence:
/// each is red.
#[test]
fn a_drifted_or_silent_fallback_is_red() {
    let mut wrong = Vec::new();
    let doc = json!({"params": {"input_soft_fallback": 250000,
                                "script_inline": "N = _int(\"input_soft_fallback\", 250000)\n"},
                     "contract": {"settings": {"input_soft_fallback": {"default": 250000}}}});
    check("x/y", &doc, &mut wrong);
    assert_eq!(wrong.len(), 4, "{wrong:?}");
    assert!(wrong[0].contains("params.input_soft_fallback = 250000"));
    assert!(wrong[1].contains("settings.input_soft_fallback.default"));
    assert!(wrong[2].contains("reads input_soft_fallback with \"250000\""));
    assert!(wrong[3].contains("never says"));
    let mut wrong = Vec::new();
    let doc = json!({"params": {"input_soft_fallback": 17500,
        "script_inline": "N = _int(\"input_soft_fallback\", 17500)\n\
                          sys.stderr.write(\"x: no catalog row for %s\" % m)\n"}});
    check("x/y", &doc, &mut wrong);
    assert!(wrong.is_empty(), "{wrong:?}");
    assert!(names_the_row("\"input_soft_fallback_row\": \"a/b\""));
    assert!(!names_the_row("\"embed_input_soft_fallback_row\": \"a/b\""));
}

/// One dispatch on a tool call: (stderr, the `input_soft` handed to the tool).
fn dispatch(cell: &str, header: Value) -> (String, Value) {
    let script = meclaw_testing::shipped_script(&format!("../../templates/{cell}/config.json"));
    let stdin = json!({
        "envelope": {"header": header},
        "body": {"messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                               "text": "{\"name\": \"search\", \"arguments\": {}}"}]},
        "params": {"input_soft_fallback": 17500},
    })
    .to_string();
    let out = meclaw_testing::run_shipped_script(&script, &stdin);
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{cell}: {err}");
    let emitted: Value = sj::from_str(String::from_utf8_lossy(&out.stdout).trim())
        .unwrap_or_else(|e| panic!("{cell}: {e}: {err}"));
    (err, emitted[1]["header"]["input_soft"].clone())
}

#[test]
fn a_dispatch_without_a_window_says_it_falls_back() {
    for cell in ["coder-pipeline/dispatch", "research-assistant/dispatch"] {
        if !ships(cell) {
            continue;
        }
        let (err, soft) = dispatch(cell, json!({"hop": {"model": "some/model"}}));
        assert_eq!(soft, json!(FALLBACK), "{cell}");
        assert!(
            err.contains("no catalog row for some/model"),
            "{cell}: {err}"
        );
        let (err, _) = dispatch(cell, json!({}));
        assert!(err.contains("no catalog row for ?"), "{cell}: {err}");
        // A window on the hop: no fallback, nothing said.
        let (err, soft) = dispatch(cell, json!({"hop": {"input_soft": 1000}}));
        assert_eq!(soft, json!(1000), "{cell}");
        assert!(!err.contains(SAID), "{cell}: {err}");
    }
}
