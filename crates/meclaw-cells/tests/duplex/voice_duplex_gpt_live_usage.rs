//! D48, DK -- what a duplex call used, and how long it has been running.
//!
//! **The promise.** With `emit_usage` set, a duplex call reports its usage on
//! the `usage` lane: a running total at every meter reading, and one `final`
//! total at the end of the call -- the sum over EVERY session of the call,
//! renewals included, because a call renewed at every fact turn spans many
//! sessions and the last one alone is a fraction of the bill. A session that
//! never closed counts with its last meter reading, and the `final` then says
//! `complete: false`. With `call_marks_s` set, the call is marked on the `call_mark` lane.
//! Both are off by default: a cell that sets neither emits neither.
//!
//! **How it is measured.** At the seam, with the fake provider of
//! `tests/support/duplex_cell.rs`: the test plays the model's meter and its
//! closes, and reads the handler's emissions. Event-driven throughout -- the
//! exact timing of the marks is pinned in virtual time by the unit tests of
//! `src/voice/call_marks.rs`, never here.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{
    Live, boot_fake, boot_lingering, boot_renewing, duplex_params, emission_route, handover_msg,
    header, marker,
};
use meclaw_cells::voice::contract::DuplexEvent;
use meclaw_core::serde_json::{Value, json};

/// A renewal opens this long after a session took charge of the call.
const RENEW_AFTER_MS: u64 = 300;

/// A grace no test here reaches: the takeover comes from the handover block.
const NO_GRACE_MS: u64 = 20_000;

/// The next `usage` emission that satisfies `wanted`, as `(seconds, final)`.
async fn usage_where(live: &mut Live, wanted: impl Fn(f64, bool) -> bool) -> (f64, bool, Value) {
    loop {
        let e = live.emission("usage").await;
        let seconds = header(&e, "seconds")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("usage carries seconds: {e}"));
        let last = header(&e, "final")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| panic!("usage carries final: {e}"));
        if wanted(seconds, last) {
            return (seconds, last, e);
        }
    }
}

/// A close grace the lingering model always runs into: it never finalises.
const SHORT_GRACE_MS: u64 = 50;

/// Two sessions of one call: the final usage is what both closed with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gpt_live_usage_sums_across_renews() {
    let params = duplex_params(json!({"default_mode": "auto", "emit_usage": true}));
    let mut live = boot_renewing(params, RENEW_AFTER_MS, NO_GRACE_MS).await;
    let (_client, _hello) = live.connect("session=usage-1&mode=auto").await;
    let mut first = live.session().await;

    // A meter reading of the first session is the call's running total.
    first
        .say(DuplexEvent::Usage {
            seconds: 5.0,
            usage_ratio: None,
        })
        .await;
    let (seconds, last, e) = usage_where(&mut live, |_, _| true).await;
    assert_eq!((seconds, last), (5.0, false), "{e}");
    assert_eq!(header(&e, "call_id"), Some(&json!("usage-1")), "{e}");
    assert_eq!(header(&e, "session_id"), Some(&json!("usage-1")), "{e}");
    assert!(e["messages"].as_array().is_some_and(Vec::is_empty), "{e}");

    // The renewal takes over on its handover; the first session retires.
    let _renewed = live.emission("renewed").await;
    let second = live.session().await;
    live.send(handover_msg("usage-1", "Handover.", 1)).await;
    let ended = tokio::time::timeout(marker(), live.ended.recv())
        .await
        .expect("HANG: the old session's audio ends after the takeover")
        .expect("the fake is running");
    assert_eq!(ended, 1, "it is the first session that retires");

    // The retired session closes with what it used: the call's total moves.
    first
        .say(DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 12.0,
        })
        .await;
    let (seconds, last, e) = usage_where(&mut live, |s, _| s >= 12.0).await;
    assert_eq!((seconds, last), (12.0, false), "{e}");
    first.finish();

    // The caller hangs up and the session in charge closes: final = the sum
    // over both sessions -- not its own 30, not the retired 12.
    let mut second = second;
    drop(_client);
    second
        .say(DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 30.0,
        })
        .await;
    second.finish();
    let (seconds, _, e) = usage_where(&mut live, |_, last| last).await;
    assert_eq!(seconds, 42.0, "the final usage sums every session: {e}");
    assert_eq!(header(&e, "call_id"), Some(&json!("usage-1")), "{e}");
    assert_eq!(
        header(&e, "complete"),
        Some(&json!(true)),
        "every session closed: {e}"
    );
}

