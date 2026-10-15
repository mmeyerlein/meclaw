//! T20: McpCellFactory::validate_params delegates to McpParams::parse.

use meclaw_cells::mcp::factory::McpCellFactory;
use meclaw_colony::CellFactory;
use serde_json::json;
use std::sync::Arc;

#[test]
fn validate_accepts_minimal_required() {
    let f: Arc<McpCellFactory> = Arc::new(McpCellFactory);
    f.validate_params(&json!({"endpoint":"https://x"})).unwrap();
}

/// GH #489: naming no provider is a state, not a config error, so the
/// pre-spawn validation accepts it. The refusals it still owes are pinned in
/// `mcp_params_parse.rs` (unknown transport, both transports at once, an empty
/// stdio `command`).
#[test]
fn validate_accepts_a_config_that_names_no_provider() {
    let f: Arc<McpCellFactory> = Arc::new(McpCellFactory);
    f.validate_params(&json!({})).expect("GH #489");
}
