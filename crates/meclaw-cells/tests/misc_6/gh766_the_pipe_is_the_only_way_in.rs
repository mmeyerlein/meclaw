//! GH #766 (wave G, T4) — CDP over a pipe, with no port and no new crate.
//!
//! The ruling this file measures (OR-G5): a browser is reached over
//! `--remote-debugging-pipe`, through a shell shim that puts its two
//! descriptors on fd 3 and fd 4, and the WebSocket on a loopback port is not an
//! option — it would open a port and need a dependency, and this substrate adds
//! neither. The shim is the whole risk in that sentence, so the fixture checks
//! the descriptors before it answers a single call and says what it found.
//!
//! Five arms:
//!
//! (a) the flags are the measured ones, in order, and none of them is a sandbox
//!     flag (R-G11);
//! (b) the browser answers over the pipe, and the answer says `3=pipe 4=pipe`;
//! (c) a call nobody answers ends as `cdp_timeout` and names its method — the
//!     A-timeout, which is the only one there is for a long-running cell;
//! (d) a browser that dies fails what was waiting instead of leaving it hanging;
//! (e) the child goes when its pipes do;
//! (f) an event nobody is listening for does not take the answers with it.

use meclaw_cells::browser::{BrowserParams, cdp};
use meclaw_cells::stdio_child::ChildExit;
use meclaw_core::serde_json::json;
use std::time::Duration;

/// The browser double. Started through the same shim a real browser is.
const FIXTURE: &str = env!("CARGO_BIN_EXE_cdp_browser_fixture");

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

/// Params pointing at the fixture, in `mode`, plus whatever `extra` adds.
fn params(mode: &str, extra: meclaw_core::JsonValue) -> BrowserParams {
    let mut v = json!({
        "chromium_path": FIXTURE,
        "extra_args": [format!("--fixture-mode={mode}")],
        // `params.sandbox` is required since OR-G54: a cell whose cap was
        // simply left out used to get an uncapped browser. A test double is
        // not a browser, so it declares the escape hatch rather than a cgroup
        // cap the host may not be able to delegate.
        "sandbox": {"trust": "trusted"},
    });
    if let Some(obj) = extra.as_object() {
        for (k, val) in obj {
            v.as_object_mut()
                .expect("an object")
                .insert(k.clone(), val.clone());
        }
    }
    BrowserParams::parse(&v).expect("the fixture params parse")
}

/// A profile directory that goes away with the test.
fn profile(td: &tempfile::TempDir) -> std::path::PathBuf {
    td.path().join("profile")
}

