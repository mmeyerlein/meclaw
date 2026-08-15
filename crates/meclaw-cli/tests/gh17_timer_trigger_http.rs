//! GH #17: a scheduled lane can be triggered once, now, over the HTTP API.
//!
//! The colony under test is the shape the issue was written about: a `timer`
//! carrying a config-born nightly schedule (`0 0 3 * * *`) that emits into a
//! lane. Config-born means the schedule is a birth snapshot in the cell db, so
//! editing the config and restarting changes nothing, and a direct message to
//! the lane is not the same event -- the run context is minted by the schedule.
//! The only honest way in is the timer's own op surface, and that surface was
//! unreachable from outside.
//!
//! Two halves, both pinned here:
//!
//!   1. THE ENVELOPE. The op body as documented carried no central UBF slot, so
//!      the ingress answered 422 and the op never left the HTTP layer. An op
//!      message honestly has no conversational turns, and `"messages": []` says
//!      exactly that -- with it the body is valid UBF and the op fields ride
//!      along as the cell-specific top-level slots the format allows. Both rows
//!      are asserted: the slot-less body still gets its 422, the enveloped one
//!      gets 202 and reaches the cell.
//!   2. THE OP. `trigger` fires an EXISTING schedule once. It is routed through
//!      the I/O task's `Fire` frame, so the emission is produced by the same
//!      `handle_event` a cron tick runs -- which is what the issue's "the run is
//!      indistinguishable from a cron-fired one" demands. The receipt is the
//!      full auto-header set on the message that reached the lane.
//!
//! Positive receipt throughout: the lane message and its headers, never an empty
//! dead-letter queue or an absent error.

use meclaw_cli::{Cli, run_with_hooks};
use std::net::SocketAddr;
use std::time::Duration;

/// The seeded schedule. Fixed rather than minted: the trigger has to name the
/// row the config gave birth to, which is the whole point of the op.
const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-0000000000c1";

