//! GH #1006: the `web` cell tells the app how full each viewer's line is.
//!
//! A measured 2-D world app showed the gap this closes: the cell answered 8–10
//! bundles a second while the browser got nothing for 2–5 s and then a burst,
//! and the app had no signal to pace by — it guessed 250 ms between tranches,
//! the same for a wall screen and a phone. The cell now meters every viewer's
//! send queue and, when the display opts in with `viewer_events: ["backlog"]`,
//! reports `viewer:backlog` on its out-edge: edge-triggered, at most once a
//! second while `high`. The read-only op `viewers` answers the same numbers on
//! demand.
//!
//! These locks hold function and edge only, so they stand under the gate's
//! parallel suite: who is reported, in which order, and how often per repeat
//! interval (which the cell's clock enforces). Every number with a time or a
//! cost in it — the 500 ms, the 60 s at ≤ 61, p95 ≤ 1 s, the fast viewers'
//! frames per second, the counting cost — is measured once, alone, by
//! `gh1006_a_slow_viewer_is_reported_measure.rs` (`#[ignore]`). The
//! edge-trigger bound itself is a unit lock on the meter with an injected clock
//! (`web::backlog`), and so are "exactly once until the repeat" for the slow
//! viewer (GH #1011), "exactly one clear" once the reader catches up
//! (GH #1008) and the resync lock (`web::io`).

#[path = "support/web_fixture.rs"]
mod web_fixture;

use meclaw_core::serde_json::json;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use web_fixture::backlog_lab::{
    App, Lab, MOUNT, PASS, SLOW, Until, drive, drive_until, drive_until_paced, join, lab,
    lab_paced, num, opted_in, row_of, start,
};

/// T1, function: one slow viewer among three is reported `high`, and the two
/// fast ones never are; its reports start with `high` and alternate — never a
/// `clear` without a `high` before it, never two in a row. The plan's
/// "exactly once until the one-second repeat" is the meter lock of the same
/// name on an injected clock (`web::backlog`): the age threshold reads the
/// wall clock, so on a loaded host a starved write loop makes real
/// high/clear cycles inside the second (GH #1011, as #1008).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1006_a_slow_viewer_raises_backlog_high() {
    let mut live = start(opted_in()).await;
    let a = join(live.port, 0).await;
    let b = join(live.port, 0).await;
    let slow = join(live.port, SLOW).await;
    let mut round = 0;
    // Past the first `high` by more than the repeat interval, so the fast
    // viewers had time to show up if they were ever reported.
    let _ = drive_until(
        &mut live,
        &slow,
        true,
        Duration::from_millis(1200),
        &mut round,
    )
    .await;

    let heard = live.heard().await;
    assert!(
        heard
            .iter()
            .all(|h| h.session != a.session && h.session != b.session),
        "the fast viewers are never reported: {heard:?}"
    );
    let mine: Vec<bool> = heard
        .iter()
        .filter(|h| h.session == slow.session)
        .map(|h| h.high)
        .collect();
    assert_eq!(
        mine.first(),
        Some(&true),
        "a high for the slow viewer first: {heard:?}"
    );
    assert!(
        mine.windows(2).all(|w| w[0] || w[1]),
        "a clear only after a high, never two in a row: {heard:?}"
    );
    live.join.abort();
}

/// T2, function: once the slow viewer catches up it is reported `clear`.
/// The app keeps writing the full tranche for a while (only the reader
/// changes), then stops; the last report for the viewer is `clear`, and the
/// reports alternate — never a `clear` without a `high` before it, never two
/// in a row. How many cycles a loaded host adds is not this lock's question:
/// the age threshold reads the wall clock, so a starved write loop makes real
/// high/clear cycles (GH #1008). The plan's "exactly one clear" is the meter
/// lock of the same name on an injected clock (`web::backlog`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1006_a_caught_up_viewer_is_reported_clear() {
    let mut live = start(opted_in()).await;
    let slow = join(live.port, SLOW).await;
    let mut round = 0;
    let _ = drive_until(&mut live, &slow, true, Duration::ZERO, &mut round).await;
    slow.rate.store(0, Ordering::Relaxed);
    let _ = drive(&mut live, Duration::from_secs(1), &mut round).await;
    // The app stops: the reader drains the queue and the write after the last
    // frame reports the level. Bounded wait, a failure marker.
    let deadline = Instant::now() + Duration::from_secs(30);
    let heard = loop {
        let heard = live.heard().await;
        if heard.last().is_some_and(|h| !h.high) || Instant::now() >= deadline {
            break heard;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        heard.iter().all(|h| h.session == slow.session),
        "only the slow viewer is reported: {heard:?}"
    );
    let mut level = false;
    for h in &heard {
        assert!(
            h.high || level,
            "a clear only after a high, never two in a row: {heard:?}"
        );
        level = h.high;
    }
    assert!(
        heard.last().is_some_and(|h| !h.high),
        "the caught-up viewer ends clear: {heard:?}"
    );
    live.join.abort();
}

