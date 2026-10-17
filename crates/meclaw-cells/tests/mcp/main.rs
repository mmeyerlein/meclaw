//! Test harness `mcp` of `meclaw-cells`: one binary, one module per old file.
//! Written by scripts/harness_fold.py from tests/HARNESS.toml -- edit the table, not this file.

pub mod fitness_mcp;
#[path = "../support/gh1059_grants.rs"]
pub mod gh1059_grants;
pub mod gh268_the_shipped_mcp_token_reaches_the_wire;
pub mod gh489_an_unnamed_mcp_provider_is_idle_not_failed;
pub mod gh96_mcp_sandbox_profile;
pub mod mcp_cell_split_io;
pub mod mcp_db_round_trip;
pub mod mcp_emit_smoke;
pub mod mcp_factory_resume;
pub mod mcp_factory_validate;
pub mod mcp_handle_event_state;
pub mod mcp_handle_list_tools;
pub mod mcp_handle_tool_call_errors;
pub mod mcp_handle_tool_call_success;
pub mod mcp_init_failure_restart_to_failed;
pub mod mcp_io_frames_smoke;
pub mod mcp_io_loop;
pub mod mcp_jsonrpc_smoke;
pub mod mcp_params_message;
pub mod mcp_params_parse;
pub mod mcp_public_reexport;
pub mod mcp_shutdown_sanity;
pub mod mcp_stdio_lifecycle;
pub mod mcp_stdio_params;
pub mod mcp_wire_against_mock;
#[path = "../mock_mcp.rs"]
pub mod mock_mcp;
pub mod mock_mcp_self_test;
pub mod stdio_child_env;
pub mod stdio_child_reaping;
pub mod stdio_child_serve;
pub mod stdio_child_spawn;
