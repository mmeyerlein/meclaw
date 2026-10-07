//! GH #1034 — an `llm` cell that a `swap_nodes` puts in place of an `llm` cell
//! is its TWIN: what it does not bring itself, it takes from the original —
//! the instructions, the durable slots, the reasoning budget.
//!
//! A model swap is two diff entries: a template for the twin (the original's
//! standing `config.json` with another model) and a `swap_nodes` that swings
//! the original's edges onto it. Before this module the twin was born from its
//! template alone. Measured in a lab colony (06.10.2026):
//!
//! * twins cut from the standing `config.json` had no `seed/`, so a judge whose
//!   instructions live in its `system` table answered like a chat assistant,
//!   and every state downstream of it stayed empty (107/582 artefacts, 0 needs
//!   rows) — while the colony reported no error;
//! * every brain swapped in behind a curator held 0 of the 47–57 tools its
//!   curator ledger listed (talky 0/52, talky-chat 0/47, cogny 0/57). A curator
//!   hands its durable slots (`system.tools`, the sidecar contract, …) to its
//!   brain only when a slot CHANGES (`templates/collector/README.md`, lane
//!   `menu`); the new brain started with none and the curator believed it held
//!   them, so the model ran without tools and answered in prose;
//! * a twin that ran without the original's reasoning params measured another
//!   product: one spent 2048 of 2048 output tokens on reasoning and wrote
//!   nothing.
//!
//! The door COMPLETES the twin and never refuses it, because a deliberate
//! change is the main case of a swap: another model often needs other
//! reasoning params (a provider refuses an effort beside a reasoning limit,
//! GH #854), and a twin may be cut to carry other instructions on purpose.
//!
//! 1. **budget** ([`inherit_budget`], before the twin's `config.json` is
//!    patched) — a twin that declares NONE of [`BUDGET_KEYS`] (its template's
//!    params and the entry's override params together) takes the original's
//!    as a set: the budget the original RUNS on, its birth params with every
//!    run-time params update on top (the `cell.db` `params` overlay a model push
//!    writes). A twin that declares one keeps its own, and nothing of the
//!    original's is mixed in — an inherited effort beside a declared limit is
//!    exactly the request a provider refuses.
//! 2. **seed** ([`carry_seed`], before the twin's `cell.db` is seeded) — a twin
//!    born without `seed/` gets the original's, byte for byte; one that brings
//!    its own keeps it.
//! 3. **system rows** ([`carry_system`], after the seed) — every slot the twin
//!    does not hold from its own seed comes from the original: the curator's
//!    durable slots and the identity packs written at run time, and, for a twin
//!    without a seed of its own, the original's whole table row for row (its
//!    current values, not the seed's). The curator's belief "my brain holds
//!    these" stays true across the swap, and the twin's first call offers the
//!    model the tools the original's last call offered.
//!
//! Only `llm` → `llm`: the `system` table is where an `llm` cell keeps what it
//! IS, so a successor without it is the same cell broken. Every other type's
//! seed is data (a `store`'s rows, a policy), and swapping it for another
//! implementation with other data is what a swap is for. A swap onto a cell
//! that already stands (the `{name}` form without staging — the documented
//! swing-back) is no birth and keeps its own table.
//!
//! **Why the colony reads the original's `cell.db` here.** The
//! database-isolation rule binds all Rust code, the substrate included; this
//! read is a named exception to it in `docs/meclaw-overview.md`
//! (§ Database isolation and the `swap_nodes` row), the same class as
//! `seed_rows` (GH #456, the colony as write authority) and the read-only
//! boot reads in `bootstrap.rs`. It is a reader only: the original's file is
//! opened `SQLITE_OPEN_READ_ONLY` (`open_original`), only its `system` and
//! `params` tables are read, only on the swap path of a twin's birth, and its
//! schema is never migrated. The colony already builds and seeds every
//! `cell.db` at instantiation
//! ([`crate::mutation::stage::seed_cell_db_if_present`]), and the twin's
//! database is one it is building. Everything here runs while the twin is
//! still in `.staging`, so a failure leaves the live tree untouched.

