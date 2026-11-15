//! Wave d58-3, strand `schnell` (GH #1529, contract point 4) -- the curator's
//! policy and the lead name things after what they do, never after a model
//! cell of a composition.
//!
//! WHY this file exists: the curator is omni (R-58-7) and the lead chooses
//! between targets that a composition names (GH #1474): neither knows which
//! cells a composition wires. Yet the policy and the lead still carried
//! names of talky's cells in their identifiers and params -- `quick_scope`,
//! `quick_items`, `QUICK_NO_KINDS`, `quick_speculative`, `quick_max_iter`,
//! `memory_to_brain`, `brain_busy` --, a reader took them for a property of
//! one product, and a composition whose fast cell has another name read a
//! knob named after a cell it does not have. Now they are named after their
//! function (`spec_*`, `fast_*`, ...). The rule runs on the model cells of
//! the shipped compositions with a curator, read off the tree (`cell.type`
//! `llm`), never on a list in this file: an identifier, a short string or a
//! param key of the two scripts whose underscore parts hold such a cell's
//! name is a finding.
//!
//! Reads the shipped templates; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The names of the model cells (`cell.type` `llm`) of every shipped
/// composition that wires a curator (a template directory with a `curator`
/// cell), versioned copies (`name@x.y.z`) left out.
fn composition_model_cells() -> Vec<String> {
    let mut out = Vec::new();
    let root = repo("templates");
    let mut comps: Vec<_> = std::fs::read_dir(&root)
        .expect("templates")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("curator").is_dir())
        .filter(|p| !p.to_string_lossy().contains('@'))
        .collect();
    comps.sort();
    for comp in comps {
        let mut cells: Vec<_> = std::fs::read_dir(&comp)
            .expect("composition")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.join("config.json").is_file())
            .collect();
        cells.sort();
        for cell in cells {
            let cfg = read_json(&cell.join("config.json"));
            let name = cell
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if cfg["cell"]["type"] == json!("llm") && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// The finder: every NAME token, every string of 3 to 60 letters, dots,
/// colons or underscores, and every param key of a script whose lower-case
/// underscore parts hold one of the cell names as a run of whole parts.
const FINDER: &str = r#"
import io, json, sys, tokenize, re
inp = json.load(sys.stdin)
cells = [c.lower().split("_") for c in inp["cells"]]
def hit(name):
    parts = [p for p in re.split(r"[^a-z0-9]+", name.lower()) if p]
    for c in cells:
        for i in range(len(parts) - len(c) + 1):
            if parts[i:i + len(c)] == c:
                return True
    return False
out = []
for script in inp["programs"]:
    seen = set()
    for t in tokenize.generate_tokens(io.StringIO(script["src"]).readline):
        text = t.string
        if t.type == tokenize.NAME or (t.type == tokenize.STRING
                and re.fullmatch(r"[\"'][A-Za-z_.:]{3,60}[\"']", text)):
            if hit(text.strip("\"'")) and text not in seen:
                seen.add(text)
                out.append("%s:%d %s" % (script["cell"], t.start[0], text))
    for k in script["params"]:
        if hit(k):
            out.append("%s params %s" % (script["cell"], k))
print(json.dumps(out))
"#;

/// The script and the param keys of a shipped cell config.
fn script(cell: &str, cfg: &Value) -> Value {
    let params: Vec<String> = cfg["params"]
        .as_object()
        .map(|m| {
            m.keys()
                .filter(|k| *k != "script_inline")
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    json!({"cell": cell, "src": cfg["params"]["script_inline"], "params": params})
}

/// Pin: no identifier, short string or param key of the policy and the lead
/// carries the name of a model cell of a composition.
#[test]
fn gh_d583_schnell_no_composition_cell_name_in_identifiers() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let cells = composition_model_cells();
    assert!(
        cells.len() >= 2,
        "check of the setup: the shipped compositions have model cells: {cells:?}"
    );
    let doc = json!({"cells": cells,
                     "programs": [script("curator/policy", &cell_config("policy")),
                                  script("lead/lead", &lead_config("lead"))]});
    let out = run_python(FINDER, &doc.to_string());
    assert!(
        out.status.success(),
        "the finder runs: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let found: Vec<String> = sj::from_slice(&out.stdout).expect("a JSON list");
    assert!(
        found.is_empty(),
        "names of model cells of a composition ({cells:?}) in the policy and the lead: {found:#?}"
    );
}
