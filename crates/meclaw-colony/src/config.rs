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
    /// GH #185 — the cell's own statement that it is an ingress, and which
    /// standard header keys it mints at birth. Absent ⇒ not an ingress.
    pub ingress: meclaw_core::IngressBlock,
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
    /// `-1` = persistent. Spec: `docs/config.md` l.42. Wired up in phase 13
    /// in `cell_task_stateful` verdrahtet (Slices 13-K/13-L).
    #[serde(default)]
    pub timeout: i64,
    /// Phase-13: optional idle timeout in milliseconds. `None` means "use the
    /// colony default" (`DEFAULT_IDLE_TIMEOUT_MS`). Propagated into `PlannedCell`
    /// in slice 13-B-3 and, from slice 13-K on, in the
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
    /// GH #62: which template this node was instantiated from, and when.
    /// Written exactly once, by the instantiation (`patch_and_substitute_config`),
    /// and never again — the same write that mints `cell.id`. `None` for a node
    /// that was not born from a template (a hand-written tree, an `adopt`ed
    /// directory, anything instantiated before this field existed). See
    /// `docs/config.md` § `cell` → `provenance`.
    #[serde(default)]
    pub provenance: Option<NodeProvenance>,
    /// GH #159: this cell may be served as a surface over HTTP.
    ///
    /// In the `cell` block rather than in `params` because this block is what the
    /// colony reads to decide how it runs a cell, and serving it is the colony's
    /// job. One field, every cell type, one parser — [`crate::surface::parse_decl`].
    ///
    /// Absent by default, which is what every `config.json` written before this
    /// field existed means. See `docs/meclaw-overview.md` § surfaces.
    #[serde(default)]
    pub surface: Option<crate::surface::SurfaceDecl>,
}

/// GH #62: the template identity an instantiated node carries with it.
///
/// Lives in the instance's own `config.json` (`cell.provenance`), because the
/// instance is a **detached copy**: `template.json` is stripped at staging, and
/// an exported or restored tree has to be able to name its own origin without
/// the colony that created it. `colony.db`'s `registry` table carries the same
/// three values as a query index (see `ColonyWriteOp::SetRegistryProvenance`);
/// the file is the source, the table is the index.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct NodeProvenance {
    /// Resolved template name as declared in the template's `template.json`
    /// (`echo`), NOT the reference the mutation used (`echo@1.2.3`).
    pub template: String,
    /// Resolved template version, or `None` when the template declares none.
    /// Absent from the serialized form in that case — "this template has no
    /// version" is a different fact from "the version is unknown".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_version: Option<String>,
    /// Unix seconds at which this node was instantiated. Same unit as every
    /// `created_at` in `colony.db`.
    pub instantiated_at: i64,
}

/// Parsed representation of a hive cell's `config.json` `params` block.
#[derive(Debug, Deserialize)]
// P1-7: a hive's params block carries `graph` and (GH #133) `ports`
// (Befund 21 — no `dead_letters` override). Reject genuinely-unknown keys hard.
#[serde(deny_unknown_fields)]
pub struct HiveParams {
    /// Graph topology hints for this hive scope.
    #[serde(default)]
    pub graph: GraphHints,
    /// GH #133 — **opt-in** port declaration: the short names of the direct
    /// children that are this hive's external entry points.
    ///
    /// `None` (key absent) is the historical, open behaviour: every interior
    /// node may be wired from anywhere. `Some(names)` **seals** the scope —
    /// the mutation validation then rejects an `add_edges` endpoint that
    /// reaches an interior node past the port (see
    /// [`crate::mutation::port_boundary`]). An empty list is legal and means
    /// "the hive path itself is the only address" (transit only).
    ///
    /// Names are short names of DIRECT children (no `/`, no `.`/`..`) — a port
    /// is a member of this scope, not a node somewhere below it.
    #[serde(default)]
    pub ports: Option<Vec<String>>,
    /// GH #147 — **opt-in** drain pairing: ports of this hive whose refusal (or
    /// any other declared) route must be consumed outside the hive once the
    /// port is wired from outside.
    ///
    /// `None` (key absent) is the historical behaviour: a parent may wire an
    /// ingress and leave the matching egress a dead end. `Some(list)` makes the
    /// pairing a rule the mutation validation enforces (see
    /// [`crate::mutation::required_drains`]).
    #[serde(default)]
    pub required_drains: Option<Vec<DrainSpec>>,
    /// GH #173 — **opt-in** contract: the lanes this hive accepts at its own
    /// path and the lanes it emits back out of it.
    ///
    /// A hive is the abstraction boundary (`docs/meclaw-overview.md` § Die
    /// Hive-Grenze), and until this field existed it was the one unit a person
    /// instantiates that had nothing machine-readable to check an instantiation
    /// against — the interface was prose in `description`, and the prose named
    /// cells three levels down. A declaration in terms of `hop.route` values
    /// says the same thing in the only vocabulary that survives a
    /// reimplementation: a replacement template with a different inside can
    /// satisfy the same lanes.
    ///
    /// `None` (key absent) is the historical behaviour and stays vacuous. See
    /// [`crate::mutation::hive_contract`] for what is checked.
    #[serde(default)]
    pub contract: Option<HiveContractSpec>,
}

