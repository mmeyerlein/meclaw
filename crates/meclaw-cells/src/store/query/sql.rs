//! The one render path: query IR + [`Catalog`] → parameterized SQL.
//!
//! Two rules hold for every string this module produces:
//! 1. identifiers come from the catalog, keywords and operator symbols from a
//!    Rust enum — caller text is bound, never formatted;
//! 2. an `or_null` predicate is always parenthesised, so it cannot degenerate
//!    into `a = ? AND b > ? OR b IS NULL` under conjunction (which on a
//!    `delete` would be a mass delete).

use super::catalog::{Catalog, CatalogError};
use super::{Filter, OrderTerm, Predicate};
use rusqlite::types::Value as SqlValue;

/// Render the statement tail: `ORDER BY … LIMIT ?`.
///
/// Column names come from the catalog, the direction keyword from the [`Dir`]
/// enum, and the limit is appended to `vals` as a bound parameter — the tail
/// contains no caller-supplied character.
///
/// [`Dir`]: super::Dir
pub fn render_tail(
    order_by: &[OrderTerm],
    limit: Option<i64>,
    cat: &Catalog,
    vals: &mut Vec<SqlValue>,
) -> Result<String, CatalogError> {
    render_tail_qualified(order_by, limit, cat, vals, false)
}

/// As [`render_tail`], with the same table qualification as
/// [`render_where_qualified`] (needed by the `search` join).
pub fn render_tail_qualified(
    order_by: &[OrderTerm],
    limit: Option<i64>,
    cat: &Catalog,
    vals: &mut Vec<SqlValue>,
    qualify: bool,
) -> Result<String, CatalogError> {
    let mut out = String::new();
    if !order_by.is_empty() {
        let terms = order_by
            .iter()
            .map(|t| {
                Ok(format!(
                    "\"{}\" {}",
                    qualified(cat, &t.col, qualify)?,
                    t.dir.keyword()
                ))
            })
            .collect::<Result<Vec<_>, CatalogError>>()?;
        out.push_str(&format!(" ORDER BY {}", terms.join(", ")));
    }
    if let Some(n) = limit {
        out.push_str(" LIMIT ?");
        vals.push(SqlValue::Integer(n));
    }
    Ok(out)
}

/// Render filters into a `WHERE` clause plus its bound values. An empty filter
/// list renders the empty string — the Phase-9 "no where" behaviour.
pub fn render_where(
    filters: &[Filter],
    cat: &Catalog,
) -> Result<(String, Vec<SqlValue>), CatalogError> {
    render_where_qualified(filters, cat, false)
}

/// As [`render_where`], but every column is prefixed with the (catalog-owned)
/// table name. The `search` op joins the base table against its FTS index, and
/// both carry the indexed column names — unqualified, `"claim" = ?` would be
/// ambiguous.
pub fn render_where_qualified(
    filters: &[Filter],
    cat: &Catalog,
    qualify: bool,
) -> Result<(String, Vec<SqlValue>), CatalogError> {
    if filters.is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    let mut clauses = Vec::with_capacity(filters.len());
    let mut vals = Vec::new();
    for f in filters {
        let col = qualified(cat, &f.col, qualify)?;
        clauses.push(render_predicate(&col, &f.pred, &mut vals));
    }
    Ok((format!(" WHERE {}", clauses.join(" AND ")), vals))
}

/// `col` or `table"."col` — both halves catalog-owned, so the caller's string
/// never survives into the statement either way.
fn qualified(cat: &Catalog, want: &str, qualify: bool) -> Result<String, CatalogError> {
    let col = cat.column(want)?;
    Ok(if qualify {
        format!("{}\".\"{col}", cat.table())
    } else {
        col.to_string()
    })
}

fn render_predicate(col: &str, pred: &Predicate, vals: &mut Vec<SqlValue>) -> String {
    match pred {
        Predicate::Cmp(op, v) => {
            vals.push(v.clone());
            format!("\"{col}\" {} ?", op.symbol())
        }
        Predicate::In(list) => {
            let marks = list.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            vals.extend(list.iter().cloned());
            format!("\"{col}\" IN ({marks})")
        }
        Predicate::IsNull(true) => format!("\"{col}\" IS NULL"),
        Predicate::IsNull(false) => format!("\"{col}\" IS NOT NULL"),
        Predicate::OrNull(inner) => {
            format!(
                "({} OR \"{col}\" IS NULL)",
                render_predicate(col, inner, vals)
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::query::parse::parse_filters;
    use meclaw_core::serde_json::json;

    fn cat(conn: &rusqlite::Connection, t: &str) -> Catalog {
        Catalog::load(conn, t).unwrap()
    }

    /// Backward-compatibility lock: a Phase-9 equality payload must still render
    /// the exact same clause, character for character.
    #[test]
    fn legacy_equality_renders_byte_identical_to_phase9() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
            .unwrap();
        let f = parse_filters(Some(&json!({"id": 2, "name": "b"}))).unwrap();
        let (clause, vals) = render_where(&f, &cat(&c, "items")).unwrap();
        assert_eq!(clause, " WHERE \"id\" = ? AND \"name\" = ?");
        assert_eq!(vals.len(), 2);
    }

    #[test]
    fn operator_forms_render_parameterized() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE f (a TEXT, b TEXT)", []).unwrap();
        let cat = cat(&c, "f");

        let f = parse_filters(Some(
            &json!({"a": {"lte": "T"}, "b": {"or_null": {"gt": "T"}}}),
        ))
        .unwrap();
        let (clause, vals) = render_where(&f, &cat).unwrap();
        assert_eq!(clause, " WHERE \"a\" <= ? AND (\"b\" > ? OR \"b\" IS NULL)");
        assert_eq!(vals.len(), 2);

        let f2 = parse_filters(Some(&json!({"a": {"in": [1, 2, 3]}}))).unwrap();
        assert_eq!(
            render_where(&f2, &cat).unwrap().0,
            " WHERE \"a\" IN (?,?,?)"
        );

        let f3 = parse_filters(Some(&json!({"a": {"is_null": false}}))).unwrap();
        assert_eq!(
            render_where(&f3, &cat).unwrap().0,
            " WHERE \"a\" IS NOT NULL"
        );

        let f4 = parse_filters(Some(&json!({"a": {"or_null": {"in": [1, 2]}}}))).unwrap();
        assert_eq!(
            render_where(&f4, &cat).unwrap().0,
            " WHERE (\"a\" IN (?,?) OR \"a\" IS NULL)"
        );
    }

    #[test]
    fn empty_filters_render_nothing() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE f (a TEXT)", []).unwrap();
        let (clause, vals) = render_where(&[], &cat(&c, "f")).unwrap();
        assert!(clause.is_empty());
        assert!(vals.is_empty());
    }

    #[test]
    fn unknown_column_in_where_is_a_catalog_error() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE f (a TEXT)", []).unwrap();
        let f = parse_filters(Some(&json!({"nope": 1}))).unwrap();
        assert!(matches!(
            render_where(&f, &cat(&c, "f")),
            Err(CatalogError::UnknownColumn(_))
        ));
    }
}
