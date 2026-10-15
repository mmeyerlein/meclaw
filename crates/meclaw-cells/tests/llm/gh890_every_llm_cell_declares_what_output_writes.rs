//! GH #890 (F15 of the gate wave, OR-KX-G8) -- every shipped `llm` cell
//! declares every hop key the cell writes.
//!
//! The `llm` cell writes its hop header in one module
//! (`crates/meclaw-cells/src/llm/output.rs`), and that module names every key
//! it writes in ONE constant, `meclaw_cells::llm::HOP_KEYS`, with its JSON
//! type. Its own unit test holds the constant to the emitters; this sweep holds
//! the templates to the constant. Before it, `latency_ms`, `tokens_cached` and
//! `cost` were written on every answer and declared by none of the 18 shipped
//! `llm` cells, and nothing failed, because the contract check leaves
//! undeclared slots open (`meclaw-core/src/contract.rs`). The key list is read
//! from the constant, never copied here: a key the cell grows is a red sweep
//! until every template says so.
//!
//! One wording per key, the one the cells that already declared a key used
//! (GH #886 did the same for `model`): `{"type": <type>, "required": false}`.
//! `required: false` because no key is on every emission -- an error carries no
//! usage, an answer no `error_code`, and `talky`/`cogny` also emit credential
//! requests that carry none of them. `finish_reason` is the one exception: the
//! cells carry it in four forms (with or without the `values` list, required
//! or not where the cell also asks for credentials), all four true, so only
//! its type is held.
//!
//! Static: no colony, no provider. It reads whatever `templates/` carries, so
//! it is green in the published tree too (see `MIN_LLM_CELLS`).

use meclaw_cells::llm::HOP_KEYS;
use serde_json::{Value, json};

/// The floor of `gh886_every_llm_cell_declares_hop_model.rs`: 18 `llm` cells in
/// the development tree, 12 in the published subset, counted 2026-09-28. The
/// smaller count, so the sweep cannot pass by finding nothing.
const MIN_LLM_CELLS: usize = 12;

/// The one key whose wording varies between true declarations.
const TYPE_ONLY: &str = "finish_reason";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn the_wording(ty: &str) -> Value {
    json!({"type": ty, "required": false})
}

/// Every way `config` falls short of declaring `HOP_KEYS`, one line each.
fn complaints(config: &Value) -> Vec<String> {
    let Some(hop) = config.pointer("/contract/emits/hop") else {
        return vec!["declares no `contract.emits.hop`".to_string()];
    };
    let mut out = Vec::new();
    for (key, ty) in HOP_KEYS {
        match hop.get(*key) {
            None => out.push(format!("`contract.emits.hop` does not declare `{key}`")),
            Some(d) if *key == TYPE_ONLY => {
                if d.get("type") != Some(&json!(ty)) {
                    out.push(format!("`{key}` is {d}; its type is `{ty}`"));
                }
            }
            Some(d) if *d == the_wording(ty) => {}
            Some(d) => out.push(format!(
                "`{key}` is {d}; the shipped wording is {}",
                the_wording(ty)
            )),
        }
    }
    out
}

#[test]
fn every_shipped_llm_cell_declares_what_output_writes() {
    assert!(
        HOP_KEYS.iter().any(|(k, _)| *k == TYPE_ONLY),
        "the exception names a key the cell no longer writes"
    );
    let mut judged = 0usize;
    let mut findings: Vec<String> = Vec::new();
    let mut stack = vec![repo("templates")];
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
            let raw = std::fs::read_to_string(&path).expect("a config.json is readable");
            // Parseability is not this gate's question (the boot judges it).
            let Ok(config) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            if config.pointer("/cell/type").and_then(|t| t.as_str()) != Some("llm") {
                continue;
            }
            judged += 1;
            let rel = path.strip_prefix(repo("")).unwrap_or(&path).to_path_buf();
            for c in complaints(&config) {
                findings.push(format!("{}: {c}", rel.display()));
            }
        }
    }
    findings.sort();
    assert!(
        judged >= MIN_LLM_CELLS,
        "the sweep judged only {judged} llm cells -- it is not finding the tree"
    );
    assert!(
        findings.is_empty(),
        "these llm cells do not declare every hop key the cell writes \
         (meclaw_cells::llm::HOP_KEYS, GH #890):\n  {}",
        findings.join("\n  ")
    );
}

#[test]
fn the_verdict_holds_every_key_to_its_wording() {
    // Same function as the sweep, fabricated input, no file touched.
    let full: serde_json::Map<String, Value> = HOP_KEYS
        .iter()
        .map(|(k, ty)| (k.to_string(), the_wording(ty)))
        .collect();
    let with_hop = |hop: serde_json::Map<String, Value>| json!({"contract": {"emits": {"hop": Value::Object(hop)}}});

    // 1. Every key in its wording passes.
    assert!(complaints(&with_hop(full.clone())).is_empty());
    // 2. No `contract.emits.hop` at all.
    assert_eq!(complaints(&json!({"contract": {"emits": {}}})).len(), 1);
    // 3. One key missing is one complaint naming it.
    let mut missing = full.clone();
    missing.remove("latency_ms");
    let c = complaints(&with_hop(missing));
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(c[0].contains("latency_ms"), "{c:?}");
    // 4. `required: true` -- no key is on every emission.
    let mut required = full.clone();
    required.insert("cost".into(), json!({"type": "number", "required": true}));
    assert_eq!(complaints(&with_hop(required)).len(), 1);
    // 5. The wrong type.
    let mut typed = full.clone();
    typed.insert(
        "cache_expires_at".into(),
        json!({"type": "number", "required": false}),
    );
    assert_eq!(complaints(&with_hop(typed)).len(), 1);
    // 6. `finish_reason` in any true form passes, with the wrong type it fails.
    let mut answer_only = full.clone();
    answer_only.insert(
        TYPE_ONLY.into(),
        json!({"type": "string", "values": ["stop", "length", "tool_calls",
               "content_filter", "error"], "required": true}),
    );
    assert!(complaints(&with_hop(answer_only)).is_empty());
    let mut wrong = full;
    wrong.insert(
        TYPE_ONLY.into(),
        json!({"type": "number", "required": false}),
    );
    assert_eq!(complaints(&with_hop(wrong)).len(), 1);
}
