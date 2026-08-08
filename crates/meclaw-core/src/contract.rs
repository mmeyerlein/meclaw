//! Contract `emits` validation — the generic mechanism behind Debt-Register
//! P13/D-010a. Translates the compact `EmitSpec` map (docs/config.md § emits,
//! l.90) into two Draft-2020-12 documents (body + hop) and compiles them
//! with the `jsonschema` crate (already a dependency; same engine as the UBF
//! validator in `schema.rs`). The `jsonschema` dependency is encapsulated here.

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

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

/// Pre-extracted required-key views of a `consumes` block, one entry per
/// compartment: `(key, type-token)` for every `required: true` ConsumeSpec.
/// Compile is infallible (pure projection) — unlike `CompiledEmits` there is
/// no schema to build.
#[derive(Debug, Clone, Default)]
pub struct CompiledConsumes {
    body: Vec<(String, String)>,
    context: Vec<(String, String)>,
    hop: Vec<(String, String)>,
}

impl CompiledConsumes {
    /// Project the `required: true` entries of all three compartments.
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
        }
    }

    /// True iff no compartment carries a required key (check is vacuous).
    pub fn is_vacuous(&self) -> bool {
        self.body.is_empty() && self.context.is_empty() && self.hop.is_empty()
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