#[test]
fn the_shim_puts_the_pipe_on_three_and_four() {
    let p = params("ok", json!({}));
    let flags = cdp::flags(&p, std::path::Path::new("/tmp/profile"));
    let head: Vec<&str> = flags.iter().take(11).map(String::as_str).collect();
    assert_eq!(
        head,
        vec![
            "--headless=new",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            "--hide-scrollbars",
            "--mute-audio",
            "--disable-extensions",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-sync",
            "--metrics-recording-only",
        ],
        "the measured flags, in the measured order"
    );
    assert_eq!(flags[11], "--user-data-dir=/tmp/profile");
    assert_eq!(flags[12], "--disk-cache-dir=/tmp/profile/cache");
    assert_eq!(
        flags[13], "--remote-debugging-pipe",
        "the transport is the pipe, and there is no port anywhere"
    );
    assert_eq!(
        flags[14], "--fixture-mode=ok",
        "the operator's flags come after the cell's, never instead of them"
    );
    assert!(
        !flags.iter().any(|f| f.contains("sandbox")),
        "not one sandbox flag: the browser's sandbox is its package's (R-G11): {flags:?}"
    );
    let spec = cdp::child_spec(&p, std::path::Path::new("/tmp/profile"));
    assert_eq!(spec.program, "/bin/sh");
    assert_eq!(spec.args[1], cdp::FD_SHIM);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_browser_answers_over_the_pipe() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (pipe, _events, reaper) =
        cdp::spawn_browser(&params("ok", json!({})), &profile(&td)).expect("the browser starts");
    let version = tokio::time::timeout(MARKER, pipe.call("Browser.getVersion", json!({}), None))
        .await
        .expect("within the failure-marker window")
        .expect("the browser answers");
    assert_eq!(
        version["fixtureFds"], "3=pipe 4=pipe",
        "the shim put the pipes where CDP expects them — this is the measurement \
         OR-G5 rests on: {version}"
    );
    assert_eq!(version["protocolVersion"], "1.3");
    // A second call proves the mux hands out ids rather than reusing one.
    let context = pipe
        .call("Target.createBrowserContext", json!({}), None)
        .await
        .expect("a context");
    assert!(
        context["browserContextId"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{context}"
    );
    reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_nobody_answers_is_a_timeout_and_not_a_hang() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (pipe, _events, reaper) = cdp::spawn_browser(
        &params("mute", json!({"external_timeout_ms": 400})),
        &profile(&td),
    )
    .expect("the browser starts");
    let started = std::time::Instant::now();
    let e = tokio::time::timeout(MARKER, pipe.call("Browser.getVersion", json!({}), None))
        .await
        .expect("the call ends on its own, which is the whole point")
        .expect_err("nobody answered");
    assert_eq!(e.error_code(), "cdp_timeout");
    assert!(
        e.detail().contains("Browser.getVersion"),
        "the refusal names its method: {}",
        e.detail()
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "and it ends at the A-timeout, not at some backstop: {:?}",
        started.elapsed()
    );
    reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_browser_that_dies_fails_what_was_waiting() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (pipe, _events, reaper) = cdp::spawn_browser(
        &params("die-after-first", json!({"external_timeout_ms": 20_000})),
        &profile(&td),
    )
    .expect("the browser starts");
    pipe.call("Browser.getVersion", json!({}), None)
        .await
        .expect("the one answer this browser gives");
    let e = tokio::time::timeout(
        MARKER,
        pipe.call("Target.createBrowserContext", json!({}), None),
    )
    .await
    .expect("a dead browser does not leave a call hanging for its A-timeout")
    .expect_err("the browser is gone");
    assert_eq!(
        e.error_code(),
        "browser_crashed",
        "a death is a death, not a slow answer: {}",
        e.detail()
    );
    reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_child_goes_when_the_pipes_do() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (pipe, events, mut reaper) =
        cdp::spawn_browser(&params("ok", json!({})), &profile(&td)).expect("the browser starts");
    pipe.call("Browser.getVersion", json!({}), None)
        .await
        .expect("it is up");
    let pid = reaper.pid().expect("the child is running");
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "the control: it really is there"
    );
    // And NOTHING else. There used to be a `terminate` on the next line, which
    // sends SIGTERM and then SIGKILL — so the arm was green whether or not the
    // pipes had anything to do with it. What is measured is the child leaving
    // ON ITS OWN, so the only thing that happens here is the pipes going.
    drop(pipe);
    drop(events);
    let deadline = std::time::Instant::now() + MARKER;
    let exit = loop {
        if let Some(exit) = reaper.exited() {
            break exit;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the child outlived its pipes, and nothing signalled it"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        matches!(exit, ChildExit::Code(0)),
        "it left on stdin's EOF, not on a signal: {exit:?}"
    );
    assert!(
        profile(&td).is_dir(),
        "the profile directory is the cell's to remove (T9), not the pipe's"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_nobody_hears_does_not_take_the_answers_with_it() {
    // (f) The lock on the T7 substrate fix. The mux used to RETURN when the
    // event channel was closed, and every outstanding and every later call
    // then failed with "the browser died" — about a browser that was fine.
    // The only test of that commit held the receiver, which is exactly the
    // condition under which the defect cannot appear.
    let td = tempfile::TempDir::new().expect("tempdir");
    let (pipe, events, reaper) =
        cdp::spawn_browser(&params("ok", json!({})), &profile(&td)).expect("the browser starts");
    pipe.call("Browser.getVersion", json!({}), None)
        .await
        .expect("it is up");
    // Nobody is listening for events any more.
    drop(events);
    // And now make the browser produce some: a navigation is followed by
    // `Target.targetInfoChanged` and `Page.loadEventFired`.
    let session = {
        let context = pipe
            .call("Target.createBrowserContext", json!({}), None)
            .await
            .expect("a context");
        let target = pipe
            .call(
                "Target.createTarget",
                json!({"url": "about:blank", "browserContextId": context["browserContextId"]}),
                None,
            )
            .await
            .expect("a target");
        let attached = pipe
            .call(
                "Target.attachToTarget",
                json!({"targetId": target["targetId"], "flatten": true}),
                None,
            )
            .await
            .expect("a session");
        attached["sessionId"]
            .as_str()
            .expect("a session id")
            .to_string()
    };
    pipe.call(
        "Page.navigate",
        json!({"url": "https://example.com/"}),
        Some(&session),
    )
    .await
    .expect("the navigation itself");

    // Three more, after the events that had nowhere to go.
    for n in 0..3 {
        pipe.call("Browser.getVersion", json!({}), None)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "call {n} after a discarded event: {} — the browser is fine, \
                     and a mux that ended over a closed event channel is not",
                    e.detail()
                )
            });
    }
    reaper.terminate(Duration::from_millis(500)).await;
}
