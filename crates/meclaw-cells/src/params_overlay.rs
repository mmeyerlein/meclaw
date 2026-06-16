//! Phase-16 β — generic runtime params-overlay core (W4b model, type-agnostic).
//!
//! W4b (`config.md` § Zugriff Z.20) built the params-update-per-message model
//! for the `llm` cell only. β extracts the mechanics into this type-generic
//! core so the other stateful/long-running cell types (`store`, `timer`, `mcp`,
//! `proxy`) reuse it:
//!
//! - [`OverlayParams`] — the per-type seam: `KNOWN_KEYS` / `IMMUTABLE_KEYS`
//!   const sets + a `parse` that doubles as the validate ≡ spawn parser path.
//! - [`apply_update`] — pure, all-or-nothing merge with the CENTRAL immutable
//!   check (no partial apply; mirrors the W4b `llm` semantics byte-for-byte).
//! - [`restore`] — wake/respawn replay of the `cell.db` overlay over the
//!   birth-params.
//! - `upsert_param_overlay` / `read_params_overlay` / `persist_params_overlay`
//!   / `merge_params_overlay` — the key/value-agnostic `cell.db` persistence,
//!   moved 1:1 from `llm/state.rs` (the `params` table is shared substrate,
//!   `meclaw_colony::persist::schema`).

use meclaw_core::serde_json::{self, Value};

/// A cell-type params struct that supports the W4b runtime params-update
/// overlay. Generic over the concrete params type; the per-type key-sets +
/// parse live in the impl, the overlay mechanics (immutable-check, merge,
/// persist, restore) are the type-agnostic free fns in this module.
pub trait OverlayParams: serde::Serialize + Sized {
    /// Top-level param keys the cell recognizes. An update key outside this set
    /// is rejected (no silent no-op).
    const KNOWN_KEYS: &'static [&'static str];
    /// Param keys that may NOT change at runtime (credentials / identity). An
    /// update touching one of these is rejected (no partial apply).
    const IMMUTABLE_KEYS: &'static [&'static str];
    /// Validate + construct from a raw `params` Value. This is the SAME path
    /// `validate_params` and `spawn_cell` route through (parser-invariant).
    fn parse(raw: &Value) -> Result<Self, String>;
}

/// Why a params-update was rejected. The `detail()` text NEVER echoes a param
/// value (secret-hygiene) — only the offending key name / a type-shape note.
/// Moved 1:1 from `llm/params.rs` (W4b).
#[derive(Debug)]
pub enum ParamUpdateError {
    /// An immutable key (per `IMMUTABLE_KEYS`) was present in the update.
    Immutable(String),
    /// A key outside `KNOWN_KEYS` was present in the update.
    Unknown(String),
    /// The merged params failed `OverlayParams::parse` (wrong value type, etc.).
    Invalid(String),
}

impl ParamUpdateError {
    /// Human-readable reject reason for the cell's error emission. Never echoes
    /// a param value — only the key name (`Immutable`/`Unknown`) or the parse
    /// error message (`Invalid`, which references the failing mutable field only;
    /// immutable secrets are rejected before any value reaches `parse`).
    pub fn detail(&self) -> String {
        match self {
            ParamUpdateError::Immutable(key) => {
                format!("params update rejected: '{key}' is immutable")
            }
            ParamUpdateError::Unknown(key) => {
                format!("params update rejected: unknown param '{key}'")
            }
            ParamUpdateError::Invalid(msg) => {
                format!("params update rejected: {msg}")
            }
        }
    }
}

/// Current unix time in whole seconds — the `updated_at` stamp for overlay
/// UPSERTs. Shared by the β cell-type slices (store/timer/mcp/proxy) so the
/// timestamp helper is not duplicated per type.
pub fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_secs() as i64
}

/// Apply a runtime params-update (W4b). Pure — no IO.
///
/// `update` is the top-level `params` body-slot of a params-update message
/// (config.md § Zugriff Z.20): a partial map of param keys to new values,
/// last-write-wins. Returns the merged params plus the overlay pairs to persist
/// in `cell.db` ([`persist_params_overlay`]). The caller applies neither on
/// `Err` — all-or-nothing, no partial apply.
///
/// Reject rules (loud, no partial apply), checked CENTRALLY here:
/// - an `IMMUTABLE_KEYS` key is present → `Immutable` (immutable wins over
///   otherwise-valid keys),
/// - a key outside `KNOWN_KEYS` is present → `Unknown` (a typo'd key would
///   otherwise silently no-op),
/// - the merged result fails `P::parse` (wrong value type, etc.) → `Invalid`.
pub fn apply_update<P: OverlayParams>(
    current: &P,
    update: &serde_json::Map<String, Value>,
) -> Result<(P, Vec<(String, Value)>), ParamUpdateError> {
    for key in update.keys() {
        if P::IMMUTABLE_KEYS.contains(&key.as_str()) {
            return Err(ParamUpdateError::Immutable(key.clone()));
        }
        if !P::KNOWN_KEYS.contains(&key.as_str()) {
            return Err(ParamUpdateError::Unknown(key.clone()));
        }
    }
    // Merge update over the current params (serialized to a Value) and re-parse
    // — this validates value types AND enforces the type's parse invariants.
    let mut base = serde_json::to_value(current).expect("OverlayParams serialize");
    let base_obj = base
        .as_object_mut()
        .expect("OverlayParams serializes to a JSON object");
    for (key, value) in update {
        base_obj.insert(key.clone(), value.clone());
    }
    let merged = P::parse(&base).map_err(ParamUpdateError::Invalid)?;
    let overlay: Vec<(String, Value)> =
        update.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    Ok((merged, overlay))
}

