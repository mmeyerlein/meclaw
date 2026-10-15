//! GH #886 -- every shipped `llm` cell declares `hop.model`, in ONE wording.
//!
//! The `llm` cell writes the model the provider actually served into the
//! header of every answer (`crates/meclaw-cells/src/llm/output.rs`,
//! `emit_assistant_turn`); the error path puts it into `meta` only
//! (`emit_error`), so the slot is `required: false`. The cost report groups
//! by it (`docs/costs.en.md`, `$.hop.model`). Ten cells declared it and eight
//! did not -- `talky/brain`, `cogny/brain`, `llm-registry/translate`,
//! `summarizer/writer`, `coder-pipeline/{planner,coder,reviewer}`,
//! `research-assistant/planner` -- and nothing failed, because the contract
//! check leaves undeclared slots open (`meclaw-core/src/contract.rs`). The
//! contract simply did not say what the cell emits. Two phrasings of one
//! promise are two promises, so the sweep holds every cell to the wording the
//! ten already used, copied rather than reformulated.
//!
//! Static: no colony, no provider. It reads whatever `templates/` carries,
//! so it is green in the published tree too (see `MIN_LLM_CELLS`).

use serde_json::{Value, json};

/// Counted, not estimated (2026-09-28): 18 `llm` cells in the development
/// tree, 12 in the published subset. The floor is the smaller count, so the
/// sweep cannot pass by finding nothing, and a cell that leaves the published
/// subset is re-counted here instead of silently shrinking the sweep.
const MIN_LLM_CELLS: usize = 12;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The one wording, as `templates/memory-hive/closer/config.json` ships it.
fn the_wording() -> Value {
    json!({"type": "string", "required": false})
}

/// `None` iff `config` declares `contract.emits.hop.model` exactly as
/// [`the_wording`]; otherwise what is wrong, in one line.
fn complaint(config: &Value) -> Option<String> {
    let Some(hop) = config.pointer("/contract/emits/hop") else {
        return Some("declares no `contract.emits.hop`".to_string());
    };
    match hop.get("model") {
        None => Some("`contract.emits.hop` does not declare `model`".to_string()),
        Some(m) if *m == the_wording() => None,
        Some(m) => Some(format!(
            "`contract.emits.hop.model` is {m}; the shipped wording is {}",
            the_wording()
        )),
    }
}

#[test]
fn every_shipped_llm_cell_declares_the_model_it_ran_on() {
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
            let config: Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                // Not this gate's question -- `gh221_shipped_template_versions`
                // and the boot itself judge parseability.
                Err(_) => continue,
            };
            if config
                .get("cell")
                .and_then(|c| c.get("type"))
                .and_then(|t| t.as_str())
                != Some("llm")
            {
                continue;
            }
            judged += 1;
            let rel = path.strip_prefix(repo("")).unwrap_or(&path).to_path_buf();
            if let Some(c) = complaint(&config) {
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
        "these llm cells emit `hop.model` (output.rs, emit_assistant_turn) but \
         do not declare it in the one wording \
         {{\"type\": \"string\", \"required\": false}} (GH #886):\n  {}",
        findings.join("\n  ")
    );
}

#[test]
fn the_verdict_takes_only_the_one_wording() {
    // Same function as the sweep, fabricated input, no file touched.
    let with_hop = |hop: Value| json!({"contract": {"emits": {"hop": hop}}});

    // 1. The one wording passes.
    assert_eq!(
        complaint(&with_hop(
            json!({"model": {"type": "string", "required": false}})
        )),
        None
    );
    // 2. No `contract.emits.hop` at all.
    assert!(complaint(&json!({"contract": {"emits": {}}})).is_some());
    // 3. `hop` without `model`.
    assert!(complaint(&with_hop(json!({"route": {"type": "string"}}))).is_some());
    // 4. `required: true` -- the error path does not write the slot.
    assert!(
        complaint(&with_hop(
            json!({"model": {"type": "string", "required": true}})
        ))
        .is_some()
    );
    // 5. The wrong type.
    assert!(
        complaint(&with_hop(
            json!({"model": {"type": "number", "required": false}})
        ))
        .is_some()
    );
    // 6. An extra key: a second phrasing is a second promise.
    assert!(
        complaint(&with_hop(json!({"model": {
            "type": "string",
            "required": false,
            "description": "the served model"
        }})))
        .is_some()
    );
}
