//! GH #766 (wave G, fix round 1) — the CDP-event seam, driven through the cell.
//!
//! What `io.rs`'s `translate` promises had no test at all: the claims
//! "`Target.targetInfoChanged` becomes a `page`" and "`Inspector.targetCrashed`
//! becomes `suspended`" were read off the register by the tests that had just
//! written it. Three plan arms hang off this seam (T7 i, T8 d, T10 e), so it is
//! driven here the way the substrate drives it — a whole `run_io`, a real child
//! over the real pipe, and the events read off the channel the handler holds.
//!
//! Four arms:
//!
//! (a) a navigation's `Target.targetInfoChanged` reaches the handler as a
//!     `page` carrying the address WITH its fragment and the title (OR-G39);
//! (b) `Inspector.targetCrashed` reaches it as `Crashed`, state `suspended`;
//! (c) a crashed page gives its viewers back instead of holding them on a
//!     renderer that is gone;
//! (d) a page displaced by `max_pages` says `closed` on the event lane — the
//!     app that owns its card hears about it, and the row goes with it.

use meclaw_cells::browser::{
    Browser, BrowserCommand, BrowserEvent, BrowserIo, BrowserParams, PageState, io,
};
use meclaw_colony::{LinkFrame, SurfaceRegistry};
use meclaw_core::Path;
use meclaw_core::serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// The browser double. Started through the same shim a real browser is.
const FIXTURE: &str = env!("CARGO_BIN_EXE_cdp_browser_fixture");

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

