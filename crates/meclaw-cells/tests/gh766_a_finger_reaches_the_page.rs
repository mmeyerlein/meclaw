//! GH #766 (wave G, T7) — a finger on the screen is a click in the page.
//!
//! The way back of the link. Six shapes of text (OR-G25), and not one of them a
//! message: a keystroke per message would put a person's typing through the
//! router, the log and the edge table, and the link the picture comes down is
//! already open and already carries the page in its topic.
//!
//! Three of the mappings were measured rather than reasoned about (befund 02
//! § D), and each has an arm here: a click is THREE calls, touch needs no
//! emulation switch, and a run of text is one `Input.insertText` while Enter
//! stays a key of its own.
//!
//! Two more arms are about what the page says back. `url` and `title` come from
//! `Target.targetInfoChanged` with the fragment intact (OR-G39), and they are
//! still intact on the second emission — the app's PDF anchor is a fragment,
//! and an address that lost it would be navigated again forever (OR-G37). And
//! `content_type` is asked once per load and left EMPTY when nobody answers,
//! rather than guessed at.

use meclaw_cells::browser::{Browser, BrowserParams, CdpEvent, PageState};
use meclaw_core::serde_json::{Value, json};
use std::time::Duration;

/// The browser double. Started through the same shim a real browser is.
const FIXTURE: &str = env!("CARGO_BIN_EXE_cdp_browser_fixture");

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

