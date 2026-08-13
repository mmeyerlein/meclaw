//! Phase-8 LlmCell state-IO + pure helpers.
//!
//! - `flatten_to_leaves` walks a UBF system-subtree into flat (slot_path, leaf)
//!   pairs for KV-upsert into `cell.db.system`.
//! - `upsert_system_leaf` + `read_system_tree` — SQL ops against the
//!   Phase-6.5 `system`-Tabelle.
//! - `replace_last_input` — SQL op against the Phase-6.5 `last_input`
//!   single-row table (forensic-only, never read back — cell-types.md Z.74).
//! - `EXPECTED_SCHEMA_VERSION` + `check_schema_version` — cell.db
//!   schema version check for LlmCellFactory.
//! - `system_first_persist` — atomic transaction wrapper that drives the
//!   writers in Q2 system-first order (consumed by `handle()` steps 2/3).

use meclaw_core::serde_json::{self, Value};

/// Walks a UBF system-subtree and returns flat `(dotted-leaf-path, leaf-json)` pairs.
///
/// A "leaf" is a JSON object containing `text` or `text_id` keys. Recursion stops
/// at the first such key. The `prefix` is empty for the root call; recursive calls
/// extend it with `.`-separated path segments.
///
/// Since GH #86 a `text_id` leaf can no longer ARRIVE: the substrate resolves
/// the whole class into `{"text": …}` at the delivery boundary. The key stays in
/// the stop condition so this walk keeps agreeing with the resolver's leaf
/// definition and with `read_system_tree`, which can still read a row written
/// before that boundary existed — such residue fails the call loudly at read
/// via [`check_text_id_residue`] (GH #95).
///
/// Output order is unspecified (HashMap iteration). Caller should sort if stable
/// order matters (tests do).
pub(crate) fn flatten_to_leaves(tree: &Value, prefix: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    walk(tree, prefix, &mut out);
    out
}

fn walk(node: &Value, path: &str, out: &mut Vec<(String, Value)>) {
    let Some(obj) = node.as_object() else { return };
    if obj.contains_key("text") || obj.contains_key("text_id") {
        out.push((path.to_string(), node.clone()));
        return;
    }
    for (k, v) in obj {
        let next_path = if path.is_empty() {
            k.clone()
        } else {
            format!("{path}.{k}")
        };
        walk(v, &next_path, out);
    }
}

/// GH #95 guard — reject a system tree read back from `cell.db` while it
/// still holds unresolved `{text_id}` leaves (pre-#86 residue).
///
/// Since GH #86 the substrate resolves the `{text_id}` pointer class at the
/// delivery boundary, so no such leaf arrives from a delivery any more. A row
/// written BEFORE that boundary existed never crosses it again, and
/// `concat_system_prompt` (which stops at `text`) would silently drop its
/// content from the system prompt. Loud-at-read (GH #95 ruling): the call
/// fails with a regular cell error naming every offending slot, instead of a
/// silently shortened prompt. No panic, no restart — the row stays in
/// `cell.db` and is overwritten by re-sending the slot with inline text.
///
/// The leaf definition is shared with the resolver via [`flatten_to_leaves`];
/// a leaf carrying `text_id` NEXT TO `text` is residue too (the resolver
/// never produced that shape). Offending slots are reported sorted.
pub(crate) fn check_text_id_residue(tree: &Value) -> Result<(), String> {
    let mut offenders: Vec<String> = flatten_to_leaves(tree, "")
        .into_iter()
        .filter(|(_, leaf)| leaf.as_object().is_some_and(|o| o.contains_key("text_id")))
        .map(|(slot, _)| slot)
        .collect();
    if offenders.is_empty() {
        return Ok(());
    }
    offenders.sort();
    let slots = offenders
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "system slot(s) {slots}: unresolved {{text_id}} leaf in cell.db — pre-#86 residue \
         (GH #95). The substrate resolves this pointer class at the delivery boundary since \
         GH #86; a row persisted before that boundary existed never crosses it again, and \
         its content would silently drop out of the system prompt. Re-send the slot with \
         inline text to overwrite the row"
    ))
}

/// UPSERT a single system-leaf into cell.db.system. Idempotent.
///
/// `slot_path` is the dotted leaf path (e.g. "identity.soul").
/// `leaf_json` is the UBF-leaf object, `{"text":"..."}` since GH #86 resolved
/// the `{"text_id":"..."}` form at the delivery boundary. The full JSON object
/// is serialized into the `value` column; the kind discriminator lives inside
/// the JSON (Plan § 4 Q1-Mapping).
pub(crate) fn upsert_system_leaf(
    conn: &rusqlite::Connection,
    slot_path: &str,
    leaf_json: &Value,
    now: i64,
) -> rusqlite::Result<()> {
    let value = serde_json::to_string(leaf_json).expect("UBF leaf json serialize");
    conn.execute(
        "INSERT INTO system (slot_path, value, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(slot_path) DO UPDATE SET
             value = excluded.value,
             updated_at = excluded.updated_at",
        rusqlite::params![slot_path, value, now],
    )?;
    Ok(())
}

