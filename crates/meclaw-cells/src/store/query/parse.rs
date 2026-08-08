//! The one parse path: store op payload (JSON) → query IR. Pure — no database
//! handle, no SQL string. Validation happens here so that `validate ≡ execute`:
//! whatever this module accepts is exactly what the renderer will emit.

use super::{
    Cmp, DISTANCE_COLUMN, Dir, Filter, MAX_DEPTH_CAP, MAX_DEPTH_DEFAULT, MAX_NODES_CAP,
    MAX_NODES_DEFAULT, OrderTerm, Predicate, SimilarSpec, TRAVERSE_CTE, TraverseSpec,
};
use crate::store::ops::json_to_sql_value;
use meclaw_core::serde_json::{Map, Value};

/// Parse the optional `where` argument into filters.
///
/// A bare (non-object) value keeps its Phase-9 meaning `eq`, so every payload
/// that worked before this package still renders the identical SQL. An object
/// value is the explicit operator form and must carry exactly one known
/// operator key — an unknown key is a loud reject, never a silent fallback to
/// "equality against a JSON literal".
pub fn parse_filters(where_v: Option<&Value>) -> Result<Vec<Filter>, String> {
    let Some(w) = where_v else {
        return Ok(Vec::new());
    };
    let obj = w.as_object().ok_or("where must be JSON object")?;
    let mut out = Vec::with_capacity(obj.len());
    for (col, spec) in obj {
        out.push(Filter {
            col: col.clone(),
            pred: parse_predicate(col, spec)?,
        });
    }
    Ok(out)
}

fn parse_predicate(col: &str, spec: &Value) -> Result<Predicate, String> {
    match spec {
        Value::Object(_) => parse_operator_object(col, spec, true),
        other => Ok(Predicate::Cmp(Cmp::Eq, json_to_sql_value(Some(other)))),
    }
}

/// `top_level` is false inside an `or_null` wrapper: that is what pins the
/// nesting depth to exactly one and keeps `or_null(is_null)` out.
fn parse_operator_object(col: &str, spec: &Value, top_level: bool) -> Result<Predicate, String> {
    let obj = spec
        .as_object()
        .ok_or_else(|| format!("where.{col}: operator spec must be an object"))?;
    let mut it = obj.iter();
    let (key, val) = it
        .next()
        .ok_or_else(|| format!("where.{col}: empty operator object"))?;
    if it.next().is_some() {
        return Err(format!(
            "where.{col}: operator object must carry exactly one key"
        ));
    }
    let cmp = match key.as_str() {
        "eq" => Cmp::Eq,
        "neq" => Cmp::Neq,
        "lt" => Cmp::Lt,
        "lte" => Cmp::Lte,
        "gt" => Cmp::Gt,
        "gte" => Cmp::Gte,
        "in" => return parse_in(col, val),
        "is_null" if top_level => {
            return val
                .as_bool()
                .map(Predicate::IsNull)
                .ok_or_else(|| format!("where.{col}.is_null must be a bool"));
        }
        "or_null" if top_level => {
            return Ok(Predicate::OrNull(Box::new(parse_operator_object(
                col, val, false,
            )?)));
        }
        other => return Err(format!("where.{col}: unknown operator {other:?}")),
    };
    Ok(Predicate::Cmp(cmp, json_to_sql_value(Some(val))))
}

fn parse_in(col: &str, val: &Value) -> Result<Predicate, String> {
    let arr = val
        .as_array()
        .ok_or_else(|| format!("where.{col}.in must be an array"))?;
    if arr.is_empty() {
        return Err(format!("where.{col}.in must not be empty"));
    }
    if arr.iter().any(|v| v.is_array() || v.is_object()) {
        return Err(format!("where.{col}.in accepts scalars only"));
    }
    Ok(Predicate::In(
        arr.iter().map(|v| json_to_sql_value(Some(v))).collect(),
    ))
}

