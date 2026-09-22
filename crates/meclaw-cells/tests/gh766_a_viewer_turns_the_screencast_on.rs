//! GH #766 (wave G, T6) — one viewer turns the picture on, and only a viewer.
//!
//! The single switch of this cell type (R-G4): `viewers` empty ↔ not empty.
//! There is no timer, no level and no curator value in it — whether a window is
//! OPEN is said by its level, and a level is system-wide, so all outputs join
//! or none do and a dock cut never takes a window. What this cell counts is
//! sockets that want a picture, which is rendering and nothing else.
//!
//! Seven arms:
//!
//! (a) one join is ONE `Page.startScreencast`, not one per viewer;
//! (b) a frame arrives as sixteen bytes of head and then the picture, and the
//!     head carries the CSS size and the scroll offset;
//! (c) two viewers, each every frame, still one screencast;
//! (d) the last one leaving stops it and leaves the page standing;
//! (e) a join on a page nobody holds is refused in this cell's own words;
//! (f) a viewer that stops reading is given up, and the other keeps its picture;
//! (g) the viewport of a join reaches the page ONE level deep (OR-G32).

use meclaw_cells::browser::{Browser, BrowserParams, PageState, Viewport, head, viewport_of_join};
use meclaw_colony::LinkFrame;
use meclaw_core::serde_json::{Value, json};
use std::time::Duration;

/// The browser double. Started through the same shim a real browser is.
const FIXTURE: &str = env!("CARGO_BIN_EXE_cdp_browser_fixture");

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

