//! GH #766 (wave G, g5) — the three claims only a real browser can settle.
//!
//! Every one of these went past a green test suite, because the test double
//! next door is more generous than a browser is: it announces
//! `Target.targetInfoChanged` after every navigation whether anybody asked or
//! not, and it sends screencast frames on a timer whether the page moved or
//! not. A packaged Chromium does neither. So the arms below run against the
//! browser named in `MECLAW_CHROMIUM` and SKIP when there is none — a cheap
//! arm that only asks the fixture would agree with the fixture again.
//!
//! Measured on 2026-09-19 against Chromium 153.0.8010.36 (snap), over the same
//! `--remote-debugging-pipe` the cell uses:
//!
//! * with no discovery enabled, `Target.targetInfoChanged` arrives **not once**
//!   — and with `Target.setDiscoverTargets` it arrives at navigation START,
//!   carrying the URL in the `title` field, and never again with the settled
//!   one. `Target.getTargetInfo` after `Page.loadEventFired` answers with the
//!   real title and the full address including the fragment;
//! * `Page.startScreencast` yields exactly ONE frame and then nothing while the
//!   page stands still; a second `startScreencast` without a stop is refused
//!   (`-32000 Screencast is already active`); `stopScreencast` +
//!   `startScreencast` yields one frame again — the keyframe a viewer who
//!   joined late needs;
//! * a browser child that is killed closes the pipe, and the I/O half used to
//!   answer that by RETURNING, which the substrate reads as `DeathKind::Normal`
//!   and removes the cell from the registry instead of restarting it.
//!
//! Arms:
//!
//! (a) B-G1 — after the load, the report the CELL sends carries the page's own
//!     title and its content type (driven through `io::run_io`, fix round 2);
//! (b) B-G3 — a second viewer joining a page that stands still gets a picture;
//! (c) B-G2 — a killed browser child does not end the I/O half quietly;
//! (d) B-G8 — the profile directory goes when the cell does;
//! (e) B-G2, the other half — a `Died` event makes the handler panic, which is
//!     the only exit this substrate restarts a cell on (`mcp/io.rs:96`).

