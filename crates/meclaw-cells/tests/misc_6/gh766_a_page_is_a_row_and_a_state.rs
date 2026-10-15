//! GH #766 (wave G, T5) — four verbs, one register, one row per page.
//!
//! Against the bare I/O half and the browser double: no colony, no mailbox, no
//! edges. What is measured here is what a browser cell KNOWS — which pages it
//! holds, in which identity, in which state — and the two rules that make that
//! knowledge safe to act on: a `page` emission reports a CHANGE, and a page
//! that somebody might be looking at is never given up to make room.
//!
//! The ninth arm is the one that replaces a knob. R-G11 struck the sandbox
//! switch, and what stands in its place is this: after the spawn the cell looks
//! at the processes below the pid it remembers, and a browser whose children
//! never left this user namespace gets no page at all.

use meclaw_cells::browser::{Browser, BrowserParams, PageState, Verb, Viewport, parse_verb};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use std::time::Duration;

/// The browser double. Started through the same shim a real browser is.
const FIXTURE: &str = env!("CARGO_BIN_EXE_cdp_browser_fixture");

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

fn params(mode: &str, extra: Value) -> BrowserParams {
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

/// A started browser whose sandbox check has passed, with its event side and
/// its temp directory.
///
/// The event receiver is handed back and held for the test's life, because a
/// cell holds it: a fixture that dropped it would be measuring a browser
/// nobody is listening to.
async fn ready(
    mode: &str,
    extra: Value,
) -> (
    Browser,
    tokio::sync::mpsc::Receiver<meclaw_cells::browser::CdpEvent>,
    tempfile::TempDir,
) {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (mut browser, events) =
        Browser::start(params(mode, extra), td.path().join("profile")).expect("the browser starts");
    tokio::time::timeout(MARKER, browser.ready())
        .await
        .expect("within the failure-marker window")
        .expect("the fixture's children leave this namespace");
    (browser, events, td)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_opens_are_three_pages_one_context_and_one_browser() {
    let (mut browser, _events, _td) = ready("own-namespace", json!({})).await;
    for n in 1..=3 {
        browser
            .in_open(
                &format!("card-{n}"),
                &format!("https://example.com/{n}"),
                "default",
                None,
            )
            .await
            .expect("a page");
    }
    assert_eq!(browser.register.pages.len(), 3);
    assert_eq!(
        browser.register.contexts.len(),
        1,
        "one identity is one context, however many windows it has"
    );
    assert!(
        browser
            .register
            .pages
            .values()
            .all(|e| e.state == PageState::Opening),
        "a page is opening until the browser says otherwise"
    );
    let targets: std::collections::HashSet<String> = browser
        .register
        .pages
        .values()
        .filter_map(|e| e.target.clone())
        .collect();
    assert_eq!(targets.len(), 3, "three windows, three targets");
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_open_is_a_receipt_and_not_a_second_page() {
    let (mut browser, _events, _td) = ready("own-namespace", json!({})).await;
    browser
        .in_open("card-1", "https://example.com/a", "default", None)
        .await
        .expect("a page");
    let target = browser.register.pages["card-1"].target.clone();
    browser
        .in_open("card-1", "https://example.com/a", "default", None)
        .await
        .expect("the same page");
    assert_eq!(browser.register.pages.len(), 1, "no second window");
    assert_eq!(
        browser.register.pages["card-1"].target, target,
        "and not even a second target: an app re-emitting a card must not open one"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_page_with_another_address_navigates() {
    let (mut browser, _events, _td) = ready("own-namespace", json!({})).await;
    browser
        .in_open("card-1", "https://example.com/a", "default", None)
        .await
        .expect("a page");
    let target = browser.register.pages["card-1"].target.clone();
    let report = browser
        .in_open("card-1", "https://example.com/b#anchor", "default", None)
        .await
        .expect("it moves");
    assert_eq!(browser.register.pages.len(), 1);
    assert_eq!(
        browser.register.pages["card-1"].target, target,
        "same window"
    );
    assert_eq!(
        report.row.url, "https://example.com/b#anchor",
        "the fragment is part of the address, always (OR-G39)"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_takes_the_page_and_leaves_the_identity() {
    let (mut browser, _events, _td) = ready("own-namespace", json!({})).await;
    browser
        .in_open("card-1", "https://example.com/a", "alex", None)
        .await
        .expect("a page");
    let context_id = browser.register.contexts["alex"].clone();
    let report = browser.in_close("card-1").await.expect("it closes");
    assert_eq!(report.state, PageState::Closed);
    assert!(!browser.register.has("card-1"), "the page is gone");
    assert_eq!(
        browser.register.contexts["alex"], context_id,
        "closing a window is not logging out (R-G3)"
    );
    // And the context close is the logging out.
    browser
        .in_context_close("alex")
        .await
        .expect("the identity goes");
    assert!(!browser.register.contexts.contains_key("alex"));
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ninth_page_takes_a_suspended_one_or_nothing_at_all() {
    let (mut browser, _events, _td) = ready("own-namespace", json!({"max_pages": 3})).await;
    for n in 1..=3 {
        browser
            .in_open(
                &format!("card-{n}"),
                "https://example.com/",
                "default",
                None,
            )
            .await
            .expect("a page");
    }
    let e = browser
        .in_open("card-4", "https://example.com/", "default", None)
        .await
        .expect_err("nothing is suspended, so nothing is given up");
    assert_eq!(e.error_code(), "too_many_pages");
    assert!(
        e.detail().contains('3'),
        "the refusal names the cap: {}",
        e.detail()
    );
    assert_eq!(browser.register.pages.len(), 3, "and nothing was displaced");

    browser.register.set_state("card-2", PageState::Suspended);
    browser
        .in_open("card-4", "https://example.com/", "default", None)
        .await
        .expect("now there is room");
    assert!(!browser.register.has("card-2"), "the suspended one went");
    assert!(
        browser.register.has("card-1") && browser.register.has("card-3"),
        "and only the suspended one"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_state_that_did_not_move_is_not_an_emission() {
    let (mut browser, _events, _td) = ready("own-namespace", json!({})).await;
    browser
        .in_open("card-1", "https://example.com/a", "default", None)
        .await
        .expect("a page");
    assert_eq!(
        browser.register.set_state("card-1", PageState::Active),
        Some(PageState::Opening),
        "a change is a change"
    );
    assert_eq!(
        browser.register.set_state("card-1", PageState::Active),
        None,
        "and the same state twice is not"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_verb_about_a_page_nobody_holds_names_its_page() {
    let (mut browser, _events, _td) = ready("own-namespace", json!({})).await;
    for e in [
        browser
            .in_navigate("card-9", "https://example.com/")
            .await
            .expect_err("no page"),
        browser.in_close("card-9").await.expect_err("no page"),
    ] {
        assert_eq!(e.error_code(), "unknown_page");
        assert!(
            e.detail().contains("card-9"),
            "a caller with two windows in flight has to learn which one: {}",
            e.detail()
        );
    }
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[test]
fn the_tool_call_form_is_the_one_the_app_sends() {
    // The shape `harness::parse::parse_tool_call` reads, and the shape G3 will
    // send: `name` and `arguments` INSIDE the turn's text, because the UBF turn
    // schema is closed (OR-G33).
    let text = json!({
        "name": "in_open",
        "arguments": {"page": "card-1", "url": "https://example.com/", "context": "alex",
                      "viewport": {"width": 960, "height": 600, "dpr": 2.0, "mobile": false}}
    })
    .to_string();
    let msg = MessageBuilder::new(Path::new("/browser"))
        .body(Body::Inline(json!({
            "messages": [{"origin": "tool", "type": "tool_call", "id": "c1", "text": text}]
        })))
        .build();
    let parsed = parse_verb(&msg).expect("the app's shape reads");
    assert_eq!(parsed.call_id, "c1");
    assert_eq!(
        parsed.verb,
        Verb::Open {
            page: "card-1".to_string(),
            url: "https://example.com/".to_string(),
            context: "alex".to_string(),
            viewport: Some(Viewport {
                width: 960,
                height: 600,
                dpr: 2.0,
                mobile: false
            }),
        }
    );

    // A flat body is not this form, and is refused as such.
    let flat = MessageBuilder::new(Path::new("/browser"))
        .body(Body::Inline(json!({
            "page": "card-1", "url": "https://example.com/", "messages": []
        })))
        .build();
    let e = parse_verb(&flat).expect_err("a flat body is not a tool call");
    assert!(e.contains("tool_call"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_browser_whose_children_share_this_namespace_gets_no_page() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (mut browser, _events) = Browser::start(
        // A short watch, so the refusal that has to wait out the whole window
        // does not wait out five seconds of it.
        params("same-namespace", json!({"external_timeout_ms": 800})),
        td.path().join("profile"),
    )
    .expect("the process starts");
    let e = tokio::time::timeout(MARKER, browser.ready())
        .await
        .expect("the check ends on its own")
        .expect_err("a browser without its sandbox does not get a page");
    assert_eq!(
        e.error_code(),
        "spawn_failed",
        "fail-closed, and there is no knob to say otherwise (R-G11): {}",
        e.detail()
    );
    assert!(
        e.detail().contains("sandbox") && e.detail().contains("packaged browser"),
        "the refusal tells an operator what to do about it: {}",
        e.detail()
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;

    // The control: the same fixture, with a child that leaves the namespace.
    let (mut good, _events2, _td2) = ready("own-namespace", json!({})).await;
    good.in_open("card-1", "https://example.com/", "default", None)
        .await
        .expect("a browser whose sandbox holds gets its page");
    good.reaper.terminate(Duration::from_millis(500)).await;
}
