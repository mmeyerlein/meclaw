//! Phase-8 LlmCell state-IO + pure helpers.
//!
//! - `flatten_to_leaves` walks a UBF system-subtree into flat (slot_path, leaf)
//!   pairs for KV-upsert into `cell.db.system`.
//! - `parse_system_write` is the same walk on the INCOMING side, and it also
//!   reads the GH #264 `$replace` marker: the roots whose subtree a message
//!   revokes before its own leaves land.
//! - `upsert_system_leaf` + `delete_system_subtree` + `read_system_tree` — SQL
//!   ops against the Phase-6.5 `system`-Tabelle.
//! - `replace_last_input` — SQL op against the Phase-6.5 `last_input`
//!   single-row table (forensic-only, never read back — cell-types.md Z.74).
//! - `EXPECTED_SCHEMA_VERSION` + `check_schema_version` — cell.db
//!   schema version check for LlmCellFactory.
//! - `system_first_persist` — atomic transaction wrapper that drives the
//!   writers in Q2 system-first order (consumed by `handle()` steps 2/3).

use crate::llm::system_gate::{GateReject, SystemGate};
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

/// The reserved marker key inside an INCOMING `system` subtree (GH #264).
///
/// `"$replace": true` in a node means: below this node, exactly what this
/// message carries holds. See [`parse_system_write`].
pub(crate) const REPLACE_MARKER: &str = "$replace";

/// What one message wants to do to the persistent `system` tree.
///
/// `leaves` are the flat `(slot_path, leaf)` pairs to UPSERT — the same shape
/// [`flatten_to_leaves`] produces, with every reserved `$`-key stripped out of
/// the leaf. `replace_roots` are the dotted paths whose subtree is to be
/// dropped FIRST, so that what stands below them afterwards is exactly what
/// `leaves` puts there. An empty string is a legal root and names the whole
/// tree.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SystemWrite {
    /// Slot paths to upsert, with their leaf objects.
    pub(crate) leaves: Vec<(String, Value)>,
    /// Dotted roots whose subtree is revoked before the upserts run.
    pub(crate) replace_roots: Vec<String>,
}

/// Read one message's `system` subtree into the write it describes (GH #264).
///
/// Same walk as [`flatten_to_leaves`], plus the replace marker. Three rules,
/// all of them loud rather than lenient — a marker that is silently ignored
/// hands the writer a revocation that never happened:
///
/// * `"$replace": true` in a node makes that node's dotted path a replace root.
///   `false` is an explicit no-op (so a writer may compute the flag), any other
///   value is a shape error.
/// * Every OTHER key starting with `$` is a shape error. The prefix is reserved
///   so a misspelled marker cannot pass as an ordinary slot name.
/// * The marker never becomes part of a leaf: it is stripped before the leaf is
///   stored, or the row would carry it back out of `cell.db` on every read.
///
/// The root call passes an empty prefix, so a marker at the top of `system`
/// yields the empty root — "the whole tree is exactly this".
pub(crate) fn parse_system_write(tree: &Value) -> Result<SystemWrite, String> {
    let mut w = SystemWrite::default();
    walk_write(tree, "", &mut w)?;
    Ok(w)
}

fn walk_write(node: &Value, path: &str, out: &mut SystemWrite) -> Result<(), String> {
    let Some(obj) = node.as_object() else {
        return Ok(());
    };
    for key in obj.keys().filter(|k| k.starts_with('$')) {
        if key != REPLACE_MARKER {
            return Err(format!(
                "system slot '{}': reserved key '{key}' (GH #264). Keys starting with '$' \
                 inside a system subtree are reserved by the substrate; the only one defined \
                 is '{REPLACE_MARKER}'. Nothing was written",
                display_root(path)
            ));
        }
    }
    if let Some(marker) = obj.get(REPLACE_MARKER) {
        match marker.as_bool() {
            Some(true) => out.replace_roots.push(path.to_string()),
            Some(false) => {}
            None => {
                return Err(format!(
                    "system slot '{}': '{REPLACE_MARKER}' must be a boolean (GH #264). It \
                     says whether the paths below this node that are absent from this message \
                     are to be dropped; a non-boolean has no such reading. Nothing was written",
                    display_root(path)
                ));
            }
        }
    }
    if obj.contains_key("text") || obj.contains_key("text_id") {
        let mut leaf = obj.clone();
        leaf.retain(|k, _| !k.starts_with('$'));
        out.leaves.push((path.to_string(), Value::Object(leaf)));
        return Ok(());
    }
    for (k, v) in obj.iter().filter(|(k, _)| !k.starts_with('$')) {
        let next_path = if path.is_empty() {
            k.clone()
        } else {
            format!("{path}.{k}")
        };
        walk_write(v, &next_path, out)?;
    }
    Ok(())
}

