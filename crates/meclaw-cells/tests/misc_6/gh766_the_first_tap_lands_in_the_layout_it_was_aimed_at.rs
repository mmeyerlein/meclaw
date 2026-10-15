//! GH #766 (wave G, g9, finding B-G16) — a gesture is not reshaped underneath.
//!
//! A viewer's coordinates are CSS pixels of the page's viewport, measured
//! against the ONE picture it holds. A click is three frames — `move`, `down`,
//! `up` — and the viewer computes all three against that same picture, because
//! no new one can arrive between them. So the page must not change shape in the
//! middle: a reshape on the `move` puts the `down` that is already on its way
//! into a layout that has moved.
//!
//! Measured in a colony on 2026-09-20, monitor profile 1920×1080 on a page the
//! application opened at 1280×800: the first click of that output was sent to
//! 640,400, the page was 1920 wide by the time it landed, a field at
//! `left: 50%` now sat at 960 and was missed, and the `Input.insertText` behind
//! it went nowhere — which is B-G16. The SECOND click of the same output, aimed
//! at a frame already 1920 wide, put its whole run into the field and into the
//! page's own echo.
//!
//! g7 put the input before the shape WITHIN one call, and that is right and not
//! enough. R-G5 is kept: the shape is still the profile of whoever last
//! touched, and the next frame is still drawn for them — it changes one gesture
//! later.

use meclaw_cells::browser::{Browser, BrowserParams, CdpEvent, Input, reshapes_the_page};
use meclaw_core::serde_json::{Value, json};
use std::time::Duration;

/// The browser double. Started through the same shim a real browser is.
const FIXTURE: &str = env!("CARGO_BIN_EXE_cdp_browser_fixture");

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

fn params() -> BrowserParams {
    BrowserParams::parse(&json!({
        "chromium_path": FIXTURE,
        "extra_args": ["--fixture-mode=own-namespace", "--fixture-frame-ms=5"],
        "sandbox": {"trust": "trusted"},
    }))
    .expect("the fixture params parse")
}

/// Every `Emulation.` call the double echoed within `window`, and no picture.
async fn reshapes(
    events: &mut tokio::sync::mpsc::Receiver<CdpEvent>,
    window: Duration,
) -> Vec<Value> {
    let deadline = std::time::Instant::now() + window;
    let mut out = Vec::new();
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), events.recv()).await {
            Ok(Some(event)) if event.method == "Fixture.input" => {
                if event.params["method"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("Emulation.setDeviceMetricsOverride")
                {
                    out.push(event.params["params"].clone());
                }
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => panic!("the browser's pipe closed"),
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shape_of_a_new_sender_waits_for_the_end_of_its_gesture() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (mut browser, mut events) =
        Browser::start(params(), td.path().join("profile")).expect("the browser starts");
    tokio::time::timeout(MARKER, browser.ready())
        .await
        .expect("within the failure-marker window")
        .expect("the sandbox check passes");
    browser
        .in_open("card-1", "https://example.com/a", "default", None)
        .await
        .expect("a page");
    // The page is opened at the application's shape, and that one call is the
    // only reshape so far.
    assert!(
        !reshapes(&mut events, Duration::from_millis(600))
            .await
            .is_empty(),
        "(a) the page is switched to its opening shape"
    );

    let mut admitted = browser
        .join(
            "card-1",
            &json!({"viewport": {"width": 390, "height": 844, "dpr": 3.0, "mobile": true}}),
        )
        .await
        .expect("a viewer");

    // (b) The two frames a viewer sends before it can possibly have seen a new
    // picture must leave the page exactly as it drew that picture.
    for frame in [
        r#"{"type":"pointer","kind":"move","x":640,"y":400}"#,
        r#"{"type":"pointer","kind":"down","x":640,"y":400}"#,
    ] {
        let (refused, _) = browser.on_input("card-1", admitted.id, frame).await;
        assert!(refused.is_none(), "{refused:?}");
    }
    let during = reshapes(&mut events, Duration::from_millis(900)).await;
    assert!(
        during.is_empty(),
        "(b) the layout moved under a gesture already aimed: {during:?}"
    );
    assert_eq!(
        browser.register.pages["card-1"].viewport.width, 1280,
        "(b) and the register says the page is still the size the finger saw"
    );

    // (c) The release ends the gesture, and that is when the page becomes the
    // sender's — R-G5, one gesture later.
    let (refused, _) = browser
        .on_input(
            "card-1",
            admitted.id,
            r#"{"type":"pointer","kind":"up","x":640,"y":400}"#,
        )
        .await;
    assert!(refused.is_none(), "{refused:?}");
    let after = reshapes(&mut events, Duration::from_millis(900)).await;
    let switched = after
        .first()
        .expect("(c) the release switches the page to the shape of whoever touched");
    assert_eq!(switched["width"], 390.0);
    assert_eq!(switched["height"], 844.0);
    assert_eq!(
        browser.register.pages["card-1"].viewport.width, 390,
        "(c) and the register says the same thing the page does"
    );

    while admitted.link.from_cell.try_recv().is_ok() {}
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

/// The rule itself, in one place: positional inputs never, the rest always.
#[test]
fn only_an_input_without_coordinates_or_the_end_of_a_gesture_reshapes() {
    let yes = [
        r#"{"type":"pointer","kind":"up","x":1,"y":1}"#,
        r#"{"type":"touch","kind":"end","points":[{"x":1,"y":1}]}"#,
        r#"{"type":"key","kind":"down","key":"Enter","code":"Enter","text":"","mods":0}"#,
        r#"{"type":"text","text":"Grüße 🦊"}"#,
        r#"{"type":"navigate","url":"about:back"}"#,
    ];
    let no = [
        r#"{"type":"pointer","kind":"move","x":1,"y":1}"#,
        r#"{"type":"pointer","kind":"down","x":1,"y":1}"#,
        r#"{"type":"touch","kind":"start","points":[{"x":1,"y":1}]}"#,
        r#"{"type":"touch","kind":"move","points":[{"x":1,"y":1}]}"#,
        r#"{"type":"wheel","x":1,"y":1,"dx":0,"dy":120}"#,
    ];
    for frame in yes {
        let parsed = Input::parse(frame).expect("a frame this cell knows");
        assert!(reshapes_the_page(&parsed), "{frame} may reshape the page");
    }
    for frame in no {
        let parsed = Input::parse(frame).expect("a frame this cell knows");
        assert!(
            !reshapes_the_page(&parsed),
            "{frame} is aimed at a layout and must not move it"
        );
    }
}