fn params(extra: Value) -> BrowserParams {
    let mut v = json!({
        "chromium_path": FIXTURE,
        "extra_args": ["--fixture-mode=own-namespace", "--fixture-frame-ms=5"],
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

/// A started browser holding one page, and the CDP events it will produce.
async fn with_one_page(
    extra: Value,
) -> (
    Browser,
    tokio::sync::mpsc::Receiver<meclaw_cells::browser::CdpEvent>,
    tempfile::TempDir,
) {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (mut browser, events) =
        Browser::start(params(extra), td.path().join("profile")).expect("the browser starts");
    tokio::time::timeout(MARKER, browser.ready())
        .await
        .expect("within the failure-marker window")
        .expect("the sandbox check passes");
    browser
        .in_open("card-1", "https://example.com/", "default", None)
        .await
        .expect("a page");
    (browser, events, td)
}

/// Drive the browser's own event side until `page` has a frame for `link`.
async fn next_binary(
    browser: &mut Browser,
    events: &mut tokio::sync::mpsc::Receiver<meclaw_cells::browser::CdpEvent>,
    from_cell: &mut tokio::sync::mpsc::Receiver<LinkFrame>,
) -> Vec<u8> {
    let deadline = std::time::Instant::now() + MARKER;
    loop {
        if let Ok(LinkFrame::Binary(bytes)) = from_cell.try_recv() {
            return bytes;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no picture reached the viewer"
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
async fn one_join_is_one_screencast_and_a_frame_carries_its_head() {
    let (mut browser, mut events, _td) = with_one_page(json!({})).await;
    let mut admitted = browser
        .join(
            "card-1",
            &json!({"viewport": {"width": 960, "height": 600, "dpr": 2.0}}),
        )
        .await
        .expect("a viewer is admitted");
    assert!(
        browser.register.pages["card-1"].casting,
        "(a) the first viewer started the picture"
    );
    assert_eq!(
        browser.register.pages["card-1"].state,
        PageState::Active,
        "and the page says so"
    );
    let report = admitted.report.take().expect("a state change worth saying");
    assert_eq!(report.state, PageState::Active);

    // (b) the head, then the picture.
    let bytes = next_binary(&mut browser, &mut events, &mut admitted.link.from_cell).await;
    assert!(bytes.len() > 16, "a frame is a head and a picture");
    assert_eq!(
        &bytes[..16],
        // LITERAL bytes, not `head(…)`: this is the wire G2 draws against, and
        // a head compared with the function that wrote it proves that the
        // function agrees with itself. 960 = 0x0000_03C0 and 600 = 0x0000_0258
        // most significant byte FIRST; little-endian would be [192, 3, 0, 0].
        &[0, 0, 3, 192, 0, 0, 2, 88, 0, 0, 0, 0, 0, 0, 0, 120],
        "four big-endian u32s — width, height, scroll x, scroll y — in CSS pixels"
    );
    assert_eq!(
        &bytes[..16],
        &head(960, 600, 0, 120),
        "and `head()`, which is what a test of G2's may reach for, writes the same"
    );
    assert_eq!(
        &bytes[16..],
        &[0xFF, 0xD8, 0xFF, 0xE0],
        "and the picture came through the strict decoder unchanged"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_viewers_are_two_copies_of_one_screencast() {
    let (mut browser, mut events, _td) = with_one_page(json!({})).await;
    let mut first = browser.join("card-1", &json!({})).await.expect("one");
    let mut second = browser.join("card-1", &json!({})).await.expect("two");
    assert!(
        second.report.is_none(),
        "(c) the second viewer changes nothing: the switch is empty ↔ not empty"
    );
    assert_eq!(browser.register.pages["card-1"].viewers.len(), 2);
    let a = next_binary(&mut browser, &mut events, &mut first.link.from_cell).await;
    let b = next_binary(&mut browser, &mut events, &mut second.link.from_cell).await;
    assert_eq!(a[..16], b[..16], "each of them gets the picture");

    // (d) the last one out stops it, and the page stays where it is.
    assert!(
        browser.left("card-1", first.id).await.is_none(),
        "one left, one watching"
    );
    let report = browser
        .left("card-1", second.id)
        .await
        .expect("the last one out is a state change");
    assert_eq!(report.state, PageState::Background);
    assert!(
        !browser.register.pages["card-1"].casting,
        "the picture stopped"
    );
    assert!(
        browser.register.has("card-1"),
        "and the page did not: nobody watching is not nobody wanting"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_join_on_a_page_nobody_holds_is_this_cells_own_refusal() {
    let (mut browser, _events, _td) = with_one_page(json!({})).await;
    let refused = browser
        .join("card-9", &json!({}))
        .await
        .err()
        .expect("no such page");
    assert_eq!(refused.status, 404);
    assert!(
        refused.detail.contains("card-9"),
        "the sentence a viewer reads is the cell's, and it names the page: {}",
        refused.detail
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_viewer_that_stops_reading_is_given_up_and_the_other_is_not() {
    let (mut browser, mut events, _td) =
        with_one_page(json!({"screencast": {"max_fps": 500}})).await;
    let mut reader = browser.join("card-1", &json!({})).await.expect("one");
    let sleeper = browser.join("card-1", &json!({})).await.expect("two");
    // The sleeper never reads: its queue fills, and the cell gives it up rather
    // than waiting for it.
    //
    // This loop is the one test of this cell whose floor used to be SECONDS.
    // A link queue holds `meclaw_colony::surfaces::LINK_QUEUE` = 64 frames, and
    // the pace of this loop is set by two things the TEST owns: the cell hands
    // out one acknowledgement per page per `flush_acks`, which the loop calls
    // only when `events.recv()` times out, and it acknowledges nothing before
    // `ack_interval` = 1000 / max_fps ms has passed. With the default 20 fps
    // and a 50 ms recv window that was one frame per ~50 ms, 64 × 50 ms = 3.2 s
    // before anything could be observed (3.3 s measured serially), and under
    // the gate's four-way parallelism the arm ran out of a 30 s marker once
    // (`GATE-SUMMARY strand 8929a71 9/10 594s RED`, 30.096 s) and out of a
    // 120 s one five times (GH #771). Widening the window twice moved the
    // number; it did not measure anything. `max_fps: 500` makes every
    // acknowledgement due after 2 ms and a 5 ms recv window flushes it at once,
    // so a frame costs milliseconds and the floor is 64 × ~5 ms ≈ 0.3 s. The
    // 30 s convention is a failure marker again, not a budget, and the sentence
    // it measures — "the cell never waits for a viewer" — is what a marker is
    // for. (Side effect, harmless here: with a 2 ms interval the busy window of
    // `on_frame` is 4 ms, so the throttle never engages in this arm — which is
    // also the one thing that could have turned its acknowledgements into
    // two a second under load.)
    let deadline = std::time::Instant::now() + MARKER;
    loop {
        if browser.register.pages["card-1"].viewers.len() == 1 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "(f) the cell waited for a viewer, which it must never do"
        );
        match tokio::time::timeout(Duration::from_millis(5), events.recv()).await {
            Ok(Some(event)) if event.method == "Page.screencastFrame" => {
                browser.on_frame(&event).await;
            }
            Ok(Some(_)) | Err(_) => browser.flush_acks().await,
            Ok(None) => panic!("the browser's pipe closed"),
        }
        // The reader keeps reading, so only one of the two can fill up.
        while reader.link.from_cell.try_recv().is_ok() {}
    }
    assert!(
        browser.register.pages["card-1"]
            .viewers
            .contains_key(&reader.id),
        "the one that kept reading kept its picture"
    );
    drop(sleeper);
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[test]
fn the_viewport_of_a_join_is_one_level_deep() {
    // (g) The door hands on the join payload minus the mount, one level. A cell
    // that also read a nested one would make the difference unobservable.
    let v = viewport_of_join(
        &json!({"viewport": {"width": 390, "height": 844, "dpr": 3.0, "mobile": true}}),
    )
    .expect("a shape");
    assert_eq!(
        v,
        Viewport {
            width: 390,
            height: 844,
            dpr: 3.0,
            mobile: true
        }
    );
    assert!(
        viewport_of_join(&json!({"params": {"viewport": {"width": 390, "height": 844}}})).is_none(),
        "two levels bring nothing"
    );
}