/// Parse the optional `order_by` argument: an array of `{"col", "dir"?}`.
/// `dir` is validated against the closed set `asc`/`desc` and stored as an enum,
/// so an injection-shaped direction is a reject, never a rendered token.
pub fn parse_order_by(v: Option<&Value>) -> Result<Vec<OrderTerm>, String> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let arr = v.as_array().ok_or("order_by must be an array")?;
    arr.iter()
        .map(|t| {
            let o = t.as_object().ok_or("order_by entry must be an object")?;
            let col = o
                .get("col")
                .and_then(|c| c.as_str())
                .ok_or("order_by entry missing col")?
                .to_string();
            let dir = match o.get("dir").map(|d| d.as_str().unwrap_or("")) {
                None | Some("asc") => Dir::Asc,
                Some("desc") => Dir::Desc,
                Some(other) => {
                    return Err(format!("order_by.dir must be asc|desc, got {other:?}"));
                }
            };
            Ok(OrderTerm { col, dir })
        })
        .collect()
}

/// Parse the optional `limit` argument.
///
/// No default and no cap (plan ruling R3): an implicit limit would silently
/// truncate every pre-P3 select, and a cap protects against nothing that is not
/// already unbounded today. The value is bound as a parameter, never formatted.
pub fn parse_limit(v: Option<&Value>) -> Result<Option<i64>, String> {
    match v {
        None => Ok(None),
        Some(x) => match x.as_i64() {
            Some(n) if n >= 1 => Ok(Some(n)),
            _ => Err("limit must be an integer >= 1".into()),
        },
    }
}

/// Parse the mandatory `columns` projection shared by `select`/`search`/`similar`.
fn parse_columns(args: &Map<String, Value>) -> Result<Vec<String>, String> {
    let arr = args
        .get("columns")
        .and_then(|v| v.as_array())
        .ok_or("missing columns array")?;
    let cols: Vec<String> = arr
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or("columns entry not string")
                .map(|s| s.to_string())
        })
        .collect::<Result<_, _>>()?;
    if cols.is_empty() {
        return Err("columns must declare at least one entry".into());
    }
    Ok(cols)
}

/// Parse a `traverse` op payload (memory-spec A.2.4).
///
/// Guards are mandatory by construction: both have a default, both have a hard
/// cap, and a value beyond the cap is rejected rather than clamped — a caller
/// who asked for depth 9 must not believe they got depth 9.
pub fn parse_traverse(args: &Map<String, Value>) -> Result<TraverseSpec, String> {
    let table = args
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or("missing table")?;
    if table == TRAVERSE_CTE {
        return Err(format!(
            "table must not be named {TRAVERSE_CTE:?} — that is the traversal's own CTE"
        ));
    }
    let role = |name: &str| -> Result<String, String> {
        args.get(name)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("missing {name} column role"))
    };
    let optional_role = |name: &str| -> Result<Option<String>, String> {
        match args.get(name) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(format!("{name} column role must be a string")),
        }
    };
    let max_depth = parse_guard(
        args.get("max_depth"),
        "max_depth",
        MAX_DEPTH_DEFAULT,
        MAX_DEPTH_CAP,
    )?;
    let max_nodes = parse_guard(
        args.get("max_nodes"),
        "max_nodes",
        MAX_NODES_DEFAULT,
        MAX_NODES_CAP,
    )?;
    let start = parse_start(args.get("start"), max_nodes)?;
    let columns = match args.get("columns") {
        None => Vec::new(),
        Some(_) => parse_columns(args)?,
    };
    Ok(TraverseSpec {
        src: role("src")?,
        dst: role("dst")?,
        kind: optional_role("kind")?,
        weight: optional_role("weight")?,
        start,
        columns,
        filters: parse_filters(args.get("where"))?,
        max_depth,
        max_nodes,
    })
}

/// One traversal guard: optional, integer, within `[1, cap]`. Out of range is a
/// loud reject — silent clamping is the failure mode this repo has banned.
fn parse_guard(v: Option<&Value>, name: &str, default: i64, cap: i64) -> Result<i64, String> {
    let Some(x) = v else { return Ok(default) };
    match x.as_i64() {
        Some(n) if (1..=cap).contains(&n) => Ok(n),
        _ => Err(format!("{name} must be an integer between 1 and {cap}")),
    }
}

