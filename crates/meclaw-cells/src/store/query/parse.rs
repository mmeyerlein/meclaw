//! The one parse path: store op payload (JSON) → query IR. Pure — no database
//! handle, no SQL string. Validation happens here so that `validate ≡ execute`:
//! whatever this module accepts is exactly what the renderer will emit.

use super::{Cmp, Dir, Filter, OrderTerm, Predicate};
use crate::store::ops::json_to_sql_value;
use meclaw_core::serde_json::Value;

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

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

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
