//! Track T (#104) — fitness battery for the `bash` cell.
//!
//! Coding pipelines stand on this cell running builds and test suites, so the
//! battery pins the load-bearing edges of its contract (`docs/cell-types.md`
//! § bash, phase-7 slice-2 conventions):
//!
//! - exit codes are DATA, not errors: `exit != 0` is a normal `tool_result`
//!   with `header.exit_code`, and signal-death is the `-1` convention;
//! - stderr travels inside `text` behind the sentinel markers, flagged by the
//!   mandatory `had_stderr` header;
//! - the operation timeout (rule 12, concept A) is a typed error that ends the
//!   command instead of hanging the round;
//! - one-shot only: no cwd/env state survives between two calls;
//! - a `params.sandbox` deny is a real denial (probe-gated like
//!   `sandbox_isolation.rs` — the capability is proven, not assumed);
//! - stdout volume passes through uncut at coding-relevant sizes.

#[path = "support_fitness.rs"]
mod support;

use meclaw_cells::BashCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;
use std::sync::Arc;
use support::{ToolRig, assert_error, assert_normal_result, header_of, text_of};

fn rig_with(params: meclaw_core::JsonValue) -> ToolRig {
    ToolRig::spawn(
        Arc::new(BashCellFactory) as Arc<dyn CellFactory>,
        "/bash",
        params,
    )
}

fn rig() -> ToolRig {
    rig_with(json!({"max_concurrency": 2, "external_timeout_ms": 20000}))
}

