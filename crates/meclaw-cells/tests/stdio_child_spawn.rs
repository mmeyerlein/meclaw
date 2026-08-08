//! P7 block 1 — the stdio-child core's spawn/framing layer, driven against a
//! real child process (the `line_json_test_server` fixture binary).
//!
//! Step 1.1 pins the D2 assumption from `plans/p7-stdio-child-core.md`: cargo
//! auto-discovers `src/bin/*.rs` (autobins) and exports the resulting path to
//! integration tests of the same package as `CARGO_BIN_EXE_<name>`. If that
//! assumption were wrong, this file would not compile — which is exactly the
//! falsification the plan asks for.

use meclaw_cells::stdio_child::{ChildSpec, StdioChild};

/// Path to the fixture child process. Resolved at compile time by cargo.
const FIXTURE: &str = env!("CARGO_BIN_EXE_line_json_test_server");

/// A spec for the fixture in the given mode, with a short kill grace so the
/// timeout paths stay fast.
fn fixture_spec(mode: &str) -> ChildSpec {
    ChildSpec {
        program: FIXTURE.to_string(),
        args: vec![mode.to_string()],
        env: Vec::new(),
        cwd: None,
        kill_grace_ms: 500,
    }
}

#[test]
fn fixture_binary_exists_and_is_executable() {
    let path = std::path::Path::new(FIXTURE);
    assert!(path.is_file(), "fixture binary missing at {FIXTURE}");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .expect("stat fixture")
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "fixture binary is not executable: mode {mode:o}"
        );
    }
}

#[tokio::test]
async fn spawn_starts_the_child_and_exposes_its_pid() {
    let child = StdioChild::spawn(&fixture_spec("echo")).expect("spawn fixture");
    let pid = child.pid().expect("child has a pid while running");
    assert!(pid > 1, "implausible pid {pid}");

    // /proc proves the process really exists (Linux dev/CI target).
    #[cfg(target_os = "linux")]
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "no /proc entry for freshly spawned pid {pid}"
    );
}

#[tokio::test]
async fn spawn_reports_a_missing_program_as_spawn_error() {
    let mut spec = fixture_spec("echo");
    spec.program = "/nonexistent/line_json_test_server".to_string();
    let err = match StdioChild::spawn(&spec) {
        Ok(_) => panic!("missing program must fail"),
        Err(e) => e,
    };
    assert!(
        err.detail().starts_with("spawn failed:"),
        "unexpected detail: {}",
        err.detail()
    );
}

#[tokio::test]
async fn frames_round_trip_through_the_child_one_line_each() {
    use meclaw_cells::stdio_child::Frame;
    use serde_json::json;

    let t = std::time::Duration::from_secs(5);
    let mut child = StdioChild::spawn(&fixture_spec("echo")).expect("spawn fixture");

    child
        .write_frame(&json!({"n": 1}), t)
        .await
        .expect("write 1");
    child
        .write_frame(&json!({"n": 2}), t)
        .await
        .expect("write 2");

    for expected in 1..=2 {
        match child.read_frame().await.expect("read") {
            Some(Frame::Json(v)) => assert_eq!(v["n"], expected),
            other => panic!("expected json frame {expected}, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn request_once_returns_the_matching_answer_and_skips_foreign_frames() {
    use serde_json::json;

    let t = std::time::Duration::from_secs(5);
    let mut child = StdioChild::spawn(&fixture_spec("echo")).expect("spawn fixture");

    // Park a foreign answer in the stream first: the echo fixture will emit it
    // before the answer request_once is waiting for.
    child
        .write_frame(&json!({"id": "a"}), t)
        .await
        .expect("write foreign");

    let answer = child
        .request_once(&json!({"id": "b", "v": 7}), |v| v["id"] == "b", t)
        .await
        .expect("request_once");
    assert_eq!(answer["v"], 7, "wrong frame correlated: {answer}");
}

#[tokio::test]
async fn request_once_times_out_against_a_silent_child() {
    use serde_json::json;

    let mut child = StdioChild::spawn(&fixture_spec("hang")).expect("spawn fixture");
    let err = match child
        .request_once(
            &json!({"id": "x"}),
            |v| v["id"] == "x",
            std::time::Duration::from_millis(200),
        )
        .await
    {
        Ok(v) => panic!("silent child must not answer, got {v}"),
        Err(e) => e,
    };
    assert_eq!(err.detail(), "timeout");
}

#[tokio::test]
async fn terminate_lets_a_well_behaved_child_exit_on_stdin_close() {
    use meclaw_cells::stdio_child::ChildExit;

    let child = StdioChild::spawn(&fixture_spec("echo")).expect("spawn fixture");
    let exit = child.terminate(std::time::Duration::from_secs(5)).await;
    assert_eq!(
        exit,
        ChildExit::Code(0),
        "echo must end cleanly on stdin EOF"
    );
}

#[tokio::test]
async fn terminate_kills_a_hanging_child_after_the_grace_and_reaps_it() {
    use meclaw_cells::stdio_child::ChildExit;

    let child = StdioChild::spawn(&fixture_spec("hang")).expect("spawn fixture");
    let pid = child.pid().expect("pid");

    // Semantic timing discriminator, kept tight on purpose: the grace must
    // actually elapse (>= 200ms) and the kill must follow promptly (< 5s).
    let started = std::time::Instant::now();
    let exit = child.terminate(std::time::Duration::from_millis(200)).await;
    let elapsed = started.elapsed();

    assert_eq!(exit, ChildExit::Signal, "a hanging child must be signalled");
    assert!(
        elapsed >= std::time::Duration::from_millis(200),
        "grace was skipped: {elapsed:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "kill took too long: {elapsed:?}"
    );

    #[cfg(target_os = "linux")]
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "pid {pid} still present after terminate -- not reaped"
    );
    let _ = pid;
}
