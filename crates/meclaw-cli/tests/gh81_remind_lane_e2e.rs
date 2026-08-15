//! GH #81: a scheduled lane, end to end, from an agent tool call.
//!
//! The issue's consequence was that `remind` could not be a tool lane the way
//! `bash` is: the timer read its op off the body's top level, and it acked
//! nothing, so an agent needed a bespoke bridge cell that translated the shape
//! AND fabricated the `tool_result` the loop waits for. Nothing downstream would
//! ever produce one.
//!
//! The colony under test is the committed fixture
//! `tests/fixtures/gh81-remind-lane` -- the whole lane and nothing else:
//!
//! ```text
//!   (agent turn over HTTP) -> /dispatch --hop.tool_name == 'remind'--> /remind
//!                                              --hop.msg_type == 'timer_op_ack'--> /ack
//!                                              --hop.msg_type == 'timer_op_error'--> /drain
//!                                              --hop.schedule_name == '...'--> /notify
//! ```
//!
//! `dispatch` is the ordinary dispatcher every tool loop already has: it unwraps
//! the assistant's `{name, arguments}` into a `tool_call` turn and sets
//! `hop.tool_name`. Nothing about it knows the timer. That is the whole point --
//! the timer is reached the way `bash` and `file` are reached.
//!
//! Deterministic by construction: the schedule is parked in 2099 and is fired by
//! an explicit second tool call (`op: trigger`, GH #17), so no assertion waits on
//! a wall clock.

use meclaw_cli::{Cli, run_with_hooks};
use std::net::SocketAddr;
use std::time::Duration;

/// Fixed rather than minted: the second tool call has to name the row the first
/// one created, which is what "an agent drives this" means.
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-0000000000e1";

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Copies the committed fixture tree into a TempDir -- never boot in place
/// (`colony.db`/`cell.db` are created at runtime).
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("read_dir").flatten() {
        let to = dst.join(entry.file_name());
        if entry.file_type().expect("file_type").is_dir() {
            copy_dir_recursive(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("copy");
        }
    }
}

async fn get_json(addr: &SocketAddr, path: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("http://{addr}{path}"))
        .send()
        .await
        .expect("GET")
        .json()
        .await
        .expect("json")
}

async fn post_message(addr: &SocketAddr, payload: serde_json::Value) -> (u16, serde_json::Value) {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/messages"))
        .json(&payload)
        .send()
        .await
        .expect("POST /messages");
    let status = resp.status().as_u16();
    (status, resp.json().await.expect("json"))
}

/// Every message the log holds for `prefix`, OLDEST FIRST (the endpoint answers
/// newest first, and this test reads lanes as sequences).
async fn lane(addr: &SocketAddr, prefix: &str) -> Vec<serde_json::Value> {
    let body = get_json(
        addr,
        &format!("/colony/messages?to_path_prefix={prefix}&limit=200"),
    )
    .await;
    let mut rows = body["messages"].as_array().cloned().unwrap_or_default();
    rows.reverse();
    rows
}

