use crate::factory::ContractView;
use meclaw_core::JsonValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Parsed representation of the `contract` block in a cell's `config.json`.
///
/// Extended in Paket 7 with `emits` (P13/D-010a). The `From<ContractBlock> for
/// ContractView` still only maps `multi_send_capable`; the fallible
/// `emits`-compile step is done separately (B3) so boot errors can be surfaced
/// as `BootstrapError` rather than silently swallowed inside an infallible `From`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ContractBlock {
    /// Whether this cell may emit multiple messages per input.
    pub multi_send_capable: bool,
    /// Declared output contract — body + hop EmitSpec maps (P13/D-010a).
    pub emits: meclaw_core::EmitsBlock,
    /// Contract version (presence-enforced; non-empty free-form string,
    /// no semver constraint — config.md § contract).
    pub version: Option<String>,
    /// Declared settings map (presence-enforced; may be empty).
    pub settings: Option<std::collections::BTreeMap<String, meclaw_core::SettingSpec>>,
    /// Declared input contract (presence-enforced; may be empty).
    pub consumes: Option<meclaw_core::ConsumesBlock>,
}

/// Hard presence check for the builder-mandatory contract keys
/// (config.md § contract, Enforcement-Stufen-Tabelle): `version`
/// (non-empty string), `settings` (object), `consumes` (object) MUST be
/// present. Hive configs are exempt (their contract block is not
/// evaluated). Returns the FIRST missing/invalid key.
pub fn validate_contract_presence(block: &ContractBlock) -> Result<(), String> {
    match &block.version {
        None => return Err("contract.version missing".into()),
        Some(v) if v.is_empty() => {
            return Err("contract.version must be a non-empty string".into());
        }
        Some(_) => {}
    }
    if block.settings.is_none() {
        return Err("contract.settings missing".into());
    }
    if block.consumes.is_none() {
        return Err("contract.consumes missing".into());
    }
    Ok(())
}

impl From<ContractBlock> for ContractView {
    /// Maps `multi_send_capable` into `ContractView`. The `emits` field is NOT
    /// compiled here — compilation is fallible and must travel through the boot /
    /// mutation path as a `Result` (see B3/B4). `emits: None, validate_emits: false`
    /// are safe defaults; the real compile happens via `compile_contract_view` (B3).
    /// `consumes: None` likewise — the real compile happens in
    /// `compile_contract_view` (Slice 2).
    fn from(c: ContractBlock) -> Self {
        Self {
            multi_send_capable: c.multi_send_capable,
            emits: None,
            validate_emits: false,
            consumes: None,
        }
    }
}

/// Parsed representation of a cell's `config.json`.
#[derive(Debug, Deserialize)]
pub struct ParsedConfig {
    /// Cell identity header.
    pub cell: CellHeader,
    /// Optional free-form parameters block.
    #[serde(default)]
    pub params: JsonValue,
    /// Optional contract block; defaults to all-false if absent.
    #[serde(default)]
    pub contract: ContractBlock,
}

/// The `cell` section of a `config.json`.
#[derive(Debug, Deserialize)]
pub struct CellHeader {
    /// Cell type identifier (e.g. `"echo"`, `"llm"`).
    #[serde(rename = "type")]
    pub cell_type: String,
    /// Phase 5+: Max-respawn attempts. `None` means "use default (5)";
    /// `Some(0)` means no respawns at all.
    #[serde(default)]
    pub restart_limit: Option<u32>,
    /// Phase-13: Hot/Cold-Modus. `0` = Idle-Modell (default), `>0` = One-Shot,
    /// `-1` = persistent. Spec: `docs/config.md` Z.42. Wird in Phase 13
    /// in `cell_task_stateful` verdrahtet (Slices 13-K/13-L).
    #[serde(default)]
    pub timeout: i64,
    /// Phase-13: optionaler Idle-Timeout in Millisekunden. `None` heißt
    /// "Colony-Default verwenden" (`DEFAULT_IDLE_TIMEOUT_MS`). Wird in
    /// Slice 13-B-3 in `PlannedCell` propagiert und ab Slice 13-K im
    /// Idle-Watchdog ausgewertet.
    #[serde(default)]
    pub idle_timeout_ms: Option<u64>,
    /// Phase-1 parse-acceptance only; enforcement is Paket 3 / P6 (substrate B-backstop).
    /// `0`/`-1` = no backstop per docs/config.md.
    #[serde(default)]
    pub message_timeout: Option<i64>,
    /// Phase-5+ per-cell bounded-mpsc capacity override; resolved against
    /// colony.json mailbox_default_capacity at spawn (Paket 1).
    #[serde(default)]
    pub mailbox_size: Option<usize>,
}

