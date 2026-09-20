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
        _ => None,
    }
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

/// Every page this cell has said `closed` about so far.
fn closed_so_far(events: &mut mpsc::Receiver<BrowserEvent>, seen: &mut Vec<String>) {
    while let Ok(event) = events.try_recv() {
        if let BrowserEvent::Page(report) = event
            && report.state == PageState::Closed
        {
            seen.push(report.row.page.clone());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_open_is_given_up_to_make_room() {
    let Some(chromium) = packaged() else {
        eprintln!("SKIP: no MECLAW_CHROMIUM — only a browser mints the targets a ceiling counts");
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
    let mut closed = Vec::new();
    closed_so_far(&mut events_rx, &mut closed);
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

    drop(commands_tx);
    tokio::time::timeout(MARKER, &mut half)
        .await
        .expect("the half ends when the handler goes")
        .expect("and it ends without panicking");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_oldest_sleeping_page_is_the_one_that_makes_room() {
    let Some(chromium) = packaged() else {
        eprintln!("SKIP: no MECLAW_CHROMIUM — only a browser mints the targets a ceiling counts");
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
    let mut closed = Vec::new();
    closed_so_far(&mut events_rx, &mut closed);
    assert_eq!(
        closed,
        vec!["card-1".to_string()],
        "B-G21: exactly one page made room, it was the oldest sleeping one, and \
         the cell said so out loud (OR-G7)"
    );

    drop(commands_tx);
    tokio::time::timeout(MARKER, &mut half)
        .await
        .expect("the half ends when the handler goes")
        .expect("and it ends without panicking");
}
