//! Phase-10-D: `mcp`-Cell. Long-Running Doppel-Task auf dem 10-A-Substrat.
//! Bridge zu einem externen MCP-Anbieter via HTTP+JSON-RPC. POC-Scope:
//! drei Methoden — `initialize`, `tools/list`, `tools/call`. Siehe
//! `docs/cell-types.md` § `mcp` (Z.461–481).

/// `McpCell` struct, `McpIo` struct, and `LongRunningCell` implementation with
/// `split_io` + `run_io`-delegation. `handle` and `handle_event` follow in T16–T19.
pub mod cell;

/// DDL, load, and upsert helpers for the `mcp_discovery_cache` table in `cell.db`.
pub mod db;

/// Emit-Helpers for `McpCell`: `emit_tool_result_success` and `emit_tool_result_error`.
pub mod emit;

/// `McpCellFactory` struct + `CellFactory` impl with `validate_params` (T20) and
/// `spawn_cell` stub (T21). Helper `provider_key_from_path` fuer System-Tools-Slot-Prefix.
pub mod factory;

/// I/O-Sub-Task frames: `McpEvent`, `McpReconfig`, `RunIoConfig`. `run_io` folgt in T12.
pub mod io;

/// JSON-RPC 2.0 envelope types (request, response, error) and request-id generation.
pub mod jsonrpc;

/// Parsed configuration parameters for the mcp cell.
pub mod params;

/// Pure parser for the `tool_call`-tail-turn: extracts `name` + `arguments` from
/// `text` (JSON-string) and `id` (UBF-required call id). Phase-9-store convention.
pub mod parse;

/// `McpClient` (reqwest-Wrapper) + `call_rpc`-Helper + `McpError` + Protocol-Konstanten.
pub mod wire;

pub use factory::McpCellFactory;
