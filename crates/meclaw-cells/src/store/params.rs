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

    /// R9: identifiers declared here are formatted into `CREATE TABLE` DDL, so
    /// they are gated by syntax at parse time — one parse path, so a hostile
    /// schema never reaches `validate` OR `spawn`.
    #[test]
    fn params_schema_rejects_injection_shaped_identifiers() {
        assert!(StoreParams::parse(&json!({"schema":{"t\"x":{"a":"int"}}})).is_err());
        assert!(StoreParams::parse(&json!({"schema":{"t":{"a\"b":"int"}}})).is_err());
        assert!(StoreParams::parse(&json!({"schema":{"sqlite_x":{"a":"int"}}})).is_err());
        assert!(StoreParams::parse(&json!({"schema":{"t_fts":{"a":"int"}}})).is_err());
        assert!(StoreParams::parse(&json!({"schema":{"9t":{"a":"int"}}})).is_err());
        // the shipped shapes stay valid
        assert!(
            StoreParams::parse(&json!({"schema":{"pending_extraction":{"batch_id":"text"}}}))
                .is_ok()
        );
    }

    #[test]
    fn parses_and_validates_fts_declaration() {
        let p = StoreParams::parse(&json!({
            "schema": {"facts": {"id":"text","claim":"text"}},
            "fts": {"facts": ["claim"]}
        }))
        .unwrap();
        assert_eq!(p.fts["facts"], vec!["claim".to_string()]);

        // table not declared in schema
        assert!(
            StoreParams::parse(&json!({"schema":{"t":{"a":"text"}},"fts":{"x":["a"]}})).is_err()
        );
        // column not in that table
        assert!(
            StoreParams::parse(&json!({"schema":{"t":{"a":"text"}},"fts":{"t":["b"]}})).is_err()
        );
        // empty column list
        assert!(StoreParams::parse(&json!({"schema":{"t":{"a":"text"}},"fts":{"t":[]}})).is_err());
        // int columns cannot be indexed
        assert!(
            StoreParams::parse(&json!({"schema":{"t":{"a":"int"}},"fts":{"t":["a"]}})).is_err()
        );
        // a `rank` column would collide with the bm25 result column
        assert!(
            StoreParams::parse(&json!({"schema":{"t":{"a":"text","rank":"text"}},
                                       "fts":{"t":["a"]}}))
            .is_err()
        );
        // fts must be an object of arrays of strings
        assert!(StoreParams::parse(&json!({"schema":{"t":{"a":"text"}},"fts":[]})).is_err());
        assert!(StoreParams::parse(&json!({"schema":{"t":{"a":"text"}},"fts":{"t":[1]}})).is_err());
        // absent fts is the default
        assert!(
            StoreParams::parse(&json!({"schema":{"t":{"a":"text"}}}))
                .unwrap()
                .fts
                .is_empty()
        );
    }

    #[test]
    fn fts_is_immutable_at_runtime() {
        let p = StoreParams::parse(&json!({"schema":{"t":{"a":"text"}}})).unwrap();
        let upd = json!({"fts": {"t": ["a"]}}).as_object().unwrap().clone();
        assert!(
            crate::params_overlay::apply_update(&p, &upd).is_err(),
            "fts is bootstrap-only, like schema"
        );
    }

    /// The overlay core round-trips params through `to_value` → merge → `parse`;
    /// an empty `fts` must serialize to an ABSENT key, not `{}`/`null`.
    #[test]
    fn empty_fts_round_trips_losslessly() {
        let p = StoreParams::parse(&json!({"schema":{"t":{"a":"text"}}})).unwrap();
        let v = meclaw_core::serde_json::to_value(&p).unwrap();
        assert!(v.get("fts").is_none(), "empty fts must not serialize");
        StoreParams::parse(&v).expect("round trip must parse");
    }

    /// The shipped template must keep parsing — the gate is a hardening, not a
    /// migration (compat receipt: plans/p3-fixtures/identifier-compat-scan.py).
    #[test]
    fn memory_hive_template_params_still_parse() {
        let raw = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../builder/templates/memory-hive/store/config.json"
        ))
        .unwrap();
        let v: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
        StoreParams::parse(&v["params"]).expect("shipped template must stay valid");
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
    /// Opt-in full-text indexes: `{ "<table>": ["<column>", …] }` (P3).
    ///
    /// A separate top-level key rather than an entry inside `schema`, because a
    /// `schema` table entry is a column→type map — an `"fts"` key there would be
    /// indistinguishable from a column named `fts` (plan ruling R6). Validated
    /// against `schema` right here, so the one parse path covers it.
    ///
    /// Known limit: only tables declared in `schema` can carry an index —
    /// tables created at runtime via the `create_table` op cannot (the FTS DDL
    /// is bootstrap work of the factory, not a message effect).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub fts: BTreeMap<String, Vec<String>>,
}

impl crate::params_overlay::OverlayParams for StoreParams {
    /// `schema` (immutable) + `query_timeout_ms` (mutable, Weg C).
    const KNOWN_KEYS: &'static [&'static str] = &["schema", "query_timeout_ms", "fts"];
    /// `schema` is bootstrap-only — it is baked into `cell.db` via DDL at spawn;
    /// changing it at runtime would desync the live tables from the declared
    /// schema. Rejected on any update attempt.
    const IMMUTABLE_KEYS: &'static [&'static str] = &["schema", "fts"];
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
            crate::store::ddl::check_new_identifier("params.schema table", table)?;
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
                crate::store::ddl::check_new_identifier(
                    &format!("params.schema.{table} column"),
                    col,
                )?;
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
        let fts = parse_fts(obj.get("fts"), &schema)?;
        Ok(StoreParams {
            schema,
            query_timeout_ms,
            fts,
        })
    }
}

/// Validate `params.fts` against the already-parsed `schema` (R6).
///
/// Every rule is a closed-set check against the declared tables and columns, so
/// a declaration that survives here can always be turned into DDL.
fn parse_fts(
    raw: Option<&Value>,
    schema: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let Some(raw) = raw else {
        return Ok(BTreeMap::new());
    };
    let obj = raw.as_object().ok_or("params.fts must be an object")?;
    let mut out = BTreeMap::new();
    for (table, cols_v) in obj {
        let declared = schema
            .get(table)
            .ok_or_else(|| format!("params.fts.{table}: not declared in params.schema"))?;
        if declared.contains_key("rank") {
            return Err(format!(
                "params.fts.{table}: table has a column named \"rank\", which collides with the \
                 bm25 result column of the search op"
            ));
        }
        let arr = cols_v
            .as_array()
            .ok_or_else(|| format!("params.fts.{table} must be an array of column names"))?;
        if arr.is_empty() {
            return Err(format!("params.fts.{table} must name at least one column"));
        }
        let mut cols = Vec::with_capacity(arr.len());
        for c in arr {
            let col = c
                .as_str()
                .ok_or_else(|| format!("params.fts.{table}: column names must be strings"))?;
            match declared.get(col).map(String::as_str) {
                Some("text") | Some("json") => {}
                Some(other) => {
                    return Err(format!(
                        "params.fts.{table}.{col}: type {other:?} cannot be indexed (text/json only)"
                    ));
                }
                None => {
                    return Err(format!(
                        "params.fts.{table}.{col}: not a column of that table"
                    ));
                }
            }
            cols.push(col.to_string());
        }
        out.insert(table.clone(), cols);
    }
    Ok(out)
}