fn params(mode: &str, mount: &str, extra: Value) -> BrowserParams {
    let mut v = json!({
        "mount": mount,
        "chromium_path": FIXTURE,
        "extra_args": [format!("--fixture-mode={mode}")],
        // Required since OR-G54; a test double declares the escape hatch
        // rather than a cap the host may not be able to delegate.
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

/// A running I/O half, with the two channels the substrate would hold.
struct Half {
    commands: mpsc::Sender<BrowserCommand>,
    events: mpsc::Receiver<BrowserEvent>,
    joined: tokio::task::JoinHandle<()>,
    _td: tempfile::TempDir,
}

impl Half {
    async fn start(mode: &str, mount: &str, extra: Value) -> Self {
        let td = tempfile::TempDir::new().expect("tempdir");
        let profile = td.path().join("profile");
        let (events_tx, events) = mpsc::channel(64);
        let (commands, commands_rx) = mpsc::channel(16);
        let joined = tokio::spawn(io::run_io(
            BrowserIo::new(
                params(mode, mount, extra),
                profile,
                Path::new("/browser"),
                Arc::new(SurfaceRegistry::new()),
            ),
            events_tx,
            commands_rx,
        ));
        Self {
            commands,
            events,
            joined,
            _td: td,
        }
    }

    /// One verb, answered the way the handler answers it.
    async fn open(&self, page: &str, url: &str) -> Result<(), String> {
        let (answer, wait) = oneshot::channel();
        self.commands
            .send(BrowserCommand::Open {
                page: page.to_string(),
                url: url.to_string(),
                context: "alex".to_string(),
                viewport: None,
                answer,
            })
            .await
            .expect("the half is listening");
        match tokio::time::timeout(MARKER, wait)
            .await
            .expect("the verb is answered")
            .expect("the half answered")
        {
            Ok(_) => Ok(()),
            Err(e) => Err(e.error_code().to_string()),
        }
    }

    /// The next event the handler would see that `want` accepts.
    async fn next<T>(&mut self, what: &str, mut want: impl FnMut(&BrowserEvent) -> Option<T>) -> T {
        let deadline = std::time::Instant::now() + MARKER;
        loop {
            assert!(std::time::Instant::now() < deadline, "{what}");
            match tokio::time::timeout(Duration::from_millis(200), self.events.recv()).await {
                Ok(Some(event)) => {
                    if let Some(out) = want(&event) {
                        return out;
                    }
                }
                Ok(None) => panic!("the I/O half went away: {what}"),
                Err(_) => {}
            }
        }
    }

    async fn end(self) {
        drop(self.commands);
        let _ = tokio::time::timeout(MARKER, self.joined).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_target_that_changed_reaches_the_handler_as_a_page() {
    let mut half = Half::start("own-namespace", "browser-a", json!({})).await;
    let anchor = "https://example.com/doc.pdf#toolbar=0&view=FitH";
    half.open("card-1", anchor).await.expect("a page");
    // (a) The fixture sends `Target.targetInfoChanged` after every navigation.
    // Nothing in this test touches the register: what is measured is the whole
    // path from the wire through `translate` to the handler's channel.
    let report = half
        .next("no page emission carried the title", |event| match event {
            BrowserEvent::Page(report) if !report.title.is_empty() => Some(report.clone()),
            _ => None,
        })
        .await;
    assert_eq!(
        report.title, "A fixture page",
        "the title exists nowhere but in that event"
    );
    assert_eq!(
        report.row.url, anchor,
        "and the address arrived with its fragment (OR-G39)"
    );
    half.end().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crashed_renderer_reaches_the_handler_as_a_suspended_page() {
    let mut half = Half::start("crash-renderer", "browser-b", json!({})).await;
    half.open("card-1", "https://example.com/a")
        .await
        .expect("a page");
    // (b) The same seam, the other arm.
    let report = half
        .next("no crash reached the handler", |event| match event {
            BrowserEvent::Crashed { report } => Some(report.clone()),
            _ => None,
        })
        .await;
    assert_eq!(report.state, PageState::Suspended);
    assert_eq!(report.row.page, "card-1");
    half.end().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crashed_page_gives_its_viewers_back() {
    // (c) Against the I/O value rather than the whole half, because what has
    // to be visible is the viewer's own link. A crashed page that KEPT its
    // viewers was the worst of both: they watched a renderer that was gone,
    // `join` never saw an empty set again so no rejoin restarted the picture,
    // and `idle_since` stayed `None` so `due_suspends` never looked either.
    let td = tempfile::TempDir::new().expect("tempdir");
    let (mut browser, mut events) = Browser::start(
        params("crash-renderer", "browser-c", json!({})),
        td.path().join("profile"),
    )
    .expect("the browser starts");
    tokio::time::timeout(MARKER, browser.ready())
        .await
        .expect("within the failure-marker window")
        .expect("the sandbox check passes");
    browser
        .in_open("card-1", "https://example.com/a", "alex", None)
        .await
        .expect("a page");
    let mut viewer = browser.join("card-1", &json!({})).await.expect("a viewer");
    assert_eq!(browser.register.pages["card-1"].viewers.len(), 1);

    let crashed = {
        let deadline = std::time::Instant::now() + MARKER;
        loop {
            assert!(std::time::Instant::now() < deadline, "no crash arrived");
            if let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(200), events.recv()).await
                && event.method == "Inspector.targetCrashed"
            {
                break event;
            }
        }
    };
    let out = browser.on_event(crashed).expect("the seam says something");
    assert!(matches!(out, BrowserEvent::Crashed { .. }));
    assert!(
        browser.register.pages["card-1"].viewers.is_empty(),
        "the viewers went with the renderer"
    );
    assert!(
        browser.register.pages["card-1"].idle_since.is_some(),
        "and the page is counting down again, so it can be given up"
    );
    let told = viewer
        .link
        .from_cell
        .try_recv()
        .expect("the viewer was told");
    match told {
        LinkFrame::Close { code, reason } => {
            assert_eq!(code, 4410);
            assert!(reason.contains("join again"), "{reason}");
        }
        other => panic!("a closed link, not {other:?}"),
    }
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_displaced_page_says_closed_to_the_app_that_owned_it() {
    // (d) The second half of OR-G7. A displacement used to be silent: no
    // `page {state:"closed"}`, so the app kept a card whose page was gone, and
    // no `db::delete_page` — that hangs off the closed state — so the next
    // life of this cell reopened the page it had just given up and spent the
    // place again.
    let mut half = Half::start(
        "own-namespace",
        "browser-d",
        json!({"max_pages": 2, "suspend_after_ms": 80}),
    )
    .await;
    half.open("card-1", "https://example.com/1")
        .await
        .expect("one");
    half.open("card-2", "https://example.com/2")
        .await
        .expect("two");
    // Nobody is watching either of them, so both give their targets up. The
    // third page is what displaces one.
    half.next("no page suspended", |event| match event {
        BrowserEvent::Page(report) if report.state == PageState::Suspended => Some(()),
        _ => None,
    })
    .await;
    half.open("card-3", "https://example.com/3")
        .await
        .expect("a suspended page made room");
    let closed = half
        .next("the displaced page said nothing", |event| match event {
            BrowserEvent::Page(report) if report.state == PageState::Closed => Some(report.clone()),
            _ => None,
        })
        .await;
    assert!(
        closed.row.page == "card-1" || closed.row.page == "card-2",
        "the page that was given up says so by name: {}",
        closed.row.page
    );
    assert_eq!(
        closed.row.state, "closed",
        "and as the state the handler writes `db::delete_page` from"
    );
    half.end().await;
}