/// Read all system-rows and reconstruct a nested JSON tree.
///
/// Each row's `slot_path` (e.g. "identity.soul") is split on `.` and walked
/// into the result tree. Leaves (the parsed `value` JSON) are placed at the
/// leaf position. Empty table → returns `{}`.
pub(crate) fn read_system_tree(conn: &rusqlite::Connection) -> rusqlite::Result<Value> {
    let mut stmt = conn.prepare("SELECT slot_path, value FROM system")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut tree = serde_json::Map::new();
    for row in rows {
        let (path, raw_value) = row?;
        let leaf: Value = serde_json::from_str(&raw_value)
            .expect("system.value must be parseable JSON (written by upsert_system_leaf)");
        insert_into_tree(&mut tree, &path, leaf);
    }
    Ok(Value::Object(tree))
}

fn insert_into_tree(obj: &mut serde_json::Map<String, Value>, dotted_path: &str, leaf: Value) {
    let mut parts = dotted_path.split('.').peekable();
    let mut cursor = obj;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            cursor.insert(part.to_string(), leaf);
            return;
        }
        let entry = cursor
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        cursor = entry
            .as_object_mut()
            .expect("system tree shape: intermediate is always object");
    }
}

/// Replace the cell.db.last_input row (id=1) with the given messages[]-array.
///
/// The input is the UBF messages-array as JSON Value (not the full body —
/// just the array). Stored as JSON-serialized text per Plan § 4 Q1 mapping.
pub(crate) fn replace_last_input(
    conn: &rusqlite::Connection,
    messages_array: &Value,
    now: i64,
) -> rusqlite::Result<()> {
    let json = serde_json::to_string(messages_array).expect("messages array json serialize");
    conn.execute(
        "INSERT OR REPLACE INTO last_input (id, message_json, received_at) VALUES (1, ?, ?)",
        rusqlite::params![json, now],
    )?;
    Ok(())
}

/// Atomic persist of system-leaves + optional messages[] in ONE transaction.
///
/// Q2 system-first order: all leaves UPSERTed first, then (if Some) messages
/// replaced. Single tx → no partial state if cancelled mid-write (Backstop B
/// from § 9 handle()-Reihenfolge).
///
/// Empty `system_leaves` is allowed (e.g. message-only input). `None`
/// `messages_array` skips the last_input write entirely (system-only input).
pub(crate) fn system_first_persist(
    conn: &mut rusqlite::Connection,
    system_leaves: &[(String, Value)],
    messages_array: Option<&Value>,
    now: i64,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for (slot_path, leaf) in system_leaves {
        upsert_system_leaf(&tx, slot_path, leaf, now)?;
    }
    if let Some(msgs) = messages_array {
        replace_last_input(&tx, msgs, now)?;
    }
    tx.commit()
}

/// LlmCell's expected cell.db schema version. Phase-6.5 set schema_version=1
/// as the shared substrate version (`meclaw_colony::persist::schema`).
/// If a future Phase bumps schema_version, LlmCell's check_schema_version
/// here must update — and the migration is the CellFactory's job per
/// Phase-6.5-convention.
pub(crate) const EXPECTED_SCHEMA_VERSION: u32 = 1;