/// Polls `prefix` until it holds `n` rows, or fails with the whole log attached.
async fn await_lane(addr: &SocketAddr, prefix: &str, n: usize) -> Vec<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut rows = Vec::new();
    while tokio::time::Instant::now() < deadline {
        rows = lane(addr, prefix).await;
        if rows.len() >= n {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let all = get_json(addr, "/colony/messages?limit=100").await;
    let dlq = get_json(addr, "/colony/dead_letters").await;
    panic!("{prefix} never reached {n} rows, got {rows:?}\nlog: {all}\ndlq: {dlq}");
}

/// The hop compartment of a logged row.
fn hop_of(row: &serde_json::Value) -> serde_json::Value {
    let headers: serde_json::Value =
        serde_json::from_str(row["headers_json"].as_str().expect("headers_json"))
            .expect("headers parse");
    headers["hop"].clone()
}

/// One assistant turn the way a provider emits a tool call.
fn agent_tool_call(call_id: &str, arguments: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "target": "/dispatch",
        "body": {
            "messages": [{
                "origin": "assistant",
                "type": "tool_call",
                "id": call_id,
                "text": serde_json::json!({
                    "name": "remind",
                    "arguments": arguments.to_string()
                }).to_string()
            }]
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_tool_call_schedules_a_lane_and_gets_its_tool_result_back() {
    let td = tempfile::TempDir::new().expect("tempdir");
    copy_dir_recursive(&fixture_path("gh81-remind-lane"), td.path());

    let cli = Cli {
        root: td.path().into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some("127.0.0.1:0".parse().expect("bind")),
        daemon: false,
        validate: false,
        validate_strict: false,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6673,
        sandbox_probe: false,
        stdio_format: meclaw_cli::StdioFormat::Text,
    };
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });
    let addr = tokio::time::timeout(Duration::from_secs(30), addr_rx)
        .await
        .expect("the colony must bind HTTP within 30s")
        .expect("addr hook");

    // The lanes are silent: no schedule exists yet, and nothing else addresses them.
    assert!(lane(&addr, "/notify").await.is_empty());
    assert!(lane(&addr, "/ack").await.is_empty());

    // --- 1. The agent schedules something. Ordinary tool-call shape. ---
    let (status, body) = post_message(
        &addr,
        agent_tool_call(
            "call-remind-add",
            serde_json::json!({
                "op": "add",
                "schedule_id": SCHEDULE_ID,
                "schedule_name": "assistant-reminder",
                "at": "2099-01-01T09:00:00Z",
                "emit_to": "/notify",
                "emit_body": {"messages": [
                    {"origin": "user", "type": "text", "text": "stretch your legs"}
                ]}
            }),
        ),
    )
    .await;
    assert_eq!(status, 202, "the agent turn must reach the colony: {body}");

    // The ack is the point of #81: a tool_result on the inbound id, produced by
    // the timer itself, with no bridge cell anywhere in the topology.
    let acks = await_lane(&addr, "/ack", 1).await;
    let hop = hop_of(&acks[0]);
    assert_eq!(hop["msg_type"], "timer_op_ack", "hop: {hop}");
    assert_eq!(hop["op"], "add", "hop: {hop}");
    assert_eq!(hop["schedule_id"], SCHEDULE_ID, "hop: {hop}");
    let payload = acks[0]["body_payload"].as_str().expect("body_payload");
    assert!(
        payload.contains("call-remind-add"),
        "the ack must carry the inbound tool_call id, got: {payload}"
    );
    assert!(
        payload.contains("tool_result"),
        "and it must be a tool_result turn, got: {payload}"
    );

    // Nothing fired yet: the schedule is parked in 2099.
    assert!(
        lane(&addr, "/notify").await.is_empty(),
        "a 2099 schedule must not fire during the test"
    );

    // --- 2. The agent fires it. Same lane, same shape, second call id. ---
    let (status, body) = post_message(
        &addr,
        agent_tool_call(
            "call-remind-trigger",
            serde_json::json!({"op": "trigger", "schedule_id": SCHEDULE_ID}),
        ),
    )
    .await;
    assert_eq!(status, 202, "{body}");

    let fired = await_lane(&addr, "/notify", 1).await;
    assert_eq!(fired.len(), 1, "exactly one firing: {fired:?}");
    let hop = hop_of(&fired[0]);
    assert_eq!(hop["schedule_id"], SCHEDULE_ID, "hop: {hop}");
    assert_eq!(hop["schedule_name"], "assistant-reminder", "hop: {hop}");
    assert!(hop["fired_at"].is_string(), "hop: {hop}");
    let payload = fired[0]["body_payload"].as_str().expect("body_payload");
    assert!(
        payload.contains("stretch your legs"),
        "the lane receives the schedule's own emit_body, got: {payload}"
    );

    // The trigger is acked on its own id, so a loop can close both calls.
    let acks = await_lane(&addr, "/ack", 2).await;
    let trigger_ack = acks
        .iter()
        .find(|r| {
            r["body_payload"]
                .as_str()
                .is_some_and(|p| p.contains("call-remind-trigger"))
        })
        .unwrap_or_else(|| panic!("no ack for the trigger call: {acks:?}"));
    assert_eq!(hop_of(trigger_ack)["op"], "trigger");

    // --- 3. A refused op answers on the same id instead of going silent. ---
    let (status, body) = post_message(
        &addr,
        agent_tool_call(
            "call-remind-bogus",
            serde_json::json!({
                "op": "remove",
                "schedule_id": "0190a3f2-0000-7000-8000-00000000dead"
            }),
        ),
    )
    .await;
    assert_eq!(status, 202, "{body}");

    let drained = await_lane(&addr, "/drain", 1).await;
    let hop = hop_of(&drained[0]);
    assert_eq!(hop["error_code"], "schedule_not_found", "hop: {hop}");
    let payload = drained[0]["body_payload"].as_str().expect("body_payload");
    assert!(
        payload.contains("call-remind-bogus"),
        "an error on a tool lane closes the loop too, got: {payload}"
    );

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(30), join).await;
}
