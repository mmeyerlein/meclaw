//! Contract `emits` validation — the generic mechanism behind Debt-Register
//! P13/D-010a. Translates the compact `EmitSpec` map (docs/config.md § emits,
//! l.90) into two Draft-2020-12 documents (body + hop) and compiles them
//! with the `jsonschema` crate (already a dependency; same engine as the UBF
//! validator in `schema.rs`). The `jsonschema` dependency is encapsulated here.

use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn default_required() -> bool {
    true
}

/// One field's emit specification: compact form per docs/config.md l.90.
#[derive(Debug, Clone, Deserialize)]
pub struct EmitSpec {
    /// JSON value type: `string`/`number`/`boolean`/`object`/`array`/`blob_uuid`.
    #[serde(rename = "type")]
    pub ty: String,
    /// Optional enum whitelist — only meaningful for `type: string` (l.99).
    #[serde(default)]
    pub values: Option<Vec<String>>,
    /// Whether the key must be present. Defaults to `true` (l.100).
    #[serde(default = "default_required")]
    pub required: bool,
}

/// The `emits` block: `body` (content slots) + `hop` (routing metadata),
/// each a map `key → EmitSpec` (docs/config.md l.78). Cells only emit hop.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmitsBlock {
    /// Declared body slots this cell writes.
    #[serde(default)]
    pub body: BTreeMap<String, EmitSpec>,
    /// Declared hop keys this cell writes (cells only emit hop).
    #[serde(default)]
    pub hop: BTreeMap<String, EmitSpec>,
}

/// GH #185 — a cell's declaration that it is an **ingress**: the point where a
/// message enters the colony from outside.
///
/// Not a message compartment and not an `emits` sibling — cells emit `hop`
/// only, and `context` stays edge authority (config.md § emits). This block
/// says something about the cell's PLACE, in the same spirit as
/// `consumes.topology` (GH #160): "messages are born at me, and these are the
/// standard header keys they are born with".
///
/// It replaces an inference. The build-time reachability check used to treat
/// any node without an incoming edge as the graph entry, which is not a
/// property of a cell at all: the ordinary connector — a proxy that accepts
/// inbound traffic and gets the answers routed back — has an incoming edge and
/// lost the branch, while an unrelated rewiring elsewhere could hand it back.
/// A declaration is answerable locally and does not change its mind.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IngressBlock {
    /// The `context` keys this cell mints when it creates a message. Bounded
    /// by the standard header convention (`INGRESS_CONTEXT_KEYS`): a cell may
    /// NARROW the standard set, never widen it — anything else reaches
    /// `context` through an edge `set_context`.
    pub context: BTreeSet<String>,
}

/// GH #260 — a cell's declaration that the writes the **substrate** answers on
/// its behalf are bounded to its own owning scope.
///
/// The type-neutral twin of `store`'s `params.write_surface`, and it exists
/// because that one cannot reach far enough. `params` belongs to a cell type;
/// the substrate is above every cell type and must not read it. So a boundary
/// the substrate enforces has to be declared where every cell type declares in
/// the same grammar — the `contract` block — in the same spirit as
/// `consumes.topology` (GH #160) and `contract.ingress` (GH #185): a statement
/// about this cell's PLACE, not about a message.
///
/// The two are deliberately not derived from one another. `params.write_surface`
/// bounds the ops a `store`'s `handle()` runs; this one bounds the ops the
/// substrate runs before `handle()` is ever reached (today: the `transfer`
/// slot's `import`). A cell that wants both sealed declares both — and a cell
/// type without params of its own can still seal the substrate half.
///
/// `Open` is the default, so absence changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteSurface {
    /// No bound — any sender may reach a substrate-answered write. Default.
    #[default]
    Open,
    /// Only senders inside this cell's own parent scope may reach a
    /// substrate-answered write. Fail-closed: a message with no sender at all
    /// (a source message — HTTP ingress, an event) is outside by definition.
    Internal,
}

