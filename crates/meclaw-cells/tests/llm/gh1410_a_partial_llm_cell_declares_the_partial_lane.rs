//! GH #1410 -- an `llm` cell with `partial_unit` on declares the hop keys of
//! the `partial` lane.
//!
//! The partial keys (`route`, `seq`, `final`, `partial_of`) live in their own
//! list, `meclaw_cells::llm::PARTIAL_HOP_KEYS`, not in `HOP_KEYS`: only a cell
//! that streams partials writes them, so only such a cell has to declare them
//! (the GH #890 sweep holds every cell to `HOP_KEYS`). This sweep reads every
//! `llm` cell under `templates/`; one whose params carry `partial_unit` at all
//! -- on, or shipped `off` and switched on by a set (GH #1411 A1) -- must
//! declare each key as `{"type": <type>, "required": false}` (no key is on
//! every emission: the answer carries none of them); `route` may keep a
//! `values` list when the list names `partial`.
//!
//! Static: no colony, no provider. A tree without such a cell passes; the
//! verdict itself is pinned on fabricated input below.

use meclaw_cells::llm::PARTIAL_HOP_KEYS;
use serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Whether `config` can stream partials: its params carry `partial_unit` at
/// all. A set switches it on through `override_params`, which this static
/// sweep never sees -- so a cell shipped `off` declares the lane as well
/// (GH #1411 A1: `./quick_spec` ships `off` and is switched on by the set,
/// like `./quick` and `./brain`).
fn can_stream_partials(config: &Value) -> bool {
    config.pointer("/params/partial_unit").is_some()
}

/// Whether `d` declares the `route` key so that `partial` passes: the
/// wording, or the wording with a `values` list that names `partial`.
fn route_admits_partial(d: &Value, ty: &str) -> bool {
    let Some(m) = d.as_object() else {
        return false;
    };
    let values_ok = match m.get("values") {
        None => true,
        Some(v) => v
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x == "partial")),
    };
    m.get("type") == Some(&json!(ty))
        && m.get("required") == Some(&json!(false))
        && values_ok
        && m.keys()
            .all(|k| ["type", "required", "values", "description"].contains(&k.as_str()))
}

/// Every way `config` falls short of declaring the partial lane.
fn complaints(config: &Value) -> Vec<String> {
    if !can_stream_partials(config) {
        return Vec::new();
    }
    let hop = config.pointer("/contract/emits/hop");
    PARTIAL_HOP_KEYS
        .iter()
        .filter_map(|(key, ty)| {
            let want = json!({"type": ty, "required": false});
            match hop.and_then(|h| h.get(*key)) {
                Some(d) if *key == "route" && route_admits_partial(d, ty) => None,
                Some(d) if *d == want => None,
                Some(d) => Some(format!("`{key}` is {d}; the wording is {want}")),
                None => Some(format!("`contract.emits.hop` does not declare `{key}`")),
            }
        })
        .collect()
}

#[test]
fn every_llm_cell_with_partials_declares_the_partial_lane() {
    let mut findings = Vec::new();
    let mut stack = vec![repo("templates")];
    let mut llm_cells = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) != Some("config.json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(config) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            if config.pointer("/cell/type").and_then(Value::as_str) != Some("llm") {
                continue;
            }
            llm_cells += 1;
            for c in complaints(&config) {
                findings.push(format!("{}: {c}", path.display()));
            }
        }
    }
    assert!(
        llm_cells > 0,
        "the sweep found no llm cell -- it is not finding the tree"
    );
    assert!(
        findings.is_empty(),
        "llm cells with partial_unit on that do not declare the partial lane \
         (meclaw_cells::llm::PARTIAL_HOP_KEYS, GH #1410):\n  {}",
        findings.join("\n  ")
    );
}

#[test]
fn the_verdict_asks_only_a_partial_cell() {
    let full: serde_json::Map<String, Value> = PARTIAL_HOP_KEYS
        .iter()
        .map(|(k, ty)| (k.to_string(), json!({"type": ty, "required": false})))
        .collect();
    let cell = |unit: Option<&str>, hop: serde_json::Map<String, Value>| {
        let mut c =
            json!({"cell": {"type": "llm"}, "contract": {"emits": {"hop": Value::Object(hop)}}});
        if let Some(u) = unit {
            c["params"] = json!({"partial_unit": u});
        }
        c
    };
    // Without the param: nothing asked.
    assert!(complaints(&cell(None, serde_json::Map::new())).is_empty());
    // Shipped `off` but switchable (GH #1411 A1): asked like a cell that is on.
    assert_eq!(
        complaints(&cell(Some("off"), serde_json::Map::new())).len(),
        PARTIAL_HOP_KEYS.len()
    );
    assert!(complaints(&cell(Some("off"), full.clone())).is_empty());
    // On: every key, in its wording.
    assert!(complaints(&cell(Some("sentence"), full.clone())).is_empty());
    assert_eq!(
        complaints(&cell(Some("sentence"), serde_json::Map::new())).len(),
        PARTIAL_HOP_KEYS.len()
    );
    let mut wrong = full.clone();
    wrong.insert("final".into(), json!({"type": "string", "required": false}));
    assert_eq!(complaints(&cell(Some("sentence"), wrong)).len(), 1);
    // A route with a `values` list passes only when the list names `partial`.
    let mut listed = full.clone();
    listed.insert(
        "route".into(),
        json!({"type": "string", "values": ["credential_request", "partial"], "required": false}),
    );
    assert!(complaints(&cell(Some("off"), listed)).is_empty());
    let mut closed = full;
    closed.insert(
        "route".into(),
        json!({"type": "string", "values": ["credential_request"], "required": false}),
    );
    assert_eq!(complaints(&cell(Some("off"), closed)).len(), 1);
}
