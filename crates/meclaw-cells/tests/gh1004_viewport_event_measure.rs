//! GH #1004: the numbers behind the viewport event, measured once and alone.
//!
//! `#[ignore]`: wall-clock figures, and a gate that runs them beside a parallel
//! suite measures its neighbours (ledger OR-H4-20). The function and order
//! locks stay in the gate (`gh1004_viewport_event.rs` and the socket's unit
//! locks); this test prints every number and checks it against the plan's
//! bound (plan W4 § 4, README § 2 A10):
//!
//! - T1: the reply to a semantic event at the socket, while bundles run,
//!   p95 ≤ 40 ms (target 20);
//! - T4: a 150-ms event cadence beside a 250-ms bundle cadence for 20 s: the
//!   gap between consecutive event emissions at the colony p50 ≤ 160 / p95 ≤
//!   200 ms, with 1 and with 3 sockets; the handler's remaining wait (sent →
//!   emitted) is printed, and p95 > 30 ms is a finding for the report, not a
//!   failure here (plan W4 § 2: the handler's priority is not this strand's);
//! - L-W6: one `object:set` open while 20 bundles run: every diff on the socket
//!   ≤ 50 ms after its bundle's answer left the cell.
//!
//! The handler's wait is the app's bundle, and a bundle's cost is the build
//! profile's (gh1001 T3: 400 updates ≈ 139 ms unoptimised, ≤ 20 ms optimised),
//! so the plan's T4 bound is a statement about an optimised build; the test
//! prints the bundle's own `duration_ms` and the profile beside the gaps.
//!
//! Run: `scripts/strand.sh test 'test(=gh1004_measure_viewport_numbers)' --
//! --run-ignored only --cargo-profile release`.

#[path = "support/web_fixture.rs"]
mod web_fixture;

use meclaw_core::serde_json::{Value, json};
use std::time::{Duration, Instant};
use web_fixture::{Lab, Shape, bundle_message, is_event, update};

const FIGURES: usize = 4_000;
const PER_BUNDLE: usize = 400;
/// The deployed hook's cadence (`panReport`, plan W4 § 1).
const EVENT_EVERY: Duration = Duration::from_millis(150);
/// The deployed app's bundle cadence.
const BUNDLE_EVERY: Duration = Duration::from_millis(250);
const RUN: Duration = Duration::from_secs(20);

fn pool_bundle(round: usize) -> Vec<Value> {
    (0..PER_BUNDLE)
        .map(|j| {
            let i = (round * PER_BUNDLE + j) % FIGURES;
            update(&format!("fig-{i}"), json!({"x": (round * 7 + j) as i64}))
        })
        .collect()
}

/// p-quantile of milliseconds (nearest rank).
fn q(samples: &[f64], p: f64) -> f64 {
    if samples.is_empty() {
        return f64::NAN;
    }
    let mut s = samples.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    let idx = ((s.len() - 1) as f64 * p).round() as usize;
    s[idx]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

/// What one cadence run measured.
struct Cadence {
    /// Gap between consecutive emissions of one viewer's events, ms.
    gaps: Vec<f64>,
    /// Sent → emitted, ms.
    waits: Vec<f64>,
    /// Sent → reply at the socket, ms.
    replies: Vec<f64>,
    /// The app's bundles, by the cell's own clock (`duration_ms`): the
    /// handler's wait behind them is what an event pays on top of the hook.
    bundles: Vec<f64>,
}

/// T1 + T4: `viewers` sockets send at the hook's cadence while an app writes
/// at its own.
async fn cadence(viewers: usize) -> Cadence {
    let mut lab = Lab::start(Shape::with_figures(FIGURES)).await;
    let mut socks = Vec::new();
    for _ in 0..viewers {
        socks.push(lab.viewer().await);
    }
    let mut emissions = lab.take_emissions();
    let mailbox = lab.mailbox();
    let t0 = Instant::now();

    // The colony side: every event emission, stamped on arrival.
    let heard = tokio::spawn(async move {
        let mut out: Vec<(Instant, u64, u64)> = Vec::new();
        let mut bundles: Vec<f64> = Vec::new();
        while let Ok(Some(e)) = tokio::time::timeout(Duration::from_secs(5), emissions.recv()).await
        {
            if !is_event(&e) {
                if let Some(d) = e.content["header"]["duration_ms"].as_f64() {
                    bundles.push(d);
                }
            } else {
                let value = &e.content["event"]["value"];
                out.push((
                    Instant::now(),
                    value["v"].as_u64().expect("v"),
                    value["n"].as_u64().expect("n"),
                ));
            }
        }
        (out, bundles)
    });

    // The app: one pool bundle every 250 ms.
    let app = tokio::spawn(async move {
        let mut tick = tokio::time::interval(BUNDLE_EVERY);
        let mut round = 0;
        while t0.elapsed() < RUN {
            tick.tick().await;
            if mailbox
                .send(bundle_message(pool_bundle(round)))
                .await
                .is_err()
            {
                break;
            }
            round += 1;
        }
    });

    // The hook: one viewport event every 150 ms per socket, reading
    // everything the socket is sent in between.
    let mut hooks = Vec::new();
    for (vi, mut v) in socks.into_iter().enumerate() {
        hooks.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(EVENT_EVERY);
            let mut sent: Vec<Instant> = Vec::new();
            let mut replies: Vec<f64> = Vec::new();
            let prefix = format!("v{vi}e");
            loop {
                // The send happens after the `select!`, which has dropped the
                // read by then: both need the socket.
                let due = tokio::select! {
                    _ = tick.tick() => true,
                    frame = v.next_frame() => {
                        if frame[3] == json!("phx_reply")
                            && let Some(r) = frame[1].as_str()
                            && let Some(n) = r.strip_prefix(&prefix)
                            && let Ok(n) = n.parse::<usize>()
                            && let Some(at) = sent.get(n)
                        {
                            replies.push(ms(at.elapsed()));
                        }
                        false
                    }
                };
                if !due {
                    continue;
                }
                if t0.elapsed() >= RUN {
                    break;
                }
                let n = sent.len();
                sent.push(Instant::now());
                v.push_event(
                    &format!("{prefix}{n}"),
                    "viewport",
                    json!({"v": vi, "n": n, "vx": n * 3, "vy": n * 2}),
                )
                .await;
            }
            (sent, replies)
        }));
    }

    let mut sent_by: Vec<Vec<Instant>> = Vec::new();
    let mut replies = Vec::new();
    for h in hooks {
        let (s, r) = h.await.expect("hook");
        sent_by.push(s);
        replies.extend(r);
    }
    app.await.expect("app");
    let (heard, bundles) = heard.await.expect("heard");

    let mut gaps = Vec::new();
    let mut waits = Vec::new();
    for (vi, sent) in sent_by.iter().enumerate() {
        let mine: Vec<&(Instant, u64, u64)> =
            heard.iter().filter(|(_, v, _)| *v == vi as u64).collect();
        for w in mine.windows(2) {
            gaps.push(ms(w[1].0 - w[0].0));
        }
        for (at, _, n) in &mine {
            if let Some(s) = sent.get(*n as usize) {
                waits.push(ms(at.saturating_duration_since(*s)));
            }
        }
        println!(
            "gh1004 T4 viewers={viewers} socket={vi}: sent {} heard {}",
            sent.len(),
            mine.len()
        );
    }
    drop(lab);
    Cadence {
        gaps,
        waits,
        replies,
        bundles,
    }
}