/// Verify cell.db schema_version matches what LlmCell expects.
///
/// Reads the `meta` table's schema_version (set by `setup_cell_db` to "1" in
/// Phase 6.5). Returns `Ok(())` on match, `Err(msg)` on mismatch. Used by
/// LlmCellFactory::spawn_cell to reject incompatible cell.dbs before
/// cell_task_stateful spawns.
pub(crate) fn check_schema_version(conn: &rusqlite::Connection) -> Result<(), String> {
    let actual = meclaw_colony::persist::read_schema_version(conn)
        .map_err(|e| format!("cell.db schema_version read failed: {e}"))?;
    if actual != EXPECTED_SCHEMA_VERSION {
        return Err(format!(
            "cell.db schema_version mismatch: expected {}, found {}",
            EXPECTED_SCHEMA_VERSION, actual
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        check_schema_version, check_text_id_residue, flatten_to_leaves, read_system_tree,
        replace_last_input, system_first_persist, upsert_system_leaf,
    };
    use meclaw_colony::persist::open_or_create_cell_db;
    use meclaw_core::serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn flatten_single_text_leaf() {
        let tree = json!({"identity": {"soul": {"text": "S"}}});
        let out = flatten_to_leaves(&tree, "");
        assert_eq!(
            out,
            vec![("identity.soul".to_string(), json!({"text":"S"}))]
        );
    }

    /// The `text_id` stop condition is retained for pre-GH-#86 rows: no such
    /// leaf can arrive from a delivery any more, but a `cell.db` written before
    /// the boundary resolved that class may still hold one, and the walk must
    /// keep treating it as a leaf rather than descending into it. Such a
    /// residual row fails the call loudly at read (`check_text_id_residue`,
    /// GH #95 ruling) instead of silently dropping out of the prompt.
    #[test]
    fn flatten_two_leaves_under_identity() {
        let tree = json!({"identity": {"soul": {"text":"A"}, "body": {"text_id":"01H"}}});
        let mut out = flatten_to_leaves(&tree, "");
        out.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            out,
            vec![
                ("identity.body".to_string(), json!({"text_id":"01H"})),
                ("identity.soul".to_string(), json!({"text":"A"})),
            ]
        );
    }

    #[test]
    fn flatten_includes_tools_subtree() {
        let tree = json!({"tools": {"calc": {"text":"{\"name\":\"calc\"}"}}});
        let out = flatten_to_leaves(&tree, "");
        assert_eq!(
            out,
            vec![(
                "tools.calc".to_string(),
                json!({"text":"{\"name\":\"calc\"}"})
            )]
        );
    }

    // ───── GH #95: pre-#86 {text_id} residue is loud at read ─────

    #[test]
    fn residue_check_passes_a_pure_text_tree() {
        let tree = json!({"identity": {"soul": {"text": "S"}, "body": {"text": "B"}}});
        check_text_id_residue(&tree).unwrap();
    }

    #[test]
    fn residue_check_flags_a_text_id_only_leaf_with_its_slot_path() {
        let tree = json!({"identity": {"soul": {"text": "S"}, "body": {"text_id": "01H"}}});
        let err = check_text_id_residue(&tree).unwrap_err();
        assert!(err.contains("'identity.body'"), "must name the slot: {err}");
        assert!(
            err.contains("pre-#86 residue"),
            "must name the origin: {err}"
        );
        assert!(err.contains("GH #95"), "must name the issue: {err}");
    }

    /// A leaf carrying `text_id` NEXT TO `text` is residue too — the resolver
    /// never produced that shape, and silently preferring the `text` half
    /// would hide the unresolved rest.
    #[test]
    fn residue_check_flags_a_mixed_leaf_carrying_a_text_id_rest() {
        let tree = json!({"identity": {"body": {"text": "a", "text_id": "01H"}}});
        let err = check_text_id_residue(&tree).unwrap_err();
        assert!(err.contains("'identity.body'"), "must name the slot: {err}");
    }

    /// The tools sub-slot is NOT exempt (mirrors the resolver, GH #86): a
    /// residual pointer under `tools` must get the same story, not the
    /// misleading `extract_tools` "leaf has no text field".
    #[test]
    fn residue_check_scans_the_tools_subtree_too() {
        let tree = json!({"tools": {"calc": {"text_id": "01H"}}});
        let err = check_text_id_residue(&tree).unwrap_err();
        assert!(err.contains("'tools.calc'"), "must name the slot: {err}");
    }

    #[test]
    fn residue_check_names_every_offending_slot_sorted() {
        let tree = json!({
            "b": {"text_id": "x"},
            "a": {"text_id": "y"},
            "c": {"text": "ok"},
        });
        let err = check_text_id_residue(&tree).unwrap_err();
        let pos_a = err.find("'a'").expect("slot a named");
        let pos_b = err.find("'b'").expect("slot b named");
        assert!(pos_a < pos_b, "slots must be sorted: {err}");
        assert!(!err.contains("'c'"), "clean slot must not be named: {err}");
    }

    #[test]
    fn upsert_system_leaf_inserts_row() {
        let td = TempDir::new().unwrap();
        let cell_db = td.path().join("cell.db");
        let conn = open_or_create_cell_db(&cell_db).unwrap();
        upsert_system_leaf(&conn, "identity.soul", &json!({"text":"S"}), 100).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM system WHERE slot_path='identity.soul'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, r#"{"text":"S"}"#);
    }

    #[test]
    fn upsert_system_leaf_overwrites_existing_row() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        upsert_system_leaf(&conn, "x", &json!({"text":"A"}), 1).unwrap();
        upsert_system_leaf(&conn, "x", &json!({"text":"B"}), 2).unwrap();
        let v: String = conn
            .query_row("SELECT value FROM system WHERE slot_path='x'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, r#"{"text":"B"}"#);
    }

    #[test]
    fn read_system_tree_reconstructs_nested_json() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        upsert_system_leaf(&conn, "identity.soul", &json!({"text":"S"}), 1).unwrap();
        upsert_system_leaf(&conn, "facts.user", &json!({"text":"U"}), 2).unwrap();
        let tree = read_system_tree(&conn).unwrap();
        assert_eq!(
            tree,
            json!({
                "identity": {"soul": {"text":"S"}},
                "facts":    {"user": {"text":"U"}}
            })
        );
    }

    #[test]
    fn read_system_tree_empty_returns_empty_object() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let tree = read_system_tree(&conn).unwrap();
        assert_eq!(tree, json!({}));
    }

    #[test]
    fn read_system_tree_handles_deep_nesting() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        upsert_system_leaf(
            &conn,
            "identity.soul.detail.color",
            &json!({"text":"blue"}),
            1,
        )
        .unwrap();
        let tree = read_system_tree(&conn).unwrap();
        assert_eq!(
            tree,
            json!({
                "identity": {"soul": {"detail": {"color": {"text":"blue"}}}}
            })
        );
    }

    #[test]
    fn replace_last_input_inserts_row() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let msgs = json!([{"origin":"user", "type":"text", "text":"Hi"}]);
        replace_last_input(&conn, &msgs, 100).unwrap();
        let v: String = conn
            .query_row("SELECT message_json FROM last_input WHERE id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let parsed: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(&v).unwrap();
        assert_eq!(parsed, msgs);
    }

    #[test]
    fn replace_last_input_overwrites_existing() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        replace_last_input(
            &conn,
            &json!([{"origin":"user","type":"text","text":"A"}]),
            1,
        )
        .unwrap();
        replace_last_input(
            &conn,
            &json!([{"origin":"user","type":"text","text":"B"}]),
            2,
        )
        .unwrap();
        let stored: String = conn
            .query_row("SELECT message_json FROM last_input WHERE id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let parsed: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed, json!([{"origin":"user","type":"text","text":"B"}]));
    }

    #[test]
    fn check_schema_version_matches_for_fresh_db() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        assert!(check_schema_version(&conn).is_ok());
    }

    #[test]
    fn system_first_persist_writes_both_atomically() {
        let td = TempDir::new().unwrap();
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let leaves = vec![("identity.soul".to_string(), json!({"text":"S"}))];
        let msgs = json!([{"origin":"user","type":"text","text":"Hi"}]);
        system_first_persist(&mut conn, &leaves, Some(&msgs), 100).unwrap();
        let sys_value: String = conn
            .query_row(
                "SELECT value FROM system WHERE slot_path='identity.soul'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sys_value, r#"{"text":"S"}"#);
        let li_value: String = conn
            .query_row("SELECT message_json FROM last_input WHERE id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let parsed: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(&li_value).unwrap();
        assert_eq!(parsed, msgs);
    }

    #[test]
    fn system_first_persist_without_messages_does_not_touch_last_input() {
        let td = TempDir::new().unwrap();
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        // Pre-write a last_input row so we can verify it stays.
        replace_last_input(
            &conn,
            &json!([{"origin":"user","type":"text","text":"OLD"}]),
            1,
        )
        .unwrap();
        let leaves = vec![("x".to_string(), json!({"text":"new-system-leaf"}))];
        system_first_persist(&mut conn, &leaves, None, 200).unwrap();
        let sys: String = conn
            .query_row("SELECT value FROM system WHERE slot_path='x'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(sys, r#"{"text":"new-system-leaf"}"#);
        let li: String = conn
            .query_row("SELECT message_json FROM last_input WHERE id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let parsed: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(&li).unwrap();
        assert_eq!(parsed[0]["text"], "OLD");
    }

    #[test]
    fn system_first_persist_with_empty_leaves_and_messages_writes_only_messages() {
        let td = TempDir::new().unwrap();
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let msgs = json!([{"origin":"user","type":"text","text":"M"}]);
        system_first_persist(&mut conn, &[], Some(&msgs), 100).unwrap();
        let li: String = conn
            .query_row("SELECT message_json FROM last_input WHERE id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let parsed: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(&li).unwrap();
        assert_eq!(parsed, msgs);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn check_schema_version_rejects_mismatch() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        // Manually corrupt the schema_version to simulate a mismatch.
        conn.execute("UPDATE meta SET value='2' WHERE key='schema_version'", [])
            .unwrap();
        let r = check_schema_version(&conn);
        assert!(r.is_err());
        let err = r.unwrap_err();
        assert!(
            err.contains("expected 1, found 2"),
            "error must mention both versions: {err}"
        );
    }
}
