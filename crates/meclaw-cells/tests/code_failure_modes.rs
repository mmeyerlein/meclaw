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
//! 7. stderr-on-exit-0 IS persisted as a warn line in log.jsonl (GH #44)

use meclaw_cells::code::{CodeCell, CodeParams, Script};
use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::subscriber::Subscriber;
use tracing::{Event, Level, Metadata, span};

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

/// Minimal WARN-only subscriber that renders every WARN event as one
/// `name=value …` line. Adapted from `colony_config.rs::WarnCapture` and
/// `meclaw-colony/tests/paket_1_message_timeout_boot_warn.rs` — same shape, but
/// it records ALL fields so an assertion can inspect the `stderr` field, not
/// just the literal message.
struct WarnCapture {
    lines: Arc<Mutex<Vec<String>>>,
}

impl Subscriber for WarnCapture {
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        *meta.level() == Level::WARN
    }
    fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }
    fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut visitor = LineVisitor { parts: Vec::new() };
        event.record(&mut visitor);
        self.lines.lock().unwrap().push(visitor.parts.join(" "));
    }
    fn enter(&self, _: &span::Id) {}
    fn exit(&self, _: &span::Id) {}
}

struct LineVisitor {
    parts: Vec<String>,
}

impl Visit for LineVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.parts.push(format!("{}={value}", field.name()));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // `%value` (Display) and a literal message both arrive via Debug.
        self.parts.push(format!("{}={value:?}", field.name()));
    }
}

/// Drive one `handle()` call under the WARN-capturing subscriber; returns the
/// emissions plus every WARN line emitted during the call.
async fn run_capturing_warns(cell: &CodeCell) -> (Vec<meclaw_core::CellEmission>, Vec<String>) {
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let (otx, mut orx) = mpsc::channel(8);
    let sink = make_sink(otx);
    // `set_default` is thread-local, but on a multi_thread runtime the awaited
    // handle() future can migrate to another worker via work stealing and lose
    // the capture (seen as `captured: []` on the 2-core CI runner). Binding the
    // subscriber to the FUTURE keeps it across polls on every thread.
    {
        use tracing::instrument::WithSubscriber;
        cell.handle(mk_msg(), &sink)
            .with_subscriber(WarnCapture {
                lines: Arc::clone(&lines),
            })
            .await;
    }
    drop(sink);
    let mut outs = Vec::new();
    while let Some(em) = orx.recv().await {
        outs.push(em);
    }
    // Read via the lock, NOT `Arc::try_unwrap` — under parallel cargo load the
    // tracing dispatch's Arc clone can briefly outlive the guard scope
    // (colony_config.rs lesson). The Vec is fully populated synchronously.
    let captured = lines.lock().unwrap().clone();
    (outs, captured)
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
async fn stderr_on_exit_0_is_persisted_as_a_warn_line() {
    // Proves: cell-types.md § code promises that `header.had_stderr` is set
    // and the stderr content lands in `log.jsonl` at warn level (GH #44). The
    // body stays clean (test 6 above), so the warn line is the ONLY place the
    // stderr content of a successful run survives.
    let cell = CodeCell::new(CodeParams {
        runner: "python3".into(),
        script: Script::Inline(
            r#"import sys,json
print("CODE_STDERR_MARKER_ALPHA", file=sys.stderr)
sys.stdout.write(json.dumps({"messages":[{"origin":"assistant","type":"text","text":"clean output"}]}))"#.into()
        ),
        external_timeout_ms: Some(10_000),
        max_concurrency: None,
    }, false, None, false);

    let (outs, warns) = run_capturing_warns(&cell).await;

    // (a) Regression: the header flag stays as it is.
    assert_eq!(outs.len(), 1, "exactly one emission");
    assert_eq!(outs[0].content["header"]["exit_code"], 0);
    assert_eq!(
        outs[0].content["header"]["had_stderr"], true,
        "stderr produced → had_stderr must be true"
    );
    // (b) The stderr CONTENT is persisted as a warn line.
    assert!(
        warns.iter().any(|l| l.contains("CODE_STDERR_MARKER_ALPHA")),
        "stderr content must be logged at warn level; captured: {warns:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clean_exit_0_without_stderr_logs_nothing() {
    // Regression guard for the warn line above: a script that writes NO stderr
    // must not produce an empty warn line.
    let cell = CodeCell::new(CodeParams {
        runner: "python3".into(),
        script: Script::Inline(
            r#"import sys,json; sys.stdout.write(json.dumps({"messages":[{"origin":"assistant","type":"text","text":"quiet"}]}))"#.into()
        ),
        external_timeout_ms: Some(10_000),
        max_concurrency: None,
    }, false, None, false);

    let (outs, warns) = run_capturing_warns(&cell).await;

    assert_eq!(outs.len(), 1, "exactly one emission");
    assert_eq!(
        outs[0].content["header"]["had_stderr"], false,
        "no stderr → had_stderr must be false"
    );
    assert!(
        warns.is_empty(),
        "a stderr-free successful run must log no warn line; captured: {warns:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_failed_keeps_stderr_in_the_error_message_and_logs_no_warn_line() {
    // Regression: exit != 0 keeps the existing failure form (stderr in the
    // bash-sentinel block inside the error message, cell-types.md § code
    // failure model). The warn line belongs to the exit-0 path only; a
    // failing script emits an error message instead.
    let cell = CodeCell::new(
        CodeParams {
            runner: "python3".into(),
            script: Script::Inline(
                r#"import sys; print("CODE_STDERR_MARKER_BETA", file=sys.stderr); sys.exit(3)"#
                    .into(),
            ),
            external_timeout_ms: Some(10_000),
            max_concurrency: None,
        },
        false,
        None,
        false,
    );

    let (outs, warns) = run_capturing_warns(&cell).await;

    assert_eq!(outs.len(), 1, "exactly one emission");
    let h = &outs[0].content["header"];
    assert_eq!(h["error_code"], "script_failed");
    assert_eq!(h["finish_reason"], "error");
    assert_eq!(h["exit_code"], 3);
    assert_eq!(h["had_stderr"], true);
    let text = outs[0].content["messages"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("CODE_STDERR_MARKER_BETA"),
        "failure path must keep stderr in the error message; got: {text}"
    );
    assert!(
        warns.is_empty(),
        "the failure path carries stderr in the message, not in a warn line; captured: {warns:?}"
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
