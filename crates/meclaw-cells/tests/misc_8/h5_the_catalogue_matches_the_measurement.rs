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
//! 1. every `active` or `explicit` row on a chat, responses or native
//!    Messages wire names its params;
//! 2. every such row has a measurement;
//! 3. the row's list, read on the sampling fields, is the measured `taken`
//!    set in the cell's order (`accepted_unverified` never counts);
//! 4. every held measurement passed the checks a catalogue list rests on
//!    (K1 shape, K2 usage, K3 params, K7 no stream);
//! 5. the tool's field list is the cell's (`SAMPLING_PARAMS`);
//! 6. the native `claude-haiku-5-5` row (GH #1243) is one of those rows;
//! 7. a row measured on a pinned provider (`provider_order` in its record)
//!    pins exactly that provider without fallbacks, and a row that pins one
//!    was measured on it (GH #1260).
//!
//! Guarded like every other template-reading test (GH #49); the tool lies
//! under `workshop/`, which never travels, so 5 skips without it.

use meclaw_core::serde_json::{self, Map, Value};
use std::path::{Path, PathBuf};

/// The conformance tool; its `SAMPLING_PARAMS` mirrors the cell's.
const TOOL: &str = "workshop/tools/llm-conformance/conformance.py";

/// The wires a sampling list governs (`decisions` rows carry none). GH #1243:
/// `messages`, the native wire of a `provider: "anthropic"` cell, sends its
/// sampling fields by the same `supported_params` list.
const CHAT_WIRES: &[&str] = &["chat_completions", "responses", "messages"];

/// GH #1243: the native Claude row of the test colonies and village figures.
const HAIKU: &str = "claude-haiku-5-5";

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

/// GH #1085: the directory the tool records a row under -- its `slug_of`:
/// a `local` row (a server on the operator's own hardware) under
/// `local__<slug>`, every other row under its slug.
fn record_slug(row: &Map<String, Value>) -> String {
    let s = slug(&model_id(row));
    if is_local(row) {
        format!("local__{s}")
    } else {
        s
    }
}

/// A row the operator's own server runs (`provider` `local`).
fn is_local(row: &Map<String, Value>) -> bool {
    row.get("provider").and_then(|v| v.as_str()) == Some("local")
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

/// The reachable (`active` or `explicit`) rows on a chat or responses wire.
fn chat_rows(path: &Path) -> Vec<Map<String, Value>> {
    std::fs::read_to_string(path)
        .expect("catalogue")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Map<String, Value>>(l).expect("a json row"))
        .filter(|r| !r.contains_key("schema"))
        // GH #1025: an `explicit` row is pushed to a brain like an active one
        // (override, tier, model id), so its params are held to the same lock.
        .filter(|r| {
            matches!(
                r.get("status").and_then(|v| v.as_str()),
                Some("active" | "explicit")
            )
        })
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

fn measured_path(row: &Map<String, Value>) -> PathBuf {
    conformance_dir()
        .join(record_slug(row))
        .join("measured.json")
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
    // GH #1085: a local server answers every field with 200, so its
    // measurement verifies none (`accepted_unverified`) and its row names no
    // list -- the cell then sends the fields unverified, as it did before
    // GH #993. `h5_the_catalogue_says_what_was_measured` holds it to that.
    let bare: Vec<String> = rows
        .iter()
        .filter(|r| !is_local(r))
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
        .filter(|row| !measured_path(row).is_file())
        .map(model_id)
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
        let file = measured_path(&row);
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

/// GH #1243: the native row is held to its measurement like every chat row
/// (the three tests above then name its params, its record and its list);
/// its `response_ms` against `latency.json` is held by the registry
/// template's quick-tier test, like every timed row's.
#[test]
fn h5_the_native_haiku_row_rests_on_its_measurement() {
    let Some(path) = catalogue() else {
        eprintln!("llm-registry did not travel into this tree -- skipped (GH #49)");
        return;
    };
    let rows = chat_rows(&path);
    let Some(row) = rows.iter().find(|r| model_id(r) == HAIKU) else {
        panic!(
            "no held row `{HAIKU}` on a wire of {CHAT_WIRES:?} in {}",
            path.display()
        );
    };
    let field = |k: &str| row.get(k).and_then(|v| v.as_str()).unwrap_or_default();
    assert_eq!(
        (field("provider"), field("wire_dialect"), field("status")),
        ("anthropic", "messages", "explicit"),
        "{HAIKU}: the native Messages row, reached by name only"
    );
}

/// The provider order a row's package pins: `provider_extra.provider.order`,
/// or `None` for a row on the gateway's default routing.
fn pinned_order(row: &Map<String, Value>) -> Option<Value> {
    row.get("package")
        .and_then(|p| p.get("provider_extra"))
        .and_then(|e| e.get("provider"))
        .and_then(|p| p.get("order"))
        .cloned()
}

/// GH #1260 (Lab P1): the same open-weight model answered a tool call in a
/// p50 of 1624 ms on the gateway's default routing and of 231 ms pinned to
/// one fast host. So a row's `response_ms` is a claim about a provider: a
/// row measured pinned (`provider_order` in `measured.json`, written by the
/// tool's `--provider-order`) carries that order in `provider_extra` with
/// `allow_fallbacks: false` -- a silent fallback would serve the slow host
/// under the fast time -- and a row that pins a provider rests on a
/// measurement on exactly that provider.
#[test]
fn h5_a_pinned_measurement_is_a_pinned_row() {
    let Some(path) = catalogue() else {
        eprintln!("llm-registry did not travel into this tree -- skipped (GH #49)");
        return;
    };
    let mut wrong = Vec::new();
    for row in chat_rows(&path) {
        let id = model_id(&row);
        let file = measured_path(&row);
        if !file.is_file() {
            continue; // h5_every_active_chat_row_was_measured names it
        }
        let measured = read_json(&file).get("provider_order").cloned();
        let pinned = pinned_order(&row);
        if measured != pinned {
            wrong.push(format!(
                "{id}: measured on {measured:?}, the row pins {pinned:?} ({})",
                file.display()
            ));
            continue;
        }
        if pinned.is_some() {
            let fallbacks = row["package"]["provider_extra"]["provider"].get("allow_fallbacks");
            if fallbacks != Some(&Value::Bool(false)) {
                wrong.push(format!(
                    "{id}: pins {pinned:?} but allow_fallbacks is {fallbacks:?}, not false"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "provider order of catalogue and measurement differ:\n{}",
        wrong.join("\n")
    );
}
