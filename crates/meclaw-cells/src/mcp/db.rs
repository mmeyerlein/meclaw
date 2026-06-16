//! Phase-10-D: `cell.db.mcp_discovery_cache`-Schema + Helpers.
//! Sync rusqlite. Aufruf in der Factory vor `DbConn::wrap` (korridor-frei)
//! und im Handler über `DbConn::call(|c| ...)`.

use rusqlite::{Connection, params};

/// In-memory row of the `mcp_discovery_cache` table.
#[derive(Debug, Clone)]
pub struct DiscoveredTool {
    /// Tool name (primary key in the cache).
    pub name: String,
    /// JSON-serialized input schema (kept as TEXT to avoid re-validating
    /// MCP-side schemas on every read).
    pub schema_json: String,
}

/// Idempotent DDL for the discovery-cache table. Safe across restarts.
pub fn setup_mcp_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS mcp_discovery_cache (
            tool_name     TEXT PRIMARY KEY,
            schema_json   TEXT NOT NULL,
            discovered_at TEXT NOT NULL
        );",
    )
}

/// Load all cached tools, sorted by `tool_name` for deterministic output.
pub fn load_discovery_cache(conn: &Connection) -> rusqlite::Result<Vec<DiscoveredTool>> {
    let mut stmt =
        conn.prepare("SELECT tool_name, schema_json FROM mcp_discovery_cache ORDER BY tool_name")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(DiscoveredTool {
                name: r.get(0)?,
                schema_json: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Upsert the given tools. `discovered_at` is set identically for every
/// row in the batch — the handler passes a single RFC-3339-Z timestamp
/// per discovery event.
pub fn upsert_discovery_tools(
    conn: &Connection,
    tools: &[DiscoveredTool],
    discovered_at: &str,
) -> rusqlite::Result<()> {
    let sql = "INSERT INTO mcp_discovery_cache (tool_name, schema_json, discovered_at)
               VALUES (?1, ?2, ?3)
               ON CONFLICT(tool_name) DO UPDATE SET
                   schema_json   = excluded.schema_json,
                   discovered_at = excluded.discovered_at";
    let mut stmt = conn.prepare(sql)?;
    for t in tools {
        stmt.execute(params![t.name, t.schema_json, discovered_at])?;
    }
    Ok(())
}