use meclaw_cells::browser::{
    Browser, BrowserCell, BrowserCommand, BrowserEvent, BrowserIo, BrowserParams, CdpEvent,
    PageReport, io,
};
use meclaw_colony::{LinkFrame, LongRunningCell, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{OriginSink, Path};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The browser double, for the one arm that does not need a browser.
const FIXTURE: &str = env!("CARGO_BIN_EXE_cdp_browser_fixture");

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

/// A page that stands perfectly still and says what it is called.
///
/// A `data:` URL rather than a file: a snap-confined browser has its own
/// private `/tmp`, so a file this test writes is a file that browser cannot
/// read (BUILD-PREAMBLE § 3, measured 18.09.).
const STILL: &str = "data:text/html,<title>the page that says its name</title><body style='margin:0'>\
     <h1>a page that stands still</h1></body>";

/// What that page is called.
const STILL_TITLE: &str = "the page that says its name";

/// The browser this host has, or nothing.
fn packaged() -> Option<String> {
    match std::env::var("MECLAW_CHROMIUM") {
        Ok(p) if !p.trim().is_empty() => Some(p),
        _ => None,
    }
}

/// Params for the packaged browser.
///
/// `sandbox.trust: trusted` and not the cell's own cgroup cap: moving a child
/// into a sub-cgroup needs a delegated slice, which a test process run from a
/// shell does not have (B-G10). What is measured here is the browser's own
/// confinement, which `Browser::ready` checks either way.
fn real_params(chromium: &str, profile: &std::path::Path) -> BrowserParams {
    BrowserParams::parse(&json!({
        "mount": "browser",
        "chromium_path": chromium,
        "user_data_dir": profile.to_str().expect("a path"),
        "sandbox": {"trust": "trusted"},
    }))
    .expect("the packaged browser's params parse")
}

/// A profile path of this test's own, under `/tmp` because that is the one
/// place a snap-confined browser may write (BUILD-PREAMBLE § 3).
fn own_profile(tag: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!(
        "/tmp/meclaw-g5-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ))
}

/// A SECOND COPY of the event loop, for the arms that need `&mut Browser`.
///
/// It exists because arm (b) drives `join` and reads the viewer's link, and
/// neither is reachable through `io::run_io` without a socket and a surface
/// registry. It is not the loop the cell runs, and the difference is the whole
/// lesson of fix round 2: while `run_io`'s `Page.loadEventFired` arm called
/// only `note_content_type`, this copy already called `after_load`, so arm (a)
/// was green against a cell that never asked the browser for a title (review
/// C1). Any claim about WHAT THE CELL DOES belongs on `run_io`, never here.
///
/// Where it still differs from `run_io`, deliberately: it has no `commands`,
/// `links` or suspend arm at all, and it acknowledges frames whenever 50 ms
/// pass without one instead of at `next_ack()` — a faster pace than the cell's,
/// which is why nothing about pacing may be measured through it either.
async fn pump(
    browser: &mut Browser,
    events: &mut tokio::sync::mpsc::Receiver<CdpEvent>,
    for_: Duration,
) -> Vec<PageReport> {
    let deadline = Instant::now() + for_;
    let mut out = Vec::new();
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(50), events.recv()).await {
            Ok(Some(event)) if event.method == "Page.screencastFrame" => {
                out.extend(browser.on_frame(&event).await);
            }
            Ok(Some(event)) if event.method == "Page.loadEventFired" => {
                let page = event
                    .session_id
                    .as_deref()
                    .and_then(|s| browser.register.page_of_session(s));
                if let Some(page) = page {
                    out.extend(browser.after_load(&page).await);
                }
            }
            Ok(Some(event)) => {
                if let Some(BrowserEvent::Page(report)) = browser.on_event(event) {
                    out.push(report);
                }
            }
            Err(_) => browser.flush_acks().await,
            Ok(None) => panic!("the browser's pipe closed"),
        }
    }
    out
}

/// How many pictures reached this viewer since the last look.
fn pictures(from_cell: &mut tokio::sync::mpsc::Receiver<LinkFrame>) -> usize {
    let mut n = 0;
    while let Ok(frame) = from_cell.try_recv() {
        if matches!(frame, LinkFrame::Binary(_)) {
            n += 1;
        }
    }
    n
}

/// The pid of the browser that owns `profile`, by its own `--user-data-dir`.
///
/// Never a name pattern: this host runs the owner's browsers and other waves'
/// drivers, and a pattern would find them (BUILD-PREAMBLE § 4). Two things
/// have to match, not one: a packaged browser forks zygotes, a gpu process and
/// a handful of utilities, and every one of them carries the same
/// `--user-data-dir` — killing one of those leaves the pipe open and measures
/// nothing. Every one of them also carries `--type=<what it is>`, and the one
/// process the cell actually speaks to is the one WITHOUT it.
fn pid_of_profile(profile: &std::path::Path) -> Option<u32> {
    let needle = profile
        .file_name()
        .and_then(|n| n.to_str())
        .expect("the profile has a name")
        .to_string();
    for entry in std::fs::read_dir("/proc").ok()? {
        let Ok(entry) = entry else { continue };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let cmdline = String::from_utf8_lossy(&raw);
        let args: Vec<&str> = cmdline.split('\0').collect();
        if args.iter().any(|arg| arg.contains(&needle))
            && !args.iter().any(|arg| arg.starts_with("--type="))
        {
            return Some(pid);
        }
    }
    None
}

/// Every process that carries this profile, with what it says it is.
/// Diagnostic only: it is what a missing pid prints instead of nothing.
fn candidates(profile: &std::path::Path) -> Vec<String> {
    let needle = profile
        .file_name()
        .and_then(|n| n.to_str())
        .expect("the profile has a name")
        .to_string();
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in dir.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let cmdline = String::from_utf8_lossy(&raw);
        let args: Vec<&str> = cmdline.split('\0').collect();
        if args.iter().any(|arg| arg.contains(&needle)) {
            let kind: Vec<&str> = args
                .iter()
                .filter(|a| a.starts_with("--type="))
                .copied()
                .collect();
            out.push(format!(
                "{pid} {kind:?} argv0={}",
                args.first().unwrap_or(&"")
            ));
        }
    }
    out
}