/// Rebuild the effective params for a wake/respawn (W4b restore).
///
/// Reads the runtime-param overlay from `cell.db` and replays it over the
/// birth-params (`config.json` `params`, already `${VAR}`-substituted by the
/// bootstrap), then re-parses. `config.json` is never written; a `cell.db`-wipe
/// (empty overlay) restores the Bootstrap-Stand (config.md § Zugriff Z.20).
pub fn restore<P: OverlayParams>(conn: &rusqlite::Connection, birth: &Value) -> Result<P, String> {
    let overlay = read_params_overlay(conn).map_err(|e| format!("read params overlay: {e}"))?;
    let merged = merge_params_overlay(birth, &overlay);
    P::parse(&merged)
}

/// UPSERT a single runtime-param overlay entry into `cell.db.params`. Idempotent.
///
/// W4b: a params-update message persists each changed top-level param key here
/// (last-write-wins). `value_json` is the param value serialized to JSON text.
/// The overlay is replayed over the birth-params (`config.json`) on wake/respawn
/// — `config.json` itself stays untouched (config.md § Zugriff Z.20).
pub(crate) fn upsert_param_overlay(
    conn: &rusqlite::Connection,
    key: &str,
    value: &Value,
    now: i64,
) -> rusqlite::Result<()> {
    let value_json = serde_json::to_string(value).expect("param overlay value serialize");
    conn.execute(
        "INSERT INTO params (key, value, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             updated_at = excluded.updated_at",
        rusqlite::params![key, value_json, now],
    )?;
    Ok(())
}

/// Read the full runtime-param overlay from `cell.db.params`.
///
/// Returns `(param_key, value_json)` pairs. Order is unspecified (caller merges
/// onto a base object, so order is irrelevant). Empty table → empty Vec
/// (= birth-params unchanged, e.g. after a `cell.db`-wipe = Reset).
pub(crate) fn read_params_overlay(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<Vec<(String, Value)>> {
    let mut stmt = conn.prepare("SELECT key, value FROM params")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (key, raw_value) = row?;
        let value: Value = serde_json::from_str(&raw_value)
            .expect("params.value must be parseable JSON (written by upsert_param_overlay)");
        out.push((key, value));
    }
    Ok(out)
}

/// Atomic persist of a runtime-param overlay (one UPSERT per pair) in ONE tx.
///
/// W4b last-write-wins: an already-present key is overwritten. Empty `overlay`
/// is a no-op transaction. Single tx → no partial overlay if cancelled mid-write.
pub(crate) fn persist_params_overlay(
    conn: &mut rusqlite::Connection,
    overlay: &[(String, Value)],
    now: i64,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for (key, value) in overlay {
        upsert_param_overlay(&tx, key, value, now)?;
    }
    tx.commit()
}