use crate::mutation::MutationError;
use meclaw_core::JsonValue;
use meclaw_core::serde_json::Map;
use std::path::{Path, PathBuf};

/// The cell type the twin rule binds (see the module docs for why only it).
pub const TWIN_CELL_TYPE: &str = "llm";

/// The params that make up an `llm` cell's reasoning budget, inherited as ONE
/// set or not at all: how hard it deliberates (`reasoning_effort`, and
/// `reasoning`, which outranks it), how many tokens it may deliberate with
/// (`thinking_budget`) and how many it may spend on the whole answer
/// (`max_tokens`, the output cap a reasoning model fills with its reasoning
/// first — the 2048/2048 measurement above).
pub const BUDGET_KEYS: &[&str] = &[
    "reasoning_effort",
    "reasoning",
    "thinking_budget",
    "max_tokens",
];

/// The original of a twin: where it stands and its `config.json`.
#[derive(Debug, Clone)]
pub(crate) struct Original {
    dir: PathBuf,
    config: JsonValue,
}

/// The `match.name` of the `swap_nodes` entry whose with-side is `name`, both
/// resolved against `scope` (`brain-b` and `./brain-b` are one node, GH #179).
/// `None` when no swap of this diff names the node as its successor.
pub(crate) fn original_of(diff: &JsonValue, scope: &str, name: &str) -> Option<String> {
    let me = super::resolve_scoped_path(scope, name);
    diff.get("swap_nodes")?.as_array()?.iter().find_map(|s| {
        let with = s.get("with")?.get("name")?.as_str()?;
        if super::resolve_scoped_path(scope, with) != me {
            return None;
        }
        s.get("match")?.get("name")?.as_str().map(str::to_string)
    })
}

/// Is the node staged at `staging_path` a twin? Only when a swap of this diff
/// names it as the successor of a cell, and both are `llm` cells. Read from
/// the copied template, so it answers before the config is patched.
pub(crate) fn resolve(
    root: &Path,
    scope: &str,
    diff: &JsonValue,
    name: &str,
    staging_path: &Path,
) -> Option<Original> {
    let original_name = original_of(diff, scope, name)?;
    let staged = read_json(&staging_path.join("config.json"))?;
    if cell_type(&staged) != Some(TWIN_CELL_TYPE) {
        return None;
    }
    let dir = crate::path_truth::resolve_cell_dir(root, scope, &original_name);
    let config = read_json(&dir.join("config.json"))?;
    if cell_type(&config) != Some(TWIN_CELL_TYPE) {
        return None;
    }
    Some(Original { dir, config })
}

/// Before the twin's config is patched: a twin that declares none of
/// [`BUDGET_KEYS`] — in its template's params or in the entry's
/// `override_params` (`with.params` for the instantiate form) — gets the
/// original's running budget written into its staged `config.json`, so the
/// patch validates and substitutes it like any other param.
pub(crate) fn inherit_budget(
    original: &Original,
    staging_path: &Path,
    node: &JsonValue,
) -> Result<(), MutationError> {
    let cfg_path = staging_path.join("config.json");
    let mut cfg = read_json(&cfg_path).ok_or_else(|| {
        MutationError::Schema(format!(
            "swap_nodes twin (GH #1034): staged config.json does not parse ({})",
            cfg_path.display()
        ))
    })?;
    let declares = |v: Option<&JsonValue>| {
        v.and_then(|p| p.as_object())
            .is_some_and(|p| BUDGET_KEYS.iter().any(|k| p.contains_key(*k)))
    };
    if declares(cfg.get("params")) || declares(node.get("override_params")) {
        return Ok(());
    }
    let running = running_budget(original)?;
    if running.is_empty() {
        return Ok(());
    }
    let Some(obj) = cfg.as_object_mut() else {
        return Ok(());
    };
    let params = obj
        .entry("params")
        .or_insert_with(|| JsonValue::Object(Map::new()));
    let Some(params) = params.as_object_mut() else {
        return Ok(());
    };
    params.extend(running);
    let text = meclaw_core::serde_json::to_string_pretty(&cfg).map_err(|e| {
        MutationError::Schema(format!("swap_nodes twin (GH #1034): config.json: {e}"))
    })?;
    std::fs::write(&cfg_path, text).map_err(|e| {
        MutationError::Schema(format!(
            "swap_nodes twin (GH #1034): write {}: {e}",
            cfg_path.display()
        ))
    })
}

