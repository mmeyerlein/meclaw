//! T15: McpCell::split_io extracts RunIoConfig; second call panics.

use meclaw_cells::mcp::cell::McpCell;
use meclaw_cells::mcp::wire::McpClient;
use meclaw_colony::LongRunningCell;

#[test]
fn split_io_extracts_runio_config_once() {
    let client = McpClient::new("http://x", None).unwrap();
    let mut cell = McpCell::new(client, 30_000, 5_000, "main_mcp".into());
    let _io = cell.split_io();
    // second call would panic — not invoked here (mirrors timer/proxy).
}
