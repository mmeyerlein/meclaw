//! GH #1006: the numbers behind the backlog signal, measured once and alone.
//!
//! `#[ignore]`: these are wall-clock and cost figures, and a gate that runs
//! them beside a parallel suite measures its neighbours (four red gates on
//! these numbers alone, every other test green). The function and edge locks
//! stay in the gate (`gh1006_a_slow_viewer_is_reported.rs`, the meter's and
//! the fan-out's unit locks); this test prints every number and checks it
//! against the plan's bound:
//!
//! - T1: `high` ≤ 500 ms after the crossing, by the cell's own clock;
//! - T3: 60 s of a viewer that stays behind, ≤ 61 reports;
//! - T5: a fast viewer's backlog ≈ 0 (at most two frames outstanding);
//! - T7: the plan's app (halves on `high`, doubles on `clear`) keeps the slow
//!   viewer at 0 resyncs and its oldest frame at p95 ≤ 1 s, and the fast
//!   viewers at ≥ 90 % of their frames per second without the slow one; the
//!   deaf app at the same load does not (resyncs > 0 or p95 > 2 s). A second
//!   app that reacts to the last level it heard prints its line too, as a
//!   number and no verdict;
//! - T8: counting costs ≤ 5 % of the push it rides on.
//!
//! Run: `scripts/strand.sh test 'test(=gh1006_measure_backlog_numbers)' --run-ignored`
//! or any nextest run with `--run-ignored only` and this filter.

#[path = "support/web_fixture.rs"]
mod web_fixture;

use meclaw_cells::web::WebEvent;
use meclaw_cells::web::backlog::{BacklogPolicy, Meter, Outbox, Queued};
use meclaw_cells::web::socket::ViewerMsg;
use meclaw_core::serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use web_fixture::backlog_lab::{App, HIGH_MS, Lab, Until, lab};

/// One run of the lab, as a line.
fn line(name: &str, l: &Lab) -> String {
    format!(
        "{name}: {:.1} s, {} passes, resyncs {}, oldest p95 {} ms (max {} ms), fast {:.2} frames/s, \
         fast max {} B, frame {} B, high {} clear {}, first-high lag {:?} ms, fast reports {}",
        l.secs,
        l.passes,
        l.resyncs,
        l.p95_ms,
        l.max_ms,
        l.fast_fps,
        l.fast_max_bytes,
        l.frame_bytes,
        l.highs,
        l.clears,
        l.first_high_lag_ms,
        l.fast_reports
    )
}

/// T8: the meter against the push it rides on — encoding the diff and a
/// channel, ten viewers, a 400-slot diff. Best of forty interleaved short
/// rounds each way, so a disturbed round does not count.
fn counting_cost() -> f64 {
    let slots: meclaw_core::serde_json::Map<String, meclaw_core::JsonValue> = (0..400)
        .map(|i| {
            (
                i.to_string(),
                json!(format!("<i data-x=\"{i}\" data-y=\"{i}\">fig {i}</i>")),
            )
        })
        .collect();
    let diff = meclaw_core::JsonValue::Object(slots);
    const ROUNDS: usize = 20;
    const VIEWERS: usize = 10;
    let encode = || meclaw_surface::frames::push(&json!("1"), "lv:c", "diff", diff.clone());
    let reporting = || {
        let (tx, rx) = mpsc::channel::<Queued>(4);
        let (etx, erx) = mpsc::channel::<WebEvent>(64);
        let policy = BacklogPolicy {
            report: true,
            ..BacklogPolicy::default()
        };
        let meter = Arc::new(Meter::new(policy, Some(etx)));
        meter.joined("/", "s1");
        (Outbox::new(tx, meter), rx, erx)
    };
    let mut plain: Vec<(mpsc::Sender<ViewerMsg>, mpsc::Receiver<ViewerMsg>)> =
        (0..VIEWERS).map(|_| mpsc::channel(4)).collect();
    let mut metered: Vec<(Outbox, mpsc::Receiver<Queued>, mpsc::Receiver<WebEvent>)> =
        (0..VIEWERS).map(|_| reporting()).collect();
    let mut without = Duration::MAX;
    let mut with = Duration::MAX;
    for _ in 0..40 {
        let t = Instant::now();
        for _ in 0..ROUNDS {
            for (tx, rx) in plain.iter_mut() {
                tx.try_send(ViewerMsg::Frame(encode())).expect("room");
                while let Ok(m) = rx.try_recv() {
                    let _ = std::hint::black_box(m);
                }
            }
        }
        without = without.min(t.elapsed());
        let t = Instant::now();
        for _ in 0..ROUNDS {
            for (out, rx, _) in metered.iter_mut() {
                out.try_send(ViewerMsg::Frame(encode())).expect("room");
                while let Ok(q) = rx.try_recv() {
                    out.meter().writing(q.at);
                    out.meter().written(q.at, q.bytes);
                    let _ = std::hint::black_box(q);
                }
            }
        }
        with = with.min(t.elapsed());
    }
    eprintln!("gh1006 MEASURE T8: without {without:?}, with {with:?}");
    with.as_secs_f64() / without.as_secs_f64()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "wall-clock and cost numbers; run alone (see the module doc)"]
