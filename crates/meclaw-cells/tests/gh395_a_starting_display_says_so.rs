//! GH #395 — "not ready yet" and "no such route" stop being the same answer.
//!
//! A `web` cell starts as two halves. The I/O half binds the port and answers
//! as soon as its task runs; the handler half builds the page map in `on_start`.
//! The bind does not wait for that publish, so there is a window in which the
//! display is reachable and answers `404` for a page its own seed declares. It
//! is small and it closes on its own — and it was observed about one run in
//! three on an 8-core box, with the cell installed into a running colony, where
//! the window is widest.
//!
//! Why it needed fixing rather than tolerating: `404` from this cell is a
//! **meaningful** answer. The `pages` table is the only route source (R-W8-3),
//! so `404` means "no such route" — and for a moment after boot it also meant
//! "not ready". Nothing on the wire told them apart, and a reverse proxy in
//! front (R-W8-2) could not either, so a health check that fired early marked a
//! healthy display broken.
//!
//! WHY THIS TEST DRIVES `run_io` DIRECTLY
//! ======================================
//! The defect is a race, and a test that raced it back would be the flake it is
//! meant to remove. `WebIo::new` and `run_io` are both public, so the readiness
//! channel can be held at `false` for as long as the assertion needs and then
//! released — the window becomes a thing the test opens and closes rather than
//! something it hopes to catch. What is under test is the listener's answer,
//! which is exactly the half that owns the status code.
//!
//! The A1′ contract is untouched by the fix and by this file: the I/O half still
//! binds first and still never returns voluntarily while live.

use meclaw_cells::web::io::{WebIo, run_io};
use meclaw_cells::web::render::PageMap;
use meclaw_cells::web::{Asset, AssetMap};
use meclaw_testing::free_port;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

/// GET until the listener answers at all, or the deadline passes.
///
/// Deliberately returns the FIRST response the listener gives, whatever its
/// status: this file is about which status that is, so a helper that retried
/// until it liked the answer would assert nothing.
async fn get_once_up(url: &str) -> reqwest::Response {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match reqwest::get(url).await {
            Ok(r) => return r,
            Err(e) if Instant::now() < deadline => {
                let _ = e;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(e) => panic!("the listener never came up on {url}: {e}"),
        }
    }
}

/// Everything the caller has to keep alive for the listener to stay up.
///
/// Not decoration: `run_io` reads the CLOSING of the reconfig channel as "the
/// handler half is gone" and returns. Dropping these senders is therefore a
/// shutdown request, and a first draft of this file that let them fall out of
/// scope produced four tests whose listener was never up — which is a fair
/// description of what the substrate promises, just not what was being tested.
struct Alive {
    _assets: watch::Sender<Arc<AssetMap>>,
    _push: mpsc::Sender<meclaw_cells::web::cell::WebReconfig>,
    _reconfig: mpsc::Sender<meclaw_cells::web::cell::WebReconfig>,
    _events: mpsc::Receiver<meclaw_cells::web::cell::WebEvent>,
}

/// A listener with the readiness channel in the test's hand.
///
/// Returns the port, the readiness sender, the pages sender (so a publish can
/// be made real rather than simulated), the keep-alive bundle, and the join
/// handle.
fn listener() -> (
    u16,
    watch::Sender<bool>,
    watch::Sender<Arc<PageMap>>,
    Alive,
    tokio::task::JoinHandle<()>,
) {
    let port = free_port();
    let (pages_tx, pages_rx) = watch::channel(Arc::new(PageMap::new()));
    let (assets_tx, assets_rx) = watch::channel(Arc::new(AssetMap::new()));
    let (ready_tx, ready_rx) = watch::channel(false);
    let (push_tx, push_rx) = mpsc::channel(8);
    let io = WebIo::new(
        "127.0.0.1".to_string(),
        port,
        "/display",
        pages_rx,
        assets_rx,
        ready_rx,
        push_rx,
    );
    let (events_tx, events_rx) = mpsc::channel(8);
    let (reconfig_tx, reconfig_rx) = mpsc::channel(8);
    let join = tokio::spawn(run_io(io, events_tx, reconfig_rx));
    (
        port,
        ready_tx,
        pages_tx,
        Alive {
            _assets: assets_tx,
            _push: push_tx,
            _reconfig: reconfig_tx,
            _events: events_rx,
        },
        join,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn before_the_first_publish_the_display_answers_503_not_404() {
    let (port, ready_tx, _pages_tx, _alive, join) = listener();

    // The window, held open. Every route is "not ready", including the one a
    // seed would declare — the display has no page map to make a statement from
    // yet, so it makes none.
    for path in ["/", "/seeded-page", "/nothing-here"] {
        let resp = get_once_up(&format!("http://127.0.0.1:{port}{path}")).await;
        assert_eq!(
            resp.status().as_u16(),
            503,
            "a display that has not published yet must not claim {path} does not \
             exist — that is the GH #395 conflation, and a proxy in front acts \
             on the difference"
        );
        let body = resp.text().await.expect("body");
        assert!(
            body.contains("starting"),
            "the 503 should say what it is waiting for, not just refuse: {body:?}"
        );
    }

    drop(ready_tx);
    join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_the_first_publish_a_route_that_does_not_exist_is_404_again() {
    let (port, ready_tx, _pages_tx, _alive, join) = listener();

    // Wait for the listener, still in the window.
    let first = get_once_up(&format!("http://127.0.0.1:{port}/")).await;
    assert_eq!(first.status().as_u16(), 503, "precondition: still starting");

    // The handler publishes. An EMPTY map is published on purpose: a display
    // with zero pages is a legitimate state under R-W8-3 and must go back to
    // answering 404 — which is precisely why readiness could not be expressed
    // as "is the page map empty".
    ready_tx.send(true).expect("the listener is still up");

    let resp = get_once_up(&format!("http://127.0.0.1:{port}/nothing-here")).await;
    assert_eq!(
        resp.status().as_u16(),
        404,
        "once the display is ready, a route nothing declares is a genuine 404 — \
         the fix must not turn every miss into a permanent 503"
    );

    join.abort();
}

/// The acceptance criterion, stated as one assertion: the two facts are
/// distinguishable on the wire, at the same URL, with nothing else changed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_facts_are_told_apart_at_one_url() {
    let (port, ready_tx, _pages_tx, _alive, join) = listener();
    let url = format!("http://127.0.0.1:{port}/some-route");

    let starting = get_once_up(&url).await.status().as_u16();
    ready_tx.send(true).expect("the listener is still up");
    let missing = reqwest::get(&url).await.expect("get").status().as_u16();

    assert_ne!(
        starting, missing,
        "before and after the first publish, the same URL must not answer the \
         same thing — that identity WAS the defect"
    );
    assert_eq!((starting, missing), (503, 404));

    join.abort();
}

/// A file is not an exception: the asset map is published in the same
/// `on_start`, so a request for one in the window is "not ready" too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_request_in_the_window_is_not_a_404_either() {
    let (port, ready_tx, _pages_tx, _alive, join) = listener();
    let resp = get_once_up(&format!("http://127.0.0.1:{port}/style.css")).await;
    assert_eq!(
        resp.status().as_u16(),
        503,
        "the assets snapshot is published by the same `on_start`, so a file \
         request in the window is the same 'not ready', not a missing file"
    );
    // Named so the import is load-bearing rather than decorative: this is the
    // type the published map holds.
    let _: Option<Asset> = None;
    drop(ready_tx);
    join.abort();
}
