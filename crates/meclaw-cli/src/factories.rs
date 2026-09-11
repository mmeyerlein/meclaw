//! Phase-9: factory registry wiring for all built-in cell types.
//!
//! Maps cell-type strings to their `CellFactory` implementations. The five
//! phase-7 tool cells (file, bash, edit, web_fetch, web_search) plus the
//! phase-8 `llm` cell, the phase-9 `store` cell and the phase-9 `code` cell are
//! registered here. `base_path`/`endpoint` and other instance-specific params
//! come from the `override_params` of the node definitions, NOT from the
//! registration.
//!
//! **Layer**: `meclaw-cli` is the bootstrap layer — it may import
//! `meclaw-cells`. `meclaw-colony` may NOT (layering invariant from the phase-7
//! start).

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::harness::HarnessCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::subcolony::SubcolonyCellFactory;
use meclaw_cells::vault::VaultCellFactory;
use meclaw_cells::{
    BashCellFactory, EditCellFactory, FileCellFactory, LlmCellFactory, McpCellFactory,
    ProxyCellFactory, TimerCellFactory, VoiceCellFactory, WebCellFactory, WebFetchCellFactory,
    WebSearchCellFactory,
};
use meclaw_colony::{CellFactoryRegistry, SurfaceRegistry};
use std::sync::Arc;

/// Build the registry of built-in cell-type factories for Phase 9.
///
/// Registered templates:
/// - `"bash"` → `BashCellFactory` (stateless shell one-shot)
/// - `"code"` → `CodeCellFactory` (stateless script runner, Python 3)
/// - `"edit"` → `EditCellFactory` (find_replace + insert_at_line)
/// - `"file"` → `FileCellFactory` (stateless file CRUD with security boundary)
/// - `"llm"` → `LlmCellFactory` (stateful LLM-completion via HTTP provider)
/// - `"store"` → `StoreCellFactory` (stateful SQLite store-Cell)
/// - `"web_fetch"` → `WebFetchCellFactory` (stateless HTTP GET)
/// - `"web_search"` → `WebSearchCellFactory` (generic JSON search wrapper)
/// - `"proxy"` → `ProxyCellFactory` (long-running chat-platform ingress/egress)
/// - `"timer"` → `TimerCellFactory` (long-running scheduled-tick source)
/// - `"mcp"` → `McpCellFactory` (long-running MCP tool bridge)
/// - `"harness"` → `HarnessCellFactory` (long-running agent-harness supervisor)
/// - `"vault"` → `VaultCellFactory` (sealed secret store; no operation returns a secret)
/// - `"voice"` → `VoiceCellFactory` (long-running voice channel bridge: audio in, turns out)
///
/// Returns an owned `CellFactoryRegistry` (`HashMap<String, Arc<dyn CellFactory>>`).
/// Callers move or clone as needed.
///
/// `surfaces` is the process's mount table (ADR-0031). The surface factories
/// (`web`, `voice`) are handed the same `Arc`, so a cell that mounts by name is
/// reachable from the one listener and from `GET /colony/surfaces`; every other
/// factory ignores it.
///
/// GH #434: the key set of this registry is kept set-equal to the shipped
/// catalogue in `docs/cell-types.md` § Overview (minus `hive`, a scope marker
/// with no factory) by
/// `crates/meclaw-cli/tests/gh325_the_registry_spawns_what_the_catalogue_lists.rs`.
/// A new entry here is an undocumented capability until that table grows with
/// it — in **both** language editions.
pub fn built_in_factories(surfaces: Arc<SurfaceRegistry>) -> CellFactoryRegistry {
    let mut reg = CellFactoryRegistry::new();
    reg.insert("bash".to_string(), Arc::new(BashCellFactory));
    reg.insert("code".to_string(), Arc::new(CodeCellFactory));
    reg.insert("edit".to_string(), Arc::new(EditCellFactory));
    reg.insert("file".to_string(), Arc::new(FileCellFactory));
    reg.insert("llm".to_string(), Arc::new(LlmCellFactory));
    reg.insert("store".to_string(), Arc::new(StoreCellFactory));
    reg.insert("web_fetch".to_string(), Arc::new(WebFetchCellFactory));
    reg.insert("web_search".to_string(), Arc::new(WebSearchCellFactory));
    // Befund 3: long-running factories — exist in `meclaw-cells` but were
    // unreachable from the binary until now (bootstrap / mutation rejected
    // `unknown_cell_type` for proxy/timer/mcp topologies).
    reg.insert("proxy".to_string(), Arc::new(ProxyCellFactory));
    reg.insert("timer".to_string(), Arc::new(TimerCellFactory));
    reg.insert("mcp".to_string(), Arc::new(McpCellFactory));
    reg.insert("harness".to_string(), Arc::new(HarnessCellFactory));
    // P9: a whole child colony, driven as one cell over the JSON stdio wire.
    reg.insert("subcolony".to_string(), Arc::new(SubcolonyCellFactory));
    // GH #151: the vault. A stateful cell type whose route surface has no read
    // on it — see `meclaw_cells::vault`.
    reg.insert("vault".to_string(), Arc::new(VaultCellFactory));
    // GH #380: the display substrate. Long-running like proxy/timer/mcp, and
    // deliberately multiple — each instance registers its own mount on the one
    // listener (`web@2.0.0`: no port, no bind).
    reg.insert(
        "web".to_string(),
        Arc::new(WebCellFactory::new(Arc::clone(&surfaces))),
    );
    // Wave voice-cell: the second mounted channel bridge. Long-running and
    // deliberately multiple, like `web` — each instance registers its own mount
    // on the one listener (`voice@2.0.0`: no port, no bind).
    reg.insert(
        "voice".to_string(),
        Arc::new(VoiceCellFactory::new(surfaces)),
    );
    reg
}