/// Replay a runtime-param overlay over the birth-params object (pure).
///
/// W4b restore (wake/respawn): the persisted overlay ([`read_params_overlay`])
/// is merged over the birth-params Value (the `config.json` `params` block,
/// already `${VAR}`-substituted by the bootstrap) — last-write-wins per key. The
/// result is re-parsed by the caller. `config.json` itself is never written.
/// Empty overlay → birth unchanged (= Reset / `cell.db`-wipe = Bootstrap-Stand).
///
/// `birth` is expected to be a JSON object (a valid `params` block); a non-object
/// birth is returned unchanged (let the downstream `parse` reject it).
pub(crate) fn merge_params_overlay(birth: &Value, overlay: &[(String, Value)]) -> Value {
    let mut merged = birth.clone();
    if let Some(obj) = merged.as_object_mut() {
        for (key, value) in overlay {
            obj.insert(key.clone(), value.clone());
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_colony::persist::open_or_create_cell_db;
    use meclaw_core::serde_json::json;
    use tempfile::TempDir;

    // ---- generic core (β): a minimal OverlayParams to exercise the seam ----

    #[derive(serde::Serialize, Debug)]
    struct Dummy {
        id: String,
        label: String,
    }
    impl OverlayParams for Dummy {
        const KNOWN_KEYS: &'static [&'static str] = &["id", "label"];
        const IMMUTABLE_KEYS: &'static [&'static str] = &["id"];
        fn parse(raw: &Value) -> Result<Self, String> {
            let id = raw
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or("id required")?
                .to_string();
            let label = raw
                .get("label")
                .and_then(|v| v.as_str())
                .ok_or("label required")?
                .to_string();
            Ok(Dummy { id, label })
        }
    }

    #[test]
    fn apply_update_rejects_immutable_central() {
        let d = Dummy {
            id: "x".into(),
            label: "a".into(),
        };
        let upd = json!({"id": "y"}).as_object().unwrap().clone();
        let err = apply_update(&d, &upd).unwrap_err();
        assert!(matches!(err, ParamUpdateError::Immutable(ref k) if k == "id"));
        assert!(
            !err.detail().contains("y"),
            "detail leaks value: {}",
            err.detail()
        );
    }

    #[test]
    fn apply_update_rejects_unknown_central() {
        let d = Dummy {
            id: "x".into(),
            label: "a".into(),
        };
        let upd = json!({"labl": "b"}).as_object().unwrap().clone();
        let err = apply_update(&d, &upd).unwrap_err();
        assert!(matches!(err, ParamUpdateError::Unknown(ref k) if k == "labl"));
    }

    #[test]
    fn apply_update_changes_mutable_and_returns_overlay() {
        let d = Dummy {
            id: "x".into(),
            label: "a".into(),
        };
        let upd = json!({"label": "b"}).as_object().unwrap().clone();
        let (new, overlay) = apply_update(&d, &upd).unwrap();
        assert_eq!(new.label, "b");
        assert_eq!(new.id, "x");
        assert_eq!(overlay, vec![("label".to_string(), json!("b"))]);
    }

    #[test]
    fn apply_update_immutable_wins_over_valid_keys_no_partial() {
        let d = Dummy {
            id: "x".into(),
            label: "a".into(),
        };
        let upd = json!({"label": "b", "id": "y"})
            .as_object()
            .unwrap()
            .clone();
        let err = apply_update(&d, &upd).unwrap_err();
        assert!(matches!(err, ParamUpdateError::Immutable(ref k) if k == "id"));
    }

    // ---- persistence (moved 1:1 from llm/state.rs, W4b regression-locked) ----

    #[test]
    fn params_overlay_round_trips() {
        let td = TempDir::new().unwrap();
        let mut conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        persist_params_overlay(
            &mut conn,
            &[
                ("model".to_string(), json!("gpt-4o-mini")),
                ("temperature".to_string(), json!(0.3)),
            ],
            100,
        )
        .unwrap();
        let mut overlay = read_params_overlay(&conn).unwrap();
        overlay.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            overlay,
            vec![
                ("model".to_string(), json!("gpt-4o-mini")),
                ("temperature".to_string(), json!(0.3)),
            ]
        );
    }

    #[test]
    fn params_overlay_is_last_write_wins() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        upsert_param_overlay(&conn, "model", &json!("gpt-4o"), 1).unwrap();
        upsert_param_overlay(&conn, "model", &json!("gpt-4o-mini"), 2).unwrap();
        let overlay = read_params_overlay(&conn).unwrap();
        assert_eq!(overlay, vec![("model".to_string(), json!("gpt-4o-mini"))]);
    }

    #[test]
    fn merge_overlay_empty_equals_birth() {
        // g-Kern: Reset (cell.db-Wipe = empty overlay) ⇒ birth-params unchanged.
        let birth = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        assert_eq!(merge_params_overlay(&birth, &[]), birth);
    }

    #[test]
    fn merge_overlay_overrides_present_key_keeps_others() {
        let birth = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let merged = merge_params_overlay(&birth, &[("model".to_string(), json!("gpt-4o-mini"))]);
        assert_eq!(
            merged,
            json!({"provider": "openai", "model": "gpt-4o-mini", "api_key": "x"})
        );
    }

    #[test]
    fn merge_overlay_adds_key_absent_in_birth() {
        let birth = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let merged = merge_params_overlay(&birth, &[("temperature".to_string(), json!(0.2))]);
        assert_eq!(merged["temperature"], json!(0.2));
        assert_eq!(merged["model"], json!("gpt-4o"));
    }

    #[test]
    fn params_overlay_empty_table_reads_empty() {
        let td = TempDir::new().unwrap();
        let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        assert!(read_params_overlay(&conn).unwrap().is_empty());
    }
}