// ---------------------------------------------------------------- exit codes

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exit_zero_is_a_normal_result_with_full_header_set() {
    let mut r = rig();
    let em = r.call(json!({"command": "echo hello"}), "c1").await;

    assert_normal_result(&em, "c1");
    assert_eq!(text_of(&em), "hello\n");
    assert_eq!(header_of(&em, "operation"), "bash");
    assert_eq!(header_of(&em, "exit_code"), 0);
    assert_eq!(header_of(&em, "had_stderr"), false);
    assert_eq!(header_of(&em, "bytes"), 6, "bytes = len of text");
    assert!(
        header_of(&em, "duration_ms").is_u64(),
        "duration_ms is mandatory"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_nonzero_exit_is_a_normal_result_carrying_the_exact_code() {
    // A red test suite is exit 1, a missing binary 127 — the brain reads the
    // code and decides. An error_code here would push every failing build onto
    // the error lane and away from the tool round.
    let mut r = rig();
    let em = r.call(json!({"command": "exit 7"}), "c1").await;

    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "exit_code"), 7);

    let em = r
        .call(json!({"command": "definitely-no-such-cmd-424242"}), "c2")
        .await;
    assert_normal_result(&em, "c2");
    assert_eq!(
        header_of(&em, "exit_code"),
        127,
        "command-not-found is the shell's 127, a result, not a cell error"
    );
    assert_eq!(header_of(&em, "had_stderr"), true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signal_killed_command_reports_the_minus_one_convention() {
    let mut r = rig();
    let em = r.call(json!({"command": "kill -9 $$"}), "c1").await;

    assert_normal_result(&em, "c1");
    assert_eq!(
        header_of(&em, "exit_code"),
        -1,
        "signal death is the documented -1 convention, not an error"
    );
}

// -------------------------------------------------------------------- stderr

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stderr_arrives_behind_the_sentinel_markers_byte_exact() {
    let mut r = rig();
    let em = r
        .call(json!({"command": "echo out; echo err >&2"}), "c1")
        .await;

    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "had_stderr"), true);
    // The exact sentinel layout is contract: an LLM consumer parses THIS form.
    assert_eq!(
        text_of(&em),
        "out\n\n##meclaw-stderr-start##\nerr\n##meclaw-stderr-end##"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_compiler_style_stderr_only_failure_keeps_stdout_and_stderr_separable() {
    // Exit != 0 with empty stdout and a multi-line stderr — the shape of every
    // failing build. stdout must stay empty in front of the marker.
    let mut r = rig();
    let em = r
        .call(
            json!({"command": "echo 'error: line one' >&2; echo 'error: line two' >&2; exit 2"}),
            "c1",
        )
        .await;

    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "exit_code"), 2);
    assert_eq!(header_of(&em, "had_stderr"), true);
    assert_eq!(
        text_of(&em),
        "\n##meclaw-stderr-start##\nerror: line one\nerror: line two\n##meclaw-stderr-end##"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clean_stdout_carries_no_sentinel_at_all() {
    let mut r = rig();
    let em = r.call(json!({"command": "printf clean"}), "c1").await;
    assert!(
        !text_of(&em).contains("##meclaw-stderr"),
        "no stderr, no marker: {:?}",
        text_of(&em)
    );
    assert_eq!(header_of(&em, "had_stderr"), false);
}

// ------------------------------------------------------------------- timeout

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_operation_timeout_is_a_typed_error_not_a_hang() {
    // Rule 12 concept A: the cell's own timeout fires first and produces a
    // regular error message — the substrate backstop never has to kill the cell.
    let mut r = rig_with(json!({"max_concurrency": 1, "external_timeout_ms": 300}));
    let started = std::time::Instant::now();
    let em = r
        .call(json!({"command": "sleep 30; echo late"}), "c1")
        .await;

    assert_error(&em, "timeout");
    assert_eq!(
        header_of(&em, "had_stderr"),
        false,
        "error paths always set had_stderr"
    );
    // Spec conformance (cell-types.md § bash, phase-7 conventions): the -1
    // bullet covers every killed/abnormal termination, and a timeout IS a
    // kill, and the spec adds error_code: timeout on top. The header must
    // carry both, so an agent routing on exit_code never sees a hole.
    assert_eq!(
        header_of(&em, "exit_code"),
        -1,
        "a timed-out (killed) command reports the -1 convention"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "the child was killed at the timeout, not awaited"
    );
    assert!(
        !text_of(&em).contains("late"),
        "no partial output leaks from a killed command"
    );
}

// ------------------------------------------------------------------ one-shot

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_shot_semantics_no_env_or_cwd_survives_between_calls() {
    // bash is one-shot by ruling (2026-06-08): every call is a fresh shell.
    // An agent that exports a variable or cd's must carry that state itself.
    let mut r = rig_with(json!({"max_concurrency": 1, "external_timeout_ms": 20000}));

    let first = r
        .call(
            json!({"command": "cd /tmp && export FITNESS_T104=set && echo $FITNESS_T104:$PWD"}),
            "c1",
        )
        .await;
    assert_eq!(
        text_of(&first),
        "set:/tmp\n",
        "the state exists IN call one"
    );

    let second = r
        .call(json!({"command": "echo ${FITNESS_T104:-unset}:$PWD"}), "c2")
        .await;
    let out = text_of(&second);
    assert!(
        out.starts_with("unset:"),
        "the export did not survive the call boundary: {out:?}"
    );
    assert!(
        !out.trim_end().ends_with(":/tmp"),
        "the cwd did not survive the call boundary: {out:?}"
    );
}

// ------------------------------------------------------------- input hygiene

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_empty_or_unparsable_command_is_invalid_input() {
    let mut r = rig();

    let em = r.call(json!({}), "c1").await;
    assert_error(&em, "invalid_input");

    let em = r.call(json!({"command": ""}), "c2").await;
    assert_error(&em, "invalid_input");

    let em = r.call_raw_text("not json at all", "c3").await;
    assert_error(&em, "invalid_input");
}

// ------------------------------------------------------------- stdout volume

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_64_kib_stdout_passes_through_uncut() {
    // Build logs and test output are routinely tens of KiB. 64 KiB also pins
    // that full pipes cannot deadlock the child (the cell reads both pipes
    // concurrently). Deliberately below any plausible future cap (GH #83
    // discusses caps for web_fetch; a bash sibling would default >= 256 KiB).
    let mut r = rig();
    let em = r
        .call(
            json!({"command": "head -c 65536 /dev/zero | tr '\\0' 'a'"}),
            "c1",
        )
        .await;

    assert_normal_result(&em, "c1");
    let text = text_of(&em);
    assert_eq!(text.len(), 65536, "nothing was cut");
    assert!(text.bytes().all(|b| b == b'a'), "and nothing was mangled");
    assert_eq!(header_of(&em, "bytes"), 65536);
    assert!(
        header_of(&em, "truncated").is_null(),
        "an uncut body carries no truncated marker"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_utf8_stdout_is_delivered_lossy_not_fatal() {
    // A stray binary byte in build output must not kill the round. The cell
    // converts lossily (U+FFFD), which is the documented from_utf8_lossy path.
    let mut r = rig();
    let em = r.call(json!({"command": "printf 'a\\377b'"}), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(text_of(&em), "a\u{FFFD}b");
}

// ------------------------------------------------------------------- sandbox

/// Probe-gated like `sandbox_isolation.rs`: a missing kernel capability skips
/// visibly instead of failing (a red test on an old kernel hardens nobody).
fn have_landlock(test: &str) -> bool {
    match meclaw_cells::sandbox::landlock_abi() {
        Some(abi) => {
            eprintln!("[{test}] landlock abi {abi}");
            true
        }
        None => {
            eprintln!("[{test}] SKIPPED: no Landlock on this kernel (needs Linux 5.13+)");
            false
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restricted_sandbox_denies_reads_outside_the_allowlist() {
    const T: &str = "fitness_bash::a_restricted_sandbox_denies_reads_outside_the_allowlist";
    if !have_landlock(T) {
        return;
    }
    let allowed = tempfile::TempDir::new().unwrap();
    let forbidden = tempfile::TempDir::new().unwrap();
    std::fs::write(allowed.path().join("ok.txt"), b"ok").unwrap();
    std::fs::write(forbidden.path().join("secret.txt"), b"CONTENT_T104_DENY").unwrap();

    let mut r = rig_with(json!({
        "max_concurrency": 1,
        "external_timeout_ms": 20000,
        "sandbox": {
            "trust": "restricted",
            "network": "allow",
            "filesystem": {"read": [allowed.path().to_str().unwrap()]}
        }
    }));

    // Control first: inside the allowlist the same operation succeeds —
    // without it a broken profile would "prove" isolation by failing.
    let ok = r
        .call(
            json!({"command": format!("cat {}", allowed.path().join("ok.txt").display())}),
            "c1",
        )
        .await;
    assert_normal_result(&ok, "c1");
    assert_eq!(header_of(&ok, "exit_code"), 0);
    assert_eq!(text_of(&ok), "ok");

    let denied = r
        .call(
            json!({"command": format!("cat {}", forbidden.path().join("secret.txt").display())}),
            "c2",
        )
        .await;
    assert_normal_result(&denied, "c2");
    assert_ne!(
        header_of(&denied, "exit_code"),
        0,
        "the read outside the allowlist must fail"
    );
    let t = text_of(&denied);
    assert!(
        t.contains("Permission denied") && !t.contains("CONTENT_T104_DENY"),
        "denied loudly, no CONTENT leaked (the path may appear in the shell error): {t:?}"
    );
}
