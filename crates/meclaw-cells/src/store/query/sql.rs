//! The one render path: query IR + [`Catalog`] → parameterized SQL.
//!
//! Two rules hold for every string this module produces:
//! 1. identifiers come from the catalog, keywords and operator symbols from a
//!    Rust enum — caller text is bound, never formatted;
//! 2. an `or_null` predicate is always parenthesised, so it cannot degenerate
//!    into `a = ? AND b > ? OR b IS NULL` under conjunction (which on a
//!    `delete` would be a mass delete).

use super::catalog::{Catalog, CatalogError};
use super::{
    DISTANCE_COLUMN, Filter, OrderTerm, Predicate, SimilarSpec, TRAVERSE_CTE, TraverseSpec,
};
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

/// Render the traversal CTE plus its bound values (P4, memory-spec A.2.4).
///
/// Shape decisions that are load-bearing:
/// - **cycle elimination per path**: the walk carries the visited nodes in a
///   `char(30)`-delimited `path` string, so a node is never revisited *on the
///   same path* while the same node may still be reached via another path.
///   Paths are the result, so a global visited set would drop real answers.
/// - **no `ORDER BY`**: SQLite runs the recursion as a co-routine, so the outer
///   `LIMIT` actually stops the walk. Sorting would force materialization and
///   turn the fan-out guard into a payload guard only. Scoring belongs to the
///   caller (`code` cell), which is why paths carry depth and weight.
/// - **`max_nodes + 1`** is bound as the limit: one row beyond the cap is how
///   truncation becomes visible instead of silent.
/// - projected edge columns are positional (`e0`, `e1`, …), so an edge column
///   named `depth`, `node` or `path` cannot collide with the CTE's own columns.
pub fn render_traverse(
    spec: &TraverseSpec,
    cat: &Catalog,
) -> Result<(String, Vec<SqlValue>), CatalogError> {
    let table = cat.table();
    let src = cat.column(&spec.src)?;
    let dst = cat.column(&spec.dst)?;
    // Edge attributes reported per path: kind, weight, then the projection.
    let mut edge_cols: Vec<&str> = Vec::new();
    if let Some(k) = &spec.kind {
        edge_cols.push(cat.column(k)?);
    }
    let weight = spec.weight.as_ref().map(|w| cat.column(w)).transpose()?;
    if let Some(w) = weight {
        edge_cols.push(w);
    }
    for c in &spec.columns {
        edge_cols.push(cat.column(c)?);
    }

    let mut cte_cols = vec!["node".to_string(), "depth".to_string()];
    if weight.is_some() {
        cte_cols.push("weight_sum".to_string());
    }
    cte_cols.push("path".to_string());
    cte_cols.extend((0..edge_cols.len()).map(|i| format!("e{i}")));

    let mut vals: Vec<SqlValue> = Vec::new();
    let anchors = spec
        .start
        .iter()
        .map(|node| {
            vals.push(node.clone());
            vals.push(node.clone());
            let mut a = String::from("SELECT ?, 0");
            if weight.is_some() {
                a.push_str(", 0");
            }
            a.push_str(", char(30) || ? || char(30)");
            for _ in &edge_cols {
                a.push_str(", NULL");
            }
            a
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");

    let mut step = format!("SELECT \"{table}\".\"{dst}\", mc_traverse.depth + 1");
    if let Some(w) = weight {
        step.push_str(&format!(
            ", mc_traverse.weight_sum + COALESCE(\"{table}\".\"{w}\", 0)"
        ));
    }
    step.push_str(&format!(
        ", mc_traverse.path || \"{table}\".\"{dst}\" || char(30)"
    ));
    for c in &edge_cols {
        step.push_str(&format!(", \"{table}\".\"{c}\""));
    }
    step.push_str(&format!(
        " FROM {TRAVERSE_CTE} JOIN \"{table}\" ON \"{table}\".\"{src}\" = {TRAVERSE_CTE}.node \
         WHERE {TRAVERSE_CTE}.depth < ? \
         AND instr({TRAVERSE_CTE}.path, char(30) || \"{table}\".\"{dst}\" || char(30)) = 0"
    ));
    vals.push(SqlValue::Integer(spec.max_depth));
    let (where_clause, where_vals) = render_where_qualified(&spec.filters, cat, true)?;
    if let Some(rest) = where_clause.strip_prefix(" WHERE ") {
        step.push_str(&format!(" AND {rest}"));
        vals.extend(where_vals);
    }
    vals.push(SqlValue::Integer(spec.max_nodes + 1));

    let cols = cte_cols.join(", ");
    Ok((
        format!(
            "WITH RECURSIVE {TRAVERSE_CTE}({cols}) AS ({anchors} UNION ALL {step}) \
             SELECT {cols} FROM {TRAVERSE_CTE} WHERE depth > 0 LIMIT ?"
        ),
        vals,
    ))
}

/// Render a `similar` statement plus its bound values (P4, memory-spec A.2.5).
///
/// Three things are structural rather than optional:
/// 1. the vector column is prefiltered with `IS NOT NULL` — rows queued for
///    embedding backfill (memory-spec B.1.1) would otherwise rank FIRST under
///    `ORDER BY … ASC`, because SQL sorts NULL to the top;
/// 2. the distance is an aliased result column, so `hamming` is evaluated once
///    per surviving row and the ordering can reference the alias;
/// 3. `rowid` is the tiebreaker, so equal distances keep a stable order.
///
/// The op does NOT enforce the caller's embedding-generation discipline
/// (`model_id`) — that filter belongs in `where`; a breach becomes a loud
/// `hamming: length mismatch` instead of a silently wrong ranking.
pub fn render_similar(
    spec: &SimilarSpec,
    cat: &Catalog,
) -> Result<(String, Vec<SqlValue>), CatalogError> {
    let table = cat.table();
    let vec_col = cat.column(&spec.vector_column)?;
    let projection = spec
        .columns
        .iter()
        .map(|c| Ok(format!("\"{table}\".\"{}\"", cat.column(c)?)))
        .collect::<Result<Vec<_>, CatalogError>>()?
        .join(", ");
    let mut vals: Vec<SqlValue> = vec![SqlValue::Text(spec.vector.clone())];
    let (where_clause, where_vals) = render_where_qualified(&spec.filters, cat, true)?;
    vals.extend(where_vals);
    let filter = match where_clause.strip_prefix(" WHERE ") {
        Some(rest) => format!(" AND {rest}"),
        None => String::new(),
    };
    let tail = if spec.order_by.is_empty() {
        let mut t = format!(" ORDER BY \"{DISTANCE_COLUMN}\" ASC, \"{table}\".\"rowid\" ASC");
        if let Some(n) = spec.limit {
            t.push_str(" LIMIT ?");
            vals.push(SqlValue::Integer(n));
        }
        t
    } else {
        render_tail_qualified(&spec.order_by, spec.limit, cat, &mut vals, true)?
    };
    let stmt = format!(
        "SELECT {projection}, hamming(\"{table}\".\"{vec_col}\", ?) AS \"{DISTANCE_COLUMN}\" \
         FROM \"{table}\" WHERE \"{table}\".\"{vec_col}\" IS NOT NULL{filter}{tail}"
    );
    Ok((stmt, vals))
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

    /// The traversal CTE, pinned character for character. Everything in the
    /// string is either catalog-owned (`entity_edges`, `src_entity`, …) or a
    /// fixed literal (`mc_traverse`, `char(30)`, `instr`, `COALESCE`); the start
    /// nodes, the depth guard, the filter values and the node cap are all `?`.
    #[test]
    fn traverse_renders_the_recursive_cte_with_guards_and_cycle_elimination() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute(
            "CREATE TABLE entity_edges (src_entity TEXT, dst_entity TEXT, edge_kind TEXT, \
             weight INTEGER, episode_id TEXT, valid_until TEXT)",
            [],
        )
        .unwrap();
        let v = meclaw_core::serde_json::json!({
            "table":"entity_edges","src":"src_entity","dst":"dst_entity",
            "kind":"edge_kind","weight":"weight","columns":["episode_id"],
            "start":["e:marcus"],"where":{"valid_until":{"is_null":true}},
            "max_depth":3,"max_nodes":10});
        let spec = crate::store::query::parse::parse_traverse(v.as_object().unwrap()).unwrap();
        let (stmt, vals) = render_traverse(&spec, &cat(&c, "entity_edges")).unwrap();
        assert_eq!(
            stmt,
            "WITH RECURSIVE mc_traverse(node, depth, weight_sum, path, e0, e1, e2) AS (\
             SELECT ?, 0, 0, char(30) || ? || char(30), NULL, NULL, NULL \
             UNION ALL \
             SELECT \"entity_edges\".\"dst_entity\", mc_traverse.depth + 1, \
             mc_traverse.weight_sum + COALESCE(\"entity_edges\".\"weight\", 0), \
             mc_traverse.path || \"entity_edges\".\"dst_entity\" || char(30), \
             \"entity_edges\".\"edge_kind\", \"entity_edges\".\"weight\", \
             \"entity_edges\".\"episode_id\" \
             FROM mc_traverse JOIN \"entity_edges\" \
             ON \"entity_edges\".\"src_entity\" = mc_traverse.node \
             WHERE mc_traverse.depth < ? \
             AND instr(mc_traverse.path, char(30) || \"entity_edges\".\"dst_entity\" || char(30)) = 0 \
             AND \"entity_edges\".\"valid_until\" IS NULL) \
             SELECT node, depth, weight_sum, path, e0, e1, e2 FROM mc_traverse WHERE depth > 0 LIMIT ?"
        );
        // two binds per start node, the depth guard, and max_nodes + 1
        assert_eq!(vals.len(), 4);
        assert_eq!(vals[2], SqlValue::Integer(3), "max_depth is bound");
        assert_eq!(
            vals[3],
            SqlValue::Integer(11),
            "one row beyond the cap detects truncation"
        );
    }

    /// Without the optional roles the CTE carries no weight column and no edge
    /// attributes — no invented zero values.
    #[test]
    fn traverse_without_optional_roles_renders_a_minimal_cte() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE e (s TEXT, d TEXT)", []).unwrap();
        let v = meclaw_core::serde_json::json!({
            "table":"e","src":"s","dst":"d","start":["a","b"]});
        let spec = crate::store::query::parse::parse_traverse(v.as_object().unwrap()).unwrap();
        let (stmt, vals) = render_traverse(&spec, &cat(&c, "e")).unwrap();
        assert!(
            stmt.starts_with("WITH RECURSIVE mc_traverse(node, depth, path) AS ("),
            "{stmt}"
        );
        assert!(
            stmt.contains(
                "SELECT ?, 0, char(30) || ? || char(30) UNION ALL \
                               SELECT ?, 0, char(30) || ? || char(30) UNION ALL"
            ),
            "{stmt}"
        );
        assert_eq!(
            vals.len(),
            6,
            "two starts × two binds, depth guard, node cap"
        );
    }

    /// The `similar` statement, pinned character for character: the implicit
    /// `IS NOT NULL` prefilter (R9) comes first, the distance is an aliased
    /// result column, the ordering references that alias, `rowid` is the
    /// deterministic tiebreaker, and every caller value is a `?`.
    #[test]
    fn similar_renders_prefilter_rank_and_tiebreaker() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute(
            "CREATE TABLE e (owner_id TEXT, model_id TEXT, blob TEXT)",
            [],
        )
        .unwrap();
        let v = meclaw_core::serde_json::json!({
            "table":"e","columns":["owner_id"],"vector_column":"blob","vector":"AA==",
            "where":{"model_id":"m1"},"limit":5});
        let spec = crate::store::query::parse::parse_similar(v.as_object().unwrap()).unwrap();
        let (stmt, vals) = render_similar(&spec, &cat(&c, "e")).unwrap();
        assert_eq!(
            stmt,
            "SELECT \"e\".\"owner_id\", hamming(\"e\".\"blob\", ?) AS \"distance\" \
             FROM \"e\" WHERE \"e\".\"blob\" IS NOT NULL AND \"e\".\"model_id\" = ? \
             ORDER BY \"distance\" ASC, \"e\".\"rowid\" ASC LIMIT ?"
        );
        assert_eq!(vals.len(), 3, "vector, filter value, limit — all bound");
    }

    /// An explicit `order_by` wins over the distance ranking (same rule as
    /// `search`), and the distance column is still reported.
    #[test]
    fn similar_lets_an_explicit_order_by_win() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE e (owner_id TEXT, blob TEXT)", [])
            .unwrap();
        let v = meclaw_core::serde_json::json!({
            "table":"e","columns":["owner_id"],"vector_column":"blob","vector":"AA==",
            "order_by":[{"col":"owner_id","dir":"desc"}]});
        let spec = crate::store::query::parse::parse_similar(v.as_object().unwrap()).unwrap();
        let (stmt, _) = render_similar(&spec, &cat(&c, "e")).unwrap();
        assert!(
            stmt.ends_with("ORDER BY \"e\".\"owner_id\" DESC"),
            "explicit order wins: {stmt}"
        );
        assert!(
            stmt.contains("AS \"distance\""),
            "distance is still reported"
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