/// Before the twin's `cell.db` is seeded: a twin without `seed/` gets the
/// original's, byte for byte. Returns whether the twin brought a seed of its
/// own, which [`carry_system`] needs to know.
pub(crate) fn carry_seed(original: &Original, staging_path: &Path) -> Result<bool, MutationError> {
    let twin_seed = staging_path.join("seed");
    if twin_seed.exists() {
        return Ok(true);
    }
    let original_seed = original.dir.join("seed");
    if original_seed.is_dir() {
        crate::mutation::stage::copy_dir_recursive(&original_seed, &twin_seed)?;
    }
    Ok(false)
}

/// After the twin's seed: every `system` slot the twin does not hold from a
/// seed of its own comes from the original, `updated_at` included. A twin
/// without a seed of its own takes the original's table whole — the current
/// values, which a run-time write may have moved away from the seed's.
///
/// An original that never spawned has no `cell.db`; its rows would be its
/// seed's, which the twin already holds or chose to replace, so nothing is
/// copied.
pub(crate) fn carry_system(
    original: &Original,
    staging_path: &Path,
    own_seed: bool,
) -> Result<(), MutationError> {
    let src = original.dir.join("cell.db");
    if !src.exists() {
        return Ok(());
    }
    let err = |what: &str, e: rusqlite::Error| {
        MutationError::Schema(format!(
            "swap_nodes twin (GH #1034): {what} ({}): {e}",
            original.dir.display()
        ))
    };
    let rows: Vec<(String, String, i64)> = {
        let conn = open_original(&src).map_err(|e| err("open the original's cell.db", e))?;
        let mut stmt = conn
            .prepare("SELECT slot_path, value, updated_at FROM system ORDER BY slot_path")
            .map_err(|e| err("read the original's system rows", e))?;
        let read: rusqlite::Result<Vec<(String, String, i64)>> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .and_then(Iterator::collect);
        read.map_err(|e| err("read the original's system rows", e))?
    };
    let mut conn = rusqlite::Connection::open(staging_path.join("cell.db"))
        .map_err(|e| err("open the twin's staged cell.db", e))?;
    crate::persist::setup_cell_db(&conn).map_err(|e| err("set up the twin's cell.db", e))?;
    let tx = conn
        .transaction()
        .map_err(|e| err("write the twin's system rows", e))?;
    if !own_seed {
        tx.execute("DELETE FROM system", [])
            .map_err(|e| err("write the twin's system rows", e))?;
    }
    for (slot_path, value, updated_at) in &rows {
        tx.execute(
            "INSERT OR IGNORE INTO system (slot_path, value, updated_at) VALUES (?, ?, ?)",
            rusqlite::params![slot_path, value, updated_at],
        )
        .map_err(|e| err("write the twin's system rows", e))?;
    }
    tx.commit()
        .map_err(|e| err("write the twin's system rows", e))?;
    Ok(())
}

