//! GH #766 (wave G, g8) — a second viewer joins a page, and the stream lives.
//!
//! **B-G20** (`plans/welle-g-2026-09-15/berichte/g4.md` § Beweis, dritter Lauf):
//! one viewer on an animating page read 100 → 195 frames in 5 s (19 fps) with a
//! `1280×800` buffer. With two or three viewers at once every one of them saw
//! `frames {}` and the canvas kept its HTML default size — and afterwards a
//! viewer left alone on that same page never saw a picture again either. The
//! page stayed `active`, and the journal carried no screencast error. G2, G5,
//! G11 and G13 all hang off this.
//!
//! The contract asks for exactly two things here (§ 2, R-G5, OR-G8): every
//! viewer sees something, and all of them see the SAME stream.
//!
//! Measured on 2026-09-20 against Chromium 153.0.8010.36 (snap) over the same
//! `--remote-debugging-pipe` the cell uses, driving the cell's own join
//! sequence by hand (`plans/welle-g-2026-09-15/berichte/g8.md` § die Messung):
//!
//! | what was asked | what the browser answered |
//! |---|---|
//! | `Page.startScreencast` on a moving page, every frame acknowledged | 90 frames in 5 s |
//! | `Page.stopScreencast` + `Page.startScreencast` (the cell's keyframe) | `{}` and `{}` — no error, 89 frames in the next 5 s |
//! | the same pair a second time | `{}` and `{}` — 90 frames in the next 5 s |
//! | a cast whose frames stop being acknowledged | 0 frames, for ever |
//!
//! So the suspected `-32000 Screencast is already active` never happens: the
//! cell stops before it starts, and stop-and-start is harmless at CDP level.
//! What the last row says is the real shape of this defect, and it is measured
//! here at the seam the substrate drives, not at the probe.
//!
//! The arms need a real browser and SKIP without one (OR-G.g5.4).

use meclaw_cells::browser::{BrowserCommand, BrowserIo, BrowserParams, io};
use meclaw_colony::{Link, LinkFrame, LinkRequest, SurfaceRegistry};
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
        "/tmp/meclaw-g8-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ))
}

/// Read everything a viewer has, for `window`, and count the pictures.
///
/// A viewer that stops reading is given up with `4408` (`client_too_slow`), so
/// a measuring loop that does not drain measures its own silence.
async fn count_for(links: &mut [(&str, Link)], window: Duration) -> Vec<usize> {
    let mut seen = vec![0usize; links.len()];
    let deadline = Instant::now() + window;
    while Instant::now() < deadline {
        for (i, (_who, link)) in links.iter_mut().enumerate() {
            while let Ok(frame) = link.from_cell.try_recv() {
                if matches!(frame, LinkFrame::Binary(_)) {
                    seen[i] += 1;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    seen
}

async fn join(surfaces: &SurfaceRegistry, mount: &str, page: &str) -> Link {
    surfaces
        .open_link(
            mount,
            LinkRequest {
                session: Some(page.to_string()),
                params: json!({"viewport": {"width": 800, "height": 600, "dpr": 1.0}}),
                ..Default::default()
            },
        )
        .await
        .expect("the browser holds the mount")
        .unwrap_or_else(|r| panic!("the join of {page} was refused: {}", r.detail))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_viewer_joins_and_everybody_keeps_seeing_the_same_stream() {
    let Some(chromium) = packaged() else {
        eprintln!(
            "SKIP: no MECLAW_CHROMIUM — what a second join does to a running \
             cast is only measurable at a packaged browser"
        );
        return;
    };
    let profile = own_profile("share");
    let mount = "browser-g8-share";
    let surfaces = Arc::new(SurfaceRegistry::new());
    let params = BrowserParams::parse(&json!({
        "mount": mount,
        "chromium_path": chromium,
        "user_data_dir": profile.to_str().expect("a path"),
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

    let (answer, opened) = tokio::sync::oneshot::channel();
    commands_tx
        .send(BrowserCommand::Open {
            page: "card-1".to_string(),
            url: SPIN.to_string(),
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

    // ------------------------------------------------------------------ (a)
    // One viewer, the ordinary case — the one that was already green.
    let mut viewers = vec![("first", join(&surfaces, mount, "card-1").await)];
    let alone = count_for(&mut viewers, Duration::from_secs(4)).await;
    assert!(
        alone[0] >= 10,
        "(a) one viewer on a page that never stops moving saw {} pictures in 4 s",
        alone[0]
    );

    // ------------------------------------------------------------------ (b)
    // The second output joins the same page. Both have to keep seeing it.
    viewers.push(("second", join(&surfaces, mount, "card-1").await));
    let together = count_for(&mut viewers, Duration::from_secs(5)).await;
    assert!(
        together.iter().all(|n| *n >= 10),
        "(b) B-G20: a second viewer joined the page and the stream stopped for \
         everybody — pictures in 5 s: first={} second={} (before the join the \
         first was reading {} in 4 s)",
        together[0],
        together[1],
        alone[0]
    );

    // ------------------------------------------------------------------ (c)
    // And a third, because the proof run drives three outputs.
    viewers.push(("third", join(&surfaces, mount, "card-1").await));
    let three = count_for(&mut viewers, Duration::from_secs(5)).await;
    assert!(
        three.iter().all(|n| *n >= 10),
        "(c) B-G20: with three viewers on one page the stream stopped — \
         pictures in 5 s: {three:?}"
    );

    // ------------------------------------------------------------------ (d)
    // The others go. Whoever is left alone still sees the page.
    viewers.truncate(1);
    let after = count_for(&mut viewers, Duration::from_secs(4)).await;
    assert!(
        after[0] >= 10,
        "(d) B-G20: after the other viewers left, the one left alone never saw \
         a picture again — {} in 4 s",
        after[0]
    );

    drop(viewers);
    drop(commands_tx);
    tokio::time::timeout(MARKER, &mut half)
        .await
        .expect("the half ends when the handler goes")
        .expect("and it ends without panicking");
}
