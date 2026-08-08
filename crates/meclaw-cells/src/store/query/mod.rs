//! Query IR for the store cell: one parse path from the JSON op payload to a
//! typed representation ([`parse`]), one render path from that representation to
//! parameterized SQL. Nothing else builds store SQL.
//!
//! The IR carries caller-supplied column names as plain strings on purpose —
//! they are resolved against the live SQLite catalog at render time, and only
//! the catalog's own spelling is ever formatted into a statement.

pub mod catalog;
pub mod parse;
pub mod sql;

use rusqlite::types::Value as SqlValue;

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

/// One `where` entry: the caller's column name plus its predicate.
#[derive(Debug)]
pub struct Filter {
    /// Caller-supplied column name — catalog-resolved before rendering.
    pub col: String,
    /// The predicate to apply to that column.
    pub pred: Predicate,
}
