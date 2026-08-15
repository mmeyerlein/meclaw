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

#[test]
fn validate_rejects_missing_endpoint() {
    let f: Arc<McpCellFactory> = Arc::new(McpCellFactory);
    let err = f.validate_params(&json!({})).unwrap_err();
    assert!(err.contains("endpoint"), "got: {err}");
}