/// T4: without `viewer_events` the cell reports nothing and the log grows by
/// exactly what it grew by before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1006_no_backlog_event_without_opt_in() {
    let mut live = start(json!({"mount": MOUNT})).await;
    let _slow = join(live.port, SLOW).await;
    let mut round = 0;
    let samples = drive(&mut live, Duration::from_secs(4), &mut round).await;
    // The meter still counts: the op reads it whether or not events are on.
    assert!(
        samples
            .iter()
            .any(|(_, rows)| rows.iter().any(|r| num(r, "bytes") > 0)),
        "the slow viewer was behind"
    );
    assert!(live.heard().await.is_empty(), "no backlog report");
    assert_eq!(
        live.emitted.load(Ordering::Relaxed),
        0,
        "no emission besides the replies"
    );
    live.join.abort();
}

/// T5: `viewers` names each viewer's backlog — one row per viewer in the
/// documented shape; the slow one's bytes are there and its oldest frame
/// ages, and a fast one never holds as much as the slow one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1006_the_viewers_op_reports_the_backlog() {
    let mut live = start(opted_in()).await;
    let a = join(live.port, 0).await;
    let b = join(live.port, 0).await;
    let slow = join(live.port, SLOW).await;
    let mut round = 0;
    // Until the slow viewer is behind, and a while past it: since the partial
    // diffs (GH #1001) a full tranche is small enough that a 50 KB/s reader
    // keeps up for a few seconds on a loaded host (measured: 12 samples over
    // 3 s, every oldest_ms 0). The lock is about the row, not the timing.
    let (samples, drove) = drive_until_paced(
        &mut live,
        &slow,
        true,
        Duration::from_millis(1500),
        &mut round,
        PASS,
    )
    .await;
    let (_, last) = samples.last().expect("samples");
    assert_eq!(last.len(), 3, "three viewers: {last:?}");
    for r in last {
        for key in [
            "session_id",
            "route",
            "frames",
            "bytes",
            "oldest_ms",
            "resyncs_total",
            "screen",
        ] {
            assert!(r.get(key).is_some(), "{key} is part of the row: {r}");
        }
        assert!(
            r["screen"].is_null(),
            "no screen class is reported yet: {r}"
        );
    }
    let s = row_of(last, &slow.session).expect("the slow row");
    assert!(
        num(s, "bytes") > 0,
        "slow bytes: {s}: {}",
        no_backlog_why(&slow, &drove)
    );
    assert_eq!(s["route"], json!("/"));
    let ages: Vec<u64> = samples
        .iter()
        .filter_map(|(_, rows)| row_of(rows, &slow.session))
        .map(|r| num(r, "oldest_ms"))
        .collect();
    let half = ages.len() / 2;
    let early = ages[..half].iter().copied().max().unwrap_or(0);
    let late = ages[half..].iter().copied().max().unwrap_or(0);
    assert!(
        late > early,
        "the slow viewer's oldest frame ages: {ages:?}"
    );
    let max_of = |session: &str| {
        samples
            .iter()
            .filter_map(|(_, rows)| row_of(rows, session))
            .map(|r| num(r, "bytes"))
            .max()
            .unwrap_or(0)
    };
    let slow_max = max_of(&slow.session);
    for fast in [&a, &b] {
        let max = max_of(&fast.session);
        assert!(
            max < slow_max,
            "a fast viewer holds less than the slow one: {max} vs {slow_max}"
        );
    }
    live.join.abort();
}

/// T7, function: the plan's app (W6 § 4) — full tranche, halved on every
/// `high`, doubled on every `clear` — hears a `high` and then a `clear`, and
/// its tranche follows the level; the deaf app at the same load hears `high`.
/// Whether the plan's app keeps the slow viewer within A9's numbers is the
/// measurement test's question.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1006_an_app_that_throttles_on_backlog_keeps_the_slow_viewer_live() {
    let plan = lab(true, App::Plan, Until::HighThenClear).await;
    eprintln!("gh1006 T7 (function): plan app {plan:?}");
    assert!(
        plan.highs >= 1 && plan.cleared_after_high,
        "high, then clear: {plan:?}"
    );
    let (first_high, k) = plan.reactions[0];
    assert!(
        first_high && k == web_fixture::backlog_lab::FIGURES / 2,
        "halved on the first high: {plan:?}"
    );
    for w in plan.reactions.windows(2) {
        let ((_, before), (high, after)) = (w[0], w[1]);
        let expected = if high {
            (before / 2).max(1)
        } else {
            (before * 2).min(web_fixture::backlog_lab::FIGURES)
        };
        assert_eq!(after, expected, "the tranche follows the level: {plan:?}");
    }
    assert_eq!(
        plan.fast_reports, 0,
        "the fast viewers are never reported: {plan:?}"
    );

    let deaf = lab(true, App::Deaf, Until::High).await;
    eprintln!("gh1006 T7 (function): deaf app {deaf:?}");
    assert!(
        deaf.highs >= 1,
        "the signal fires for the deaf app: {deaf:?}"
    );
}