#[cfg(test)]
mod tests {
    use super::built_in_factories;
    use meclaw_colony::SurfaceRegistry;
    use std::sync::Arc;

    /// The mount table every test in this module hands the registry: the
    /// factories share one, and nothing here reads it back.
    fn surfaces() -> Arc<SurfaceRegistry> {
        Arc::new(SurfaceRegistry::new())
    }

    #[test]
    fn registry_has_all_phase9_templates() {
        let reg = built_in_factories(surfaces());
        for name in &[
            "bash",
            "code",
            "edit",
            "file",
            "llm",
            "store",
            "web_fetch",
            "web_search",
        ] {
            assert!(reg.contains_key(*name), "registry missing template: {name}");
        }
    }

    /// Substrat-Fix Befund 3: the three long-running factories
    /// (`proxy`/`timer`/`mcp`) exist in `meclaw-cells` but were unreachable from
    /// the binary — bootstrap failed `UnknownCellType("proxy")` and mutations
    /// rejected `unknown_cell_type`. They must be wired into the CLI registry
    /// alongside the eight Phase-9 types (11 total).
    #[test]
    fn registry_wires_proxy_timer_mcp_factories() {
        let reg = built_in_factories(surfaces());
        for name in &["proxy", "timer", "mcp", "harness"] {
            assert!(
                reg.contains_key(*name),
                "registry missing long-running factory: {name}"
            );
        }
        assert_eq!(
            reg.len(),
            16,
            "8 Phase-9 + proxy/timer/mcp + harness + subcolony + vault + web + voice"
        );
    }

    /// Wave voice-cell: a topology declaring a `voice` cell must not fail to
    /// boot with `unknown_cell_type` — the hole the long-running factories sat
    /// in before Befund 3, and the vault after them.
    #[test]
    fn registry_wires_voice_factory() {
        let reg = built_in_factories(surfaces());
        assert!(reg.contains_key("voice"), "registry missing the voice cell");
        // A voice endpoint without a mount and a speech-to-text provider is
        // not an endpoint: both are refused here, at boot-plan time.
        assert!(
            reg["voice"]
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_err()
        );
        assert!(
            reg["voice"]
                .validate_params(&meclaw_core::serde_json::json!({
                    "mount": "voice",
                    "stt": {"provider": "echo"}
                }))
                .is_ok(),
            "the echo provider needs no credential and no voice to speak with"
        );
    }

    #[test]
    fn registry_factories_validate_minimal_params() {
        let reg = built_in_factories(surfaces());
        // file/edit/web_search need mandatory params — pure validate_params rejects an empty object.
        assert!(
            reg["file"]
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_err()
        );
        assert!(
            reg["edit"]
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_err()
        );
        assert!(
            reg["web_search"]
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_err()
        );
        // store needs a schema (mandatory) — an empty object must be rejected.
        assert!(
            reg["store"]
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_err()
        );
        // code needs a runner (mandatory) — an empty object must be rejected.
        assert!(
            reg["code"]
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_err(),
            "code validate_params must reject empty params (missing runner)"
        );
        // bash/web_fetch akzeptieren leeres Object (Defaults).
        assert!(
            reg["bash"]
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_ok()
        );
        assert!(
            reg["web_fetch"]
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_ok()
        );
    }

    /// GH #151: the vault has to be reachable from the binary, or a topology
    /// declaring one fails to boot with `unknown_cell_type` — the same hole the
    /// long-running factories sat in before Befund 3.
    #[test]
    fn registry_wires_the_vault_factory() {
        let reg = built_in_factories(surfaces());
        assert!(reg.contains_key("vault"), "registry missing the vault");
        // A vault without a broker is not a vault: validation refuses it here,
        // at boot-plan time, not at the first message.
        assert!(
            reg["vault"]
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_err()
        );
        assert!(
            reg["vault"]
                .validate_params(&meclaw_core::serde_json::json!({"broker": "/main/access/broker"}))
                .is_ok()
        );
    }

    #[test]
    fn registry_llm_validate_params_rejects_empty() {
        let reg = built_in_factories(surfaces());
        assert!(
            reg["llm"]
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_err(),
            "llm validate_params must reject empty params (missing provider/model/api_key)"
        );
    }
}
