//! Query IR for the store cell: one parse path from the JSON op payload to a
//! typed representation ([`parse`]), one render path from that representation to
//! parameterized SQL. Nothing else builds store SQL.
//!
//! The IR carries caller-supplied column names as plain strings on purpose —
//! they are resolved against the live SQLite catalog at render time, and only
//! the catalog's own spelling is ever formatted into a statement.

pub mod catalog;
pub mod hamming;
pub mod parse;
pub mod sql;

use rusqlite::types::Value as SqlValue;

/// Name of the computed rank column of the `similar` op (smaller is better,
/// like `rank` in `search`). A closed-set literal, never caller text.
pub const DISTANCE_COLUMN: &str = "distance";

/// Scalar comparison operators of the `where` form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    /// `=` — also the meaning of a bare (non-object) `where` value.
    Eq,
    /// `<>`
    Neq,
    /// `<`
    Lt,
    /// `<=`
    Lte,
    /// `>`
    Gt,
    /// `>=`
    Gte,
}

impl Cmp {
    /// The SQL symbol for this operator — a closed-set literal, never caller text.
    pub fn symbol(self) -> &'static str {
        match self {
            Cmp::Eq => "=",
            Cmp::Neq => "<>",
            Cmp::Lt => "<",
            Cmp::Lte => "<=",
            Cmp::Gt => ">",
            Cmp::Gte => ">=",
        }
    }
}

/// One predicate over a single column.
#[derive(Debug)]
pub enum Predicate {
    /// `col <op> ?` with the value bound.
    Cmp(Cmp, SqlValue),
    /// `col IN (?, ?, …)` — never empty (rejected at parse time).
    In(Vec<SqlValue>),
    /// `col IS NULL` (`true`) or `col IS NOT NULL` (`false`).
    IsNull(bool),
    /// `(<inner> OR col IS NULL)`. The inner predicate is a comparison or an
    /// in-list — never another `OrNull` and never `IsNull` (parse-enforced),
    /// because both would be semantically empty.
    OrNull(Box<Predicate>),
}

/// Sort direction of an `order_by` term — a closed set. The keyword is rendered
/// from this enum, so caller text can never reach the `ORDER BY` clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// `ASC` — the default when `dir` is omitted.
    Asc,
    /// `DESC`
    Desc,
}

impl Dir {
    /// The SQL keyword for this direction.
    pub fn keyword(self) -> &'static str {
        match self {
            Dir::Asc => "ASC",
            Dir::Desc => "DESC",
        }
    }
}

/// One `order_by` term: column plus direction.
#[derive(Debug)]
pub struct OrderTerm {
    /// Caller-supplied column name — catalog-resolved before rendering.
    pub col: String,
    /// Sort direction.
    pub dir: Dir,
}

/// Name of the recursive CTE built by `traverse` — a fixed literal, so an edge
/// table of the same name would shadow it. That case is rejected at parse time.
pub const TRAVERSE_CTE: &str = "mc_traverse";

/// Traversal depth if the caller names none, and the hard ceiling
/// (memory-spec A.2.4: "default 2, cap 5").
pub const MAX_DEPTH_DEFAULT: i64 = 2;
/// Hard ceiling for `max_depth`; a larger value is rejected, not clamped.
pub const MAX_DEPTH_CAP: i64 = 5;
/// Fan-out kill switch: paths returned if the caller names no `max_nodes`.
/// 200 is an order of magnitude above what a recall bundle consumes.
pub const MAX_NODES_DEFAULT: i64 = 200;
/// Hard ceiling for `max_nodes` — roughly 1 MB of path payload.
pub const MAX_NODES_CAP: i64 = 5000;

/// A parsed `traverse` op: multi-hop walk over a declared edge table
/// (memory-spec A.2.4). Column roles are caller text and stay unresolved until
/// render time, where the catalog decides whether they exist.
#[derive(Debug)]
pub struct TraverseSpec {
    /// Column holding the source node of an edge.
    pub src: String,
    /// Column holding the target node of an edge.
    pub dst: String,
    /// Optional edge-kind column, reported per path.
    pub kind: Option<String>,
    /// Optional weight column; when present, paths carry the accumulated sum.
    pub weight: Option<String>,
    /// Start nodes — bound values, never formatted.
    pub start: Vec<SqlValue>,
    /// Additional edge columns to report for the last edge of each path.
    pub columns: Vec<String>,
    /// Optional filter on the edge rows (applied in the recursive step).
    pub filters: Vec<Filter>,
    /// Maximum number of hops.
    pub max_depth: i64,
    /// Maximum number of paths returned; hitting it sets `truncated`.
    pub max_nodes: i64,
}

/// A parsed `similar` op: nearest-neighbour ranking over a binarized vector
/// column (memory-spec A.2.5). Column names are caller text and stay unresolved
/// until render time; the query vector is bound, never formatted.
#[derive(Debug)]
pub struct SimilarSpec {
    /// Projection — mandatory, like every other op (no `SELECT *`).
    pub columns: Vec<String>,
    /// The column holding the binarized vectors.
    pub vector_column: String,
    /// The query vector, base64 (validated at parse time, bound at render time).
    pub vector: String,
    /// Optional pre-filter. Applied before ranking; carries the caller's
    /// `model_id` discipline (memory-spec B.1.1) — the op does not enforce it.
    pub filters: Vec<Filter>,
    /// Optional explicit ordering; empty means "rank by distance".
    pub order_by: Vec<OrderTerm>,
    /// Optional row limit.
    pub limit: Option<i64>,
}

/// One `where` entry: the caller's column name plus its predicate.
#[derive(Debug)]
pub struct Filter {
    /// Caller-supplied column name — catalog-resolved before rendering.
    pub col: String,
    /// The predicate to apply to that column.
    pub pred: Predicate,
}
