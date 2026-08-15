//! P8 block 1 — environment containment for child processes.
//!
//! The observation trick: `/usr/bin/env` prints the environment it was started
//! with, one `KEY=value` per line. Those lines are not JSON, so they arrive as
//! `Frame::Malformed` — which is exactly what makes them readable here without
//! teaching any fixture about environments. An absolute path is used on
//! purpose: with a cleared environment there is no `PATH` left to resolve a
//! bare command name.

#![cfg(unix)]

use meclaw_cells::stdio_child::{ChildSpec, Frame, StdioChild};

/// Read the child's whole stdout as raw lines, until EOF.
async fn env_lines(spec: &ChildSpec) -> Vec<String> {
    let mut child = StdioChild::spawn(spec).expect("spawn /usr/bin/env");
    let mut out = Vec::new();
    while let Some(frame) = child.read_frame().await.expect("read") {
        match frame {
            Frame::Malformed(raw) => out.push(raw),
            Frame::Json(v) => out.push(v.to_string()),
        }
    }
    out
}

fn env_spec(env_clear: bool) -> ChildSpec {
    ChildSpec {
        program: "/usr/bin/env".to_string(),
        env: vec![("P8_GIVEN".to_string(), "explicit".to_string())],
        kill_grace_ms: 500,
        env_clear,
        ..ChildSpec::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn env_clear_leaves_the_child_only_what_it_was_handed() {
    let lines = env_lines(&env_spec(true)).await;

    assert!(
        lines.iter().any(|l| l == "P8_GIVEN=explicit"),
        "the explicit variable must survive the clear, got: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("HOME=")),
        "an inherited variable leaked past env_clear: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("PATH=")),
        "an inherited variable leaked past env_clear: {lines:?}"
    );
}

/// The discriminating control: without the switch the same spec inherits
/// everything. Without this test the one above could pass for the wrong reason
/// (e.g. a child that gets no environment at all under any setting).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_the_switch_the_child_inherits_as_before() {
    let lines = env_lines(&env_spec(false)).await;

    assert!(
        lines.iter().any(|l| l == "P8_GIVEN=explicit"),
        "the explicit variable must be applied either way, got: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("PATH=")),
        "the inherited environment must still be there without env_clear: {lines:?}"
    );
}
