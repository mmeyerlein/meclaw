//! Phase-9 code failure modes integration tests.
//!
//! Covers all five failure-modi declared in cell-types.md § code
//! (Z.229-232) plus a Happy-Path stderr-not-injected proof:
//!
//! 1. script_failed (exit ≠ 0)
//! 2. invalid_json (stdout not valid JSON)
//! 3. script_timeout (external_timeout_ms elapsed → child killed)
//! 4. io_error (runner not on PATH / spawn-fail)
//! 5. contract_violation (multi-send Array without multi_send_capable)
//! 6. stderr-on-exit-0 is NOT injected into output.text (only header had_stderr=true)

use meclaw_cells::code::{CodeCell, CodeParams, Script};
use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

fn make_sink(otx: mpsc::Sender<meclaw_core::CellEmission>) -> OutputSink {
    OutputSink::new(
        otx,
        Path::new("/code"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    )
}

fn mk_msg() -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/code"))
        .body(Body::Inline(json!({"messages":[]})))
        .reply_to(Path::new("/sink"))
        .build()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_failed_emits_script_failed_error_code() {
    // Proves: script_failed path (Z.229) — exit ≠ 0 with stderr output.
    // header.error_code = "script_failed", finish_reason = "error",
    // exit_code ≠ 0, had_stderr = true.
    let cell = CodeCell::new(
        CodeParams {
            runner: "python3".into(),
            script: Script::Inline(
                r#"import sys; print("err output", file=sys.stderr); sys.exit(1)"#.into(),
            ),
            external_timeout_ms: Some(10_000),
            max_concurrency: None,
        },
        false,
        None,
        false,
    );
    let (otx, mut orx) = mpsc::channel(8);
    let sink = make_sink(otx);
    cell.handle(mk_msg(), &sink).await;
    drop(sink);
    let em = orx.recv().await.unwrap();
    let h = &em.content["header"];
    assert_eq!(h["error_code"], "script_failed");
    assert_eq!(h["finish_reason"], "error");
    assert_ne!(h["exit_code"], 0);
    assert_eq!(h["had_stderr"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_json_emits_invalid_json_error_code() {
    // Proves: invalid_json path (Z.230) — stdout not valid JSON, exit 0.
    // header.error_code = "invalid_json", finish_reason = "error",
    // exit_code = 0 (script exited cleanly, output was unparseable).
    let cell = CodeCell::new(
        CodeParams {
            runner: "python3".into(),
            script: Script::Inline(r#"import sys; sys.stdout.write("not json")"#.into()),
            external_timeout_ms: Some(10_000),
            max_concurrency: None,
        },
        false,
        None,
        false,
    );
    let (otx, mut orx) = mpsc::channel(8);
    let sink = make_sink(otx);
    cell.handle(mk_msg(), &sink).await;
    drop(sink);
    let em = orx.recv().await.unwrap();
    let h = &em.content["header"];
    assert_eq!(h["error_code"], "invalid_json");
    assert_eq!(h["finish_reason"], "error");
    assert_eq!(h["exit_code"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_timeout_emits_script_timeout_error_code() {
    // Proves: script_timeout path (Z.231) — 100ms timeout, script sleeps 5s → killed.
    // header.error_code = "script_timeout", finish_reason = "error".
    // Elapsed < 3s proves with_killing_timeout actually kills the child.
    let cell = CodeCell::new(
        CodeParams {
            runner: "python3".into(),
            script: Script::Inline(r#"import time; time.sleep(5)"#.into()),
            external_timeout_ms: Some(100),
            max_concurrency: None,
        },
        false,
        None,
        false,
    );
    let (otx, mut orx) = mpsc::channel(8);
    let sink = make_sink(otx);
    let t0 = std::time::Instant::now();
    cell.handle(mk_msg(), &sink).await;
    let elapsed = t0.elapsed();
    drop(sink);
    let em = orx.recv().await.unwrap();
    let h = &em.content["header"];
    assert_eq!(h["error_code"], "script_timeout");
    assert_eq!(h["finish_reason"], "error");
    // Should not wait the full 5s — kill_after_timeout works.
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "script_timeout must kill child quickly, took {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn io_error_emits_io_error_when_runner_not_on_path() {
    // Proves: io_error path (Z.232) — spawn fails because runner binary does not exist.
    // Direct CodeParams construction — CodeParams::parse would reject
    // anything other than "python3". This bypasses validation to test
    // the io_error spawn-failure path in handle().
    // header.error_code = "io_error", finish_reason = "error".
    let cell = CodeCell::new(
        CodeParams {
            runner: "/nonexistent/python3".into(),
            script: Script::Inline(r#"print("never runs")"#.into()),
            external_timeout_ms: Some(10_000),
            max_concurrency: None,
        },
        false,
        None,
        false,
    );
    let (otx, mut orx) = mpsc::channel(8);
    let sink = make_sink(otx);
    cell.handle(mk_msg(), &sink).await;
    drop(sink);
    let em = orx.recv().await.unwrap();
    let h = &em.content["header"];
    assert_eq!(h["error_code"], "io_error");
    assert_eq!(h["finish_reason"], "error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contract_violation_multi_send_not_declared() {
    // Proves: contract_violation / multi_send_not_declared path — script emits a JSON
    // Array (multi-send) but cell was constructed with multi_send_capable=false.
    // Integration-test duplicate of code::cell::tests::array_without_multi_send_yields_
    // contract_violation — explicitly required by Phase-9-C7 spec.
    // header.error_code = "multi_send_not_declared", finish_reason = "error".
    let cell = CodeCell::new(CodeParams {
        runner: "python3".into(),
        script: Script::Inline(
            r#"import sys,json; sys.stdout.write(json.dumps([{"messages":[]},{"messages":[]}]))"#.into()
        ),
        external_timeout_ms: Some(10_000),
        max_concurrency: None,
    }, false, None, false); // multi_send_capable: false
    let (otx, mut orx) = mpsc::channel(8);
    let sink = make_sink(otx);
    cell.handle(mk_msg(), &sink).await;
    drop(sink);
    let em = orx.recv().await.unwrap();
    let h = &em.content["header"];
    assert_eq!(h["error_code"], "multi_send_not_declared");
    assert_eq!(h["finish_reason"], "error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stderr_on_exit_0_is_not_injected_into_output_text() {
    // Proves: cell-types.md Z.227 — stderr at Exit-0 is NOT injected into
    // output.text. It lives in log.jsonl only. Header had_stderr=true signals
    // that stderr output existed, but the sentinel string must NOT appear in
    // the serialised emission body.
    let cell = CodeCell::new(CodeParams {
        runner: "python3".into(),
        script: Script::Inline(
            r#"import sys,json
print("STDERR_SENTINEL_DO_NOT_INJECT", file=sys.stderr)
sys.stdout.write(json.dumps({"messages":[{"origin":"assistant","type":"text","text":"clean output"}]}))"#.into()
        ),
        external_timeout_ms: Some(10_000),
        max_concurrency: None,
    }, false, None, false);
    let (otx, mut orx) = mpsc::channel(8);
    let sink = make_sink(otx);
    cell.handle(mk_msg(), &sink).await;
    drop(sink);
    let em = orx.recv().await.unwrap();
    let h = &em.content["header"];
    assert_eq!(h["exit_code"], 0);
    assert_eq!(
        h["had_stderr"], true,
        "stderr produced → had_stderr must be true"
    );
    // Cell-types.md Z.227 proof: stderr-Sentinel must NOT appear in the
    // output body (unlike the script_failed path where stderr is injected).
    let body_str = meclaw_core::serde_json::to_string(&em.content).unwrap();
    assert!(
        !body_str.contains("STDERR_SENTINEL_DO_NOT_INJECT"),
        "stderr-Sentinel must NOT appear in output body on exit 0"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standard_headers_override_script_supplied_values() {
    // cell-types.md Z.220–225: Cell-Standard-Header exit_code,
    // duration_ms, had_stderr OVERRIDE die Skript-gesetzten Keys.
    // Proof via a script that sets header.exit_code=99 and
    // header.had_stderr=true itself — the cell must override both with the real
    // exit_code (0) and the actual had_stderr status (false, because the script
    // does not write to stderr).
    let cell = CodeCell::new(CodeParams {
        runner: "python3".into(),
        script: Script::Inline(
            r#"import sys,json; sys.stdout.write(json.dumps({"header":{"exit_code":99,"had_stderr":True,"custom_key":"keep_me"},"messages":[{"origin":"assistant","type":"text","text":"ok"}]}))"#.into()
        ),
        external_timeout_ms: Some(10_000),
        max_concurrency: None,
    }, false, None, false);
    let (otx, mut orx) = mpsc::channel(8);
    let sink = make_sink(otx);
    cell.handle(mk_msg(), &sink).await;
    drop(sink);
    let em = orx.recv().await.unwrap();
    let h = &em.content["header"];
    // Override proofs (the cell's values win):
    assert_eq!(
        h["exit_code"], 0,
        "exit_code must be the cell value (0), not the script's 99"
    );
    assert_eq!(
        h["had_stderr"], false,
        "had_stderr must be the cell value (false), not the script's true"
    );
    // Side-Beweis: andere Skript-gesetzte Header-Keys bleiben erhalten.
    assert_eq!(
        h["custom_key"], "keep_me",
        "non-standard script headers stay unchanged"
    );
    // duration_ms is set (numeric).
    assert!(
        h["duration_ms"].is_i64() || h["duration_ms"].is_u64(),
        "duration_ms must be set by the cell"
    );
}
