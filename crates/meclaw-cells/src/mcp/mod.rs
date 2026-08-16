//! Phase-10-D: the `mcp` cell. A long-running double task on the 10-A substrate.
//! Bridge to an external MCP provider via HTTP+JSON-RPC. POC scope: three
//! methods — `initialize`, `tools/list`, `tools/call`. See
//! `docs/cell-types.md` § `mcp` (l.461-481).

/// `McpCell` struct, `McpIo` struct, and `LongRunningCell` implementation with
/// `split_io` + `run_io` delegation. `handle` and `handle_event` follow in T16-T19.
pub mod cell;

/// DDL, load, and upsert helpers for the `mcp_discovery_cache` table in `cell.db`.
pub mod db;

/// Emit helpers for `McpCell`: `emit_tool_result_success` and `emit_tool_result_error`.
pub mod emit;

/// `McpCellFactory` struct + `CellFactory` impl with `validate_params` (T20) and
/// `spawn_cell` stub (T21). Helper `provider_key_from_path` for the system-tools slot prefix.
pub mod factory;

/// I/O sub-task frames: `McpEvent`, `McpReconfig`, `RunIoConfig`. `run_io` follows in T12.
pub mod io;

/// JSON-RPC 2.0 envelope types (request, response, error) and request-id generation.
pub mod jsonrpc;

/// Parsed configuration parameters for the mcp cell.
pub mod params;

/// The stdio transport: JSON-RPC over line-JSON to a child process.
pub mod stdio;

/// Pure parser for the `tool_call`-tail-turn: extracts `name` + `arguments` from
/// `text` (JSON string) and `id` (UBF-required call id). Phase-9 store convention.
pub mod parse;

/// `McpClient` (a reqwest wrapper) + the `call_rpc` helper + `McpError` + protocol constants.
pub mod wire;

pub use factory::McpCellFactory;
// GH #96: the parsed params and the transport they carry are public for the
// configuration pin in `tests/gh96_mcp_sandbox_profile.rs` — the same reason
// `subcolony::io::child_spec_for_test` is. What a test cannot see, a test
// cannot hold; and the containment of a third-party binary is exactly the
// thing that must not drift unobserved.
pub use params::{McpOverlay, McpParams, McpTransport};