/// Parsed representation of a hive cell's `config.json` `params` block.
#[derive(Debug, Deserialize)]
// P1-7: a hive's params block carries ONLY `graph` (Befund 21 — no
// `dead_letters` override). Reject genuinely-unknown keys hard.
#[serde(deny_unknown_fields)]
pub struct HiveParams {
    /// Graph topology hints for this hive scope.
    #[serde(default)]
    pub graph: GraphHints,
}

/// Graph topology hints: optional edge declarations for a hive.
#[derive(Debug, Default, Deserialize)]
pub struct GraphHints {
    /// Declared edges between cells within this hive scope.
    #[serde(default)]
    pub edges: Vec<EdgeSpec>,
}

/// Modifier applied by an edge over the two header compartments (`context`
/// and `hop`): set keys (CEL-evaluated) + delete keys, per compartment.
///
/// Per spec § Edge-Modell (docs/meclaw-overview.md Z.820-832): for each
/// compartment `set` runs before `delete`; all four fields are optional. Each
/// `set_*` value is a CEL expression source string, parsed at edge-insert. All
/// `set_*` expressions read the incoming (pre-modifier) `context.*` and `hop.*`
/// as a fixed namespace — so `set_context` can both promote a `hop` value and
/// compute over `context` (e.g. `context.iter + 1`).
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct ModifierSpec {
    /// Map `context_key → CEL expression` (string source, parsed at
    /// edge-insert). Reads the incoming `context.*`/`hop.*` namespace.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub set_context: BTreeMap<String, String>,
    /// `context` keys to remove after `set_context` (idempotent for
    /// non-existent keys).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delete_context: Vec<String>,
    /// Map `hop_key → CEL expression` (string source, parsed at edge-insert).
    /// Reads the incoming `context.*`/`hop.*` namespace.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub set_hop: BTreeMap<String, String>,
    /// `hop` keys to remove after `set_hop` (idempotent for non-existent keys).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delete_hop: Vec<String>,
}

/// A single directed edge between two cells.
#[derive(Debug, Deserialize)]
pub struct EdgeSpec {
    /// Source cell path.
    pub from: String,
    /// Destination cell path.
    pub to: String,
    /// CEL boolean expression (source string). Parsed at edge-insert
    /// (Phase 13.5-A1). `None` means "always take" (Identity).
    #[serde(default)]
    pub condition: Option<String>,
    /// Header modifier (set/delete) — pre-compiled at edge-insert
    /// (Phase 13.5-A1). `None` means identity headers.
    #[serde(default)]
    pub modifier: Option<ModifierSpec>,
}

#[cfg(test)]
mod hive_tests {
    use super::*;
    use meclaw_core::serde_json;

    #[test]
    fn parses_hive_params_with_edges() {
        let raw = serde_json::json!({
            "graph": {
                "edges": [
                    {"from": "./a", "to": "./b"}
                ]
            }
        });
        let p: HiveParams = serde_json::from_value(raw).unwrap();
        assert_eq!(p.graph.edges.len(), 1);
        assert_eq!(p.graph.edges[0].from, "./a");
        assert_eq!(p.graph.edges[0].to, "./b");
        assert!(p.graph.edges[0].condition.is_none());
        assert!(p.graph.edges[0].modifier.is_none());
    }

