//! Seed-Loader for the `llm` cell (GH #99). Reads `<cell_dir>/seed/system.jsonl`
//! and upserts its leaves into `cell.db.system`. Called by `LlmCellFactory` at
//! spawn ONLY when the `cell.db` was freshly created (`OpenStatus::Created`) —
//! the overview's no-re-seed-on-resume rule (§ Seed-Konzept).
//!
//! Format (same shape as the store seed, overview § Seed-Konzept):
//!   Line 1: `{"schema": {"slot_path": "text", "value": "json"}}`  (cross-check)
//!   Lines 2+: `{"slot_path": "identity.soul", "value": {"text": "…"}}`
//!
//! `updated_at` is NOT carried by the file — the loader stamps it at seed time,
//! exactly like the message path stamps a `system.*` update.
//!
//! Plain-text leaves only: a seeded `{"text_id": …}` leaf is a loud
//! configuration error. That pointer class is resolved at the delivery boundary
//! (GH #86); a leaf written straight into `cell.db` never passes that boundary
//! and would reach the provider unresolved.

use crate::llm::state::upsert_system_leaf;
use meclaw_core::serde_json::Value;

/// Filesystem location of the `llm` cell's system-seed file.
fn seed_path(cell_dir: &std::path::Path) -> std::path::PathBuf {
    cell_dir.join("seed").join("system.jsonl")
}

/// Seed time for the `updated_at` column, panic-free (the callers run inside the
/// colony task). A clock before the epoch yields `0` — the column is bookkeeping
/// only, and the first real `system.*` update overwrites it anyway.
fn seed_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Parse the seed file into its `(slot_path, leaf)` pairs.
///
/// Pure parse — no database, no side effects. Single parse path shared by the
/// static check ([`check_system_seed`], run in the `--validate` / bootstrap-plan
/// phase) and the loader ([`load_system_seed_if_present`]): a seed file that
/// survives validation always parses at spawn (the store's
/// validate-equals-spawn invariant, issue #56).
fn parse_system_seed(path: &std::path::Path) -> Result<Vec<(String, Value)>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("seed read {}: {e}", path.display()))?;
    let mut lines = text.lines();
    let header_line = lines
        .next()
        .ok_or_else(|| format!("seed {}: empty file", path.display()))?;
    let header: Value = meclaw_core::serde_json::from_str(header_line).map_err(|e| {
        format!(
            "seed {} line 1: expected the schema header {{\"schema\":{{…}}}}, got invalid JSON: {e}",
            path.display()
        )
    })?;
    let header_schema = header
        .get("schema")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            format!(
                "seed {} line 1: missing schema object — line 1 must be the header \
                 {{\"schema\":{{…}}}}, not a data row",
                path.display()
            )
        })?;
    for col in ["slot_path", "value"] {
        if !header_schema.contains_key(col) {
            return Err(format!(
                "seed {}: schema mismatch — column {col} missing in seed header",
                path.display()
            ));
        }
    }
    let mut rows = Vec::new();
    for (idx, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let lineno = idx + 2;
        let row: Value = meclaw_core::serde_json::from_str(line)
            .map_err(|e| format!("seed {} line {lineno}: {e}", path.display()))?;
        let row_obj = row.as_object().ok_or_else(|| {
            format!(
                "seed {} line {lineno}: row must be JSON object",
                path.display()
            )
        })?;
        let slot_path = row_obj
            .get("slot_path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                format!(
                    "seed {} line {lineno}: slot_path must be a non-empty string",
                    path.display()
                )
            })?;
        let value = row_obj
            .get("value")
            .and_then(|v| v.as_object())
            .ok_or_else(|| {
                format!(
                    "seed {} line {lineno}: value must be a UBF leaf object {{\"text\": …}}",
                    path.display()
                )
            })?;
        if value.contains_key("text_id") {
            return Err(format!(
                "seed {} line {lineno}: a {{text_id}} leaf is not seedable — the substrate \
                 resolves that pointer class at the delivery boundary (GH #86), and a leaf \
                 written straight into cell.db never passes it. Seed the resolved text instead",
                path.display()
            ));
        }
        if !value.get("text").is_some_and(|t| t.is_string()) {
            return Err(format!(
                "seed {} line {lineno}: value must carry a text string (plain-text leaves only)",
                path.display()
            ));
        }
        rows.push((slot_path.to_string(), row_obj["value"].clone()));
    }
    Ok(rows)
}

/// Static, database-free parse check of the `llm` cell's seed file.
///
/// A missing seed file stays legal (the pre-#99 normal case). Everything the
/// loader would reject is rejected here too — the two share
/// [`parse_system_seed`], so `--validate` and the spawn path agree.
pub(crate) fn check_system_seed(cell_dir: &std::path::Path) -> Result<(), String> {
    let path = seed_path(cell_dir);
    if !path.exists() {
        return Ok(());
    }
    parse_system_seed(&path).map(|_| ())
}