async fn gh1006_measure_backlog_numbers() {
    const RUN: Duration = Duration::from_secs(20);
    let mut misses: Vec<String> = Vec::new();
    let mut check = |ok: bool, what: String| {
        eprintln!("gh1006 MEASURE {} {what}", if ok { "OK  " } else { "MISS" });
        if !ok {
            misses.push(what);
        }
    };

    let base = lab(false, App::Deaf, Until::After(RUN)).await;
    eprintln!("gh1006 MEASURE {}", line("without the slow viewer", &base));
    let plan = lab(true, App::Plan, Until::After(RUN)).await;
    eprintln!(
        "gh1006 MEASURE {}",
        line("plan app (halve on high, double on clear)", &plan)
    );
    eprintln!("gh1006 MEASURE plan app tranches: {:?}", plan.reactions);
    let stateful = lab(true, App::Stateful, Until::After(RUN)).await;
    eprintln!(
        "gh1006 MEASURE {}",
        line("stateful app (number only)", &stateful)
    );
    let deaf = lab(true, App::Deaf, Until::After(RUN)).await;
    eprintln!("gh1006 MEASURE {}", line("deaf app", &deaf));
    let minute = lab(true, App::Deaf, Until::After(Duration::from_secs(60))).await;
    eprintln!("gh1006 MEASURE {}", line("deaf app, 60 s", &minute));
    let cost = counting_cost();

    let lag = deaf.first_high_lag_ms.unwrap_or(u64::MAX);
    check(
        lag <= 500,
        format!(
            "T1 high {lag} ms after the crossing (cell clock, threshold {HIGH_MS} ms), plan ≤ 500"
        ),
    );
    let reports = minute.highs + minute.clears;
    check(
        (1..=61).contains(&reports),
        format!(
            "T3 {reports} reports in {:.1} s of backlog, plan ≤ 61",
            minute.secs
        ),
    );
    check(
        deaf.fast_max_bytes <= 2 * deaf.frame_bytes,
        format!(
            "T5 fast viewer at most {} B outstanding, plan ≈ 0 (≤ two frames = {} B)",
            deaf.fast_max_bytes,
            2 * deaf.frame_bytes
        ),
    );
    check(
        plan.resyncs == 0,
        format!("T7 plan app {} resyncs, plan 0", plan.resyncs),
    );
    check(
        plan.p95_ms <= 1000,
        format!(
            "T7 plan app oldest frame p95 {} ms, plan ≤ 1000",
            plan.p95_ms
        ),
    );
    check(
        plan.fast_fps >= 0.9 * base.fast_fps,
        format!(
            "T7 fast viewers {:.2} frames/s with the slow one vs {:.2} without ({:.1} %), plan ≥ 90 %",
            plan.fast_fps,
            base.fast_fps,
            100.0 * plan.fast_fps / base.fast_fps.max(f64::MIN_POSITIVE)
        ),
    );
    check(
        deaf.resyncs > 0 || deaf.p95_ms > 2000,
        format!(
            "T7 deaf app resyncs {}, oldest frame p95 {} ms, plan resyncs > 0 or > 2000 ms",
            deaf.resyncs, deaf.p95_ms
        ),
    );
    check(
        cost <= 1.05,
        format!("T8 counting costs ×{cost:.3} of the plain push, plan ≤ 1.05"),
    );
    assert!(misses.is_empty(), "missed the plan's bounds: {misses:#?}");
}