fn write_json(path: &std::path::Path, body: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write");
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

/// Every message the log holds for the lane, OLDEST FIRST.
///
/// The endpoint answers newest first; this test reads the lane as a sequence of
/// runs, and indexing a sequence from the wrong end is the kind of mistake that
/// passes while there is only one element.
async fn lane_messages(addr: &SocketAddr) -> Vec<serde_json::Value> {
    let body = get_json(addr, "/colony/messages?to_path_prefix=/lane&limit=200").await;
    let mut rows = body["messages"].as_array().cloned().unwrap_or_default();
    rows.reverse();
    rows
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scheduled_lane_is_triggerable_once_over_the_http_api() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();

    // --- The topology: nightly timer -> lane, wired by one edge. ---
    write_json(
        &root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},
            "params":{"graph":{"edges":[{"from":"./nightly","to":"./lane"}]}}}"#,
    );
    // 03:00 daily. The test never waits for it: a schedule that could fire on
    // its own inside the test window would make the trigger unprovable.
    write_json(
        &root.join("main/nightly/config.json"),
        &format!(
            r#"{{"cell":{{"type":"timer","timeout":-1}},
                 "params":{{"query_timeout_ms":5000,
                   "schedules":[{{
                     "schedule_id":"{SCHEDULE_ID}",
                     "schedule_name":"nightly-consolidation",
                     "cron":"0 0 3 * * *",
                     "emit_to":"/lane",
                     "emit_body":{{"messages":[{{"origin":"user","type":"text",
                                                "text":"nightly-consolidation"}}]}},
                     "emit_headers":{{"msg_type":"consolidation_trigger"}}
                   }}]}},
                 "contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    );
    write_json(
        &root.join("main/lane/config.json"),
        r#"{"cell":{"type":"bash"},"params":{"command":"true"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    // --- Boot the real CLI lifecycle with the HTTP API on an ephemeral port. ---
    let cli = Cli {
        root: root.into(),
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
        tokio_console_port: 6669,
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

    // The lane is silent: the only schedule is due at 03:00, and nothing else
    // addresses it. Without this the receipt below could be a stray boot event.
    assert!(
        lane_messages(&addr).await.is_empty(),
        "the lane must be silent before the trigger; a nightly schedule is not due"
    );

    // --- 1. The envelope. The op body as it stands in the op catalogue carries
    //        no central UBF slot, and the ingress is a trust boundary. ---
    let (status, body) = post_message(
        &addr,
        serde_json::json!({
            "target": "/nightly",
            "body": {"op": "trigger", "schedule_id": SCHEDULE_ID}
        }),
    )
    .await;
    assert_eq!(
        status, 422,
        "an op body without a central UBF slot stays a 422; body: {body}"
    );
    assert_eq!(body["error"], "invalid_ubf_body", "body: {body}");

    // The same op with the slot it honestly has: no turns.
    let (status, body) = post_message(
        &addr,
        serde_json::json!({
            "target": "/nightly",
            "body": {"messages": [], "op": "trigger", "schedule_id": SCHEDULE_ID}
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "the enveloped op must pass the ingress; body: {body}"
    );

    // --- 2. The op. The lane runs, and its message carries the header set of a
    //        cron firing (cell-types.md § timer, emitted headers). ---
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut fired = Vec::new();
    while tokio::time::Instant::now() < deadline {
        fired = lane_messages(&addr).await;
        if !fired.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if fired.len() != 1 {
        let all = get_json(&addr, "/colony/messages?limit=50").await;
        let dlq = get_json(&addr, "/colony/dead_letters").await;
        panic!("one trigger must run the lane exactly once; got {fired:?}\nlog: {all}\ndlq: {dlq}");
    }

    let hop: serde_json::Value =
        serde_json::from_str(fired[0]["headers_json"].as_str().expect("headers_json"))
            .expect("headers parse");
    let hop = &hop["hop"];
    assert_eq!(hop["schedule_id"], SCHEDULE_ID, "hop: {hop}");
    assert_eq!(hop["schedule_name"], "nightly-consolidation", "hop: {hop}");
    // From `emit_headers` — the triggered run carries the schedule's own headers,
    // not a synthetic set invented by the op.
    assert_eq!(hop["msg_type"], "consolidation_trigger", "hop: {hop}");
    // A repeating schedule reports its iteration; the first run is 0. A trigger
    // that emitted through some other path would have no iteration at all.
    assert_eq!(hop["iteration_n"], 0, "hop: {hop}");
    let event_id = hop["event_id"].as_str().expect("event_id");
    assert!(
        meclaw_core::Uuid::parse_str(event_id).is_ok(),
        "event_id must be a uuid, got {event_id}"
    );
    let scheduled_at = hop["scheduled_at"].as_str().expect("scheduled_at");
    let fired_at = hop["fired_at"].as_str().expect("fired_at");
    assert!(scheduled_at.ends_with('Z'), "scheduled_at RFC-3339-Z");
    assert!(fired_at.ends_with('Z'), "fired_at RFC-3339-Z");
    assert!(
        scheduled_at <= fired_at,
        "scheduled_at <= fired_at: {scheduled_at} / {fired_at}"
    );
    // The schedule SENDS, it does not generate: the body is the one the config
    // parked, so the lane sees what a 03:00 firing would have handed it.
    let payload = fired[0]["body_payload"].as_str().expect("body_payload");
    assert!(
        payload.contains("nightly-consolidation"),
        "the lane must receive the schedule's own emit_body, got {payload}"
    );

    // --- 3. The id is the op. An unknown schedule passes the ingress (the op is
    //        colony-validated, not HTTP-validated) and is refused BY NAME. The
    //        error reply travels the timer's own out-edge, so it lands in the
    //        same lane and can be read as a positive receipt rather than as an
    //        absence: the operator who mistypes an id learns it. ---
    let (status, body) = post_message(
        &addr,
        serde_json::json!({
            "target": "/nightly",
            "body": {"messages": [], "op": "trigger",
                     "schedule_id": "0190a3f2-0000-7000-8000-0000000000ff"}
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "the op reaches the cell to be judged; body: {body}"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        fired = lane_messages(&addr).await;
        if fired.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(
        fired.len(),
        2,
        "the refusal must be observable; got {fired:?}"
    );
    // The code travels in the headers (the colony lifts `content.header` into
    // the envelope), the reason in the body.
    let refusal: serde_json::Value =
        serde_json::from_str(fired[1]["headers_json"].as_str().expect("headers_json"))
            .expect("headers parse");
    assert_eq!(
        refusal["hop"]["error_code"], "schedule_not_found",
        "an unknown id is refused by name (spec § timer, error codes); hop: {refusal}"
    );
    // And it is a refusal, not a firing: nothing ran, so nothing counted.
    assert!(
        refusal["hop"]["schedule_id"].is_null(),
        "a refused trigger emits no fire headers; hop: {refusal}"
    );
    let detail = fired[1]["body_payload"].as_str().expect("body_payload");
    assert!(
        detail.contains("0190a3f2-0000-7000-8000-0000000000ff"),
        "the refusal names the id it could not find, got {detail}"
    );

    // --- 4. Ownership: the trigger did not consume the schedule. It is still
    //        there and still due at 03:00, so the next cron slot happens as
    //        planned -- and it can be triggered again right now. ---
    let (status, body) = post_message(
        &addr,
        serde_json::json!({
            "target": "/nightly",
            "body": {"messages": [], "op": "trigger", "schedule_id": SCHEDULE_ID}
        }),
    )
    .await;
    assert_eq!(status, 202, "body: {body}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        fired = lane_messages(&addr).await;
        if fired.len() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(
        fired.len(),
        3,
        "a repeating schedule survives its trigger and can be triggered again"
    );
    // The second run counts on: a triggered firing is a firing, not a replay.
    let second: serde_json::Value =
        serde_json::from_str(fired[2]["headers_json"].as_str().expect("headers_json"))
            .expect("headers parse");
    assert_eq!(
        second["hop"]["iteration_n"], 1,
        "two triggered runs are iteration 0 and 1, exactly as two cron ticks; hop: {second}"
    );
    assert_eq!(second["hop"]["schedule_id"], SCHEDULE_ID, "hop: {second}");

    shutdown_tx.send(()).expect("shutdown");
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("shutdown must not hang")
        .expect("join")
        .expect("run_with_hooks Ok");
}
