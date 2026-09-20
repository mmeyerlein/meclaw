//! GH #766 (wave G, g7) — every page of one browser opens, and every one draws.
//!
//! Two defects of the second proof run (`plans/welle-g-2026-09-15/berichte/g4.md`
//! § Beweis, zweiter Lauf) meet here, and they turn out to be one:
//!
//! * **B-G13** — after a restart only the FIRST row of the `pages` table came
//!   back; the others were refused with `Target.createTarget: Target position
//!   can only be set for new windows`;
//! * **B-G17** — no picture reached any viewer at all, on a page that animates
//!   forever and says `active`.
//!
//! Measured on 2026-09-20 against Chromium 153.0.8010.36 (snap) over the same
//! `--remote-debugging-pipe` the cell uses:
//!
//! | what was asked | what the browser answered |
//! |---|---|
//! | `Target.createTarget` with `width`/`height`, no `newWindow`, FIRST target of a fresh context | ok — the context has no window yet, so this one makes it |
//! | the same call, second and third target of that context | `Target position can only be set for new windows` |
//! | the same call with `newWindow: true` | ok, every time, each in a window of its own |
//! | three such windows, each with a screencast, every frame acknowledged | 30 frames in 6 s EACH |
//!
//! So the size a page is opened at and a second page of the same context were
//! mutually exclusive, and the cell asked for both.
//!
//! The arms need a real browser and SKIP without one (OR-G.g5.4): the double
//! next door answers `Target.createTarget` whatever is in it.

use meclaw_cells::browser::{BrowserCommand, BrowserEvent, BrowserIo, BrowserParams, io};
use meclaw_colony::{LinkFrame, LinkRequest, SurfaceRegistry};
use meclaw_core::Path;
use meclaw_core::serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The failure-marker window (30 s convention), never a budget.
const MARKER: Duration = Duration::from_secs(30);

/// A page that never stops moving, so a missing frame is the cell's doing.
const SPIN: &str = "data:text/html,<title>spin</title><style>@keyframes s{from{transform:rotate(0)}\
     to{transform:rotate(360deg)}}div{width:200px;height:200px;background:red;\
     animation:s 1s linear infinite}</style><body style='margin:0'><div></div></body>";

fn packaged() -> Option<String> {
    match std::env::var("MECLAW_CHROMIUM") {
        Ok(p) if !p.trim().is_empty() => Some(p),
        _ => None,
    }
}