    /// Phase 13.5-A1: condition is a CEL source string (not Option<JsonValue>).
    #[test]
    fn edge_spec_parses_condition_as_string_and_modifier_as_struct() {
        let raw = r#"{
          "from": "/a",
          "to": "/b",
          "condition": "hop.foo == 'x'",
          "modifier": {
            "set_hop": {"tier": "hop.priority == 'high' ? 'gold' : 'standard'"},
            "delete_hop": ["debug_marker"]
          }
        }"#;
        let spec: EdgeSpec = serde_json::from_str(raw).expect("parse");
        assert_eq!(spec.from, "/a");
        assert_eq!(spec.to, "/b");
        assert_eq!(spec.condition.as_deref(), Some("hop.foo == 'x'"));
        let m = spec.modifier.expect("modifier set");
        assert_eq!(
            m.set_hop.get("tier").map(String::as_str),
            Some("hop.priority == 'high' ? 'gold' : 'standard'")
        );
        assert_eq!(m.delete_hop, vec!["debug_marker"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::{self, json};

    #[test]
    fn parsed_config_extracts_contract_emits() {
        let raw = r#"{"cell":{"type":"code"},"params":{},
          "contract":{"multi_send_capable":false,
            "emits":{"body":{"messages":{"type":"array"}},"hop":{}}}}"#;
        let cfg: ParsedConfig = meclaw_core::serde_json::from_str(raw).unwrap();
        assert!(cfg.contract.emits.body.contains_key("messages"));
    }

    #[test]
    fn parsed_config_missing_emits_defaults_to_empty() {
        let raw = r#"{"cell":{"type":"echo"},"params":{}}"#;
        let cfg: ParsedConfig = meclaw_core::serde_json::from_str(raw).unwrap();
        assert!(cfg.contract.emits.body.is_empty());
        assert!(cfg.contract.emits.hop.is_empty());
    }

    #[test]
    fn parsed_config_extracts_contract_multi_send_capable_true() {
        let raw = r#"{"cell":{"type":"code"},"params":{},"contract":{"multi_send_capable":true}}"#;
        let cfg: ParsedConfig = meclaw_core::serde_json::from_str(raw).unwrap();
        assert!(cfg.contract.multi_send_capable);
    }

    #[test]
    fn parsed_config_missing_contract_defaults_to_false() {
        let raw = r#"{"cell":{"type":"echo"},"params":{}}"#;
        let cfg: ParsedConfig = meclaw_core::serde_json::from_str(raw).unwrap();
        assert!(!cfg.contract.multi_send_capable);
    }

    #[test]
    fn cell_header_parses_restart_limit_from_config() {
        let raw = r#"{"cell":{"type":"echo","restart_limit":2}}"#;
        let cfg: ParsedConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.cell.restart_limit, Some(2));
    }

    #[test]
    fn cell_header_restart_limit_optional_defaults_to_none() {
        let raw = r#"{"cell":{"type":"echo"}}"#;
        let cfg: ParsedConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.cell.restart_limit, None);
    }

    #[test]
    fn cell_header_idle_timeout_default_is_none() {
        let h: CellHeader = serde_json::from_str(r#"{"type":"echo"}"#).unwrap();
        assert!(h.idle_timeout_ms.is_none());
    }

    #[test]
    fn cell_header_parses_idle_timeout_from_json() {
        let h: CellHeader =
            serde_json::from_str(r#"{"type":"echo","idle_timeout_ms":500}"#).unwrap();
        assert_eq!(h.idle_timeout_ms, Some(500));
    }

    #[test]
    fn contract_presence_rejects_missing_version_settings_consumes() {
        let full: ContractBlock = meclaw_core::serde_json::from_value(json!({
            "version": "0.1.0", "settings": {}, "consumes": {}
        }))
        .unwrap();
        assert!(validate_contract_presence(&full).is_ok());
        for missing in ["version", "settings", "consumes"] {
            let mut v = json!({"version": "0.1.0", "settings": {}, "consumes": {}});
            v.as_object_mut().unwrap().remove(missing);
            let block: ContractBlock = meclaw_core::serde_json::from_value(v).unwrap();
            let err = validate_contract_presence(&block).unwrap_err();
            assert!(err.contains(missing), "{err}");
        }
    }

    #[test]
    fn contract_presence_rejects_empty_version_string() {
        let block: ContractBlock = meclaw_core::serde_json::from_value(json!({
            "version": "", "settings": {}, "consumes": {}
        }))
        .unwrap();
        assert!(validate_contract_presence(&block).is_err());
    }

    #[test]
    fn parses_minimal_cell_with_type() {
        let raw = json!({"cell": {"type": "echo"}});
        let cfg: ParsedConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(cfg.cell.cell_type, "echo");
        assert!(cfg.params.is_null());
    }

    #[test]
    fn parses_cell_with_params_block() {
        let raw = json!({
            "cell": {"type": "echo"},
            "params": {"echo_to": "/foo"}
        });
        let cfg: ParsedConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(cfg.cell.cell_type, "echo");
        assert_eq!(cfg.params["echo_to"], "/foo");
    }
}
