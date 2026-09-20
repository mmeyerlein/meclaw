//! GH #766 (wave G, T8) — decaying, coming back, surviving a restart.
//!
//! Three answers to one question: what is a page nobody is looking at. The
//! browser is a cache and the row is the state (R-G4), so the answer is never
//! "gone": the target is given up, the identity is kept, the row stays, and the
//! page comes back when somebody wants it again.
//!
//! The fifth arm is the one that removes a conversation from the protocol
//! (OR-G17). A cell that comes back reads its own `pages` table and opens what
//! stands there. Nobody is asked, nothing is announced as missing, and there is
//! no `gone` state for an app to have to answer.

use meclaw_cells::browser::{Browser, BrowserParams, CdpEvent, PageState, db};
use meclaw_core::serde_json::{Value, json};
use std::time::Duration;

/// The browser double. Started through the same shim a real browser is.
const FIXTURE: &str = env!("CARGO_BIN_EXE_cdp_browser_fixture");

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

fn params(mode: &str, extra: Value) -> BrowserParams {
    let mut v = json!({
        "chromium_path": FIXTURE,
        "extra_args": [format!("--fixture-mode={mode}"), "--fixture-frame-ms=5"],
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

async fn ready(
    mode: &str,
    extra: Value,
) -> (
    Browser,
    tokio::sync::mpsc::Receiver<CdpEvent>,
    tempfile::TempDir,
) {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (mut browser, events) =
        Browser::start(params(mode, extra), td.path().join("profile")).expect("the browser starts");
    tokio::time::timeout(MARKER, browser.ready())
        .await
        .expect("within the failure-marker window")
        .expect("the sandbox check passes");
    (browser, events, td)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_last_viewer_out_leaves_a_page_that_then_decays() {
    let (mut browser, _events, _td) =
        ready("own-namespace", json!({"suspend_after_ms": 120})).await;
    browser
        .in_open("card-1", "https://example.com/a", "alex", None)
        .await
        .expect("a page");
    let context = browser.register.contexts["alex"].clone();
    let viewer = browser.join("card-1", &json!({})).await.expect("a viewer");
    browser
        .left("card-1", viewer.id)
        .await
        .expect("the last one out is a state change");
    assert_eq!(
        browser.register.pages["card-1"].state,
        PageState::Background,
        "(a) nobody watching is not nobody wanting"
    );

    // The deadline, not a tick: the loop sleeps until it and no longer.
    let due = browser.next_suspend().expect("a page is counting down");
    tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
    let (reports, _next) = browser.suspend_due().await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].state, PageState::Suspended);
    assert!(
        browser.register.pages["card-1"].target.is_none(),
        "the target went"
    );
    assert!(
        browser.register.has("card-1"),
        "and the row did not: it is what brings the page back"
    );
    assert_eq!(
        browser.register.contexts["alex"], context,
        "(b) the identity outlives the window it was opened in"
    );

    // And back: a navigation on a suspended page opens it again, in the same
    // identity, without anybody re-declaring it.
    browser
        .in_navigate("card-1", "https://example.com/b")
        .await
        .expect("it comes back");
    assert!(browser.register.pages["card-1"].target.is_some());
    assert_eq!(browser.register.contexts["alex"], context);
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn suspend_after_zero_never_suspends() {
    let (mut browser, _events, _td) = ready("own-namespace", json!({"suspend_after_ms": 0})).await;
    browser
        .in_open("card-1", "https://example.com/a", "default", None)
        .await
        .expect("a page");
    assert!(
        browser.next_suspend().is_none(),
        "(c) zero is an operator turning it off, not a deadline of zero"
    );
    let (reports, next) = browser.suspend_due().await;
    assert!(reports.is_empty() && next.is_none());
    assert_eq!(browser.register.pages["card-1"].state, PageState::Opening);
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renderer_that_crashes_takes_its_own_page_and_no_other() {
    let (mut browser, mut events, _td) = ready("crash-renderer", json!({})).await;
    browser
        .in_open("card-1", "https://example.com/a", "default", None)
        .await
        .expect("one page");
    browser
        .in_open("card-2", "https://example.com/b", "default", None)
        .await
        .expect("another");
    // The fixture crashed the renderer of whichever page it navigated last.
    let deadline = std::time::Instant::now() + MARKER;
    let mut crashed = None;
    while crashed.is_none() {
        assert!(std::time::Instant::now() < deadline, "no crash arrived");
        if let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(200), events.recv()).await
            && event.method == "Inspector.targetCrashed"
        {
            crashed = event
                .session_id
                .as_deref()
                .and_then(|s| browser.register.page_of_session(s));
        }
    }
    let crashed = crashed.expect("a page of ours");
    browser.register.set_state(&crashed, PageState::Suspended);
    let standing = if crashed == "card-1" {
        "card-2"
    } else {
        "card-1"
    };
    // (d) Renderer isolation is per SITE (OR-G29): the other page answers on.
    browser
        .in_navigate(standing, "https://example.com/c")
        .await
        .expect("the other page is untouched by its neighbour's crash");
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_life_opens_its_own_rows_and_says_reopened() {
    // The first life's rows, written the way the handler writes them.
    let conn = rusqlite::Connection::open_in_memory().expect("a database");
    db::setup_browser_schema(&conn).expect("ddl");
    for (n, url) in [
        ("card-1", "https://example.com/a#anchor"),
        ("card-2", "https://example.com/b"),
    ] {
        db::upsert_page(
            &conn,
            &db::PageRow {
                page: n.to_string(),
                url: url.to_string(),
                context: "alex".to_string(),
                viewport_w: 960,
                viewport_h: 600,
                viewport_dpr: 2.0,
                mobile: false,
                state: "active".to_string(),
                opened_at: 1_000,
                updated_at: 1_000,
            },
        )
        .expect("a row");
    }
    let rows = db::open_pages(&conn).expect("two rows");
    assert_eq!(rows.len(), 2);

    let (mut browser, _events, _td) = ready("own-namespace", json!({})).await;
    let reports = browser
        .reopen(&rows)
        .await
        .expect("the second life opens them");
    assert_eq!(reports.len(), 2, "(e) one report per row, and no more");
    for report in &reports {
        assert_eq!(
            report.state,
            PageState::Reopened,
            "each of them says what it is: a page this cell brought back"
        );
    }
    assert_eq!(
        reports[0].row.url, "https://example.com/a#anchor",
        "with the address it had, fragment and all"
    );
    assert_eq!(
        browser.register.contexts.len(),
        1,
        "two pages of one identity are one context again"
    );
    assert_eq!(
        browser.register.pages["card-1"].viewport.width, 960,
        "and the shape the row remembered"
    );
    assert!(
        browser.register.pages.values().all(|e| !e.casting),
        "nothing is streaming: a reopened page has no viewer, and a picture \
         nobody asked for is bandwidth for nothing"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

/// Drive the browser's own event side until a picture reaches `from_cell`.
async fn next_binary(
    browser: &mut Browser,
    events: &mut tokio::sync::mpsc::Receiver<CdpEvent>,
    from_cell: &mut tokio::sync::mpsc::Receiver<meclaw_colony::LinkFrame>,
) -> Vec<u8> {
    let deadline = std::time::Instant::now() + MARKER;
    loop {
        if let Ok(meclaw_colony::LinkFrame::Binary(bytes)) = from_cell.try_recv() {
            return bytes;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no picture reached the viewer that joined a suspended page"
        );
        match tokio::time::timeout(Duration::from_millis(50), events.recv()).await {
            Ok(Some(event)) if event.method == "Page.screencastFrame" => {
                browser.on_frame(&event).await;
            }
            Ok(Some(_)) | Err(_) => browser.flush_acks().await,
            Ok(None) => panic!("the browser's pipe closed"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_join_on_a_suspended_page_is_the_other_way_back() {
    // (f) Contract § 2 names TWO ways back out of `suspended`: `in_navigate`
    // and a JOIN. Only the first one was built, so a viewer that joined a
    // suspended page was admitted onto a page with no target: `active` in the
    // register, no picture on the wire, `idle_since` cleared so `due_suspends`
    // never looked at it again — no state change and no way out.
    let (mut browser, mut events, _td) =
        ready("own-namespace", json!({"suspend_after_ms": 60})).await;
    browser
        .in_open("card-1", "https://example.com/a#anchor", "alex", None)
        .await
        .expect("a page");
    let context = browser.register.contexts["alex"].clone();
    let due = browser
        .next_suspend()
        .expect("a page nobody joined is counting");
    tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
    let (reports, _next) = browser.suspend_due().await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].state, PageState::Suspended);
    assert!(browser.register.pages["card-1"].target.is_none());

    let mut viewer = browser
        .join(
            "card-1",
            &json!({"viewport": {"width": 960, "height": 600, "dpr": 2.0}}),
        )
        .await
        .expect("a join on a suspended page is admitted");
    assert!(
        browser.register.pages["card-1"].target.is_some(),
        "the join opened the page again: it is the second way back (§ 2)"
    );
    assert!(
        browser.register.pages["card-1"].casting,
        "and the picture is running, which is what the viewer joined for"
    );
    assert_eq!(
        browser.register.pages["card-1"].url, "https://example.com/a#anchor",
        "at the address the row remembered, fragment and all"
    );
    assert_eq!(
        browser.register.contexts["alex"], context,
        "and in the identity it was opened under"
    );
    let bytes = next_binary(&mut browser, &mut events, &mut viewer.link.from_cell).await;
    assert!(
        bytes.len() > 16,
        "a frame is a head and a picture: {}",
        bytes.len()
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}