/// The caller hangs up while the session a renewal replaced is still
/// retiring: that session never closes, and it counts with its last meter.
/// Between the takeover and the hang-up the running total holds both meters
/// rather than falling to the new session's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hang_up_during_a_handover_counts_the_retiring_meter() {
    let params = duplex_params(json!({"default_mode": "auto", "emit_usage": true}));
    let mut live = boot_renewing(params, RENEW_AFTER_MS, NO_GRACE_MS).await;
    let (client, _hello) = live.connect("session=usage-2&mode=auto").await;
    let first = live.session().await;
    first
        .say(DuplexEvent::Usage {
            seconds: 5.0,
            usage_ratio: None,
        })
        .await;
    let (seconds, _, e) = usage_where(&mut live, |_, _| true).await;
    assert_eq!(seconds, 5.0, "{e}");

    let _renewed = live.emission("renewed").await;
    let mut second = live.session().await;
    live.send(handover_msg("usage-2", "Handover.", 1)).await;
    let ended = tokio::time::timeout(marker(), live.ended.recv())
        .await
        .expect("HANG: the old session's audio ends after the takeover")
        .expect("the fake is running");
    assert_eq!(ended, 1, "it is the first session that retires");

    // The new session meters 3 while the old one has not closed: 5 + 3.
    second
        .say(DuplexEvent::Usage {
            seconds: 3.0,
            usage_ratio: None,
        })
        .await;
    let (seconds, last, e) = usage_where(&mut live, |_, _| true).await;
    assert_eq!(
        (seconds, last),
        (8.0, false),
        "the running total holds both meters: {e}"
    );

    // Hang up: the retiring session goes with the call, unclosed.
    drop(client);
    second
        .say(DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 30.0,
        })
        .await;
    second.finish();
    let (seconds, _, e) = usage_where(&mut live, |_, last| last).await;
    assert_eq!(
        seconds, 35.0,
        "the retiring session counts with its last meter: {e}"
    );
    assert_eq!(header(&e, "complete"), Some(&json!(false)), "{e}");
    drop(first);
}

/// The session in charge never closes -- its verdict times out: the call
/// still gets its `final`, exactly once, with the last meter and
/// `complete: false`. A second call is the marker for "no second final":
/// the handler takes events in order, so a second `final` of the first call
/// would reach the lane before anything of the second.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_final_usage_comes_without_a_close() {
    let params = duplex_params(json!({"default_mode": "auto", "emit_usage": true}));
    let mut live = boot_lingering(params, SHORT_GRACE_MS).await;
    let (client, _hello) = live.connect("session=usage-3&mode=auto").await;
    let session = live.session().await;
    session
        .say(DuplexEvent::Usage {
            seconds: 7.0,
            usage_ratio: None,
        })
        .await;
    let (seconds, _, e) = usage_where(&mut live, |_, _| true).await;
    assert_eq!(seconds, 7.0, "{e}");

    drop(client);
    let (seconds, last, e) = usage_where(&mut live, |_, last| last).await;
    assert_eq!(
        (seconds, last),
        (7.0, true),
        "the last meter stands in: {e}"
    );
    assert_eq!(header(&e, "complete"), Some(&json!(false)), "{e}");
    assert_eq!(header(&e, "call_id"), Some(&json!("usage-3")), "{e}");

    let (_client2, _hello2) = live.connect("session=usage-4&mode=auto").await;
    let next = live.session().await;
    next.say(DuplexEvent::Usage {
        seconds: 1.0,
        usage_ratio: None,
    })
    .await;
    let e = live.emission("usage").await;
    assert_eq!(
        header(&e, "call_id"),
        Some(&json!("usage-4")),
        "one final per call, nothing of the first call after it: {e}"
    );
    drop(session);
}

