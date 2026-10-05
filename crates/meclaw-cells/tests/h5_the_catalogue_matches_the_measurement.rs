//! GH #1000 -- the catalogue says what each model takes, and that is what was
//! measured.
//!
//! The `llm` cell sends a sampling field only when the row's
//! `supported_params` names it (GH #993). A list written by hand is a guess:
//! measured on 2026-10-05, three of six hosted rows refuse `temperature` under
//! strict routing, and a strict 200 for a field the provider does not know
//! proves nothing. So every list comes from the conformance tool
//! (`workshop/tools/llm-conformance`), whose record lies next to the cell's
//! request fixtures as `fixtures/conformance/<slug>/measured.json`, and this
//! file keeps the two equal offline:
//!
//! 1. every `active` row on a chat or responses wire names its params;
//! 2. every such row has a measurement;
//! 3. the row's list, read on the sampling fields, is the measured `taken`
//!    set in the cell's order (`accepted_unverified` never counts);
//! 4. every held measurement passed the checks a catalogue list rests on
//!    (K1 shape, K2 usage, K3 params, K7 no stream);
//! 5. the tool's field list is the cell's (`SAMPLING_PARAMS`).
//!
//! Guarded like every other template-reading test (GH #49); the tool lies
//! under `workshop/`, which never travels, so 5 skips without it.

use meclaw_core::serde_json::{self, Map, Value};
use std::path::{Path, PathBuf};

/// The conformance tool; its `SAMPLING_PARAMS` mirrors the cell's.
const TOOL: &str = "workshop/tools/llm-conformance/conformance.py";

/// The wires a sampling list governs (`decisions` rows carry none).
const CHAT_WIRES: &[&str] = &["chat_completions", "responses"];

/// The checks a catalogue list rests on (README section 4 of the wave plan:
/// K4-K6 and K8 may be `n/a`, and K8 is the cell's request, not the model).
const LIST_CHECKS: &[&str] = &["K1", "K2", "K3", "K7"];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo(rel: &str) -> PathBuf {
    crate_dir().join("../..").join(rel)
}

fn catalogue() -> Option<PathBuf> {
    let p = crate_dir().join("../../templates/llm-registry/store/seed/models.jsonl");
    p.is_file().then_some(p)
}

fn conformance_dir() -> PathBuf {
    crate_dir().join("tests/fixtures/conformance")
}

fn slug(model_id: &str) -> String {
    model_id.replace('/', "__")
}