fn params(extra_args: Vec<&str>, extra: Value) -> BrowserParams {
    let mut args = vec![
        "--fixture-mode=own-namespace".to_string(),
        "--fixture-frame-ms=5".to_string(),
    ];
    args.extend(extra_args.into_iter().map(str::to_string));
    let mut v = json!({
        "chromium_path": FIXTURE,
        "extra_args": args,
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

/// A started browser holding one page, and the events it will produce.
async fn with_one_page(
    extra_args: Vec<&str>,
    extra: Value,
) -> (
    Browser,
    tokio::sync::mpsc::Receiver<CdpEvent>,
    tempfile::TempDir,
) {
    let td = tempfile::TempDir::new().expect("tempdir");
    let (mut browser, events) =
        Browser::start(params(extra_args, extra), td.path().join("profile"))
            .expect("the browser starts");
    tokio::time::timeout(MARKER, browser.ready())
        .await
        .expect("within the failure-marker window")
        .expect("the sandbox check passes");
    browser
        .in_open("card-1", "https://example.com/a", "default", None)
        .await
        .expect("a page");
    (browser, events, td)
}

/// The next `want` inputs the browser echoed, ignoring its picture.
///
/// Bounded by a count rather than by a pause: a page being watched produces
/// frames for as long as it is watched, so "read until it goes quiet" would
/// never return.
async fn inputs(
    events: &mut tokio::sync::mpsc::Receiver<CdpEvent>,
    want: usize,
) -> Vec<(String, Value)> {
    echoes(events, want, "Input.").await
}

/// The next `want` calls the browser echoed whose method starts with
/// `want_prefix`, ignoring its picture.
async fn echoes(
    events: &mut tokio::sync::mpsc::Receiver<CdpEvent>,
    want: usize,
    want_prefix: &str,
) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let deadline = std::time::Instant::now() + MARKER;
    while out.len() < want {
        assert!(
            std::time::Instant::now() < deadline,
            "the browser echoed {} of {want} inputs",
            out.len()
        );
        match tokio::time::timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Some(event)) if event.method == "Fixture.input" => {
                let method = event.params["method"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                // The fixture echoes emulation calls on the same event; the
                // input arms count INPUTS, so the page's own shape does not
                // shift their tally.
                if method.starts_with(want_prefix) {
                    out.push((method, event.params["params"].clone()));
                }
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => panic!("the browser's pipe closed"),
        }
    }
    out
}

/// Admit one viewer and return its id.
async fn watcher(browser: &mut Browser) -> u64 {
    browser
        .join("card-1", &json!({}))
        .await
        .expect("a viewer")
        .id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_click_is_three_calls_and_a_double_click_says_so() {
    let (mut browser, mut events, _td) = with_one_page(vec![], json!({})).await;
    let link = watcher(&mut browser).await;
    for frame in [
        r#"{"type":"pointer","kind":"down","x":10,"y":20,"button":"left","clicks":1}"#,
        r#"{"type":"pointer","kind":"up","x":10,"y":20,"button":"left","clicks":1}"#,
    ] {
        let (refused, _) = browser.on_input("card-1", link, frame).await;
        assert!(refused.is_none(), "{refused:?}");
    }
    let seen = inputs(&mut events, 3).await;
    let kinds: Vec<&str> = seen
        .iter()
        .map(|(_, p)| p["type"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        kinds,
        vec!["mouseMoved", "mousePressed", "mouseReleased"],
        "(a) the pointer has to be somewhere before it can be pressed"
    );
    assert_eq!(seen[1].1["x"], 10.0);

    // (b) a second press with a count of two is the double click.
    browser
        .on_input(
            "card-1",
            link,
            r#"{"type":"pointer","kind":"down","x":10,"y":20,"button":"left","clicks":2}"#,
        )
        .await;
    let seen = inputs(&mut events, 2).await;
    assert_eq!(
        seen.last().expect("a press").1["clickCount"],
        json!(2),
        "a second press with a count of two is the double click"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn touch_wheel_and_text_are_what_the_measurement_said() {
    let (mut browser, mut events, _td) = with_one_page(vec![], json!({})).await;
    let link = watcher(&mut browser).await;
    for frame in [
        r#"{"type":"touch","kind":"start","points":[{"x":5,"y":6}]}"#,
        r#"{"type":"wheel","x":5,"y":6,"dx":0,"dy":120}"#,
        r#"{"type":"text","text":"Gruesse"}"#,
        r#"{"type":"key","kind":"down","key":"Enter","code":"Enter"}"#,
    ] {
        let (refused, _) = browser.on_input("card-1", link, frame).await;
        assert!(refused.is_none(), "{frame}: {refused:?}");
    }
    let seen = inputs(&mut events, 4).await;
    let methods: Vec<&str> = seen.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(
        methods,
        vec![
            "Input.dispatchTouchEvent",
            "Input.dispatchMouseEvent",
            "Input.insertText",
            "Input.dispatchKeyEvent",
        ],
        "(c) no emulation switch, (d) a wheel is a mouse event, (e) a run of \
         text is ONE call and Enter is a key of its own"
    );
    assert_eq!(seen[1].1["deltaY"], 120.0, "the wheel scrolled");
    assert_eq!(seen[2].1["text"], "Gruesse");

    // (f) the three history words are history, and a free address is refused.
    browser
        .on_input("card-1", link, r#"{"type":"navigate","url":"about:back"}"#)
        .await;
    let (refused, _) = browser
        .on_input(
            "card-1",
            link,
            r#"{"type":"navigate","url":"https://elsewhere.example/"}"#,
        )
        .await;
    let e = refused.expect("a viewer does not choose the address");
    assert_eq!(e.error_code(), "invalid_input");
    assert!(e.detail().contains("in_navigate"), "{}", e.detail());
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreadable_frame_is_a_refusal_and_never_a_panic() {
    let (mut browser, _events, _td) = with_one_page(vec![], json!({})).await;
    let link = watcher(&mut browser).await;
    for frame in ["not json at all", r#"{"type":"nudge"}"#, "{}"] {
        let (refused, reports) = browser.on_input("card-1", link, frame).await;
        let e = refused.expect("that is not an input");
        assert_eq!(e.error_code(), "invalid_input", "(g) {frame}");
        assert!(reports.is_empty(), "and nothing was told to anybody");
    }
    assert!(
        browser.register.has("card-1"),
        "the page is still standing after three of them"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interaction_is_told_once_and_then_kept_quiet() {
    let (mut browser, _events, _td) = with_one_page(vec![], json!({})).await;
    let link = watcher(&mut browser).await;
    let click = r#"{"type":"pointer","kind":"down","x":1,"y":1}"#;
    let (_, first) = browser.on_input("card-1", link, click).await;
    assert_eq!(first.len(), 1, "(h) the first use is told");
    assert!(
        first[0].active_at.is_some(),
        "and it is told as `active_at`, not as a state change: {:?}",
        first[0]
    );
    assert_eq!(
        first[0].state,
        PageState::Active,
        "the state did not move — the page is being watched either way"
    );
    let (_, second) = browser.on_input("card-1", link, click).await;
    assert!(
        second.is_empty(),
        "(h) and the second, two milliseconds later, is not: the throttle sits \
         in the cell, because two outputs on one page would throttle twice"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

/// The next event of `method` the browser sent, as the cell reads them.
async fn next_event(events: &mut tokio::sync::mpsc::Receiver<CdpEvent>, method: &str) -> CdpEvent {
    let deadline = std::time::Instant::now() + MARKER;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "the browser never sent {method}"
        );
        if let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(200), events.recv()).await
            && event.method == method
        {
            return event;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_address_carries_its_fragment_and_keeps_it() {
    let (mut browser, mut events, _td) = with_one_page(vec![], json!({})).await;
    // (i) What `Target.targetInfoChanged` said — read off the WIRE and handed
    // to the cell's own seam. The arm used to call `note_target_info` itself
    // and then assert the register held what it had just written, which
    // discriminates nothing about OR-G39.
    let with_anchor = "https://example.com/doc.pdf#toolbar=0&navpanes=0&view=FitH";
    browser
        .in_navigate("card-1", with_anchor)
        .await
        .expect("it moves");
    // The page was opened at another address first, so the stream carries that
    // target change too; the one this arm is about is the one that moved.
    let said = loop {
        let event = next_event(&mut events, "Target.targetInfoChanged").await;
        if event.params["targetInfo"]["url"] == with_anchor {
            break event;
        }
    };
    assert_eq!(
        said.params["targetInfo"]["url"], with_anchor,
        "the control: the browser really did send the fragment"
    );
    let out = browser.on_event(said).expect("the seam says something");
    let first = match out {
        meclaw_cells::browser::BrowserEvent::Page(report) => report,
        _ => panic!("the seam turns a target change into a page emission"),
    };
    assert_eq!(
        first.row.url, with_anchor,
        "(i) the full address, fragment and all (OR-G39)"
    );
    assert_eq!(
        first.title, "A fixture page",
        "and the title, which exists nowhere but in that event"
    );

    // (i) and still, on the next emission: a page whose anchor went missing
    // would be navigated again by the app on every report (OR-G37).
    browser.register.set_state("card-1", PageState::Background);
    let second = browser.register.report("card-1").expect("a second report");
    assert_eq!(second.row.url, with_anchor);
    assert_eq!(second.title, "A fixture page");
    assert!(
        browser
            .register
            .note_target_info(
                &browser.register.pages["card-1"]
                    .target
                    .clone()
                    .expect("a target"),
                with_anchor,
                "A fixture page"
            )
            .is_none(),
        "and saying the same thing twice is not news"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_viewport_reaches_the_page_and_not_only_the_report() {
    // (k) R-G5 was bookkeeping: the shape went into the register, the row and
    // the emission, and the PAGE was never switched. The contract calls a
    // pointer's coordinates "CSS pixels of the page viewport", so a page still
    // rendered at the browser's own default puts every tap somewhere else.
    let (mut browser, mut events, _td) = with_one_page(vec![], json!({})).await;
    let opened = echoes(&mut events, 1, "Emulation.").await;
    assert_eq!(
        opened[0].0, "Emulation.setDeviceMetricsOverride",
        "the page is switched as it is opened, before it navigates"
    );
    assert_eq!(opened[0].1["width"], 1280.0, "the default shape");
    assert_eq!(opened[0].1["height"], 800.0);

    // And a viewer with a shape of its own moves the page to it.
    let mut admitted = browser
        .join(
            "card-1",
            &json!({"viewport": {"width": 390, "height": 844, "dpr": 3.0, "mobile": true}}),
        )
        .await
        .expect("a viewer");
    // The RELEASE, not the press: a gesture is not reshaped underneath itself
    // (wave G, g9, finding B-G16 — the rule and its measurement live in
    // `gh766_the_first_tap_lands_in_the_layout_it_was_aimed_at.rs`). The
    // statement of this arm is unchanged: a viewer with a shape of its own
    // moves the page to it, and the page is switched and not only the report.
    for frame in [
        r#"{"type":"pointer","kind":"down","x":1,"y":1}"#,
        r#"{"type":"pointer","kind":"up","x":1,"y":1}"#,
    ] {
        let (refused, _) = browser.on_input("card-1", admitted.id, frame).await;
        assert!(refused.is_none(), "{refused:?}");
    }
    let seen = echoes(&mut events, 1, "Emulation.").await;
    let switched = seen
        .first()
        .expect("the page was switched to the shape the finger is on");
    assert_eq!(switched.1["width"], 390.0);
    assert_eq!(switched.1["height"], 844.0);
    assert_eq!(switched.1["deviceScaleFactor"], 3.0);
    assert_eq!(switched.1["mobile"], true);
    assert_eq!(
        browser.register.pages["card-1"].viewport.width, 390,
        "and the register says the same thing the page does"
    );
    while admitted.link.from_cell.try_recv().is_ok() {}
    browser.reaper.terminate(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_content_type_is_asked_once_and_never_guessed() {
    let (mut browser, _events, _td) = with_one_page(vec![], json!({})).await;
    let report = browser
        .note_content_type("card-1")
        .await
        .expect("the page says what it is");
    assert_eq!(report.content_type, "text/html");
    assert!(
        browser.note_content_type("card-1").await.is_none(),
        "(j) the same answer twice is not a second emission"
    );
    browser.reaper.terminate(Duration::from_millis(500)).await;

    // (j) and a question nobody answers leaves the field EMPTY. A guess would
    // put `text/html` on a PDF, and the app's anchor hangs on this value.
    let (mut mute, _events, _td2) = with_one_page(
        vec!["--fixture-mute-eval"],
        json!({"external_timeout_ms": 400}),
    )
    .await;
    let started = std::time::Instant::now();
    assert!(
        tokio::time::timeout(MARKER, mute.note_content_type("card-1"))
            .await
            .expect("the A-timeout ends it")
            .is_none(),
        "nothing is reported when nothing was learned"
    );
    assert_eq!(mute.register.pages["card-1"].content_type, "");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "and it ends at the A-timeout: {:?}",
        started.elapsed()
    );
    mute.reaper.terminate(Duration::from_millis(500)).await;
}