/// How an empty root reads in a message: the `system` tree itself.
pub(crate) fn display_root(root: &str) -> &str {
    if root.is_empty() { "<system>" } else { root }
}

/// DELETE every row at `root` and below it (GH #264).
///
/// An empty `root` clears the whole table. Otherwise the row AT the root goes
/// too — a node that used to be a leaf and is now a container is the same
/// subtree, and "below this root, exactly this holds" covers it.
///
/// The descendant test compares a fixed-length prefix against `root` plus its
/// separator rather than using `LIKE`: the slot path is caller data, and `%`
/// or `_` inside it would silently widen a `LIKE` pattern. The length is
/// counted in CHARACTERS, because that is what SQLite's `substr` counts. It also keeps the
/// match on a SEGMENT boundary, the same rule `system_writable` follows —
/// a replace at `memory.recall` never reaches `memory.recallx`.
pub(crate) fn delete_system_subtree(
    conn: &rusqlite::Connection,
    root: &str,
) -> rusqlite::Result<usize> {
    if root.is_empty() {
        return conn.execute("DELETE FROM system", []);
    }
    let under = format!("{root}.");
    conn.execute(
        "DELETE FROM system WHERE slot_path = ?1 OR substr(slot_path, 1, ?2) = ?3",
        rusqlite::params![root, under.chars().count() as i64, under],
    )
}

