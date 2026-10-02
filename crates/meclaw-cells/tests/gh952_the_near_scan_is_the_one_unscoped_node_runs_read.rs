//! R-FH-1 Auflage 2 at the store of a file space: every read of a table with
//! a `file` column names its file, bar the reads the harness exempts by name
//! (`support/file_space_hive.rs`, `Space::unscoped`). GH #944 added one: the
//! `near` scan of `./read`, which compares the file vectors of every head and
//! so reads `node_runs` across files.
//!
//! The exemption was wider than that read: ANY read of `node_runs` without a
//! `file` passed (C1 finding, review of GH #944, M-4) -- an outline read that
//! lost its file would have passed the same check that exists to catch it.
//! It covers the scan alone now: `./read`, a `select` of `file, fvec` where
//! `fvec <> ''`. These locks hold both halves against the harness itself, with
//! store operations put in by hand (no script runs here).

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The breaches `unscoped` finds in one store operation sent by `sender`.
fn breaches(sender: &str, op: Value) -> Vec<String> {
    let mut s = Space::new();
    s.store_ops.push((sender.to_string(), op));
    s.unscoped()
}

/// The `near` scan exactly as the shipped `./read` sends it (`n-scan`).
fn near_scan() -> Value {
    json!({"operation": "select", "table": "node_runs", "columns": ["file", "fvec"],
           "where": {"fvec": {"neq": ""}}, "limit": 5000,
           "order_by": [{"col": "file", "dir": "asc"}]})
}

#[test]
fn the_near_scan_reads_across_files() {
    assert_eq!(breaches("read", near_scan()), Vec::<String>::new());
}

#[test]
fn any_other_node_runs_read_without_its_file_is_a_breach() {
    let mut more_columns = near_scan();
    more_columns["columns"] = json!(["file", "fvec", "nodes"]);
    let mut wider_where = near_scan();
    wider_where["where"] = json!({});
    let mut other_key = near_scan();
    other_key["where"] = json!({"version": "ab12"});
    let cases = [
        (
            "read",
            more_columns,
            "a scan that reads more than the vectors",
        ),
        ("read", wider_where, "a scan over rows without a vector"),
        ("read", other_key, "a read by version, not by file"),
        (
            "read",
            json!({"operation": "select", "table": "node_runs",
                   "columns": ["fmt", "parser", "mark", "nodes", "links"],
                   "where": {"version": "ab12"}, "limit": 1}),
            "the outline's run row, its file lost",
        ),
        (
            "read",
            json!({"operation": "search", "table": "node_runs",
                   "where": {"fvec": {"neq": ""}}, "limit": 10}),
            "a search is not the scan",
        ),
        (
            "derive",
            near_scan(),
            "the scan from a cell that is not `./read`",
        ),
    ];
    for (sender, op, why) in cases {
        assert_eq!(breaches(sender, op).len(), 1, "{why}");
    }
}

#[test]
fn a_node_runs_read_that_names_its_file_is_no_breach() {
    let op = json!({"operation": "select", "table": "node_runs",
                    "columns": ["version", "fvec"], "where": {"file": "fh-aaaaaaaaaaa1"},
                    "limit": 1});
    assert_eq!(breaches("read", op), Vec::<String>::new());
}