/// The original's budget as it runs: birth params, the `cell.db` `params`
/// overlay on top (last write wins, the same merge the cell's wake replays).
fn running_budget(original: &Original) -> Result<Map<String, JsonValue>, MutationError> {
    let mut out = Map::new();
    if let Some(params) = original.config.get("params").and_then(|p| p.as_object()) {
        for key in BUDGET_KEYS {
            if let Some(v) = params.get(*key) {
                out.insert((*key).to_string(), v.clone());
            }
        }
    }
    let db = original.dir.join("cell.db");
    if !db.exists() {
        return Ok(out);
    }
    let err = |e: rusqlite::Error| {
        MutationError::Schema(format!(
            "swap_nodes twin (GH #1034): read the original's params overlay ({}): {e}",
            original.dir.display()
        ))
    };
    let conn = open_original(&db).map_err(err)?;
    let mut stmt = conn.prepare("SELECT key, value FROM params").map_err(err)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(err)?;
    for row in rows {
        let (key, raw) = row.map_err(err)?;
        if !BUDGET_KEYS.contains(&key.as_str()) {
            continue;
        }
        let value: JsonValue = meclaw_core::serde_json::from_str(&raw).map_err(|e| {
            MutationError::Schema(format!(
                "swap_nodes twin (GH #1034): the original's params.{key} is not JSON ({e})"
            ))
        })?;
        out.insert(key, value);
    }
    Ok(out)
}

/// Open the running original's `cell.db` as a reader: it must already exist
/// (never created) and is opened `SQLITE_OPEN_READ_ONLY`, like the boot reads
/// in `bootstrap.rs`, so the intent stands in the flag and closing the
/// connection can never checkpoint or otherwise write another cell's file.
fn open_original(path: &Path) -> rusqlite::Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    crate::persist::apply_busy_timeout(&conn)?;
    Ok(conn)
}

fn read_json(path: &Path) -> Option<JsonValue> {
    let text = std::fs::read_to_string(path).ok()?;
    meclaw_core::serde_json::from_str(&text).ok()
}

fn cell_type(cfg: &JsonValue) -> Option<&str> {
    cfg.get("cell")?.get("type")?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn the_successor_is_found_in_either_spelling() {
        let diff =
            json!({"swap_nodes": [{"match": {"name": "brain"}, "with": {"name": "./brain-b"}}]});
        assert_eq!(
            original_of(&diff, "/m", "brain-b").as_deref(),
            Some("brain")
        );
        assert_eq!(
            original_of(&diff, "/m", "./brain-b").as_deref(),
            Some("brain")
        );
        assert_eq!(original_of(&diff, "/m", "other"), None);
        assert_eq!(original_of(&json!({}), "/m", "brain-b"), None);
    }

    /// The budget travels as a set or not at all: one declared key, in the
    /// template or in the entry's override, keeps the original's whole set out.
    #[test]
    fn a_declared_key_keeps_the_originals_budget_out() {
        let td = tempfile::TempDir::new().unwrap();
        let original = Original {
            dir: td.path().join("absent"),
            config: json!({"cell": {"type": "llm"},
                "params": {"reasoning_effort": "low", "max_tokens": 4096}}),
        };
        let write = |params: JsonValue| {
            std::fs::write(
                td.path().join("config.json"),
                json!({"cell": {"type": "llm"}, "params": params}).to_string(),
            )
            .unwrap();
        };
        let params = || read_json(&td.path().join("config.json")).unwrap()["params"].clone();

        write(json!({"model": "b"}));
        inherit_budget(&original, td.path(), &json!({})).unwrap();
        assert_eq!(
            params(),
            json!({"model": "b", "reasoning_effort": "low", "max_tokens": 4096})
        );

        write(json!({"model": "b", "thinking_budget": 1024}));
        inherit_budget(&original, td.path(), &json!({})).unwrap();
        assert_eq!(params(), json!({"model": "b", "thinking_budget": 1024}));

        write(json!({"model": "b"}));
        inherit_budget(
            &original,
            td.path(),
            &json!({"override_params": {"thinking_budget": 1024}}),
        )
        .unwrap();
        assert_eq!(params(), json!({"model": "b"}));
    }
}