/// GH #314 — a cell's declaration that its database does not travel.
///
/// The `transfer` body slot is answered by the substrate above every cell type,
/// before `handle()` and before the `consumes` gate. That position is what makes
/// the operation type-agnostic, and it is also what puts it out of reach of
/// every rule a cell type enforces in its own `handle()`: the `vault`'s
/// two-caller ACL never sees a transfer, so the cell type whose whole promise is
/// that no operation returns a secret handed out its `vault_secrets` inventory,
/// its salt and its complete audit trail through a seam it cannot answer.
///
/// The fix is a **declaration**, not an exclusion list in the substrate. A list
/// of type names inside `db_transfer` would be invisible in a `config.json`,
/// invisible in a diff, and would have to be edited again for the next cell type
/// with the same need. `transfer: "none"` is answerable by reading the cell's
/// own contract, and it binds a cell type nobody has written yet.
///
/// `All` is the default, so absence changes nothing — every colony older than
/// this key keeps the behaviour it had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPolicy {
    /// Every content table of this cell's `cell.db` may be exported and
    /// imported through the `transfer` slot. Default.
    #[default]
    All,
    /// This cell's database is exempt from the `transfer` slot. Export AND
    /// import alike: a store that cannot leave also cannot be overwritten
    /// through the same seam. The substrate refuses with `transfer_exempt`
    /// before it looks at the arguments, so a refusal names no table.
    None,
}

/// The contract facts the **substrate** consults on the `transfer` slot, before
/// `handle()` runs.
///
/// Both are declarations about this cell's PLACE rather than about a message,
/// and both are answered above every cell type — so they travel together from
/// the `contract` block through the spawn helpers into the seam, as one value
/// rather than as a widening list of parameters.
///
/// The default is the pre-declaration behaviour in both fields: an open write
/// surface and a database that travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransferBounds {
    /// GH #260 — who may reach the WRITE half of the seam (the `import`).
    pub write_surface: WriteSurface,
    /// GH #314 — whether this cell's database answers the seam at all.
    pub policy: TransferPolicy,
}

impl TransferBounds {
    /// The bounds of a cell that declares `contract.transfer: "none"` and
    /// nothing else — the vault's declaration, and the shape a test states
    /// without building a whole contract block.
    pub fn exempt() -> Self {
        Self {
            write_surface: WriteSurface::default(),
            policy: TransferPolicy::None,
        }
    }

    /// True iff this cell's database is exempt from the `transfer` slot.
    pub fn is_exempt(&self) -> bool {
        self.policy == TransferPolicy::None
    }
}

/// One field's consume spec — type only (no enum/values on the read side).
#[derive(Debug, Clone, Deserialize)]
pub struct ConsumeSpec {
    /// JSON value type: `string`/`number`/`boolean`/`object`/`array`/`blob_uuid`.
    #[serde(rename = "type")]
    pub ty: String,
    /// Whether the key must be present. Defaults to `true`.
    #[serde(default = "default_required")]
    pub required: bool,
}

/// One `contract.settings` entry (config.md § SettingSpec): a declared,
/// builder-visible configuration knob. Presence-enforced as of the
/// pre-integration hardening; the VALUES are not substrate-interpreted.
#[derive(Debug, Clone, Deserialize)]
pub struct SettingSpec {
    /// Type token: string|number|boolean|object|array.
    #[serde(rename = "type")]
    pub ty: String,
    /// Secret marker (default false).
    #[serde(default)]
    pub secret: bool,
    /// Optional default value.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
}

/// The `consumes` block: body slots + the two header compartments (context, hop).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConsumesBlock {
    /// Declared body slots this cell reads.
    #[serde(default)]
    pub body: BTreeMap<String, ConsumeSpec>,
    /// Declared context keys this cell reads (durable compartment).
    #[serde(default)]
    pub context: BTreeMap<String, ConsumeSpec>,
    /// Declared hop keys this cell reads (per-hop compartment).
    #[serde(default)]
    pub hop: BTreeMap<String, ConsumeSpec>,
    /// GH #160 — declared facts about this cell's **own place in the graph**.
    ///
    /// Not a message compartment: nothing here is validated against an incoming
    /// message, and a key declared here never makes a message invalid. It is a
    /// capability declaration in the same grammar as the three above — "this cell
    /// reads X" — and the substrate answers it at spawn with a read-only handle
    /// (`meclaw_colony::NeighbourhoodView`). The only key the substrate knows is
    /// `inbound_edges`: the paths of the edges that point AT this cell, from the
    /// authority that owns the edge table, so that a cell which must refuse an
    /// unexpected neighbour can do so without reading `colony.db` — which
    /// § Database isolation forbids, with no exceptions left.
    #[serde(default)]
    pub topology: BTreeMap<String, ConsumeSpec>,
}

