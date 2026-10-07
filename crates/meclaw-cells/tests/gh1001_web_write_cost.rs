//! GH #1001 — a write to the `web` cell costs what it touched, not the page.
//!
//! Measured on a deployed page of this shape (MESSUNG.md § 6 L-W2, MESSUNG-V2c.md § 3): every
//! write re-rendered every route, three SQL reads per object, so a one-op
//! write cost 51–66 ms on a pool of 1 000 figures and 129–150 ms (p50) on
//! 4 000, and a bundle of 400 figure updates 68 ms. These locks run the same
//! page shape in the lab (`support/web_fixture.rs`) through a real cell and
//! read the `duration_ms` the cell answers with.
//!
//! Thresholds carry a factor of two over the target (OR-H4-7): the target of
//! the wave is ≤ 10 ms per write and ≤ 20 ms p95 per 400-leg bundle.

#[path = "support/web_fixture.rs"]
mod web_fixture;

use meclaw_core::serde_json::json;
use web_fixture::{Lab, Shape, duration_ms, update};

/// The `q`-quantile of `xs` (nearest rank).
fn quantile(xs: &mut [i64], q: f64) -> i64 {
    xs.sort_unstable();
    let rank = ((q * xs.len() as f64).ceil() as usize).clamp(1, xs.len());
    xs[rank - 1]
}

/// p50 of `runs` one-op writes on a page of `figures` figures, and the samples
/// it was taken from (sorted).
async fn one_op_p50(figures: usize, runs: usize) -> (i64, Vec<i64>) {
    let mut lab = Lab::start(Shape::with_figures(figures)).await;
    // One warm-up write: the first write pays for the statement cache.
    lab.call(vec![update("fig-0", json!({"x": 1}))]).await;
    let mut ds = Vec::with_capacity(runs);
    for i in 0..runs {
        let id = format!("fig-{}", (i * 7919) % figures);
        let answer = lab.call(vec![update(&id, json!({"x": i as i64}))]).await;
        ds.push(duration_ms(&answer));
    }
    let p50 = quantile(&mut ds, 0.5);
    (p50, ds)
}

/// The host's 1/5/15-minute load, for the red message only (GH #1056); empty
/// where `/proc/loadavg` does not exist.
fn host_load() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .map(|l| l.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// The one-op write lock of T2 in whole milliseconds (target 10).
const LOCK_MS: i64 = 20;

/// T2. This lab on the gate host, p50 of a one-op write: 578 ms (1 000) and
/// 979 ms (4 000) before, 1 ms and 2 ms after. The deployed page measured
/// 51–66 ms (1 000) and 129–150 ms (4 000) before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1001_write_cost_does_not_grow_with_untouched_objects() {
    let (small, small_ds) = one_op_p50(1_000, 20).await;
    let (large, large_ds) = one_op_p50(4_000, 20).await;
    println!("LAB one-op write p50: pool 1000 = {small} ms, pool 4000 = {large} ms");
    assert!(
        large <= LOCK_MS,
        "a one-op write on a pool of 4 000 costs {large} ms p50 (target 10, lock {LOCK_MS})"
    );
    // `duration_ms` is whole milliseconds, so at a few ms a ratio alone reads
    // 1 ms of jitter as 100 %. The slack is a share of the lock (a quarter of
    // its 20 ms), not a fixed 3 ms: under full-suite load a strand gate read
    // 2 ms (1 000) vs 6 ms (4 000) and failed on 3 ms slack while the cell was
    // fine (isolated 5/5 green). The regression this guards grew with the
    // pool by hundreds of ms (578 -> 979 ms), far above the slack.
    let slack = LOCK_MS as f64 / 4.0;
    // GH #1056: a red run prints every sample of both pools and the host load.
    // Measured on a 12-core build host: 20 of 20 green at load1 about 40, 13 of
    // 20 red at load1 about 90 (1-3 ms vs 7-12 ms p50). The next red says
    // whether the large pool grew as a whole or a few writes sat behind the
    // scheduler, and at what load.
    assert!(
        (large as f64) <= (small as f64 * 1.5).max(small as f64 + slack),
        "the write cost grows with untouched objects: {small} ms (1 000) vs {large} ms (4 000); \
         samples 1 000 {small_ds:?}, 4 000 {large_ds:?}; load {}",
        host_load()
    );
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
