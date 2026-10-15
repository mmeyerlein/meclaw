//! GH #1001 — a write to the `web` cell costs what it touched, not the page.
//!
//! Measured on a deployed page of this shape (MESSUNG.md § 6 L-W2, MESSUNG-V2c.md § 3): every
//! write re-rendered every route, three SQL reads per object, so a one-op
//! write cost 51–66 ms on a pool of 1 000 figures and 129–150 ms (p50) on
//! 4 000, and a bundle of 400 figure updates 68 ms. These locks run the same
//! page shape in the lab (`support/web_fixture.rs`) through a real cell and
//! read the `duration_ms` the cell answers with.
//!
//! T2 counts the work of a write (objects rendered, held slots read); its
//! milliseconds are the perf stage's (`perf_gh1001_write_cost`). The target of
//! the wave is ≤ 10 ms per write and ≤ 20 ms p95 per 400-leg bundle.

#[path = "support/web_fixture.rs"]
mod web_fixture;

use meclaw_core::serde_json::json;
use meclaw_testing::log_recorder;
use web_fixture::{Lab, Shape, duration_ms, update};

/// The `q`-quantile of `xs` (nearest rank).
fn quantile(xs: &mut [i64], q: f64) -> i64 {
    xs.sort_unstable();
    let rank = ((q * xs.len() as f64).ceil() as usize).clamp(1, xs.len());
    xs[rank - 1]
}

/// One one-op write on `lab` (a page of `figures` figures), its `duration_ms`.
async fn one_op(lab: &mut Lab, figures: usize, i: usize) -> i64 {
    let id = format!("fig-{}", (i * 7919) % figures);
    let answer = lab.call(vec![update(&id, json!({"x": i as i64}))]).await;
    duration_ms(&answer)
}

/// p50 of `runs` one-op writes on a page of `small` and one of `large`
/// figures, and the sorted samples of each.
///
/// GH #1180: the two pools are measured INTERLEAVED, one write each per round
/// and the order flipped every round, so a load burst of the host lands on
/// both pools alike. Measured one pool after the other, a burst during the
/// second pool alone read as growth: under full-suite load (load1 about 26 on
/// 24 cores) the 4 000 pool's whole sample sat at 4-19 ms while the 1 000 pool
/// before it read 1-6 ms, and the same test was green in isolation every time.
async fn paired_p50(small: usize, large: usize, runs: usize) -> ((i64, Vec<i64>), (i64, Vec<i64>)) {
    let mut lab_s = Lab::start(Shape::with_figures(small)).await;
    let mut lab_l = Lab::start(Shape::with_figures(large)).await;
    // One warm-up write each: the first write pays for the statement cache.
    lab_s.call(vec![update("fig-0", json!({"x": 1}))]).await;
    lab_l.call(vec![update("fig-0", json!({"x": 1}))]).await;
    let (mut ds, mut dl) = (Vec::with_capacity(runs), Vec::with_capacity(runs));
    for i in 0..runs {
        if i % 2 == 0 {
            ds.push(one_op(&mut lab_s, small, i).await);
            dl.push(one_op(&mut lab_l, large, i).await);
        } else {
            dl.push(one_op(&mut lab_l, large, i).await);
            ds.push(one_op(&mut lab_s, small, i).await);
        }
    }
    let ps = quantile(&mut ds, 0.5);
    let pl = quantile(&mut dl, 0.5);
    ((ps, ds), (pl, dl))
}

/// The one-op write lock of T2 in whole milliseconds (target 10), read by
/// the perf stage only.
const LOCK_MS: i64 = 20;

/// The work every one-op write of `writes` on `lab` did, as the cell reports
/// it after each re-render: `(objects rendered, held slots read)` per write.
///
/// The cell logs both counts on its `web: re-rendered` line, after the write
/// and before its answer; the test runs one lab at a time, so the lines after
/// a write's answer are that write's.
async fn work_per_write(lab: &mut Lab, figures: usize, writes: usize) -> Vec<(u64, u64)> {
    let log = log_recorder::global();
    let rerenders = || log.lines(tracing::Level::TRACE, &["web: re-rendered", "path=/web"]);
    let mut work = Vec::with_capacity(writes);
    for i in 0..writes {
        let before = rerenders().len();
        one_op(lab, figures, i).await;
        let lines = rerenders();
        let [line] = &lines[before..] else {
            panic!(
                "one write, one re-render on the pool of {figures}: {:?}",
                &lines[before..]
            )
        };
        work.push((field(line, "objects"), field(line, "slots_read")));
    }
    work
}

/// The value of ` name=<n>` in a recorded line.
fn field(line: &str, name: &str) -> u64 {
    let key = format!(" {name}=");
    let at = line
        .find(&key)
        .unwrap_or_else(|| panic!("the re-render line names `{name}`: {line}"));
    line[at + key.len()..]
        .split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("`{name}` is a count: {line}"))
}

