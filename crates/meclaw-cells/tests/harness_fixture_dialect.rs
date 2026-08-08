//! P8 block 2 — self-test of the fake harness fixture.
//!
//! The fixture is what every deterministic `harness` test runs against, so its
//! dialect is load-bearing: if it drifts from what a real agent harness emits,
//! every test above it proves the wrong thing. This file pins the shape of the
//! event stream itself, separately from the cell that consumes it.
//!
//! Vocabulary mirrored from the installed Claude Code CLI (2.1.219), verified
//! against `claude --help` and the binary's own strings — see
//! `plans/p8-harness-cell.md` § b2.

use meclaw_cells::stdio_child::{ChildSpec, Frame, StdioChild};
use serde_json::Value as JsonValue;
use std::time::Duration;

const FIXTURE: &str = env!("CARGO_BIN_EXE_stream_json_harness_fixture");

fn fixture_spec(args: &[&str]) -> ChildSpec {
    ChildSpec {
        program: FIXTURE.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        kill_grace_ms: 500,
        ..ChildSpec::default()
    }
}

/// Every JSON frame the fixture writes before it closes stdout.
async fn collect_frames(args: &[&str]) -> Vec<JsonValue> {
    let mut child = StdioChild::spawn(&fixture_spec(args)).expect("spawn fixture");
    let mut out = Vec::new();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(30), child.read_frame())
            .await
            .expect("fixture went quiet for 30s")
            .expect("read failed");
        match frame {
            Some(Frame::Json(v)) => out.push(v),
            Some(Frame::Malformed(raw)) => panic!("fixture wrote a non-JSON line: {raw}"),
            None => break,
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ok_mode_emits_init_then_assistant_then_result() {
    let frames = collect_frames(&["ok"]).await;
    assert_eq!(frames.len(), 3, "unexpected stream: {frames:?}");

    assert_eq!(frames[0]["type"], "system");
    assert_eq!(frames[0]["subtype"], "init");
    let session_id = frames[0]["session_id"]
        .as_str()
        .expect("init must carry a session_id")
        .to_string();
    assert!(
        frames[0]["model"].is_string(),
        "init must report the effective model — the audit trail depends on it"
    );

    assert_eq!(frames[1]["type"], "assistant");
    assert_eq!(frames[1]["message"]["content"][0]["type"], "text");

    assert_eq!(frames[2]["type"], "result");
    assert_eq!(frames[2]["subtype"], "success");
    assert_eq!(frames[2]["is_error"], false);
    assert_eq!(
        frames[2]["session_id"].as_str(),
        Some(session_id.as_str()),
        "the session id must be stable across the stream"
    );
    assert!(frames[2]["num_turns"].is_number());
    assert!(frames[2]["total_cost_usd"].is_number());
    assert!(frames[2]["result"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tooluse_mode_carries_a_tool_call_and_its_result() {
    let frames = collect_frames(&["tooluse"]).await;
    assert_eq!(frames.len(), 4, "unexpected stream: {frames:?}");

    let call = &frames[1]["message"]["content"][0];
    assert_eq!(call["type"], "tool_use");
    assert_eq!(call["name"], "Bash");
    let call_id = call["id"].as_str().expect("a tool call needs an id");

    let result = &frames[2]["message"]["content"][0];
    assert_eq!(frames[2]["type"], "user");
    assert_eq!(result["type"], "tool_result");
    assert_eq!(
        result["tool_use_id"].as_str(),
        Some(call_id),
        "the tool result must point back at its call"
    );
    assert_eq!(frames[3]["num_turns"], 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_crash_mode_ends_without_a_result_event() {
    let frames = collect_frames(&["crash", "7"]).await;
    assert_eq!(frames.len(), 1, "only the init event may arrive");
    assert_eq!(frames[0]["subtype"], "init");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_silent_mode_says_nothing_at_all() {
    let frames = collect_frames(&["silent"]).await;
    assert!(frames.is_empty(), "expected silence, got {frames:?}");
}

/// The cancel subject: it announces itself and then never finishes. Anything
/// that ends this process comes from outside.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stall_mode_starts_and_then_never_finishes() {
    let mut child = StdioChild::spawn(&fixture_spec(&["stall"])).expect("spawn fixture");
    for expected in ["system", "assistant"] {
        match tokio::time::timeout(Duration::from_secs(30), child.read_frame())
            .await
            .expect("fixture went quiet for 30s")
            .expect("read failed")
        {
            Some(Frame::Json(v)) => assert_eq!(v["type"], expected),
            other => panic!("expected a {expected} frame, got {other:?}"),
        }
    }
    // Tight on purpose: this asserts the ABSENCE of a third frame, so the wait
    // only needs to outlast a plausible scheduling delay.
    let third = tokio::time::timeout(Duration::from_millis(500), child.read_frame()).await;
    assert!(third.is_err(), "the stall mode produced a third frame");
    child.terminate(Duration::from_millis(200)).await;
}

/// The permission round trip, driven from this side.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ask_mode_waits_for_a_control_response_and_reports_the_decision() {
    let mut child = StdioChild::spawn(&fixture_spec(&["ask"])).expect("spawn fixture");
    let timeout = Duration::from_secs(5);

    let mut request_id = String::new();
    for _ in 0..2 {
        match child.read_frame().await.expect("read").expect("eof") {
            Frame::Json(v) if v["type"] == "control_request" => {
                assert_eq!(v["request"]["subtype"], "can_use_tool");
                assert_eq!(v["request"]["tool_name"], "Bash");
                request_id = v["request_id"].as_str().expect("request_id").to_string();
                break;
            }
            Frame::Json(_) => continue,
            other => panic!("unexpected frame {other:?}"),
        }
    }
    assert!(!request_id.is_empty(), "no control request arrived");

    child
        .write_frame(
            &serde_json::json!({
                "type": "control_response",
                "response": {"subtype": "success", "request_id": request_id,
                             "response": {"behavior": "deny", "message": "not allowed"}}
            }),
            timeout,
        )
        .await
        .expect("write the decision");

    match child.read_frame().await.expect("read").expect("eof") {
        Frame::Json(v) => {
            assert_eq!(v["type"], "result");
            assert_eq!(v["result"], "permission decision: deny");
        }
        other => panic!("expected the result frame, got {other:?}"),
    }
}

/// A real harness spawns build and search tools of its own. The fixture can do
/// the same, so the group-reaping proof has a realistic subject.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_grandchild_flag_really_starts_a_second_process() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("grandchild.pid");
    let mut child = StdioChild::spawn(&fixture_spec(&[
        "stall",
        "--grandchild",
        "--grandchild-pid-file",
        &pid_file.display().to_string(),
    ]))
    .expect("spawn fixture");

    // The init frame proves the fixture got past its own startup.
    child.read_frame().await.expect("read").expect("eof");
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .expect("no grandchild pid file")
        .trim()
        .parse()
        .expect("not a pid");
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "the grandchild {pid} is not running"
    );

    child.terminate(Duration::from_millis(200)).await;
    // Without a process group this one survives — that is the whole point of
    // the mode; the cell-level test asserts the reaping.
    let _ = std::process::Command::new("/bin/kill")
        .arg("-9")
        .arg(pid.to_string())
        .status();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_given_session_id_is_echoed_instead_of_invented() {
    let frames =
        collect_frames(&["ok", "--session-id", "11111111-2222-3333-4444-555555555555"]).await;
    assert_eq!(
        frames[0]["session_id"], "11111111-2222-3333-4444-555555555555",
        "the fixture must honour a pinned session id, like the real CLI does"
    );
}
