//! GH #766 (wave G, T9) — after the cell, nothing of the browser is left.
//!
//! Four things go with a browser cell, and the ORDER at the end of the I/O half
//! is the decision: the child first, so nothing writes into the profile while
//! it is being removed; the profile next; the mount last, so a join arriving
//! during the teardown is refused by this cell rather than falling through to
//! the listener's 404.
//!
//! Three arms:
//!
//! (a) the child's pid does not exist any more — and it is ended through ITS
//!     OWN NUMBER, never a name pattern: this host runs other people's
//!     browsers and a pattern would find them;
//! (b) the profile directory is gone, and it was `params.user_data_dir` and not
//!     the cell directory, which the No-Delete policy protects;
//! (c) the mount is free, and the next life takes the same name without a
//!     collision.

use meclaw_cells::browser::{BrowserIo, BrowserParams, io};
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
async fn nothing_of_the_browser_outlives_the_cell() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let pid_file = td.path().join("browser.pid");
    let profile = td.path().join("meclaw-browser-fixture");
    let params = BrowserParams::parse(&json!({
        "mount": "browser",
        "chromium_path": FIXTURE,
        "user_data_dir": profile.to_str().expect("a path"),
        "extra_args": [
            "--fixture-mode=own-namespace",
            format!("--fixture-pid-file={}", pid_file.display()),
        ],
        // Required since OR-G54. A test double is not a browser, so it
        // declares the escape hatch rather than a cgroup cap the host may not
        // be able to delegate.
        "sandbox": {"trust": "trusted"},
    }))
    .expect("params");

    let surfaces = Arc::new(SurfaceRegistry::new());
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(16);
    let (commands_tx, commands_rx) = tokio::sync::mpsc::channel(16);
    let half = tokio::spawn(io::run_io(
        BrowserIo::new(
            params.clone(),
            profile.clone(),
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
    // The mount is held: a second cell cannot take the name while this one has
    // it, which is what makes (c) below a real assertion rather than a tautology.
    let taken = surfaces
        .register(
            "browser",
            meclaw_colony::SurfaceEntry {
                kind: "browser",
                cell_path: Path::new("/other"),
                links: None,
            },
        )
        .await;
    assert!(taken.is_err(), "the mount is this cell's while it lives");

    // The handler goes away. That is the only thing that ends the half.
    drop(commands_tx);
    tokio::time::timeout(MARKER, half)
        .await
        .expect("the half ends when the handler does")
        .expect("and without panicking");

    // (a) through its own number.
    until("the child outlived the cell", || !alive(pid)).await;
    // (b) and the profile went with it.
    until("the profile directory outlived the cell", || {
        !profile.exists()
    })
    .await;
    assert!(td.path().is_dir(), "and only the profile: not its parent");

    // (c) the next life takes the same name.
    let deadline = std::time::Instant::now() + MARKER;
    loop {
        if surfaces
            .register(
                "browser",
                meclaw_colony::SurfaceEntry {
                    kind: "browser",
                    cell_path: Path::new("/other"),
                    links: None,
                },
            )
            .await
            .is_ok()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the mount was never given back"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