/// Does `slot` lie at or below `root`? Segment-boundary rule, empty root = all.
fn is_under(slot: &str, root: &str) -> bool {
    root.is_empty() || slot == root || slot.starts_with(&format!("{root}."))
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

/// How a `system_first_persist` call failed: the database said no, or the
/// GH #118 write gate did.
///
/// Two variants because the caller answers them differently — a SQL failure is
/// a `provider_error` (the substrate under the cell broke), a gate reject is an
/// `invalid_input` (the message asked for something it may not have).
#[derive(Debug)]
pub(crate) enum PersistError {
    /// `cell.db` refused the write.
    Sql(rusqlite::Error),
    /// The GH #118 write gate refused the write. Nothing was committed.
    Gate(GateReject),
}

impl From<rusqlite::Error> for PersistError {
    fn from(e: rusqlite::Error) -> Self {
        PersistError::Sql(e)
    }
}

/// Atomic persist of system-leaves + optional messages[] in ONE transaction.
///
/// Q2 system-first order: every `replace_roots` subtree DELETEd, then all
/// leaves UPSERTed, then (if Some) messages replaced. Single tx → no partial
/// state if cancelled mid-write (Backstop B from § 9 handle()-Reihenfolge),
/// and no window in which a revoked subtree is gone but its replacement is not
/// yet there (GH #264).
///
/// Empty `system_leaves` is allowed (e.g. message-only input). `None`
/// `messages_array` skips the last_input write entirely (system-only input).
///
/// GH #118: the **slot budget** is checked INSIDE the transaction, because it
/// is the only half of the gate that needs to know what the tree already holds.
/// Counting the rows outside the transaction would be a check against a state
/// the write no longer sees. A reject drops the transaction unopened-ended —
/// neither the system leaves NOR the `messages[]` half survive it, so a refused
/// system write can never leave the cell with a half-applied body. The
/// slot-path and per-leaf-size halves of the gate run BEFORE this call (pure,
/// no database — see `SystemGate::check_leaves`).
pub(crate) fn system_first_persist(
    conn: &mut rusqlite::Connection,
    gate: &SystemGate,
    system_leaves: &[(String, Value)],
    replace_roots: &[String],
    messages_array: Option<&Value>,
    now: i64,
) -> Result<(), PersistError> {
    let tx = conn.transaction()?;
    if !system_leaves.is_empty() || !replace_roots.is_empty() {
        // GH #264: the budget is a statement about the tree the write LEAVES
        // behind, so the rows a replace root is about to drop are already gone
        // when the novel slots are counted. Without that, a bundle that trades
        // ten keys for ten others could be refused at a limit it never crosses.
        let surviving = existing_slot_paths(&tx)?
            .into_iter()
            .filter(|p| !replace_roots.iter().any(|r| is_under(p, r)))
            .collect::<std::collections::HashSet<_>>();
        let novel = system_leaves
            .iter()
            .map(|(p, _)| p.as_str())
            .filter(|p| !surviving.contains(*p))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        gate.check_slot_budget(surviving.len(), novel)
            .map_err(PersistError::Gate)?;
    }
    for root in replace_roots {
        delete_system_subtree(&tx, root)?;
    }
    for (slot_path, leaf) in system_leaves {
        upsert_system_leaf(&tx, slot_path, leaf, now)?;
    }
    if let Some(msgs) = messages_array {
        replace_last_input(&tx, msgs, now)?;
    }
    tx.commit()?;
    Ok(())
}

/// The slot paths currently in `cell.db.system` (GH #118 slot budget).
fn existing_slot_paths(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare("SELECT slot_path FROM system")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.collect()
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
        PersistError, check_schema_version, check_text_id_residue, delete_system_subtree,
        flatten_to_leaves, parse_system_write, read_system_tree, replace_last_input,
        system_first_persist, upsert_system_leaf,
    };
    use crate::llm::system_gate::SystemGate;
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
        system_first_persist(
            &mut conn,
            &SystemGate::default(),
            &leaves,
            &[],
            Some(&msgs),
            100,
        )
        .unwrap();
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
        system_first_persist(&mut conn, &SystemGate::default(), &leaves, &[], None, 200).unwrap();
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
        system_first_persist(
            &mut conn,
            &SystemGate::default(),
            &[],
            &[],
            Some(&msgs),
            100,
        )
        .unwrap();
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

    // ───── GH #118: the slot budget, checked inside the transaction ─────

    /// A write that would push the tree past `system_max_slots` is refused, and
    /// the WHOLE transaction rolls back — the `messages[]` half of the same body
    /// must not survive a refused system write.
    #[test]
    fn a_write_over_the_slot_budget_rolls_back_everything() {
        let td = TempDir::new().unwrap();
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let gate = SystemGate::for_test(2, 65_536, &[]);
        system_first_persist(
            &mut conn,
            &gate,
            &[
                ("a".to_string(), json!({"text":"1"})),
                ("b".to_string(), json!({"text":"2"})),
            ],
            &[],
            None,
            1,
        )
        .unwrap();

        let err = system_first_persist(
            &mut conn,
            &gate,
            &[("c".to_string(), json!({"text":"3"}))],
            &[],
            Some(&json!([{"origin":"user","type":"text","text":"Hi"}])),
            2,
        )
        .unwrap_err();
        assert!(
            matches!(err, PersistError::Gate(_)),
            "must be a gate reject, got {err:?}"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "the refused slot must not have landed");
        let last_input: i64 = conn
            .query_row("SELECT COUNT(*) FROM last_input", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            last_input, 0,
            "the messages half must roll back with the refused system write"
        );
    }

    /// Overwriting slots that already exist never grows the tree — a cell parked
    /// at its budget can still refresh every slot it owns.
    #[test]
    fn refreshing_existing_slots_at_the_budget_still_commits() {
        let td = TempDir::new().unwrap();
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let gate = SystemGate::for_test(2, 65_536, &[]);
        let leaves = vec![
            ("a".to_string(), json!({"text":"1"})),
            ("b".to_string(), json!({"text":"2"})),
        ];
        system_first_persist(&mut conn, &gate, &leaves, &[], None, 1).unwrap();
        let refreshed = vec![("a".to_string(), json!({"text":"1b"}))];
        system_first_persist(&mut conn, &gate, &refreshed, &[], None, 2).unwrap();
        let v: String = conn
            .query_row("SELECT value FROM system WHERE slot_path='a'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, r#"{"text":"1b"}"#);
    }

    /// A message-only body never touches the system table, so it must not pay
    /// the budget query — and must not be refused by a full tree either.
    #[test]
    fn a_message_only_write_is_never_slot_budget_refused() {
        let td = TempDir::new().unwrap();
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let gate = SystemGate::for_test(1, 65_536, &[]);
        system_first_persist(
            &mut conn,
            &gate,
            &[("a".to_string(), json!({"text":"1"}))],
            &[],
            None,
            1,
        )
        .unwrap();
        let msgs = json!([{"origin":"user","type":"text","text":"Hi"}]);
        system_first_persist(&mut conn, &gate, &[], &[], Some(&msgs), 2).unwrap();
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

    // ───── GH #264: the replace marker, and what it deletes ─────

    /// The marker names its own node as the root, and never travels into the
    /// leaf it sits next to.
    #[test]
    fn the_marker_names_its_own_node_and_leaves_the_leaves_clean() {
        let w = parse_system_write(&json!({
            "memory": {"recall": {"$replace": true, "a": {"text": "A"}}}
        }))
        .unwrap();
        assert_eq!(w.replace_roots, vec!["memory.recall".to_string()]);
        assert_eq!(
            w.leaves,
            vec![("memory.recall.a".into(), json!({"text":"A"}))]
        );
    }

    /// A marker at the top of `system` has the empty root: the whole tree.
    #[test]
    fn a_marker_at_the_top_has_the_empty_root() {
        let w = parse_system_write(&json!({"$replace": true, "a": {"text": "A"}})).unwrap();
        assert_eq!(w.replace_roots, vec![String::new()]);
        assert_eq!(w.leaves, vec![("a".to_string(), json!({"text":"A"}))]);
    }

    /// `false` is an explicit no-op, so a writer may compute the flag instead
    /// of branching on whether to include the key at all.
    #[test]
    fn a_false_marker_revokes_nothing() {
        let w = parse_system_write(&json!({"memory": {"$replace": false, "a": {"text": "A"}}}))
            .unwrap();
        assert!(w.replace_roots.is_empty());
        assert_eq!(w.leaves.len(), 1);
    }

    /// Nested markers are the union of their roots, not a contradiction: the
    /// outer one already covers the inner, so the inner is redundant, not wrong.
    #[test]
    fn nested_markers_are_the_union_of_their_roots() {
        let w = parse_system_write(&json!({
            "memory": {"$replace": true, "recall": {"$replace": true, "a": {"text": "A"}}}
        }))
        .unwrap();
        let mut roots = w.replace_roots.clone();
        roots.sort();
        assert_eq!(
            roots,
            vec!["memory".to_string(), "memory.recall".to_string()]
        );
    }

    /// A leaf may carry the marker too — that is how a slot that used to be a
    /// container becomes a plain leaf again without leaving orphans below it.
    #[test]
    fn a_marker_on_a_leaf_is_a_root_and_the_leaf_is_stored_without_it() {
        let w = parse_system_write(&json!({"handover": {"$replace": true, "text": "H"}})).unwrap();
        assert_eq!(w.replace_roots, vec!["handover".to_string()]);
        assert_eq!(
            w.leaves,
            vec![("handover".to_string(), json!({"text":"H"}))]
        );
    }

    /// The `$` namespace is reserved so a misspelled marker cannot pass as an
    /// ordinary slot name and be silently ignored.
    #[test]
    fn any_other_dollar_key_is_a_shape_error_naming_its_node() {
        let err = parse_system_write(&json!({"memory": {"$replace_all": true}})).unwrap_err();
        assert!(err.contains("'memory'"), "must name the node: {err}");
        assert!(err.contains("$replace_all"), "must name the key: {err}");
    }

    #[test]
    fn a_non_boolean_marker_is_a_shape_error() {
        let err = parse_system_write(&json!({"memory": {"$replace": "yes"}})).unwrap_err();
        assert!(err.contains("boolean"), "must name the rule: {err}");
    }

    /// The delete is scoped to the root and stops at a segment boundary: a
    /// sibling that merely shares a character prefix is untouched.
    #[test]
    fn delete_takes_the_root_and_its_descendants_and_stops_at_the_segment() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        for path in [
            "memory",
            "memory.recall",
            "memory.recall.a",
            "memory.recallx",
            "identity",
        ] {
            upsert_system_leaf(&conn, path, &json!({"text": path}), 1).unwrap();
        }
        let removed = delete_system_subtree(&conn, "memory.recall").unwrap();
        assert_eq!(removed, 2, "the root row and its one descendant");
        let mut left: Vec<String> = conn
            .prepare("SELECT slot_path FROM system")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        left.sort();
        assert_eq!(left, vec!["identity", "memory", "memory.recallx"]);
    }

    #[test]
    fn an_empty_root_deletes_the_whole_tree() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        upsert_system_leaf(&conn, "identity", &json!({"text":"I"}), 1).unwrap();
        upsert_system_leaf(&conn, "memory.a", &json!({"text":"A"}), 1).unwrap();
        assert_eq!(delete_system_subtree(&conn, "").unwrap(), 2);
        assert_eq!(read_system_tree(&conn).unwrap(), json!({}));
    }

    /// A slot path holding a `%` must not widen the match — the reason the
    /// descendant test is a `substr` comparison and not a `LIKE`.
    #[test]
    fn a_wildcard_in_a_slot_path_does_not_widen_the_delete() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        upsert_system_leaf(&conn, "identity", &json!({"text":"I"}), 1).unwrap();
        assert_eq!(delete_system_subtree(&conn, "%").unwrap(), 0);
        assert_eq!(delete_system_subtree(&conn, "_dentity").unwrap(), 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM system", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    /// The budget is a statement about the tree the write leaves behind: a
    /// bundle at the limit that trades all its keys for new ones commits,
    /// because the ones it revokes are gone before the new ones are counted.
    #[test]
    fn a_replace_that_trades_every_key_at_the_budget_still_commits() {
        let td = TempDir::new().unwrap();
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let gate = SystemGate::for_test(2, 65_536, &[]);
        system_first_persist(
            &mut conn,
            &gate,
            &[
                ("m.a".to_string(), json!({"text":"1"})),
                ("m.b".to_string(), json!({"text":"2"})),
            ],
            &[],
            None,
            1,
        )
        .unwrap();
        system_first_persist(
            &mut conn,
            &gate,
            &[
                ("m.c".to_string(), json!({"text":"3"})),
                ("m.d".to_string(), json!({"text":"4"})),
            ],
            &["m".to_string()],
            None,
            2,
        )
        .unwrap();
        assert_eq!(
            read_system_tree(&conn).unwrap(),
            json!({"m": {"c": {"text":"3"}, "d": {"text":"4"}}})
        );
    }

    /// The delete runs INSIDE the transaction, so a refused write leaves the
    /// revoked subtree standing — never a hole where the replacement should be.
    #[test]
    fn a_refused_replace_does_not_delete_anything() {
        let td = TempDir::new().unwrap();
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let gate = SystemGate::for_test(2, 65_536, &[]);
        system_first_persist(
            &mut conn,
            &gate,
            &[
                ("m.a".to_string(), json!({"text":"1"})),
                ("keep".to_string(), json!({"text":"K"})),
            ],
            &[],
            None,
            1,
        )
        .unwrap();
        let err = system_first_persist(
            &mut conn,
            &gate,
            &[
                ("m.b".to_string(), json!({"text":"2"})),
                ("m.c".to_string(), json!({"text":"3"})),
                ("m.d".to_string(), json!({"text":"4"})),
            ],
            &["m".to_string()],
            None,
            2,
        )
        .unwrap_err();
        assert!(matches!(err, PersistError::Gate(_)), "{err:?}");
        assert_eq!(
            read_system_tree(&conn).unwrap(),
            json!({"m": {"a": {"text":"1"}}, "keep": {"text":"K"}}),
            "a refused replace rolls back its own delete"
        );
    }
}