/// Whether a pid still exists.
fn alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_packaged_browser_answers_for_itself() {
    let Some(chromium) = packaged() else {
        eprintln!(
            "SKIP: no MECLAW_CHROMIUM — the three defects of this file are only \
             visible at a packaged browser, and a fixture would agree with itself"
        );
        a_died_event_panics_the_handler().await;
        return;
    };

    // ------------------------------------------------------------------ (a)
    // Through `io::run_io` and NOT through the copy of its loop below: the
    // wiring is the claim. Fix round 2 (review C1) — the first cut of this arm
    // ran `pump`, which calls `after_load` where `run_io` called only
    // `note_content_type`, so it went green against a cell that never asked
    // the browser for a title. A test that rebuilds the event loop cannot see
    // a line missing from the real one.
    let profile = own_profile("title");
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(64);
    let (commands_tx, commands_rx) = tokio::sync::mpsc::channel(16);
    let mut half = tokio::spawn(io::run_io(
        BrowserIo::new(
            real_params(&chromium, &profile),
            profile.clone(),
            Path::new("/browser"),
            Arc::clone(&surfaces),
        ),
        events_tx,
        commands_rx,
    ));
    let (answer, opened) = tokio::sync::oneshot::channel();
    commands_tx
        .send(BrowserCommand::Open {
            page: "card-1".to_string(),
            url: STILL.to_string(),
            context: "default".to_string(),
            viewport: None,
            answer,
        })
        .await
        .expect("the half takes the verb");
    tokio::time::timeout(MARKER, opened)
        .await
        .expect("within the failure-marker window")
        .expect("the half answers")
        .expect("the page opens");
    // What the handler hears, on the channel the handler holds. The load is
    // the only thing that produces this emission, so a window and not a wait
    // on the next event: a cell that never asks says nothing at all.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen: Vec<(String, String)> = Vec::new();
    let named = loop {
        if Instant::now() >= deadline {
            break None;
        }
        match tokio::time::timeout(Duration::from_millis(200), events_rx.recv()).await {
            Ok(Some(BrowserEvent::Page(report))) => {
                seen.push((report.title.clone(), report.content_type.clone()));
                if !report.title.is_empty() {
                    break Some(report);
                }
            }
            Ok(Some(BrowserEvent::Crashed { report })) => {
                panic!("(a) the page's renderer crashed: {}", report.row.page)
            }
            Ok(Some(BrowserEvent::Failed(e) | BrowserEvent::Died(e))) => {
                panic!("(a) the half gave up: {} {}", e.error_code(), e.detail())
            }
            Ok(None) => panic!("(a) the I/O half went away"),
            Err(_) => {}
        }
    };
    let report = named.unwrap_or_else(|| {
        panic!(
            "(a) B-G1: every `page` emission the CELL sent carried an empty title, and \
             G3/G4/G7 read their proof back through it; (title, content_type) seen: {seen:?}"
        )
    });
    assert_eq!(
        report.title, STILL_TITLE,
        "(a) and it is the page's own title"
    );
    assert_eq!(
        report.content_type, "text/html",
        "(a) and the other half of `after_load` is still there: the PDF anchor of \
         the app on the other end hangs on this value, and no test but this one \
         asks the CELL for it"
    );
    drop(commands_tx);
    tokio::time::timeout(MARKER, &mut half)
        .await
        .expect("the half ends when the handler goes")
        .expect("and it ends without panicking");

    // ------------------------------------------------------------------ (b)
    let profile = own_profile("keyframe");
    let (mut browser, mut events) =
        Browser::start(real_params(&chromium, &profile), profile.clone())
            .expect("the packaged browser starts");
    tokio::time::timeout(MARKER, browser.ready())
        .await
        .expect("within the failure-marker window")
        .expect("the packaged browser's renderers leave this user namespace");
    browser
        .in_open("card-1", STILL, "default", None)
        .await
        .expect("the page opens");
    // Let the load finish before the first viewer joins. Arm (a) used to do
    // this for arm (b) as a side effect of sharing its browser; now that (a)
    // runs the real half on a browser of its own, the wait is explicit — a
    // screencast started on a page that has not painted yet produces its first
    // frame late enough to miss a three-second window.
    pump(&mut browser, &mut events, Duration::from_secs(4)).await;
    let mut first = browser
        .join(
            "card-1",
            &json!({"viewport": {"width": 800, "height": 600, "dpr": 1.0}}),
        )
        .await
        .expect("the first viewer is admitted");
    pump(&mut browser, &mut events, Duration::from_secs(3)).await;
    assert!(
        pictures(&mut first.link.from_cell) >= 1,
        "the control: whoever joins while the picture starts does get one"
    );
    // The page stands still, so the browser sends nothing. That is the state a
    // second output joins in — the ordinary one, because all outputs join or
    // none do and they do not join in the same millisecond.
    pump(&mut browser, &mut events, Duration::from_secs(2)).await;
    assert_eq!(
        pictures(&mut first.link.from_cell),
        0,
        "the control: a still page sends no frames of its own"
    );
    let mut second = browser
        .join(
            "card-1",
            &json!({"viewport": {"width": 800, "height": 600, "dpr": 1.0}}),
        )
        .await
        .expect("the second viewer is admitted");
    pump(&mut browser, &mut events, Duration::from_secs(4)).await;
    assert!(
        pictures(&mut second.link.from_cell) >= 1,
        "(b) B-G3: a viewer that joins a page which is not moving gets no \
         keyframe, so G2/G5/G11/G13 and F10 measure a dead canvas"
    );
    browser.reaper.terminate(Duration::from_millis(2_000)).await;

    // ------------------------------------------------------------- (c) + (d)
    let profile = own_profile("death");
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(32);
    let (commands_tx, commands_rx) = tokio::sync::mpsc::channel(16);
    let mut half = tokio::spawn(io::run_io(
        BrowserIo::new(
            real_params(&chromium, &profile),
            profile.clone(),
            Path::new("/browser"),
            Arc::clone(&surfaces),
        ),
        events_tx,
        commands_rx,
    ));
    // A page first, and its answer awaited: that is what says the browser is
    // UP. Killing it while `Browser::ready` is still running would measure a
    // browser that never started, which is `park` and a different claim — and
    // it is the setting OR-G17 is about, a cell that holds a row it can reopen.
    let (answer, opened) = tokio::sync::oneshot::channel();
    commands_tx
        .send(BrowserCommand::Open {
            page: "card-1".to_string(),
            url: STILL.to_string(),
            context: "default".to_string(),
            viewport: None,
            answer,
        })
        .await
        .expect("the half takes the verb");
    tokio::time::timeout(MARKER, opened)
        .await
        .expect("within the failure-marker window")
        .expect("the half answers")
        .expect("the page opens");
    let deadline = Instant::now() + MARKER;
    let pid = loop {
        if let Some(pid) = pid_of_profile(&profile) {
            break pid;
        }
        assert!(
            Instant::now() < deadline,
            "the browser that answered has a pid; profile {} ; candidates {:?}",
            profile.display(),
            candidates(&profile)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    // The child dies the way G8 kills it: its own number, and nothing else.
    // Never a name pattern — this host runs the owner's live browsers.
    let killed = std::process::Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .status()
        .expect("kill runs");
    assert!(killed.success(), "the child was killed by its own number");
    // Page reports of the load that just finished may still be in the queue;
    // what is measured is the first thing the half says about the DEATH.
    let deadline = Instant::now() + MARKER;
    let said = loop {
        let next = tokio::time::timeout(Duration::from_secs(2), events_rx.recv()).await;
        match next {
            Ok(Some(BrowserEvent::Page(_))) => {}
            Ok(Some(other)) => break Some(other),
            Ok(None) => break None,
            Err(_) => {
                assert!(
                    Instant::now() < deadline,
                    "(c) B-G2: the half said nothing at all about a browser that died"
                );
            }
        }
    };
    assert!(
        matches!(said, Some(BrowserEvent::Died(_))),
        "(c) B-G2: the death of the child reached the handler as `Failed`, \
         which it emits and then carries on from — so the I/O half returned, \
         the substrate read `DeathKind::Normal` and logged `cell ended \
         normally, removing from registry`; OR-G17 can never be shown"
    );
    let still_running = tokio::time::timeout(Duration::from_secs(1), &mut half)
        .await
        .is_err();
    assert!(
        still_running,
        "(c) and the half does not return on its own: a voluntary return is \
         the `Normal` death that removes the cell instead of restarting it"
    );

    // (d) the profile goes with the cell, on the path an abort takes too.
    half.abort();
    let _ = (&mut half).await;
    drop(commands_tx);
    let deadline = Instant::now() + MARKER;
    while profile.exists() {
        assert!(
            Instant::now() < deadline,
            "(d) B-G8: `{}` outlived the cell — it stood there empty after the \
             colony went, because a snap-confined browser writes its profile \
             into its own private /tmp and only the (empty) directory this \
             cell made is left on the host",
            profile.display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let deadline = Instant::now() + MARKER;
    while alive(pid) {
        assert!(Instant::now() < deadline, "the child outlived the cell");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // (e) needs no browser at all, and it runs last so that the arms which
    // need one are what a red run reports first.
    a_died_event_panics_the_handler().await;
}

/// (e) The handler's half of B-G2, at the double: `Died` is the one event this
/// cell answers with a panic.
///
/// A panic is not a flourish here, it is the signal: `spawn_watcher` classifies
/// a clean return as `DeathKind::Normal` → `registry.remove`, and only `Panic`
/// (or the B-backstop, which a `cell.timeout: -1` cell does not have) reaches
/// `one_for_one`. The `mcp` cell writes the same sentence at `mcp/io.rs:96`.
async fn a_died_event_panics_the_handler() {
    let params = BrowserParams::parse(&json!({
        "mount": "browser",
        "chromium_path": FIXTURE,
        "sandbox": {"trust": "trusted"},
    }))
    .expect("the fixture params parse");
    let td = tempfile::TempDir::new().expect("tempdir");
    let surfaces = Arc::new(SurfaceRegistry::new());
    let io = BrowserIo::new(
        params.clone(),
        td.path().join("profile"),
        Path::new("/browser"),
        Arc::clone(&surfaces),
    );
    let mut cell = BrowserCell::new(Path::new("/browser"), params, io);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(8);
    let sink = OriginSink::new(out_tx, Path::new("/browser"), 8);
    let mut db = meclaw_colony::DbConn::wrap(
        rusqlite::Connection::open_in_memory().expect("an in-memory cell.db"),
        None,
    );
    let died = BrowserEvent::Died(meclaw_cells::browser::BrowserError::BrowserCrashed(
        "the browser's pipe closed".to_string(),
    ));
    let joined = tokio::spawn(async move {
        cell.handle_event(died, &sink, &mut db).await;
    })
    .await;
    let e = joined.expect_err("(e) a browser that died is not an ordinary event");
    assert!(
        e.is_panic(),
        "(e) and the exit is a PANIC, because that is the only one the \
         supervisor restarts a long-running cell on"
    );
    let emission = out_rx.try_recv().expect("(e) and the error goes out first");
    let content = emission.content;
    assert_eq!(
        content.get("error_code").and_then(Value::as_str),
        Some("browser_crashed"),
        "(e) the app hears WHY before the cell goes: {content}"
    );
}

/// The same two defects at the double, once the double stopped being generous.
///
/// It is worth having next to the real arm, and it is NOT what proved the
/// defects: the fixture used to announce `Target.targetInfoChanged` after
/// every navigation and to send frames on a timer forever, so both claims
/// below were green while the browser next door was red. `--fixture-still`
/// and `Target.getTargetInfo` are what make it a double of a browser rather
/// than of the cell's hopes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_still_double_shows_the_same_two_defects() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let params = BrowserParams::parse(&json!({
        "mount": "browser",
        "chromium_path": FIXTURE,
        "extra_args": ["--fixture-mode=own-namespace", "--fixture-still"],
        "sandbox": {"trust": "trusted"},
    }))
    .expect("the fixture params parse");
    let (mut browser, mut events) =
        Browser::start(params, td.path().join("profile")).expect("the double starts");
    tokio::time::timeout(MARKER, browser.ready())
        .await
        .expect("within the failure-marker window")
        .expect("the double's child leaves this namespace");
    browser
        .in_open(
            "card-1",
            "https://example.com/seat#s-2298971",
            "default",
            None,
        )
        .await
        .expect("the page opens");
    pump(&mut browser, &mut events, Duration::from_secs(2)).await;
    assert_eq!(
        browser
            .register
            .report("card-1")
            .expect("the page is held")
            .row
            .url,
        "https://example.com/seat#s-2298971",
        "the address keeps its fragment (OR-G39)"
    );

    let mut first = browser
        .join(
            "card-1",
            &json!({"viewport": {"width": 960, "height": 600}}),
        )
        .await
        .expect("the first viewer");
    pump(&mut browser, &mut events, Duration::from_secs(2)).await;
    assert!(
        pictures(&mut first.link.from_cell) >= 1,
        "the control: the first viewer gets the picture the start produced"
    );
    pump(&mut browser, &mut events, Duration::from_secs(1)).await;
    assert_eq!(
        pictures(&mut first.link.from_cell),
        0,
        "the control: a page that stands still sends nothing more"
    );
    let mut second = browser
        .join(
            "card-1",
            &json!({"viewport": {"width": 960, "height": 600}}),
        )
        .await
        .expect("the second viewer");
    pump(&mut browser, &mut events, Duration::from_secs(2)).await;
    assert!(
        pictures(&mut second.link.from_cell) >= 1,
        "B-G3: whoever joins a page that is not moving gets a keyframe"
    );
    browser.reaper.terminate(Duration::from_millis(2_000)).await;
}

/// B-G8 — an empty profile is gone the MOMENT the cell lets go of it.
///
/// The arm in the browser test above does not reproduce the finding, and that
/// is itself a measurement: there the runtime lives on, so the removal that
/// `ProfileDir` handed to `spawn_blocking` got a thread to run on. A colony
/// going down does not give it one — `cell_task_long_running` ABORTS the I/O
/// half and the process leaves — and that is why
/// `/tmp/meclaw-browser-welle-g` and `/tmp/meclaw-browser-noexe` were both
/// still standing after the proof run of 18.09.
///
/// They stood there EMPTY, which is the other half of the finding: a
/// snap-confined browser has a private `/tmp`, so the directory the host sees
/// is the one this cell made and never the one the browser filled — measured
/// 19.09., the `--user-data-dir` in the browser's own `/proc/<pid>/cmdline` is
/// not the path the cell passed. An empty directory is one `rmdir`, and one
/// `rmdir` fits inside a `Drop`.
///
/// So this is what the fix claims, and all it claims: on a runtime thread,
/// where the old code deferred, the directory is gone when `Drop` returns. No
/// sleep and no second task — anything a later task has to finish is a thing
/// an abort can take away.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_profile_is_gone_the_moment_the_cell_lets_go() {
    let profile = own_profile("drop");
    std::fs::create_dir_all(&profile).expect("the cell makes its profile");
    assert!(profile.is_dir(), "the control");
    drop(meclaw_cells::browser::ProfileDir::at(profile.clone()));
    assert!(
        !profile.exists(),
        "B-G8: `{}` was still there the instant the cell let go — the removal \
         was handed to a task, and a teardown that aborts this half never runs \
         it",
        profile.display()
    );
}