/// GH #173 — a hive's `params.contract`: its interface, stated in lanes.
///
/// Deliberately NOT the top-level `contract` block. That key is taken and means
/// something else: a CELL's `version`/`settings`/`consumes`/`emits`, where
/// `emits` is a per-output body+hop `EmitSpec` map. One word cannot carry two
/// shapes, and a hive's `params` is already where its wiring surface lives
/// (`graph`, `ports`, `required_drains`) — so this joins them.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct HiveContractSpec {
    /// Lanes a caller may send INTO the hive path.
    #[serde(default)]
    pub accepts: Vec<LaneSpec>,
    /// Lanes the hive sends back OUT through its own path.
    #[serde(default)]
    pub emits: Vec<LaneSpec>,
}

/// One lane of a hive contract: a `hop.route` value plus what it means.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LaneSpec {
    /// The `hop.route` value that IS this lane. The whole abstraction rests on
    /// this being a route and not a cell name.
    pub route: String,
    /// `context` keys a caller must have promoted by the time a message enters
    /// on this lane. **Declared, not enforced**: a promotion three edges
    /// upstream is indistinguishable from a missing one to anything that reads
    /// a single edge, so a machine check here would be guesswork. It is written
    /// down so a builder can read it, and so the hive's own prose has one place
    /// to live instead of a free-text paragraph.
    #[serde(default)]
    pub context: Vec<String>,
    /// What this lane is for, in the hive's own words. Travels verbatim into a
    /// rejection — a refusal that cannot say what it protects is a refusal
    /// people route around (same reasoning as `required_drains[].because`).
    pub because: String,
}

/// One `params.required_drains` entry of a hive: a pairing it insists on.
///
/// Two shapes, because the boundary moved under this rule (GH #237). The PORT
/// form pairs a direct child with a route that must leave the hive once that
/// child is wired from outside; a SEALED hive has no ports, so nothing outside
/// can address one and the form can never fire. The LANE form states the same
/// obligation in the vocabulary the seal left standing — *a caller that sends
/// me lane A must subscribe to lane B* — and is the one a sealed hive can use.
///
/// The port form is kept, and not only for old declarations: `params.ports`
/// is opt-in, and a hive that never sealed itself still has ports to pair.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum DrainSpec {
    /// GH #147 — pair a PORT of this hive with a route that must be drained
    /// out of it once something outside wires that port.
    Port(PortDrainSpec),
    /// GH #237 — pair two LANES of this hive's contract: a caller that sends
    /// the first must take the second.
    Lane(LaneDrainSpec),
}

/// The port form of a `required_drains` entry (GH #147).
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PortDrainSpec {
    /// Short name of the direct child that is the port (same shape as
    /// `params.ports`).
    pub port: String,
    /// The hop compartment a message on the drain route carries. Matched by
    /// RUNNING it through the edge conditions, not by comparing their text.
    pub hop: BTreeMap<String, String>,
    /// Why this pairing exists, in the hive's own words. Travels verbatim into
    /// the rejection — a refusal that cannot say what it protects is a refusal
    /// people route around.
    pub because: String,
}

/// The lane form of a `required_drains` entry (GH #237).
///
/// Both names are `hop.route` values of this hive's own `params.contract`:
/// `accepts` one it declares it accepts, `emits` one it declares it emits. A
/// name that is in neither is a declaration about a lane the hive does not
/// have, and the reader drops it rather than enforce a pairing nobody can
/// satisfy.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LaneDrainSpec {
    /// The inbound lane that TRIGGERS the obligation. Wiring it is what makes
    /// the drain necessary; a hive whose lane nobody sends to needs nothing.
    pub accepts: String,
    /// The outbound lane that must then be taken by somebody outside.
    pub emits: String,
    /// Why this pairing exists, in the hive's own words. Travels verbatim into
    /// the rejection — a refusal that cannot say what it protects is a refusal
    /// people route around.
    pub because: String,
}

/// Graph topology hints: optional edge declarations for a hive.
#[derive(Debug, Default, Deserialize)]
// W13 hardening: same strictness as `HiveParams` above — `graph` carries only
// `edges`, and a misspelled sibling is a boot error, not a silent nothing.
#[serde(deny_unknown_fields)]
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
// W13 hardening: the mutation path already rejects an unknown modifier key
// (`mutation/validate.rs`, Befund-6-Folge — the flat `{"headers.X": …}` form was
// once accepted at validate and ignored at apply). The BOOT path went through
// serde and swallowed the same shape. One strictness, both doors.
#[serde(deny_unknown_fields)]
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
    /// GH #82 (ruling 2026-08-13): when `true`, this edge RESTORES the routing
    /// budget of the message it takes — the envelope's `ttl` is lifted back to
    /// the colony's `message_default_ttl` (never lowered, so a message that was
    /// ingested with a larger budget keeps it, and never accumulated, so N
    /// restores and one restore leave the same ceiling).
    ///
    /// The fifth modifier field is the only one that touches the envelope
    /// rather than a header compartment. Envelope-setter authority is not
    /// broken by it: an edge is evaluated BY the colony, and the colony is
    /// still the only writer — the edge merely declares, in the topology JSON,
    /// that its loop is legitimate. Because a restoring edge makes its cycle
    /// unbounded by TTL, the runaway guard moves to the loop's own bound, so a
    /// restoring edge without a `condition` is rejected at config load
    /// (`BootstrapError::EdgeTtlRestoreUnconditional`) and at `add_edges`
    /// validation. The intended shape is the iteration counter the same edge
    /// already carries in `set_context`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub restore_ttl: bool,
}

