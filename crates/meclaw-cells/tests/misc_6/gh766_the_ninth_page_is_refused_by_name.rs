//! GH #766 (wave G, g8) — the page over the ceiling is refused by name.
//!
//! **B-G21** (`plans/welle-g-2026-09-15/berichte/g4.md` § Beweis, dritter Lauf):
//! marker G10 read ONE `receipt too_many_pages` — the ceiling is reachable
//! since g7's `newWindow` fix — and **eight** pages closed in the same moment,
//! against a gate of zero.
//!
//! Contract § 2 and OR-G7 close over exactly two sentences here, and this file
//! measures both at the seam the substrate drives:
//!
//! * above `max_pages` the answer is a `receipt` that says `too_many_pages`;
//! * the ONLY page that may be given up to make room is the oldest
//!   **suspended** one — never a page somebody could be looking at, and never
//!   in silence: the displacement is emitted as a `page` report of its own.
//!
//! The arms need a real browser and SKIP without one (OR-G.g5.4): only a
//! browser mints the targets a ceiling counts.

use meclaw_cells::browser::{
    BrowserCommand, BrowserEvent, BrowserIo, BrowserParams, PageState, io,
};
use meclaw_colony::SurfaceRegistry;
use meclaw_core::Path;
use meclaw_core::serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

/// A page that stands still, because a ceiling is not about motion.
const STILL: &str = "data:text/html,<title>still</title><body style='margin:0'>still</body>";

fn packaged() -> Option<String> {
    match std::env::var("MECLAW_CHROMIUM") {
        Ok(p) if !p.trim().is_empty() => Some(p),
        _ => playwright_chromium(),
    }
}

/// OR-TB.S10.B.3 (GH #1189): `browser:display` books the Playwright bundle its
/// probe started and names no path; without MECLAW_CHROMIUM the unit takes the
/// newest `chromium_headless_shell-*` under PLAYWRIGHT_BROWSERS_PATH (else
/// `~/.cache/ms-playwright`), as the CDP drivers' `findBrowser` does.
fn playwright_chromium() -> Option<String> {
    let root = match std::env::var_os("PLAYWRIGHT_BROWSERS_PATH") {
        Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
        _ => std::path::PathBuf::from(std::env::var_os("HOME")?).join(".cache/ms-playwright"),
    };
    let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("chromium_headless_shell-")
        })
        .map(|e| {
            e.path()
                .join("chrome-headless-shell-linux64/chrome-headless-shell")
        })
        .filter(|p| p.is_file())
        .collect();
    found.sort();
    found.pop().map(|p| p.to_string_lossy().into_owned())
}