/// T2. A one-op write costs what it touched, whatever the pool: the same
/// work on 1 000 figures as on 4 000 -- one object rendered and one held
/// slot read. Before, a write re-rendered every route (all 4 871 objects of
/// the lab page; 578 ms on 1 000, 979 ms on 4 000 in this lab), and after
/// GH #1001 a scan of every held slot still grew with the pool (GH #1180).
/// Both counts are deterministic where the milliseconds were not (GH #1056,
/// #1066: 11 of 20 red under load); the milliseconds moved to
/// [`perf_gh1001_write_cost`].
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1001_write_cost_does_not_grow_with_untouched_objects() {
    let _ = log_recorder::global();
    let mut work = Vec::new();
    for figures in [1_000, 4_000] {
        let mut lab = Lab::start(Shape::with_figures(figures)).await;
        work.push(work_per_write(&mut lab, figures, 5).await);
    }
    let [small, large] = &work[..] else {
        unreachable!("two pools")
    };
    assert!(
        small.iter().all(|w| *w == (1, 1)),
        "a one-op write on a pool of 1 000 renders one figure and reads one held slot: \
         (objects, slots_read) per write {small:?}"
    );
    assert_eq!(
        small, large,
        "the write's work grows with untouched objects: (objects, slots_read) per write \
         1 000 {small:?} vs 4 000 {large:?}"
    );
}

/// The host's 1/5/15-minute load, recorded beside the samples.
fn host_load() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .map(|l| l.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// Perf stage of T2 (VORGABEN TV-P2): the p50 of twenty one-op writes per
/// pool, interleaved (GH #1180), as JSON under `target/perf/`. No verdict on
/// the numbers here; the wave receipt compares them against the merge-base
/// binary. Measured before (lab, gate host): 578 ms (1 000) and 979 ms (4 000)
/// p50; after GH #1001: 1 ms and 2 ms. The lock of the wave is [`LOCK_MS`].
#[ignore = "perf stage"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn perf_gh1001_write_cost() {
    let ((small, mut small_ds), (large, mut large_ds)) = paired_p50(1_000, 4_000, 20).await;
    let p95_small = quantile(&mut small_ds, 0.95);
    let p95_large = quantile(&mut large_ds, 0.95);
    let report = json!({
        "test": "perf_gh1001_write_cost",
        "unit": "ms",
        "lock_ms": LOCK_MS,
        "pool_1000": {"p50": small, "p95": p95_small, "samples": small_ds},
        "pool_4000": {"p50": large, "p95": p95_large, "samples": large_ds},
        "load": host_load(),
    });
    println!("PERF {report}");
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .expect("target/tmp has a parent")
        .join("perf");
    std::fs::create_dir_all(&dir).expect("target/perf");
    std::fs::write(dir.join("perf_gh1001_write_cost.json"), report.to_string())
        .expect("write the perf report");
}

/// T3. Before: the deployed page measured 68 ms for 400 figure updates (pool
/// 1 000). The lock is the build profile's (400 ms unoptimised, 40 ms
/// optimised); the target of 20 ms is measured on an optimised deployment.
// A time lock tears under the parallel suite of a gate (two strand gates of
// this wave, 141–268 ms spread on a debug build), so the number is measured
// alone and read, not gated.
#[ignore = "measurement: run alone (--run-ignored), not in a gate"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1001_a_bundle_of_400_updates_stays_under_its_profile_lock() {
    let mut lab = Lab::start(Shape::with_figures(1_000)).await;
    lab.call(vec![update("fig-0", json!({"x": 1}))]).await;
    let mut ds = Vec::with_capacity(20);
    for run in 0..20 {
        let ops = (0..400)
            .map(|i| {
                update(
                    &format!("fig-{}", (i * 2 + run) % 1_000),
                    json!({"x": (run * 400 + i) as i64, "y": i as i64}),
                )
            })
            .collect();
        let answer = lab.call(ops).await;
        ds.push(duration_ms(&answer));
    }
    let p95 = quantile(&mut ds, 0.95);
    let p50 = quantile(&mut ds, 0.5);
    println!("LAB 400-leg bundle: p50 = {p50} ms, p95 = {p95} ms");
    // The gate runs this binary unoptimised, the bundled SQLite included, so
    // the release target (≤ 20 ms, A2) is measured on an optimised
    // deployment. Here, on the gate host 05.10.2026, the same lab
    // before/after: 880 → 141–268 ms p95, p50 ≈ 139 ms (one-op writes
    // 578/979 → 1/2 ms); the spread is the host's load. Lock: well over the measured debug spread, 40 ms for an
    // optimised build.
    let lock = if cfg!(debug_assertions) { 400 } else { 40 };
    assert!(
        p95 <= lock,
        "a bundle of 400 updates costs {p95} ms p95 (target 20 optimised, lock {lock})"
    );
}