/// Translate the compact `type` token into a JSON-Schema type keyword.
/// `blob_uuid` is a UUID-shaped string at the wire level → `string`.
fn json_type_for(token: &str) -> &str {
    match token {
        "blob_uuid" => "string",
        other => other, // string/number/boolean/object/array pass through;
                        // an unknown token yields an invalid type → compile fails LOUD (A3).
    }
}

/// Translate one `key → EmitSpec` map into a Draft-2020-12 object schema.
/// `additionalProperties` is left at the default (allowed): undeclared slots
/// are not an error (config.md l.71: only declared keys are constrained,
/// required keys must be present, declared keys match type/enum).
fn emitspec_map_to_schema(map: &BTreeMap<String, EmitSpec>) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<Value> = Vec::new();
    for (key, spec) in map {
        let mut prop = serde_json::Map::new();
        prop.insert(
            "type".into(),
            Value::String(json_type_for(&spec.ty).to_string()),
        );
        if let Some(values) = &spec.values {
            prop.insert(
                "enum".into(),
                Value::Array(values.iter().cloned().map(Value::String).collect()),
            );
        }
        properties.insert(key.clone(), Value::Object(prop));
        if spec.required {
            required.push(Value::String(key.clone()));
        }
    }
    serde_json::json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": Value::Array(required),
    })
}

/// Pre-compiled emit validators for one cell — body + hop.
/// Constructed at spawn (Colony side); held behind `Arc` and handed to the
/// cell. A malformed `emits` makes `compile` return `Err` so the boot /
/// mutation path can reject LOUDLY (never a silent "validation off").
pub struct CompiledEmits {
    body: jsonschema::Validator,
    hop: jsonschema::Validator,
}

impl std::fmt::Debug for CompiledEmits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompiledEmits(body, hop)")
    }
}

impl CompiledEmits {
    /// Compile both schemas. `Err(reason)` if either schema is not valid
    /// Draft-2020-12 (e.g. an unknown `type` token).
    pub fn compile(emits: &EmitsBlock) -> Result<Self, String> {
        let body_schema = emitspec_map_to_schema(&emits.body);
        let hop_schema = emitspec_map_to_schema(&emits.hop);
        let body = jsonschema::validator_for(&body_schema)
            .map_err(|e| format!("emits.body schema invalid: {e}"))?;
        let hop = jsonschema::validator_for(&hop_schema)
            .map_err(|e| format!("emits.hop schema invalid: {e}"))?;
        Ok(Self { body, hop })
    }
}

/// Split a cell `content` JSON into (header-candidate, body-candidate):
/// `content.header` (object, default `{}`) and the remaining top-level keys.
/// Mirrors Colony's `split_content_header` (overview l.672-676) but lives in
/// core so the cell can reuse it without a colony dependency.
fn split_content(content: &Value) -> (Value, Value) {
    let mut header = Value::Object(serde_json::Map::new());
    let mut body = serde_json::Map::new();
    if let Value::Object(map) = content {
        for (k, v) in map {
            if k == "header" {
                header = v.clone();
            } else {
                body.insert(k.clone(), v.clone());
            }
        }
    }
    (header, Value::Object(body))
}

