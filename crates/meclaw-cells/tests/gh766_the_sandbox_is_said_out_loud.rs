//! GH #766 (wave G, T10) — the cap, the refusal, and the sentence about a package.
//!
//! Two boundaries, and this cell owns exactly one of them. The browser's own
//! sandbox belongs to the package it came from: meclaw adds no flag, removes
//! none, and ships no switch to say otherwise (R-G11). What `params.sandbox`
//! does here is the CELL's ceiling — a cgroup cap, and nothing else (R-G13).
//!
//! Six arms:
//!
//! (a) a sixth key in the block is a boot error, and no knob stands beside it;
//! (b) the shipped shape reaches `SandboxProfile` unchanged, and carries no path;
//! (c) it is the third shape — the same document with `syscalls` is refused;
//! (d) `--no-sandbox` is absent from the flags, and no document brings it in;
//! (e) a browser that dies before it answers is `spawn_failed` with a detail
//!     that names a package, never `browser_crashed`;
//! (f) an OOM kill is `browser_crashed` with `oom_kill=<n>` and the knob's name.

use meclaw_cells::browser::{Browser, BrowserParams, cdp};
use meclaw_cells::sandbox::{NetworkPolicy, SandboxProfile};
use meclaw_core::serde_json::{Value, json};
use std::time::Duration;

/// The browser double. Started through the same shim a real browser is.
const FIXTURE: &str = env!("CARGO_BIN_EXE_cdp_browser_fixture");

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

/// The sandbox block `templates/browser/config.json` ships.
fn shipped_cap() -> Value {
    json!({
        "trust": "restricted",
        "network": "allow",
        "limits": {"memory_max_bytes": 2_000_000_000u64, "cpu_max_percent": 200, "pids_max": 512}
    })
}

fn params(sandbox: Value, extra_args: Vec<String>) -> Result<BrowserParams, String> {
    let mut args = vec!["--fixture-mode=own-namespace".to_string()];
    args.extend(extra_args);
    BrowserParams::parse(&json!({
        "chromium_path": FIXTURE,
        "extra_args": args,
        "sandbox": sandbox,
    }))
}

#[test]
fn a_sixth_key_is_a_boot_error_and_no_knob_stands_beside_it() {
    let mut sixth = shipped_cap();
    sixth["seatbelt"] = json!(true);
    let e = params(sixth, vec![]).expect_err("(a) the key set is closed");
    assert!(e.contains("seatbelt"), "{e}");
    // And the struck knob has no way back in, under any name.
    for knob in ["sandbox_mode", "no_sandbox", "allow_no_sandbox"] {
        let doc = json!({"chromium_path": FIXTURE, knob: true});
        let e = BrowserParams::parse(&doc).expect_err("there is no second way to say it");
        assert!(e.contains(knob), "{e}");
    }
}

#[test]
fn the_shipped_shape_is_a_cap_and_carries_no_path() {
    let p = params(shipped_cap(), vec![]).expect("(b) the shipped shape parses");
    match p.sandbox.expect("the block reaches the cell") {
        SandboxProfile::Restricted {
            network,
            filesystem,
            limits,
            syscalls,
        } => {
            assert_eq!(network, NetworkPolicy::Allow);
            assert!(
                filesystem.is_none(),
                "no view, so no path of anybody's machine travels in a template"
            );
            assert!(syscalls.is_none(), "and no filter");
            let caps = limits.expect("the cap IS the profile");
            assert_eq!(caps.memory_max_bytes, Some(2_000_000_000));
            assert_eq!(caps.cpu_max_percent, Some(200));
            assert_eq!(caps.pids_max, Some(512));
        }
        other => panic!("expected a restricted profile, got {other:?}"),
    }
}

#[test]
fn the_same_document_with_a_filter_is_refused() {
    let mut with_filter = shipped_cap();
    with_filter["syscalls"] = json!({"ptrace": "deny"});
    let e = params(with_filter, vec![]).expect_err("(c) a filter needs a view");
    assert!(
        e.contains("params.sandbox.filesystem") && e.contains("syscalls"),
        "the refusal says which key made the view mandatory: {e}"
    );
}

#[test]
fn no_document_puts_no_sandbox_on_the_command_line() {
    // (d) Not in the flags the cell writes …
    let p = params(shipped_cap(), vec![]).expect("params");
    let flags = cdp::flags(&p, std::path::Path::new("/tmp/profile"));
    assert!(!flags.iter().any(|f| f.contains("sandbox")), "{flags:?}");
    // … and not through the one door an operator has.
    for attempt in ["--no-sandbox", "--no-sandbox=1"] {
        let e = params(shipped_cap(), vec![attempt.to_string()])
            .expect_err("the struck knob does not come back as a flag");
        assert!(
            e.contains("--no-sandbox") && e.contains("package"),
            "and the refusal says whose the sandbox is: {e}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_browser_that_never_answered_is_a_spawn_that_failed() {
    // (e) The fixture leaves before it answers anything, with 133 and "No
    // usable sandbox" on its stderr — the shape a packaged browser takes where
    // its own sandbox cannot run. The whole arm is the CELL's startup and not
    // the pipe: on the pipe a death is `browser_crashed`, and `Browser::ready`
    // is the one place that turns it into `spawn_failed`, because an operator
    // sent after `browser_crashed` goes looking for a page that never existed.
    // The name of this test is an ADR anchor (ADR-0044), so the assert has to
    // be the sentence the name makes.
    let td = tempfile::TempDir::new().expect("tempdir");
    let p = BrowserParams::parse(&json!({
        "chromium_path": FIXTURE,
        "extra_args": ["--fixture-mode=stillborn"],
        "startup_timeout_ms": 20_000,
        "sandbox": {"trust": "trusted"},
    }))
    .expect("params");
    let (mut browser, _events) =
        Browser::start(p, td.path().join("profile")).expect("the process starts");
    let e = tokio::time::timeout(MARKER, browser.ready())
        .await
        .expect("a dead browser does not wait out the startup timeout")
        .expect_err("it never answered");
    assert_eq!(
        e.error_code(),
        "spawn_failed",
        "a browser that never answered is a spawn that failed, not a crash: {}",
        e.detail()
    );
    assert!(
        e.detail().contains("package"),
        "and the sentence says where a browser comes from: {}",
        e.detail()
    );
    assert!(
        e.detail().contains("133"),
        "with the number an operator can look up, not just the fact: {}",
        e.detail()
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[test]
fn an_oom_kill_is_said_with_its_number_and_its_knob() {
    // (f) The cap's verdict, read from the cgroup the browser sat in. A test
    // that really ran a browser out of memory would measure the kernel; what
    // has to be right here is that the number reaches the sentence.
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(
        td.path().join("memory.events"),
        "low 0\nhigh 7\nmax 88\noom 4\noom_kill 1\n",
    )
    .expect("events");
    let site = cdp::CapSite::at(td.path().to_path_buf());
    let said = cdp::death_detail(Some(&site), "the browser's pipe closed");
    assert!(said.contains("oom_kill=1"), "{said}");
    assert!(
        said.contains("memory_max_bytes"),
        "and it names the knob that did it, not just the fact: {said}"
    );
    // A death that was not the cap's says nothing extra, and a host without
    // the file is not a verdict either.
    std::fs::write(td.path().join("memory.events"), "oom_kill 0\n").expect("events");
    assert_eq!(
        cdp::death_detail(Some(&site), "killed by signal"),
        "killed by signal"
    );
    assert_eq!(
        cdp::death_detail(None, "killed by signal"),
        "killed by signal"
    );
}
