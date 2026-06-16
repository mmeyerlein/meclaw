//! Phase-9 store params: 2-stage schema map + optional query_timeout_ms.

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn parses_minimal_schema_one_table() {
        let raw = json!({
            "schema": { "items": { "id": "int", "name": "text" } }
        });
        let p = StoreParams::parse(&raw).unwrap();
        assert_eq!(p.schema.len(), 1);
        let cols = &p.schema["items"];
        assert_eq!(cols.get("id"), Some(&"int".to_string()));
        assert_eq!(cols.get("name"), Some(&"text".to_string()));
        assert!(p.query_timeout_ms.is_none());
    }

    #[test]
    fn parses_query_timeout_ms() {
        let raw = json!({
            "schema": { "t": { "c": "text" } },
            "query_timeout_ms": 12345
        });
        let p = StoreParams::parse(&raw).unwrap();
        assert_eq!(p.query_timeout_ms, Some(12345));
    }

    #[test]
    fn rejects_missing_schema() {
        let r = StoreParams::parse(&json!({}));
        assert!(r.is_err());
    }

    #[test]
    fn rejects_non_object_schema() {
        let r = StoreParams::parse(&json!({ "schema": [] }));
        assert!(r.is_err());
    }

    #[test]
    fn rejects_non_object_table_def() {
        let r = StoreParams::parse(&json!({ "schema": { "t": "text" } }));
        assert!(r.is_err());
    }

    #[test]
    fn rejects_non_string_column_type() {
        let r = StoreParams::parse(&json!({ "schema": { "t": { "c": 1 } } }));
        assert!(r.is_err());
    }
}

use meclaw_core::serde_json::Value;
use serde::Serialize;
use std::collections::BTreeMap;

/// Parsed `store` cell params.
///
/// Phase 9: `schema` is a 2-stage map `{ "<table>": { "<col>": "<type>" } }`.
///
/// Allowed column types (Phase 9): `"text"`, `"int"`, `"json"`.
/// Constraints (PK, NOT NULL, UNIQUE, defaults, indices) are deferred
/// (see brainstorm E6).
///
/// `Serialize` (β): the generic params-overlay core round-trips a params struct
/// through `serde_json::to_value` → merge → `parse`. `query_timeout_ms` carries
/// `skip_serializing_if` so a `None` serializes to an ABSENT key (not `null`) —
/// the manual `parse` rejects a `null` `query_timeout_ms`, so omitting it keeps
/// the round-trip lossless.
#[derive(Debug, Clone, Serialize)]
pub struct StoreParams {
    /// Schema definition: outer key is table name, inner key is column name,
    /// value is column type (`"text"`, `"int"`, or `"json"`).
    pub schema: BTreeMap<String, BTreeMap<String, String>>,
    /// Optional query timeout in milliseconds applied to user-facing queries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_timeout_ms: Option<u64>,
}

impl crate::params_overlay::OverlayParams for StoreParams {
    /// `schema` (immutable) + `query_timeout_ms` (mutable, Weg C).
    const KNOWN_KEYS: &'static [&'static str] = &["schema", "query_timeout_ms"];
    /// `schema` is bootstrap-only — it is baked into `cell.db` via DDL at spawn;
    /// changing it at runtime would desync the live tables from the declared
    /// schema. Rejected on any update attempt.
    const IMMUTABLE_KEYS: &'static [&'static str] = &["schema"];
    fn parse(raw: &Value) -> Result<Self, String> {
        StoreParams::parse(raw)
    }
}

impl StoreParams {
    /// Parse raw params from a JSON value.
    ///
    /// Returns `Err` with an operator-readable message on missing or malformed
    /// `schema`, unsupported column types, or non-integer `query_timeout_ms`.
    pub fn parse(raw: &Value) -> Result<Self, String> {
        let obj = raw.as_object().ok_or("params must be a JSON object")?;
        let schema_v = obj.get("schema").ok_or("params.schema required")?;
        let schema_obj = schema_v
            .as_object()
            .ok_or("params.schema must be an object")?;
        if schema_obj.is_empty() {
            return Err("params.schema must declare at least one table".into());
        }
        let mut schema = BTreeMap::new();
        for (table, cols_v) in schema_obj {
            let cols_obj = cols_v
                .as_object()
                .ok_or_else(|| format!("params.schema.{table} must be an object"))?;
            if cols_obj.is_empty() {
                return Err(format!(
                    "params.schema.{table} must declare at least one column"
                ));
            }
            let mut cols = BTreeMap::new();
            for (col, ty_v) in cols_obj {
                let ty = ty_v
                    .as_str()
                    .ok_or_else(|| format!("params.schema.{table}.{col} must be a string"))?;
                if !matches!(ty, "text" | "int" | "json") {
                    return Err(format!(
                        "params.schema.{table}.{col}: unsupported type {ty:?} (allowed: text/int/json)"
                    ));
                }
                cols.insert(col.clone(), ty.to_string());
            }
            schema.insert(table.clone(), cols);
        }
        let query_timeout_ms = match obj.get("query_timeout_ms") {
            None => None,
            Some(v) => Some(v.as_u64().ok_or("query_timeout_ms must be an integer")?),
        };
        Ok(StoreParams {
            schema,
            query_timeout_ms,
        })
    }
}