/// A renewal that closes and then fails before it took over (review re-check
/// Re-1): its close counts for the call, whichever way the connection learns
/// that it is gone -- the handover block that finds it dead, or the end of
/// its events.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_renewal_counts_its_close() {
    let params = duplex_params(json!({"default_mode": "auto", "emit_usage": true}));
    let mut live = boot_renewing(params, RENEW_AFTER_MS, NO_GRACE_MS).await;
    let (client, _hello) = live.connect("session=usage-5&mode=auto").await;
    let mut first = live.session().await;
    let _renewed = live.emission("renewed").await;
    let mut second = live.session().await;
    // Its provider ends after its close; this handle keeps its events open.
    second
        .say(DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 4.0,
        })
        .await;
    second.stop().await;
    live.send(handover_msg("usage-5", "Handover.", 1)).await;
    let warning = live.emission("error").await;
    assert_eq!(
        header(&warning, "error_code"),
        Some(&json!("duplex_warning")),
        "{warning}"
    );

    drop(client);
    first
        .say(DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 10.0,
        })
        .await;
    first.finish();
    let (seconds, _, e) = usage_where(&mut live, |_, last| last).await;
    assert_eq!(seconds, 14.0, "the failed renewal's close counts: {e}");
    assert_eq!(header(&e, "complete"), Some(&json!(true)), "{e}");
    drop(second);
}

/// A renewal that opened and failed without a close (review re-check Re-1):
/// it was billed, and nobody knows for how much -- the final says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_renewal_without_a_close_makes_the_final_incomplete() {
    let params = duplex_params(json!({"default_mode": "auto", "emit_usage": true}));
    let mut live = boot_renewing(params, RENEW_AFTER_MS, NO_GRACE_MS).await;
    let (client, _hello) = live.connect("session=usage-6&mode=auto").await;
    let mut first = live.session().await;
    let _renewed = live.emission("renewed").await;
    let mut second = live.session().await;
    second.stop().await;
    live.send(handover_msg("usage-6", "Handover.", 1)).await;
    let warning = live.emission("error").await;
    assert_eq!(
        header(&warning, "error_code"),
        Some(&json!("duplex_warning")),
        "{warning}"
    );

    drop(client);
    first
        .say(DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 10.0,
        })
        .await;
    first.finish();
    let (seconds, _, e) = usage_where(&mut live, |_, last| last).await;
    assert_eq!(seconds, 10.0, "{e}");
    assert_eq!(
        header(&e, "complete"),
        Some(&json!(false)),
        "a session that opened and never closed leaves the total open: {e}"
    );
    drop(second);
}

/// A cell that names neither knob emits neither lane. The warning after the
/// meter and the close is the marker: the handler takes events in order, so
/// anything those two emitted is in the channel before the warning's error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn usage_and_marks_off_by_default() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (_client, _hello) = live.connect("session=usage-off&mode=auto").await;
    let session = live.session().await;
    session
        .say(DuplexEvent::Usage {
            seconds: 5.0,
            usage_ratio: Some(0.1),
        })
        .await;
    session
        .say(DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 9.0,
        })
        .await;
    session
        .say(DuplexEvent::Warning {
            detail: "marker".to_string(),
            provider_code: None,
            client_event_id: None,
        })
        .await;

    let mut routes: Vec<String> = Vec::new();
    loop {
        let e = tokio::time::timeout(marker(), live.emissions.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: no error marker; saw {routes:?}"))
            .expect("the handler is running");
        let route = emission_route(&e.content).unwrap_or_default().to_string();
        if route == "error" {
            break;
        }
        routes.push(route);
    }
    assert!(
        !routes.iter().any(|r| r == "usage" || r == "call_mark"),
        "off by default, byte-equal to before: {routes:?}"
    );
}

/// A mark that falls due reaches the `call_mark` lane, naming the call and
/// its second. Only the arrival is asserted: when it fires is pinned in
/// virtual time by `call_marks::tests`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_due_mark_reaches_the_call_mark_lane() {
    let params = duplex_params(json!({"default_mode": "auto", "call_marks_s": [1]}));
    let mut live = boot_fake(params).await;
    let (_client, _hello) = live.connect("session=marks-1&mode=auto").await;
    let _session = live.session().await;

    let e = live.emission("call_mark").await;
    assert_eq!(header(&e, "elapsed_s"), Some(&json!(1)), "{e}");
    assert_eq!(header(&e, "call_id"), Some(&json!("marks-1")), "{e}");
    assert_eq!(header(&e, "engine"), Some(&json!("duplex")), "{e}");
}