/// The start nodes: a scalar or a non-empty list of scalars, bound as values.
/// The list may not itself exceed the fan-out guard.
fn parse_start(v: Option<&Value>, max_nodes: i64) -> Result<Vec<rusqlite::types::Value>, String> {
    let v = v.ok_or("missing start")?;
    let items: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        other => vec![other],
    };
    if items.is_empty() {
        return Err("start must not be empty".into());
    }
    if items.len() as i64 > max_nodes {
        return Err(format!(
            "start must not list more than max_nodes ({max_nodes}) nodes"
        ));
    }
    if items
        .iter()
        .any(|i| i.is_array() || i.is_object() || i.is_null())
    {
        return Err("start accepts non-null scalars only".into());
    }
    Ok(items.iter().map(|i| json_to_sql_value(Some(i))).collect())
}

/// Parse a `similar` op payload.
///
/// The query vector is decoded here, not at render time: the parser accepts
/// exactly what the renderer can bind (`validate ≡ execute`). `distance` is the
/// name of the computed rank column and therefore not available as a projection
/// name — a collision would silently overwrite the caller's column.
pub fn parse_similar(args: &Map<String, Value>) -> Result<SimilarSpec, String> {
    let columns = parse_columns(args)?;
    if columns.iter().any(|c| c == DISTANCE_COLUMN) {
        return Err(format!(
            "columns must not contain {DISTANCE_COLUMN:?} — it is the computed rank column"
        ));
    }
    let vector_column = args
        .get("vector_column")
        .and_then(|v| v.as_str())
        .ok_or("missing vector_column")?
        .to_string();
    let vector = args
        .get("vector")
        .and_then(|v| v.as_str())
        .ok_or("missing vector")?
        .to_string();
    let bytes =
        crate::store::query::hamming::decode_base64(&vector).map_err(|e| format!("vector: {e}"))?;
    if bytes.is_empty() {
        return Err("vector must not be empty".into());
    }
    Ok(SimilarSpec {
        columns,
        vector_column,
        vector,
        filters: parse_filters(args.get("where"))?,
        order_by: parse_order_by(args.get("order_by"))?,
        limit: parse_limit(args.get("limit"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn obj(v: &Value) -> &meclaw_core::serde_json::Map<String, Value> {
        v.as_object().unwrap()
    }

    fn traverse_base() -> Value {
        json!({"operation":"traverse","table":"entity_edges",
               "src":"src_entity","dst":"dst_entity","start":"e:n1"})
    }

    fn with(extra: Value) -> Value {
        let mut base = traverse_base();
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        base
    }

    #[test]
    fn traverse_defaults_are_depth_two_and_two_hundred_nodes() {
        let t = parse_traverse(obj(&traverse_base())).unwrap();
        assert_eq!(t.max_depth, 2);
        assert_eq!(t.max_nodes, 200);
        assert_eq!(t.start.len(), 1);
        assert!(t.kind.is_none() && t.weight.is_none());
        assert!(t.columns.is_empty() && t.filters.is_empty());
    }

    /// Guards reject instead of clamping: a caller who asks for depth 9 must
    /// learn that they did not get depth 9 (plan R3).
    #[test]
    fn traverse_rejects_guards_outside_their_caps_instead_of_clamping() {
        for bad in [
            json!({"max_depth": 6}),
            json!({"max_depth": 0}),
            json!({"max_depth": -1}),
            json!({"max_depth": "3"}),
            json!({"max_depth": 2.5}),
            json!({"max_nodes": 5001}),
            json!({"max_nodes": 0}),
            json!({"max_nodes": "200; DROP TABLE keep"}),
        ] {
            let payload = with(bad.clone());
            assert!(parse_traverse(obj(&payload)).is_err(), "must reject {bad}");
        }
        assert_eq!(
            parse_traverse(obj(&with(json!({"max_depth":5}))))
                .unwrap()
                .max_depth,
            5
        );
        assert_eq!(
            parse_traverse(obj(&with(json!({"max_nodes":5000}))))
                .unwrap()
                .max_nodes,
            5000
        );
    }

    #[test]
    fn traverse_requires_roles_and_a_non_empty_start() {
        for bad in [
            json!({"table":"e","dst":"d","start":"a"}),
            json!({"table":"e","src":"s","start":"a"}),
            json!({"table":"e","src":"s","dst":"d"}),
            json!({"table":"e","src":"s","dst":"d","start":[]}),
            json!({"table":"e","src":"s","dst":"d","start":[{"a":1}]}),
            json!({"table":"e","src":"s","dst":"d","start":"a","max_nodes":2,
                   "start_list_too_long":true}),
        ] {
            if bad.get("start_list_too_long").is_some() {
                let big = json!({"table":"e","src":"s","dst":"d","max_nodes":2,
                                 "start":["a","b","c"]});
                assert!(
                    parse_traverse(obj(&big)).is_err(),
                    "start list beyond max_nodes"
                );
                continue;
            }
            assert!(parse_traverse(obj(&bad)).is_err(), "must reject {bad}");
        }
    }

    /// The CTE name is a fixed literal; a table of that name would silently
    /// resolve to the CTE instead of the table (plan R5).
    #[test]
    fn traverse_rejects_a_table_named_like_the_cte() {
        let payload = json!({"table":"mc_traverse","src":"s","dst":"d","start":"a"});
        assert!(parse_traverse(obj(&payload)).is_err());
    }

    #[test]
    fn similar_parses_full_form() {
        let v = json!({"operation":"similar","table":"embeddings","columns":["owner_id","model_id"],
                       "vector_column":"blob","vector":"AA==","where":{"model_id":"m1"},
                       "order_by":[{"col":"owner_id","dir":"desc"}],"limit":10});
        let s = parse_similar(obj(&v)).unwrap();
        assert_eq!(s.vector_column, "blob");
        assert_eq!(
            s.columns,
            vec!["owner_id".to_string(), "model_id".to_string()]
        );
        assert_eq!(s.vector, "AA==");
        assert_eq!(s.limit, Some(10));
        assert_eq!(s.filters.len(), 1);
        assert_eq!(s.order_by.len(), 1);
    }

    /// The vector is decoded at parse time, so `validate ≡ execute`: whatever
    /// the parser accepts is something the renderer can actually bind.
    #[test]
    fn similar_rejects_missing_and_hostile_args() {
        for bad in [
            json!({"table":"e","columns":["a"],"vector":"AA=="}),
            json!({"table":"e","columns":["a"],"vector_column":"blob"}),
            json!({"table":"e","columns":[],"vector_column":"blob","vector":"AA=="}),
            json!({"table":"e","columns":["a","distance"],"vector_column":"blob","vector":"AA=="}),
            json!({"table":"e","columns":["a"],"vector_column":"blob","vector":""}),
            json!({"table":"e","columns":["a"],"vector_column":"blob","vector":"not base64!"}),
            json!({"table":"e","columns":["a"],"vector_column":"blob","vector":"\"; DROP TABLE keep; --"}),
            json!({"table":"e","columns":["a"],"vector_column":5,"vector":"AA=="}),
            json!({"table":"e","columns":"a","vector_column":"blob","vector":"AA=="}),
        ] {
            assert!(parse_similar(obj(&bad)).is_err(), "must reject {bad}");
        }
    }

    #[test]
    fn bare_value_parses_as_eq_in_key_order() {
        let f = parse_filters(Some(&json!({"a": 1, "b": "x"}))).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].col, "a");
        assert!(matches!(f[0].pred, Predicate::Cmp(Cmp::Eq, _)));
        assert_eq!(f[1].col, "b");
    }

    #[test]
    fn comparison_operators_parse() {
        for (key, want) in [
            ("eq", Cmp::Eq),
            ("neq", Cmp::Neq),
            ("lt", Cmp::Lt),
            ("lte", Cmp::Lte),
            ("gt", Cmp::Gt),
            ("gte", Cmp::Gte),
        ] {
            let f = parse_filters(Some(&json!({"c": {key: 5}}))).unwrap();
            match &f[0].pred {
                Predicate::Cmp(op, _) => assert_eq!(*op, want, "op {key}"),
                other => panic!("expected Cmp for {key}, got {other:?}"),
            }
        }
    }

    #[test]
    fn unknown_operator_key_is_rejected() {
        let e = parse_filters(Some(&json!({"c": {"lte ": 5}}))).unwrap_err();
        assert!(e.contains("unknown operator"), "got {e}");
    }

    #[test]
    fn operator_object_with_two_keys_is_rejected() {
        assert!(parse_filters(Some(&json!({"c": {"lt": 1, "gt": 0}}))).is_err());
    }

    #[test]
    fn empty_operator_object_is_rejected() {
        assert!(parse_filters(Some(&json!({"c": {}}))).is_err());
    }

    #[test]
    fn in_list_parses_and_rejects_empty() {
        let f = parse_filters(Some(&json!({"c": {"in": [1, 2, 3]}}))).unwrap();
        match &f[0].pred {
            Predicate::In(v) => assert_eq!(v.len(), 3),
            other => panic!("expected In, got {other:?}"),
        }
        let e = parse_filters(Some(&json!({"c": {"in": []}}))).unwrap_err();
        assert!(e.contains("must not be empty"), "got {e}");
        assert!(parse_filters(Some(&json!({"c": {"in": "abc"}}))).is_err());
        assert!(parse_filters(Some(&json!({"c": {"in": [[1]]}}))).is_err());
    }

    #[test]
    fn is_null_and_or_null_parse_with_depth_one() {
        assert!(matches!(
            parse_filters(Some(&json!({"c": {"is_null": true}}))).unwrap()[0].pred,
            Predicate::IsNull(true)
        ));
        assert!(matches!(
            parse_filters(Some(&json!({"c": {"is_null": false}}))).unwrap()[0].pred,
            Predicate::IsNull(false)
        ));
        assert!(parse_filters(Some(&json!({"c": {"is_null": "yes"}}))).is_err());

        let f = parse_filters(Some(&json!({"c": {"or_null": {"gt": "2026"}}}))).unwrap();
        match &f[0].pred {
            Predicate::OrNull(inner) => assert!(matches!(**inner, Predicate::Cmp(Cmp::Gt, _))),
            other => panic!("expected OrNull, got {other:?}"),
        }
        // or_null wraps a comparison or an in-list — never itself, never is_null.
        assert!(parse_filters(Some(&json!({"c": {"or_null": {"or_null": {"gt": 1}}}}))).is_err());
        assert!(parse_filters(Some(&json!({"c": {"or_null": {"is_null": true}}}))).is_err());
        assert!(parse_filters(Some(&json!({"c": {"or_null": 5}}))).is_err());
    }

    #[test]
    fn order_by_parses_direction_and_defaults_to_asc() {
        let o = parse_order_by(Some(&json!([{"col":"a"},{"col":"b","dir":"desc"}]))).unwrap();
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].col, "a");
        assert_eq!(o[0].dir, Dir::Asc);
        assert_eq!(o[1].dir, Dir::Desc);
        assert!(parse_order_by(None).unwrap().is_empty());
        assert!(parse_order_by(Some(&json!([]))).unwrap().is_empty());
    }

    #[test]
    fn order_by_rejects_injection_and_bad_shape() {
        assert!(parse_order_by(Some(&json!([{"col":"a","dir":"asc; DROP TABLE t"}]))).is_err());
        assert!(parse_order_by(Some(&json!(["a"]))).is_err());
        assert!(parse_order_by(Some(&json!([{"dir":"asc"}]))).is_err());
        assert!(parse_order_by(Some(&json!({"col":"a"}))).is_err());
    }

    #[test]
    fn limit_rejects_zero_and_non_integer() {
        assert_eq!(parse_limit(Some(&json!(50))).unwrap(), Some(50));
        assert_eq!(parse_limit(None).unwrap(), None);
        assert!(parse_limit(Some(&json!(0))).is_err());
        assert!(parse_limit(Some(&json!(-1))).is_err());
        assert!(parse_limit(Some(&json!("50"))).is_err());
        assert!(parse_limit(Some(&json!("1; DROP TABLE keep"))).is_err());
    }

    #[test]
    fn non_object_where_is_rejected() {
        assert!(parse_filters(Some(&json!(["a"]))).is_err());
        assert!(parse_filters(None).unwrap().is_empty());
        assert!(parse_filters(Some(&json!({}))).unwrap().is_empty());
    }
}