/// GH #1067: a starved app still puts the slow viewer behind.
///
/// T7 failed in a strand gate with "no high in 30 s" after 72 passes in 30 s:
/// under full-suite load the app's pass took 415 ms instead of 125, so it
/// wrote ~55 KB/s to a 50 KB/s reader and the viewer was never behind. Here
/// the app is held to one pass every 500 ms, the gate's starvation made
/// deterministic: "slow" has to mean slower than the app writes, not a fixed
/// rate the app may fall to under load.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1067_a_starved_app_still_puts_the_slow_viewer_behind() {
    let deaf = lab_paced(true, App::Deaf, Until::High, Duration::from_millis(500)).await;
    eprintln!("gh1067: starved deaf app {deaf:?}");
    assert!(deaf.highs >= 1, "{}: {deaf:?}", deaf.why_no_high());
    // Three nominal passes, not 450: the interval ticks at once, so a run
    // of n passes measures ~(n-1)/n x 500 ms plus the rest of the last pass,
    // and an early `high` after few passes lowers it (series: 467-473 ms).
    // 375 ms still says "starved": three times the nominal cadence.
    assert!(
        deaf.pass_ms >= 3 * PASS.as_millis() as u64,
        "the lab really ran starved (one pass per 500 ms): {deaf:?}"
    );
}

/// GH #1067 (review I1): a starved app still puts the slow viewer behind
/// when the test drives the passes itself.
///
/// T1, T2 and T5 drive full tranches through `drive_until` at the app's
/// cadence against a 50 KB/s reader: the same arithmetic as T7, so a gate
/// that starves the app to ~415 ms per pass leaves them without a `high` in
/// 30 s. Held to one pass every 500 ms, the drive must still get there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1067_a_starved_drive_still_raises_backlog_high() {
    let mut live = start(opted_in()).await;
    let slow = join(live.port, SLOW).await;
    let mut round = 0;
    let (_, drove) = drive_until_paced(
        &mut live,
        &slow,
        true,
        Duration::ZERO,
        &mut round,
        Duration::from_millis(500),
    )
    .await;
    eprintln!("gh1067: starved drive {drove:?}");
    assert!(
        drove.pass_ms >= 3 * PASS.as_millis() as u64,
        "the drive really ran starved (one pass per 500 ms): {drove:?}"
    );
    live.join.abort();
}

/// GH #1080: why the slow viewer's last row shows no backlog.
fn no_backlog_why(slow: &web_fixture::backlog_lab::Reader, drove: &Lab) -> String {
    if slow.frames.load(Ordering::Relaxed) == 0 {
        "no frame reached the slow viewer".to_string()
    } else {
        format!(
            "the slow viewer caught up: reader at {} B/s, app at {} ms per pass \
             (nominal {} ms) and {} B/s up to the high",
            slow.rate.load(Ordering::Relaxed),
            drove.pass_ms,
            PASS.as_millis(),
            drove.wrote_bps
        )
    }
}

/// GH #1080: a starved drive still leaves the slow viewer behind past the
/// `high`.
///
/// T5 failed in a strand gate with `bytes: 0` for the throttled viewer in
/// its last sample: under load the app slowed while the reader kept the rate
/// it had at the `high`, and the 1.5 s past it drained the queue. Held to
/// one pass every 500 ms for the whole run, the slow row still holds bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1080_a_starved_drive_keeps_the_slow_viewer_behind() {
    let mut live = start(opted_in()).await;
    let slow = join(live.port, SLOW).await;
    let mut round = 0;
    let (samples, drove) = drive_until_paced(
        &mut live,
        &slow,
        true,
        Duration::from_millis(1500),
        &mut round,
        Duration::from_millis(500),
    )
    .await;
    eprintln!("gh1080: starved drive {drove:?}");
    let (_, last) = samples.last().expect("samples");
    let s = row_of(last, &slow.session).expect("the slow row");
    assert!(
        num(s, "bytes") > 0,
        "slow bytes: {s}: {}",
        no_backlog_why(&slow, &drove)
    );
    live.join.abort();
}