fn own_profile(tag: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!(
        "/tmp/meclaw-g8-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ))
}

/// Ask for one page and say what came back: `opened` or the error's own word.
async fn open(commands: &mpsc::Sender<BrowserCommand>, page: &str) -> String {
    let (answer, opened) = tokio::sync::oneshot::channel();
    commands
        .send(BrowserCommand::Open {
            page: page.to_string(),
            url: STILL.to_string(),
            context: "default".to_string(),
            viewport: None,
            answer,
        })
        .await
        .expect("the half takes the verb");
    match tokio::time::timeout(MARKER, opened)
        .await
        .expect("within the failure-marker window")
        .expect("the half answers")
    {
        Ok(_) => "opened".to_string(),
        Err(e) => e.error_code().to_string(),
    }
}

/// Every page this cell said `closed` about, from its first event to its last.
///
/// The half answers `open` first and sends its out-of-band reports (the
/// displacement's `closed` among them) right after, so a look at what is
/// already queued when the answer lands races the half (seen red on a lane in
/// GH #1189). The one point after which no report can follow is the half's
/// end: dropping the handler's side ends it, and its event channel closes with
/// it. So read until that close, under the failure-marker window — then an
/// expected `closed` cannot be missed and an unexpected one cannot hide.
async fn closed_by_the_end(
    commands: mpsc::Sender<BrowserCommand>,
    events: &mut mpsc::Receiver<BrowserEvent>,
    half: &mut tokio::task::JoinHandle<()>,
) -> Vec<String> {
    drop(commands);
    let deadline = tokio::time::Instant::now() + MARKER;
    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect(
                "the half closes its event channel within the failure-marker \
                 window once the handler goes",
            );
        match event {
            Some(BrowserEvent::Page(report)) if report.state == PageState::Closed => {
                seen.push(report.row.page.clone());
            }
            Some(_) => {}
            None => break,
        }
    }
    tokio::time::timeout(MARKER, half)
        .await
        .expect("the half ends when the handler goes")
        .expect("and it ends without panicking");
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_open_is_given_up_to_make_room() {
    let Some(chromium) = packaged() else {
        eprintln!("SKIP: no MECLAW_CHROMIUM — only a browser mints the targets a ceiling counts");
        assert_ne!(
            std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(),
            Ok("1"),
            "SKIP under the switch: no MECLAW_CHROMIUM"
        );
        return;
    };
    let profile = own_profile("ceiling-open");
    let params = BrowserParams::parse(&json!({
        "mount": "browser-g8-ceiling-open",
        "chromium_path": chromium,
        "user_data_dir": profile.to_str().expect("a path"),
        "max_pages": 2,
        // Never suspend: then there is nothing this cell is allowed to give
        // up, and the only lawful answer to the third page is the receipt.
        "suspend_after_ms": 0,
        "sandbox": {"trust": "trusted"},
    }))
    .expect("the packaged browser's params parse");
    let (events_tx, mut events_rx) = mpsc::channel(256);
    let (commands_tx, commands_rx) = mpsc::channel(16);
    let mut half = tokio::spawn(io::run_io(
        BrowserIo::new(
            params,
            profile.clone(),
            Path::new("/browser"),
            Arc::new(SurfaceRegistry::new()),
        ),
        events_tx,
        commands_rx,
    ));

    let mut codes = Vec::new();
    for page in ["card-1", "card-2", "card-3", "card-4"] {
        codes.push(open(&commands_tx, page).await);
    }
    let closed = closed_by_the_end(commands_tx, &mut events_rx, &mut half).await;
    assert_eq!(
        codes,
        vec!["opened", "opened", "too_many_pages", "too_many_pages"],
        "B-G21: above `max_pages` the cell answers in its own word, every time"
    );
    assert!(
        closed.is_empty(),
        "B-G21: pages were given up to make room although none was suspended — \
         closed: {closed:?} (OR-G7: never a silent displacement of an open page)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_oldest_sleeping_page_is_the_one_that_makes_room() {
    let Some(chromium) = packaged() else {
        eprintln!("SKIP: no MECLAW_CHROMIUM — only a browser mints the targets a ceiling counts");
        assert_ne!(
            std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref(),
            Ok("1"),
            "SKIP under the switch: no MECLAW_CHROMIUM"
        );
        return;
    };
    let profile = own_profile("ceiling-sleep");
    let params = BrowserParams::parse(&json!({
        "mount": "browser-g8-ceiling-sleep",
        "chromium_path": chromium,
        "user_data_dir": profile.to_str().expect("a path"),
        "max_pages": 2,
        "suspend_after_ms": 1500,
        "sandbox": {"trust": "trusted"},
    }))
    .expect("the packaged browser's params parse");
    let (events_tx, mut events_rx) = mpsc::channel(256);
    let (commands_tx, commands_rx) = mpsc::channel(16);
    let mut half = tokio::spawn(io::run_io(
        BrowserIo::new(
            params,
            profile.clone(),
            Path::new("/browser"),
            Arc::new(SurfaceRegistry::new()),
        ),
        events_tx,
        commands_rx,
    ));

    // A second between the two, so that "the oldest" has a meaning no tie can
    // take away: nobody watches either, so card-1 falls asleep a second before
    // card-2 does.
    assert_eq!(open(&commands_tx, "card-1").await, "opened");
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(open(&commands_tx, "card-2").await, "opened");
    tokio::time::sleep(Duration::from_millis(3000)).await;

    assert_eq!(
        open(&commands_tx, "card-3").await,
        "opened",
        "B-G21: with two sleeping pages there IS room, and the third page takes it"
    );
    let closed = closed_by_the_end(commands_tx, &mut events_rx, &mut half).await;
    assert_eq!(
        closed,
        vec!["card-1".to_string()],
        "B-G21: exactly one page made room, it was the oldest sleeping one, and \
         the cell said so out loud (OR-G7)"
    );
}