/// The quoted names between `open` and the next `close` -- the list form of
/// both the Rust const and the Python tuple.
fn quoted_list(src: &str, open: &str, close: &str) -> Vec<String> {
    let start = src
        .find(open)
        .unwrap_or_else(|| panic!("`{open}` moved -- update this lock"))
        + open.len();
    let end = start + src[start..].find(close).expect("the list ends");
    src[start..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// The cell's `SAMPLING_PARAMS`, read from its source (the const is
/// crate-private; the source always travels).
fn sampling_params() -> Vec<String> {
    let src =
        std::fs::read_to_string(crate_dir().join("src/llm/translate.rs")).expect("translate.rs");
    quoted_list(&src, "const SAMPLING_PARAMS: &[&str] = &[", "];")
}

/// The active rows on a chat or responses wire.
fn chat_rows(path: &Path) -> Vec<Map<String, Value>> {
    std::fs::read_to_string(path)
        .expect("catalogue")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Map<String, Value>>(l).expect("a json row"))
        .filter(|r| !r.contains_key("schema"))
        .filter(|r| r.get("status").and_then(|v| v.as_str()) == Some("active"))
        .filter(|r| {
            r.get("wire_dialect")
                .and_then(|v| v.as_str())
                .is_some_and(|w| CHAT_WIRES.contains(&w))
        })
        .collect()
}

fn model_id(row: &Map<String, Value>) -> String {
    row["model_id"].as_str().expect("model_id").to_string()
}

fn listed(row: &Map<String, Value>) -> Option<Vec<String>> {
    row.get("package")
        .and_then(|p| p.get("supported_params"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
}

fn measured_path(model_id: &str) -> PathBuf {
    conformance_dir().join(slug(model_id)).join("measured.json")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("readable")).expect("json")
}

fn remeasure(model_id: &str) -> String {
    format!(
        "re-measure with `python3 workshop/tools/llm-conformance/conformance.py --model {model_id} \
         --base-url <url> --key-env <VAR> --record <dir>` and copy <dir>/{}/ (without cost.jsonl) \
         into crates/meclaw-cells/tests/fixtures/conformance/",
        slug(model_id)
    )
}

#[test]
fn h5_every_active_chat_row_names_its_params() {
    let Some(path) = catalogue() else {
        eprintln!("llm-registry did not travel into this tree -- skipped (GH #49)");
        return;
    };
    let rows = chat_rows(&path);
    assert!(!rows.is_empty(), "the catalogue has active chat rows");
    let bare: Vec<String> = rows
        .iter()
        .filter(|r| listed(r).is_none_or(|l| l.is_empty()))
        .map(model_id)
        .collect();
    assert!(
        bare.is_empty(),
        "{} of {} active chat rows carry no package.supported_params: {bare:?}",
        bare.len(),
        rows.len()
    );
}

#[test]
fn h5_every_active_chat_row_was_measured() {
    let Some(path) = catalogue() else {
        eprintln!("llm-registry did not travel into this tree -- skipped (GH #49)");
        return;
    };
    let missing: Vec<String> = chat_rows(&path)
        .iter()
        .map(model_id)
        .filter(|id| !measured_path(id).is_file())
        .map(|id| format!("{id}: {}", remeasure(&id)))
        .collect();
    assert!(
        missing.is_empty(),
        "rows without a measurement:\n{}",
        missing.join("\n")
    );
}

#[test]
fn h5_the_catalogue_says_what_was_measured() {
    let Some(path) = catalogue() else {
        eprintln!("llm-registry did not travel into this tree -- skipped (GH #49)");
        return;
    };
    let fields = sampling_params();
    let mut wrong = Vec::new();
    for row in chat_rows(&path) {
        let id = model_id(&row);
        let file = measured_path(&id);
        if !file.is_file() {
            continue; // h5_every_active_chat_row_was_measured names it
        }
        let params = read_json(&file)["params"].clone();
        // `taken` only: a strict 200 for an unlisted field, or any 200 from a
        // local server, is `accepted_unverified` and proves nothing.
        let taken: Vec<String> = fields
            .iter()
            .filter(|f| params.get(f.as_str()).and_then(|v| v.as_str()) == Some("taken"))
            .cloned()
            .collect();
        let list: Vec<String> = listed(&row)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| fields.contains(p))
            .collect();
        if list != taken {
            wrong.push(format!(
                "{id}: catalogue {list:?}, measured taken {taken:?} ({})\n  {}",
                file.display(),
                remeasure(&id)
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "catalogue and measurement differ (lists in SAMPLING_PARAMS order {fields:?}):\n{}",
        wrong.join("\n")
    );
}

#[test]
fn h5_the_measurement_is_green() {
    let mut red = Vec::new();
    let mut seen = 0;
    for entry in std::fs::read_dir(conformance_dir())
        .expect("fixtures/conformance")
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().to_string();
        let file = entry.path().join("measured.json");
        // `fake__` records come from the tool's fake provider (its own tests).
        if name.starts_with("fake__") || !file.is_file() {
            continue;
        }
        seen += 1;
        let checks = read_json(&file)["checks"].clone();
        for k in LIST_CHECKS {
            let status = checks[*k]["status"].as_str().unwrap_or("missing");
            if status != "green" {
                red.push(format!("{name} {k}: {status} -- {}", checks[*k]["note"]));
            }
        }
    }
    assert!(seen > 0, "no measured.json under fixtures/conformance");
    assert!(
        red.is_empty(),
        "a held measurement is not green:\n{}",
        red.join("\n")
    );
}

#[test]
fn h5_the_tool_mirrors_the_cell() {
    if !repo(TOOL).is_file() {
        eprintln!("SKIP: the conformance tool lies under workshop/, which did not travel");
        return;
    }
    let src = std::fs::read_to_string(repo(TOOL)).expect("conformance.py");
    let tool = quoted_list(&src, "\nSAMPLING_PARAMS = (", ")");
    assert_eq!(
        tool,
        sampling_params(),
        "the conformance tool's SAMPLING_PARAMS left the cell's (src/llm/translate.rs)"
    );
}