/// L-W6: the delay of each diff behind its bundle's answer, with one
/// `object:set` open the whole time.
async fn behind_an_open_event() -> Vec<f64> {
    const BUNDLES: usize = 20;
    let mut lab = Lab::start(Shape::with_figures(FIGURES)).await;
    let mut v = lab.viewer().await;
    let mut emissions = lab.take_emissions();
    let answers = tokio::spawn(async move {
        let mut out = Vec::new();
        while out.len() < BUNDLES {
            let Ok(Some(e)) = tokio::time::timeout(Duration::from_secs(30), emissions.recv()).await
            else {
                break;
            };
            if !is_event(&e) {
                out.push(Instant::now());
            }
        }
        out
    });
    for round in 0..BUNDLES {
        lab.enqueue(pool_bundle(round)).await;
    }
    v.push_event(
        "s1",
        "object:set",
        json!({"id": "fig-0", "prop": "y", "value": 9}),
    )
    .await;
    let mut diffs = Vec::new();
    let mut replied = false;
    while diffs.len() < BUNDLES || !replied {
        let frame = v.next_frame().await;
        if frame[3] == json!("diff") {
            diffs.push(Instant::now());
        } else if frame[3] == json!("phx_reply") && frame[1] == json!("s1") {
            replied = true;
        }
    }
    let answered = answers.await.expect("answers");
    diffs
        .iter()
        .zip(answered.iter())
        .map(|(d, a)| ms(d.saturating_duration_since(*a)))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "wall-clock numbers: run once, alone (ledger OR-H4-20)"]
async fn gh1004_measure_viewport_numbers() {
    let mut misses = Vec::new();
    for viewers in [1, 3] {
        let c = cadence(viewers).await;
        let (g50, g95) = (q(&c.gaps, 0.5), q(&c.gaps, 0.95));
        let (w50, w95) = (q(&c.waits, 0.5), q(&c.waits, 0.95));
        let (r50, r95) = (q(&c.replies, 0.5), q(&c.replies, 0.95));
        println!(
            "gh1004 T4 viewers={viewers}: gap p50 {g50:.1} / p95 {g95:.1} ms ({} gaps); \
             handler wait p50 {w50:.1} / p95 {w95:.1} ms",
            c.gaps.len()
        );
        println!(
            "gh1004 T4 viewers={viewers}: app bundle p50 {:.1} / p95 {:.1} ms ({} bundles, {} build)",
            q(&c.bundles, 0.5),
            q(&c.bundles, 0.95),
            c.bundles.len(),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "optimised"
            }
        );
        println!(
            "gh1004 T1 viewers={viewers}: reply p50 {r50:.1} / p95 {r95:.1} ms ({} replies)",
            c.replies.len()
        );
        if !(g50 <= 160.0 && g95 <= 200.0) {
            misses.push(format!(
                "T4 viewers={viewers}: gap {g50:.1}/{g95:.1} > 160/200"
            ));
        }
        if r95 > 40.0 {
            misses.push(format!("T1 viewers={viewers}: reply p95 {r95:.1} > 40"));
        }
        if w95 > 30.0 {
            println!("gh1004 FINDING viewers={viewers}: handler wait p95 {w95:.1} ms > 30 ms");
        }
    }
    let delays = behind_an_open_event().await;
    let max = delays.iter().copied().fold(0.0_f64, f64::max);
    println!(
        "gh1004 L-W6: diff behind answer p50 {:.1} / p95 {:.1} / max {max:.1} ms ({} diffs)",
        q(&delays, 0.5),
        q(&delays, 0.95),
        delays.len()
    );
    if max > 50.0 {
        misses.push(format!("L-W6: a diff {max:.1} ms behind its answer > 50"));
    }
    assert!(misses.is_empty(), "plan bounds missed: {misses:#?}");
}
