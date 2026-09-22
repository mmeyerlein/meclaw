//! GH #772 — a refusal after the browser has started ends the browser.
//!
//! `ready()` can refuse a browser that is already running: here its renderers
//! never leave the cell's user namespace (fixture mode `same-namespace`), which
//! is what a browser whose own sandbox does not hold looks like from outside.
//! The cell then parks and refuses for as long as the handler lives — and the
//! browser used to live exactly that long with it. The proof is the one the
//! teardown test makes, through the pid's own number, taken WHILE the handler
//! is still there: the cell is refusing, and the browser is gone.

use meclaw_cells::browser::{BrowserEvent, BrowserIo, BrowserParams, io};
use meclaw_colony::SurfaceRegistry;
use meclaw_core::Path;
use meclaw_core::serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// The browser double. Started through the same shim a real browser is.
const FIXTURE: &str = env!("CARGO_BIN_EXE_cdp_browser_fixture");

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

/// Wait until `f` says yes, or fail with `what`.
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + MARKER;
    while !f() {
        assert!(std::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refusal_after_the_start_ends_the_browser() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let pid_file = td.path().join("browser.pid");
    let profile = td.path().join("meclaw-browser-fixture");
    let params = BrowserParams::parse(&json!({
        "mount": "browser",
        "chromium_path": FIXTURE,
        "user_data_dir": profile.to_str().expect("a path"),
        "extra_args": [
            "--fixture-mode=same-namespace",
            format!("--fixture-pid-file={}", pid_file.display()),
        ],
        // The sandbox verdict is what refuses here; no ceiling is asked for, so
        // the test needs no cgroup delegation (the reference host has none).
        "sandbox": {"trust": "trusted"},
        // The watch for a sandboxed child is min(5 s, this): one second is
        // plenty for a double that forks at once, and it keeps the test short.
        "external_timeout_ms": 1000,
    }))
    .expect("params");

    let surfaces = Arc::new(SurfaceRegistry::new());
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(16);
    let (commands_tx, commands_rx) = tokio::sync::mpsc::channel(16);
    let half = tokio::spawn(io::run_io(
        BrowserIo::new(
            params,
            profile,
            Path::new("/browser"),
            Arc::clone(&surfaces),
        ),
        events_tx,
        commands_rx,
    ));

    until("the browser never started", || pid_file.is_file()).await;
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .expect("the pid")
        .trim()
        .parse()
        .expect("a number");
    let alive = |pid: u32| std::path::Path::new(&format!("/proc/{pid}")).exists();
    assert!(alive(pid), "the control: the browser really is running");

    // The refusal arrives as the cell's own event.
    let refusal = loop {
        match tokio::time::timeout(MARKER, events_rx.recv()).await {
            Ok(Some(BrowserEvent::Failed(e))) => break e,
            Ok(Some(_)) => continue,
            Ok(None) => panic!("the events channel closed before a refusal"),
            Err(_) => panic!("no refusal within the marker window"),
        }
    };
    assert_eq!(refusal.error_code(), "spawn_failed", "{refusal:?}");

    // The handler is still here — `commands_tx` lives — and the browser is not.
    // That is the whole finding: before GH #772 this waited the marker out.
    //
    // The leak reading comes BEFORE the wording below on purpose, so that a
    // regression names the leak instead of a sentence. It also covers a class
    // the wording structurally cannot see: `terminate` signals but does not
    // reap, so a zombie keeps its `/proc` entry while the detail still reads
    // `was ended before this refusal` — only this line falls then.
    until("the refused browser outlived the refusal", || !alive(pid)).await;

    let detail = refusal.detail();
    assert!(
        detail.contains(&format!("pid {pid}")) && detail.contains("was ended before this refusal"),
        "the refusal says what happened to the process: {detail}"
    );
    assert!(
        !half.is_finished(),
        "the half keeps refusing while the handler lives"
    );

    // And the ordinary end still ends the half.
    drop(commands_tx);
    tokio::time::timeout(MARKER, half)
        .await
        .expect("the half ends when the handler does")
        .expect("and without panicking");
}