/// `skip_serializing_if` predicate for `ModifierSpec::restore_ttl`.
///
/// Load-bearing: edge identity per spec Z.265 compares the serde-JSON of the
/// `ModifierSpec` source (`EdgeTable::contains_equal`, `EdgeMatchView`), and
/// the durable-edge round-trip re-parses that same JSON. Omitting the default
/// keeps every pre-existing edge's serialised form byte-identical, so no
/// existing edge changes identity by the mere existence of this field.
fn is_false(b: &bool) -> bool {
    !*b
}

/// A single directed edge between two cells.
///
/// **Strict by declaration (W13 hardening).** The mutation path has rejected
/// unknown edge fields since Befund 6 (`mutation/validate.rs`); the boot path did
/// not, and the two doors led into the same edge table. The failure that bought
/// the change is silent and total: `"conditon"` instead of `"condition"` produced
/// an edge with `condition: None` — an edge that fires UNCONDITIONALLY, which is
/// the opposite of what the file says, on every message, forever. A boot error is
/// the only honest answer to a topology nobody can read back.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
        assert!(
            p.ports.is_none(),
            "GH #133: without the key a hive is NOT sealed — absence is the historical behaviour"
        );
    }

    /// GH #133: `params.ports` is the opt-in port declaration. `deny_unknown_fields`
    /// used to reject the key outright, so this is also the pin that the hive
    /// params block learned exactly ONE new key.
    #[test]
    fn parses_hive_params_with_ports() {
        let p: HiveParams =
            serde_json::from_value(serde_json::json!({"ports": ["brief", "gate"]})).unwrap();
        assert_eq!(p.ports, Some(vec!["brief".to_string(), "gate".to_string()]));
        assert!(p.graph.edges.is_empty());

        // An empty list is legal: "the hive path itself is the only address".
        let p: HiveParams = serde_json::from_value(serde_json::json!({"ports": []})).unwrap();
        assert_eq!(p.ports, Some(vec![]));

        // Still `deny_unknown_fields` for everything else.
        assert!(
            serde_json::from_value::<HiveParams>(serde_json::json!({"portz": ["a"]})).is_err(),
            "a typo'd key stays a boot error"
        );
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

    /// W13 hardening: the boot path is now as strict as `mutation/validate.rs`.
    ///
    /// `conditon` is the whole finding in one word — it used to parse into an
    /// edge with no condition, i.e. an edge that fires on every message, and
    /// nothing anywhere said so.
    #[test]
    fn a_misspelled_edge_field_is_a_parse_error() {
        let raw = serde_json::json!({
            "graph": {"edges": [{"from": "./a", "to": "./b", "conditon": "hop.x == 1"}]}
        });
        let err = serde_json::from_value::<HiveParams>(raw)
            .expect_err("an unknown edge field must not parse");
        let msg = err.to_string();
        assert!(msg.contains("conditon"), "got: {msg}");
        assert!(
            msg.contains("condition"),
            "the message names the real one: {msg}"
        );
    }

    /// Same door, one level down: the modifier is the shape the mutation path
    /// has rejected since Befund 6.
    #[test]
    fn a_misspelled_modifier_field_is_a_parse_error() {
        let raw = serde_json::json!({
            "graph": {"edges": [
                {"from": "./a", "to": "./b", "modifier": {"set_contex": {"k": "1"}}}
            ]}
        });
        let err = serde_json::from_value::<HiveParams>(raw)
            .expect_err("an unknown modifier field must not parse");
        assert!(err.to_string().contains("set_contex"), "got: {err}");
    }

    /// …and the legitimate shapes keep parsing, including every modifier field.
    #[test]
    fn the_full_legitimate_edge_shape_still_parses() {
        let raw = serde_json::json!({
            "graph": {"edges": [{
                "from": "./a", "to": "./b", "condition": "hop.x == 1",
                "modifier": {
                    "set_context": {"iter": "context.iter + 1"},
                    "delete_context": ["tmp"],
                    "set_hop": {"tier": "'gold'"},
                    "delete_hop": ["debug"],
                    "restore_ttl": true
                }
            }]}
        });
        let p: HiveParams = serde_json::from_value(raw).expect("parse");
        let m = p.graph.edges[0].modifier.as_ref().expect("modifier");
        assert!(m.restore_ttl);
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
            "params": {"emitted_target": "/foo"}
        });
        let cfg: ParsedConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(cfg.cell.cell_type, "echo");
        assert_eq!(cfg.params["emitted_target"], "/foo");
    }
}
