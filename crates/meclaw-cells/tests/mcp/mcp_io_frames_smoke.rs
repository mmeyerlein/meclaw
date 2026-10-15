//! T11: io-frame types compile and expose the expected shape.

use meclaw_cells::mcp::db::DiscoveredTool;
use meclaw_cells::mcp::io::{McpEvent, McpReconfig, RunIoConfig};

#[test]
fn discovery_ready_event_constructible() {
    let _ev = McpEvent::DiscoveryReady {
        tools: vec![DiscoveredTool {
            name: "echo".into(),
            schema_json: "{}".into(),
        }],
    };
}

#[test]
fn run_io_config_constructible() {
    let c = meclaw_cells::mcp::wire::McpClient::new("http://x", None).unwrap();
    let _cfg = RunIoConfig {
        client: c,
        external_timeout_ms: 30_000,
        liveness: meclaw_colony::IoLivenessMark::disabled(),
    };
}

#[test]
fn mcp_reconfig_is_empty_enum() {
    // Type-level only — `McpReconfig` has no variants, so we cannot
    // construct one. The compile-time presence proves the trait
    // associated-type wiring works.
    fn _accepts<T>(_: T) {}
    fn _take(_r: McpReconfig) {}
}