/// Validate a cell emission `content` against its compiled `emits` contract.
/// Checks BOTH the body slots and the hop section (config.md l.78). The wire
/// section key stays `header`; the contract compartment it maps to is `hop`
/// (cells only emit hop).
/// Returns `Err` with a `body:`/`hop:`-prefixed, semicolon-joined message.
pub fn validate_emits(content: &Value, compiled: &CompiledEmits) -> Result<(), String> {
    let (hop_val, body_val) = split_content(content);
    let mut errs: Vec<String> = Vec::new();
    for e in compiled.body.iter_errors(&body_val) {
        errs.push(format!("body: {e}"));
    }
    for e in compiled.hop.iter_errors(&hop_val) {
        errs.push(format!("hop: {e}"));
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

/// Pre-extracted views of a `consumes` block: the required keys per message
/// compartment (`(key, type-token)` for every `required: true` ConsumeSpec)
/// plus the declaration sets the capability gates read. Compile is infallible
/// (pure projection) — unlike `CompiledEmits` there is no schema to build.
#[derive(Debug, Clone, Default)]
pub struct CompiledConsumes {
    body: Vec<(String, String)>,
    context: Vec<(String, String)>,
    hop: Vec<(String, String)>,
    /// Every declared `consumes.body` key, required or not (GH #323).
    /// The ingress projection above answers "must this message carry it";
    /// this one answers "does this cell read the slot at all" — the question
    /// [`Self::declares_body`] and the capability gates behind it ask.
    body_declared: Vec<String>,
    /// Declared topology facts (GH #160). Deliberately NOT part of
    /// [`Self::is_vacuous`] or of any message check: it gates a spawn-time
    /// capability, never an ingress. That gap is GH #333 — `body_declared`
    /// above had the same one until GH #323, and a topology-only declaration
    /// still loses its `NeighbourhoodView` the same silent way.
    topology: Vec<String>,
}

impl CompiledConsumes {
    /// Project the `required: true` entries of the three message compartments,
    /// plus the full declaration sets of `body` and `topology`.
    pub fn compile(block: &ConsumesBlock) -> Self {
        let required = |m: &BTreeMap<String, ConsumeSpec>| {
            m.iter()
                .filter(|(_, s)| s.required)
                .map(|(k, s)| (k.clone(), s.ty.clone()))
                .collect::<Vec<(String, String)>>()
        };
        Self {
            body: required(&block.body),
            context: required(&block.context),
            hop: required(&block.hop),
            // Every declared key, required or not: a capability declaration,
            // and "optional capability" has no meaning.
            body_declared: block.body.keys().cloned().collect(),
            topology: block.topology.keys().cloned().collect(),
        }
    }

    /// True iff this view carries nothing the substrate would consult: no
    /// required key in any compartment AND no `body` declaration that gates a
    /// capability. A vacuous view is dropped at spawn (`consumes: None`), so a
    /// non-vacuity that a gate depends on has to be counted here — an optional
    /// `consumes.body` declaration otherwise vanished before the gate saw it.
    pub fn is_vacuous(&self) -> bool {
        self.body.is_empty()
            && self.context.is_empty()
            && self.hop.is_empty()
            && self.body_declared.is_empty()
    }

    /// True iff the cell declares `consumes.body.<key>` (GH #87).
    ///
    /// Declaration, not obligation: `required` governs the ingress check
    /// (config.md § `consumes`), and a key declared `required: false` is still
    /// a key this cell reads. The two questions are answered from two
    /// projections — reading the required-key list here withheld the capability
    /// from every optional declaration, silently (GH #323).
    ///
    /// Used by the substrate to decide whether a cell gets a capability that
    /// only a declared consumer may hold — today the `attachments[]` blob
    /// reader (`meclaw_colony::AttachmentReader`).
    pub fn declares_body(&self, key: &str) -> bool {
        self.body_declared.iter().any(|k| k == key)
    }

    /// True iff the cell declares `consumes.topology.<key>` (GH #160).
    ///
    /// The same gate as [`Self::declares_body`], for the capability that hands a
    /// cell facts about its own position in the graph instead of a slot of an
    /// incoming message. A cell that declares nothing gets no handle and cannot
    /// ask.
    pub fn declares_topology(&self, key: &str) -> bool {
        self.topology.iter().any(|k| k == key)
    }
}

/// Substrate-side ingress check (config.md § consumes, l.135-138): every
/// `required: true` key must be present with the declared type — `body` keys
/// against the inline body's top-level slots, `context`/`hop` against the
/// respective header compartment. Returns a human-readable reason naming
/// compartment + key on the FIRST violation. Type tokens:
/// string|number|boolean|object|array|blob_uuid (blob_uuid = string parsing
/// as UUID).
pub fn validate_consumes(
    body: &serde_json::Value,
    headers: &crate::Headers,
    compiled: &CompiledConsumes,
) -> Result<(), String> {
    let body_map = body.as_object();
    for (key, ty) in &compiled.body {
        match body_map.and_then(|m| m.get(key)) {
            Some(v) if type_matches(v, ty) => {}
            Some(_) => {
                return Err(format!(
                    "consumes.body '{key}' has wrong type (expected {ty})"
                ));
            }
            None => return Err(format!("required consumes.body '{key}' missing")),
        }
    }
    check_compartment("context", &compiled.context, &headers.context)?;
    check_compartment("hop", &compiled.hop, &headers.hop)?;
    Ok(())
}

fn check_compartment(
    compartment: &str,
    specs: &[(String, String)],
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    for (key, ty) in specs {
        match map.get(key) {
            Some(v) if type_matches(v, ty) => {}
            Some(_) => {
                return Err(format!(
                    "consumes.{compartment} '{key}' has wrong type (expected {ty})"
                ));
            }
            None => return Err(format!("required consumes.{compartment} '{key}' missing")),
        }
    }
    Ok(())
}

fn type_matches(v: &serde_json::Value, token: &str) -> bool {
    match token {
        "string" => v.is_string(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        "blob_uuid" => v.as_str().is_some_and(|s| uuid::Uuid::parse_str(s).is_ok()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitspec_map_translates_to_object_schema_with_required_and_enum() {
        let mut map = BTreeMap::new();
        map.insert(
            "finish_reason".to_string(),
            EmitSpec {
                ty: "string".into(),
                values: Some(vec!["stop".into(), "error".into()]),
                required: true,
            },
        );
        map.insert(
            "model".to_string(),
            EmitSpec {
                ty: "string".into(),
                values: None,
                required: false,
            },
        );
        let schema = emitspec_map_to_schema(&map);
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["finish_reason"]["type"], "string");
        assert_eq!(
            schema["properties"]["finish_reason"]["enum"],
            serde_json::json!(["stop", "error"])
        );
        // the required array holds only required:true keys
        assert_eq!(schema["required"], serde_json::json!(["finish_reason"]));
    }

    #[test]
    fn blob_uuid_maps_to_json_string_type() {
        let mut map = BTreeMap::new();
        map.insert(
            "ref".to_string(),
            EmitSpec {
                ty: "blob_uuid".into(),
                values: None,
                required: true,
            },
        );
        let schema = emitspec_map_to_schema(&map);
        assert_eq!(schema["properties"]["ref"]["type"], "string");
    }

    #[test]
    fn compile_ok_for_valid_emits() {
        let emits: EmitsBlock = serde_json::from_str(
            r#"{"body":{"messages":{"type":"array"}},"hop":{"finish_reason":{"type":"string"}}}"#,
        )
        .unwrap();
        assert!(CompiledEmits::compile(&emits).is_ok());
    }

    #[test]
    fn compile_errs_loudly_for_malformed_type_token() {
        // "stringg" is not a valid JSON-Schema type → the jsonschema compile fails.
        let emits: EmitsBlock =
            serde_json::from_str(r#"{"body":{"x":{"type":"stringg"}},"hop":{}}"#).unwrap();
        let err = CompiledEmits::compile(&emits).unwrap_err();
        assert!(
            err.contains("emits.body"),
            "error names the failing section: {err}"
        );
    }

    #[test]
    fn emits_block_parses_body_and_hop_with_defaults() {
        let raw = r#"{
          "body": { "messages": { "type": "array", "required": true } },
          "hop":  { "finish_reason": { "type": "string", "values": ["stop","error"] } }
        }"#;
        let b: EmitsBlock = serde_json::from_str(raw).unwrap();
        assert_eq!(b.body["messages"].ty, "array");
        assert!(b.body["messages"].required);
        // required defaults to true (config.md l.100)
        assert!(b.hop["finish_reason"].required);
        assert_eq!(
            b.hop["finish_reason"].values.as_deref().unwrap(),
            ["stop", "error"]
        );
    }

    fn sample_compiled() -> CompiledEmits {
        let emits: EmitsBlock = serde_json::from_str(
            r#"{
              "body": {"messages": {"type":"array","required":true}},
              "hop":  {"finish_reason": {"type":"string","values":["stop","error"],"required":true}}
            }"#,
        )
        .unwrap();
        CompiledEmits::compile(&emits).unwrap()
    }

    #[test]
    fn validate_emits_accepts_conforming_content() {
        let c = sample_compiled();
        let content = serde_json::json!({
            "header": {"finish_reason": "stop"},
            "messages": [{"origin":"assistant","type":"text","text":"hi"}]
        });
        assert!(validate_emits(&content, &c).is_ok());
    }

    #[test]
    fn validate_emits_rejects_body_violation() {
        let c = sample_compiled();
        // messages is missing → required-body violation
        let content = serde_json::json!({ "header": {"finish_reason": "stop"} });
        let err = validate_emits(&content, &c).unwrap_err();
        assert!(err.contains("body"), "names body: {err}");
    }

    #[test]
    fn validate_emits_rejects_hop_violation() {
        let c = sample_compiled();
        // finish_reason is missing → required-hop violation (reviewer point 1: hop is checked)
        let content = serde_json::json!({ "messages": [] });
        let err = validate_emits(&content, &c).unwrap_err();
        assert!(err.contains("hop"), "names hop: {err}");
    }

    #[test]
    fn validate_emits_rejects_hop_enum_violation() {
        let c = sample_compiled();
        let content = serde_json::json!({
            "header": {"finish_reason": "BOGUS"}, "messages": []
        });
        assert!(validate_emits(&content, &c).is_err());
    }

    #[test]
    fn emits_block_parses_hop_section() {
        let b: EmitsBlock = serde_json::from_str(
            r#"{"body":{"messages":{"type":"array"}},"hop":{"finish_reason":{"type":"string"}}}"#,
        )
        .unwrap();
        assert_eq!(b.hop["finish_reason"].ty, "string");
    }

    #[test]
    fn consumes_block_parses_context_and_hop() {
        let c: ConsumesBlock = serde_json::from_str(
            r#"{"context":{"turn_id":{"type":"string"}},"hop":{"operation":{"type":"string","required":true}}}"#,
        )
        .unwrap();
        assert!(c.hop["operation"].required);
        assert_eq!(c.context["turn_id"].ty, "string");
    }

    #[test]
    fn validate_emits_checks_hop_section_against_wire_header() {
        // the wire key stays "header"; the contract compartment is called "hop".
        let emits: EmitsBlock = serde_json::from_str(
            r#"{"body":{"messages":{"type":"array","required":true}},"hop":{"finish_reason":{"type":"string","required":true}}}"#,
        )
        .unwrap();
        let c = CompiledEmits::compile(&emits).unwrap();
        let ok = serde_json::json!({"header":{"finish_reason":"stop"},"messages":[]});
        assert!(validate_emits(&ok, &c).is_ok());
        let bad = serde_json::json!({"messages":[]}); // finish_reason is missing
        assert!(validate_emits(&bad, &c).unwrap_err().contains("hop"));
    }

    #[test]
    fn validate_consumes_rejects_missing_required_hop_key() {
        let block: ConsumesBlock = serde_json::from_value(serde_json::json!({
            "hop": {"h1": {"type": "string"}}
        }))
        .unwrap();
        let compiled = CompiledConsumes::compile(&block);
        let headers = crate::Headers::new(); // h1 is missing
        let body = serde_json::json!({"messages": []});
        let err = validate_consumes(&body, &headers, &compiled).unwrap_err();
        assert!(err.contains("consumes.hop 'h1'"), "{err}");
    }

    #[test]
    fn validate_consumes_rejects_wrong_type_and_accepts_match() {
        let block: ConsumesBlock = serde_json::from_value(serde_json::json!({
            "context": {"c1": {"type": "number"}}
        }))
        .unwrap();
        let compiled = CompiledConsumes::compile(&block);
        let mut headers = crate::Headers::new();
        headers
            .context
            .insert("c1".into(), serde_json::json!("not-a-number"));
        assert!(
            validate_consumes(&serde_json::json!({"messages": []}), &headers, &compiled).is_err()
        );
        headers.context.insert("c1".into(), serde_json::json!(42));
        assert!(
            validate_consumes(&serde_json::json!({"messages": []}), &headers, &compiled).is_ok()
        );
    }

    #[test]
    fn validate_consumes_checks_required_body_slot_and_ignores_optional() {
        let block: ConsumesBlock = serde_json::from_value(serde_json::json!({
            "body": {"messages": {"type": "array"},
                      "meta": {"type": "object", "required": false}}
        }))
        .unwrap();
        let compiled = CompiledConsumes::compile(&block);
        let headers = crate::Headers::new();
        assert!(
            validate_consumes(&serde_json::json!({"messages": []}), &headers, &compiled).is_ok()
        );
        assert!(
            validate_consumes(&serde_json::json!({"system": {}}), &headers, &compiled).is_err()
        );
    }

    #[test]
    fn declares_body_reports_declared_slot_and_ignores_others() {
        // GH #87: the declaration signal the substrate gates the attachment
        // reader on. It answers "is the slot declared", independent of whether
        // the key is also demanded of every inbound message (GH #323).
        let block: ConsumesBlock = serde_json::from_value(serde_json::json!({
            "body": {"messages": {"type": "array"}, "attachments": {"type": "array"}}
        }))
        .unwrap();
        let compiled = CompiledConsumes::compile(&block);
        assert!(compiled.declares_body("attachments"));
        assert!(compiled.declares_body("messages"));
        assert!(!compiled.declares_body("system"));
    }

    /// GH #323: `required` governs the INGRESS check, not the declaration.
    /// A key declared `required: false` is still declared — it is read, it is
    /// just not demanded of every inbound message — so the capability gate that
    /// reads `declares_body` must see it. Before this, an optional declaration
    /// parsed, disabled nothing visibly, and silently withheld the
    /// `AttachmentReader`: no error, no dead letter, no diagnostic.
    #[test]
    fn declares_body_sees_an_optional_declaration() {
        let block: ConsumesBlock = serde_json::from_value(serde_json::json!({
            "body": {"attachments": {"type": "array", "required": false}}
        }))
        .unwrap();
        let compiled = CompiledConsumes::compile(&block);
        assert!(
            compiled.declares_body("attachments"),
            "an optional declaration is still a declaration"
        );
        // …and it stays out of the ingress projection: the key is optional.
        let headers = crate::Headers::new();
        assert!(
            validate_consumes(&serde_json::json!({"messages": []}), &headers, &compiled).is_ok(),
            "required:false must not make the key mandatory at ingress"
        );
    }

    #[test]
    fn declares_body_is_false_without_the_slot() {
        let block: ConsumesBlock = serde_json::from_value(serde_json::json!({
            "body": {"messages": {"type": "array"}}
        }))
        .unwrap();
        let compiled = CompiledConsumes::compile(&block);
        assert!(!compiled.declares_body("attachments"));
    }

    /// GH #314 — the wire vocabulary of the declaration, and its default.
    /// `"none"` and `"all"` are the only two spellings; anything else is a
    /// parse error rather than a silently permissive fallback, because a
    /// misspelled exemption that quietly means "travels" is the worst outcome
    /// this key can have.
    #[test]
    fn transfer_policy_parses_none_and_all_and_defaults_to_all() {
        assert_eq!(
            serde_json::from_str::<TransferPolicy>("\"none\"").unwrap(),
            TransferPolicy::None
        );
        assert_eq!(
            serde_json::from_str::<TransferPolicy>("\"all\"").unwrap(),
            TransferPolicy::All
        );
        assert!(serde_json::from_str::<TransferPolicy>("\"nope\"").is_err());
        assert_eq!(TransferPolicy::default(), TransferPolicy::All);
    }

    /// The bundle the substrate carries: both halves default to the
    /// pre-declaration behaviour, and `exempt()` moves exactly one of them.
    #[test]
    fn transfer_bounds_default_is_the_behaviour_that_predates_both_keys() {
        let d = TransferBounds::default();
        assert_eq!(d.write_surface, WriteSurface::Open);
        assert_eq!(d.policy, TransferPolicy::All);
        assert!(!d.is_exempt());

        let e = TransferBounds::exempt();
        assert!(e.is_exempt());
        assert_eq!(
            e.write_surface,
            WriteSurface::Open,
            "declaring one boundary must not switch on the other"
        );
    }

    #[test]
    fn validate_consumes_blob_uuid_type_requires_uuid_string() {
        let block: ConsumesBlock = serde_json::from_value(serde_json::json!({
            "hop": {"b1": {"type": "blob_uuid"}}
        }))
        .unwrap();
        let compiled = CompiledConsumes::compile(&block);
        let mut headers = crate::Headers::new();
        headers
            .hop
            .insert("b1".into(), serde_json::json!("not-a-uuid"));
        assert!(
            validate_consumes(&serde_json::json!({"messages": []}), &headers, &compiled).is_err()
        );
        headers.hop.insert(
            "b1".into(),
            serde_json::json!("018f6f7a-1234-7abc-8def-0123456789ab"),
        );
        assert!(
            validate_consumes(&serde_json::json!({"messages": []}), &headers, &compiled).is_ok()
        );
    }
}