fn own_profile(tag: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!(
        "/tmp/meclaw-g7-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ))
}

fn real_params(chromium: &str, profile: &std::path::Path) -> BrowserParams {
    BrowserParams::parse(&json!({
        "mount": "browser-g7",
        "chromium_path": chromium,
        "user_data_dir": profile.to_str().expect("a path"),
        "sandbox": {"trust": "trusted"},
    }))
    .expect("the packaged browser's params parse")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_page_of_one_browser_opens_and_draws() {
    let Some(chromium) = packaged() else {
        eprintln!(
            "SKIP: no MECLAW_CHROMIUM — a second page of one context and a picture \
             for it are only measurable at a packaged browser"
        );
        return;
    };
    let profile = own_profile("draws");
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(256);
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
    // Drain what the half says, so a full channel never becomes the finding.
    tokio::spawn(async move { while events_rx.recv().await.is_some() {} });

    // ------------------------------------------------------------------ (a)
    // Three pages, ONE context — the ordinary shape: an application opens a
    // second card for the same conversation.
    let pages = ["card-1", "card-2", "card-3"];
    let mut refused = Vec::new();
    for page in pages {
        let (answer, opened) = tokio::sync::oneshot::channel();
        commands_tx
            .send(BrowserCommand::Open {
                page: page.to_string(),
                url: SPIN.to_string(),
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
            Ok(_) => {}
            Err(e) => refused.push(format!("{page}: {} {}", e.error_code(), e.detail())),
        }
    }
    assert!(
        refused.is_empty(),
        "(a) B-G13: a second page of one context never opened, so a restart \
         brought back only the first row of the `pages` table: {refused:?}"
    );

    // ------------------------------------------------------------------ (b)
    // One viewer per page, through the door a display uses, and then count.
    let mut links = Vec::new();
    for page in pages {
        let link = surfaces
            .open_link(
                "browser-g7",
                LinkRequest {
                    session: Some(page.to_string()),
                    params: json!({"viewport": {"width": 800, "height": 600, "dpr": 1.0}}),
                    ..Default::default()
                },
            )
            .await
            .expect("the browser holds the mount")
            .unwrap_or_else(|r| panic!("(b) the join of {page} was refused: {}", r.detail));
        links.push((page, link));
    }
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut seen = vec![0usize; pages.len()];
    while Instant::now() < deadline {
        for (i, (_page, link)) in links.iter_mut().enumerate() {
            while let Ok(frame) = link.from_cell.try_recv() {
                if matches!(frame, LinkFrame::Binary(_)) {
                    seen[i] += 1;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let counted: Vec<String> = pages
        .iter()
        .zip(seen.iter())
        .map(|(p, n)| format!("{p}={n}"))
        .collect();
    assert!(
        seen.iter().all(|n| *n >= 3),
        "(b) B-G17: a page that animates forever, `active` and with a viewer on \
         it, sent no picture — frames in 6 s: {counted:?}"
    );

    drop(commands_tx);
    tokio::time::timeout(MARKER, &mut half)
        .await
        .expect("the half ends when the handler goes")
        .expect("and it ends without panicking");
}

/// The browser double, for the arm that needs no browser.
const FIXTURE: &str = env!("CARGO_BIN_EXE_cdp_browser_fixture");

/// GH #766 (wave G, g7) — the cell's own acknowledgement arm, at the seam.
///
/// `run_io`'s `sleep_until(ack_at) → flush_acks` arm is the whole flow control
/// of a screencast, and until now no test drove it: every test of this cell
/// either called `flush_acks` by hand or ran one of the three copies of the
/// event loop, which acknowledge whenever 50 ms pass without a frame rather
/// than at `next_ack()` (g5 fix round 2, "the other arms of `run_io`"). Take
/// the arm out of `run_io` and the whole suite stayed green — while every
/// screencast in the field stood after its third frame, which is exactly the
/// picture B-G17 describes.
///
/// So this arm asks the one question a second copy of the loop cannot answer:
/// does a viewer keep getting pictures past the browser's in-flight window?
/// It needs the double to be as mean as a browser about it, which it is since
/// this strand (`cdp_browser_fixture.rs`, `IN_FLIGHT`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cell_keeps_acknowledging_so_the_picture_keeps_coming() {
    let profile = own_profile("acks");
    let surfaces = Arc::new(SurfaceRegistry::new());
    let params = BrowserParams::parse(&json!({
        "mount": "browser-g7-acks",
        "chromium_path": FIXTURE,
        "extra_args": ["--fixture-mode=own-namespace", "--fixture-frame-ms=5"],
        "user_data_dir": profile.to_str().expect("a path"),
        "sandbox": {"trust": "trusted"},
    }))
    .expect("the fixture params parse");
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(256);
    let (commands_tx, commands_rx) = tokio::sync::mpsc::channel(16);
    let mut half = tokio::spawn(io::run_io(
        BrowserIo::new(
            params,
            profile.clone(),
            Path::new("/browser"),
            Arc::clone(&surfaces),
        ),
        events_tx,
        commands_rx,
    ));
    tokio::spawn(async move { while events_rx.recv().await.is_some() {} });

    let (answer, opened) = tokio::sync::oneshot::channel();
    commands_tx
        .send(BrowserCommand::Open {
            page: "card-1".to_string(),
            url: "https://example.com/".to_string(),
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
    let mut link = surfaces
        .open_link(
            "browser-g7-acks",
            LinkRequest {
                session: Some("card-1".to_string()),
                params: json!({"viewport": {"width": 800, "height": 600, "dpr": 1.0}}),
                ..Default::default()
            },
        )
        .await
        .expect("the browser holds the mount")
        .unwrap_or_else(|r| panic!("the join was refused: {}", r.detail));

    // Well past the three a cast sends unasked, and short of a budget: at the
    // default twenty frames a second this is reached in about half a second.
    let want = 12;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = 0usize;
    while Instant::now() < deadline && seen < want {
        while let Ok(frame) = link.from_cell.try_recv() {
            if matches!(frame, LinkFrame::Binary(_)) {
                seen += 1;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        seen >= want,
        "the cell stopped acknowledging, so the browser stopped drawing: {seen} \
         pictures reached the viewer in 10 s, and a cast that is never answered \
         sends three and stands"
    );

    drop(commands_tx);
    tokio::time::timeout(MARKER, &mut half)
        .await
        .expect("the half ends when the handler goes")
        .expect("and it ends without panicking");
}

/// A field whose place depends on how wide the page is, and which says so.
///
/// `left: 50%` is the whole trick: in the shape a page is opened at the field
/// sits at x=640, in the shape a phone joins with it sits at x=195. The input
/// handler navigates, because a navigation is the one thing this cell reports
/// of its own accord — the title is only fetched after a load.
const FORM: &str = "data:text/html,<title>form</title><body style='margin:0'>\
     <input id=f style='position:absolute;left:50%;top:20px;width:40px;height:40px;margin-left:-20px'>\
     <script>f.addEventListener('input',()=>{location.href='https://example.invalid/?typed='+\
     encodeURIComponent(f.value)})</script></body>";

/// GH #766 (wave G, g7) — B-G16: what is typed reaches the field.
///
/// The proof run measured `Input.insertText 'Gruesse 42 🦊'` going out and the
/// field staying empty, and G3 measured a click that never reached the page.
/// Both are the same line. Measured 2026-09-20 at Chromium 153.0.8010.36 over
/// this pipe: the cell's own sequence — `mouseMoved`, `mousePressed`,
/// `mouseReleased`, then `Input.insertText` — focuses the field and fills it,
/// exactly as written. What broke it is the ORDER in `on_input`: the sender's
/// shape was pushed into the page with `Emulation.setDeviceMetricsOverride`
/// BEFORE the input was dispatched, so every first tap of every output landed
/// in a layout that had just changed under it and that the sender had never
/// seen a picture of. A tap that misses focuses nothing, and text typed into
/// nothing goes nowhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_a_viewer_types_lands_in_the_field_it_tapped() {
    let Some(chromium) = packaged() else {
        eprintln!(
            "SKIP: no MECLAW_CHROMIUM — where a tap lands is a question about \
             layout, and the double has none"
        );
        return;
    };
    let profile = own_profile("typed");
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(256);
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
            url: FORM.to_string(),
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

    // The load first, and on the cell's own signal rather than on a clock: a
    // tap dispatched into a document that has not laid out yet lands nowhere,
    // and that made this arm flaky before it made it wrong. `after_load` is
    // what puts the page's own title into a `page` emission, so the emission
    // carrying it is the load.
    let load_by = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() < load_by,
            "the form page never finished loading"
        );
        if let Ok(Some(BrowserEvent::Page(report))) =
            tokio::time::timeout(Duration::from_millis(200), events_rx.recv()).await
            && report.title == "form"
        {
            break;
        }
    }

    // A viewer with a shape of its own — a phone, the ordinary second output.
    let link = surfaces
        .open_link(
            "browser-g7",
            LinkRequest {
                session: Some("card-1".to_string()),
                params: json!({"viewport": {"width": 390, "height": 844, "dpr": 3.0}}),
                ..Default::default()
            },
        )
        .await
        .expect("the browser holds the mount")
        .unwrap_or_else(|r| panic!("the join was refused: {}", r.detail));
    // The coordinates the client computes come from the head of the picture it
    // has, and the picture it has was drawn at the shape the page was OPENED
    // at: 1280 wide, so the field is at x=640.
    for frame in [
        r#"{"type":"pointer","kind":"down","x":640,"y":40}"#,
        r#"{"type":"pointer","kind":"up","x":640,"y":40}"#,
        r#"{"type":"text","text":"Gruesse 42"}"#,
    ] {
        link.to_cell
            .send(LinkFrame::Text(frame.to_string()))
            .await
            .expect("the viewer's frame goes in");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut urls: Vec<String> = Vec::new();
    let landed = loop {
        if Instant::now() >= deadline {
            break false;
        }
        match tokio::time::timeout(Duration::from_millis(200), events_rx.recv()).await {
            Ok(Some(BrowserEvent::Page(report))) => {
                if report
                    .row
                    .url
                    .starts_with("https://example.invalid/?typed=")
                {
                    break true;
                }
                urls.push(report.row.url.chars().take(60).collect());
            }
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => break false,
        }
    };
    assert!(
        landed,
        "B-G16: the tap focused nothing and the text went nowhere — the page \
         never navigated; addresses seen: {urls:?}"
    );

    drop(commands_tx);
    tokio::time::timeout(MARKER, &mut half)
        .await
        .expect("the half ends when the handler goes")
        .expect("and it ends without panicking");
}

/// GH #766 (wave G, g7) — B-G15: the page ceiling is reachable again.
///
/// The proof run read its whole log (22 728 lines) and found not one
/// `too_many_pages`, though `max_pages` was 8 and sixteen pages had been
/// opened. The guard was never missing — `gh766_a_page_is_a_row_and_a_state`
/// has pinned it at the double since T4. It was UNREACHABLE: with B-G13 the
/// cell could hold exactly one page per context, so `register.pages.len()`
/// never grew to the ceiling and the receipt could not be produced. This arm
/// asks the ceiling at a real browser, which is where the counting failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_page_ceiling_is_reached_at_a_real_browser() {
    let Some(chromium) = packaged() else {
        eprintln!("SKIP: no MECLAW_CHROMIUM — a ceiling a cell never reaches is the finding");
        return;
    };
    let profile = own_profile("ceiling");
    let surfaces = Arc::new(SurfaceRegistry::new());
    let params = BrowserParams::parse(&json!({
        "mount": "browser-g7-ceiling",
        "chromium_path": chromium,
        "user_data_dir": profile.to_str().expect("a path"),
        "max_pages": 2,
        "sandbox": {"trust": "trusted"},
    }))
    .expect("the packaged browser's params parse");
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(256);
    let (commands_tx, commands_rx) = tokio::sync::mpsc::channel(16);
    let mut half = tokio::spawn(io::run_io(
        BrowserIo::new(
            params,
            profile.clone(),
            Path::new("/browser"),
            Arc::clone(&surfaces),
        ),
        events_tx,
        commands_rx,
    ));
    tokio::spawn(async move { while events_rx.recv().await.is_some() {} });
    let mut codes = Vec::new();
    for page in ["card-1", "card-2", "card-3"] {
        let (answer, opened) = tokio::sync::oneshot::channel();
        commands_tx
            .send(BrowserCommand::Open {
                page: page.to_string(),
                url: SPIN.to_string(),
                context: "default".to_string(),
                viewport: None,
                answer,
            })
            .await
            .expect("the half takes the verb");
        codes.push(
            match tokio::time::timeout(MARKER, opened)
                .await
                .expect("within the failure-marker window")
                .expect("the half answers")
            {
                Ok(_) => "opened".to_string(),
                Err(e) => e.error_code().to_string(),
            },
        );
    }
    assert_eq!(
        codes,
        vec!["opened", "opened", "too_many_pages"],
        "B-G15: with `max_pages` 2 the third page is refused in this cell's own \
         words, and nothing is displaced silently (OR-G7)"
    );
    drop(commands_tx);
    tokio::time::timeout(MARKER, &mut half)
        .await
        .expect("the half ends when the handler goes")
        .expect("and it ends without panicking");
}