/// Load `seed/system.jsonl` into `cell.db.system`. A missing file is OK (silent
/// skip). Full parse first, then the upserts in ONE transaction — a parse error
/// never leaves a half-seeded `system` table behind.
///
/// Called by `LlmCellFactory::spawn_cell` ONLY when the `cell.db` was created
/// fresh (`OpenStatus::Created`); a resume never re-seeds (overview
/// § Seed-Konzept — otherwise the accumulated identity would be reset to the
/// template default at every restart).
pub(crate) fn load_system_seed_if_present(
    conn: &mut rusqlite::Connection,
    cell_dir: &std::path::Path,
) -> Result<(), String> {
    let path = seed_path(cell_dir);
    if !path.exists() {
        return Ok(());
    }
    let rows = parse_system_seed(&path)?;
    let now = seed_now_secs();
    let tx = conn
        .transaction()
        .map_err(|e| format!("seed {}: begin transaction failed: {e}", path.display()))?;
    for (slot_path, leaf) in &rows {
        upsert_system_leaf(&tx, slot_path, leaf, now).map_err(|e| {
            format!(
                "seed {} slot {slot_path}: upsert failed: {e}",
                path.display()
            )
        })?;
    }
    tx.commit()
        .map_err(|e| format!("seed {}: commit failed: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::state::read_system_tree;
    use meclaw_colony::persist::open_or_create_cell_db;
    use meclaw_core::serde_json::json;

    fn td_with_seed(body: &str) -> tempfile::TempDir {
        let td = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(td.path().join("seed")).unwrap();
        std::fs::write(td.path().join("seed/system.jsonl"), body).unwrap();
        td
    }

    #[test]
    fn loads_two_leaves_into_the_system_table() {
        let td = td_with_seed(
            r#"{"schema":{"slot_path":"text","value":"json"}}
{"slot_path":"identity","value":{"text":"I am Egon."}}
{"slot_path":"instructions.tone","value":{"text":"Be brief."}}
"#,
        );
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        load_system_seed_if_present(&mut conn, td.path()).unwrap();
        let tree = read_system_tree(&conn).unwrap();
        assert_eq!(
            tree,
            json!({
                "identity": {"text": "I am Egon."},
                "instructions": {"tone": {"text": "Be brief."}}
            })
        );
    }

    #[test]
    fn missing_seed_file_is_ok() {
        let td = tempfile::TempDir::new().unwrap();
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        load_system_seed_if_present(&mut conn, td.path()).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        check_system_seed(td.path()).unwrap();
    }

    /// A `{text_id}` leaf would bypass the GH #86 delivery-boundary resolver —
    /// loud reject, on BOTH the static check and the loader.
    #[test]
    fn rejects_a_text_id_leaf() {
        let td = td_with_seed(
            r#"{"schema":{"slot_path":"text","value":"json"}}
{"slot_path":"identity","value":{"text_id":"01H"}}
"#,
        );
        let err = check_system_seed(td.path()).expect_err("a {text_id} leaf is not seedable");
        assert!(err.contains("text_id"), "error must name the class: {err}");
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        assert!(load_system_seed_if_present(&mut conn, td.path()).is_err());
    }

    /// The reject is all-or-nothing: the good leaf on line 2 must not survive
    /// the bad leaf on line 3.
    #[test]
    fn a_rejected_row_leaves_no_partial_seed() {
        let td = td_with_seed(
            r#"{"schema":{"slot_path":"text","value":"json"}}
{"slot_path":"identity","value":{"text":"good"}}
{"slot_path":"persona","value":{"text_id":"01H"}}
"#,
        );
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        assert!(load_system_seed_if_present(&mut conn, td.path()).is_err());
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "a rejected seed writes nothing at all");
    }

    #[test]
    fn rejects_missing_header() {
        let td = td_with_seed(
            r#"{"slot_path":"identity","value":{"text":"x"}}
"#,
        );
        let err = check_system_seed(td.path()).expect_err("line 1 must be the schema header");
        assert!(err.contains("schema"), "error must name the header: {err}");
    }

    #[test]
    fn rejects_header_without_value_column() {
        let td = td_with_seed(
            r#"{"schema":{"slot_path":"text"}}
{"slot_path":"identity","value":{"text":"x"}}
"#,
        );
        let err = check_system_seed(td.path()).expect_err("header must cover both columns");
        assert!(err.contains("value"), "error must name the column: {err}");
    }

    #[test]
    fn rejects_empty_slot_path() {
        let td = td_with_seed(
            r#"{"schema":{"slot_path":"text","value":"json"}}
{"slot_path":"","value":{"text":"x"}}
"#,
        );
        assert!(check_system_seed(td.path()).is_err());
    }

    /// A subtree is not a leaf: one row is one `upsert_system_leaf`, so the
    /// author gets told to write `slot_path: "identity.soul"` instead of nesting.
    #[test]
    fn rejects_a_nested_subtree_as_value() {
        let td = td_with_seed(
            r#"{"schema":{"slot_path":"text","value":"json"}}
{"slot_path":"identity","value":{"soul":{"text":"x"}}}
"#,
        );
        let err = check_system_seed(td.path()).expect_err("a subtree is not a leaf");
        assert!(
            err.contains("text"),
            "error must name the leaf shape: {err}"
        );
    }

    #[test]
    fn rejects_unparseable_data_row() {
        let td = td_with_seed(
            r#"{"schema":{"slot_path":"text","value":"json"}}
{"slot_path":"identity","value":
"#,
        );
        assert!(check_system_seed(td.path()).is_err());
    }

    #[test]
    fn rejects_empty_file() {
        let td = td_with_seed("");
        assert!(check_system_seed(td.path()).is_err());
    }

    /// Blank lines between rows are skipped, as in the store seed.
    #[test]
    fn skips_blank_lines() {
        let td = td_with_seed(
            r#"{"schema":{"slot_path":"text","value":"json"}}

{"slot_path":"identity","value":{"text":"I am Egon."}}
"#,
        );
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        load_system_seed_if_present(&mut conn, td.path()).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
