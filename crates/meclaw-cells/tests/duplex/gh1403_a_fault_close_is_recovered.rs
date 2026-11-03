//! GH #1403 -- a provider abort a lab can force: `fault_close_at_ms`.
//!
//! **The promise.** GH #1377 recovers a call whose provider ended the session
//! in charge (`reason=content`), and until now only a fake provider could
//! play that abort: the live model does it on its own terms, so a lab could
//! not call it up. `duplex.gpt_live.fault_close_at_ms` is the declared test
//! switch for labs: that many milliseconds after a session started, the
//! shipped adapter ends it the way the provider's own `session.closed
//! reason=content` would, and says so once on the colony's lane
//! (`duplex_warning` `fault_close_fired`). What follows is the recovery that
//! exists -- `recover_max` fresh sessions with `reason: recover`, then
//! `duplex_failed` exactly once. `0`, the default, never fires, and the
//! session opens and ends byte for byte as it always did.
//!
//! **How it is measured.** The shipped `gpt_live` adapter against the mock
//! socket of `meclaw_testing::mock_gpt_live`, once inside a duplex cell
//! (`boot_recovering_with`) and once on its own. Loopback sockets, so real
//! time (development rules § 11): the switch is configuration that shortens
//! the abort, never a verdict; every wait is the named hang marker, and the
//! call's one `final` usage is the marker after which nothing more of it can
//! come. No claim here is about time.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{Live, boot_recovering_with, duplex_params, emission_route, header};
use meclaw_cells::voice::contract::{
    DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::{GptLiveDuplex, session_start_frame};
use meclaw_colony::io_liveness::IoLivenessMark;
use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::mock_gpt_live::{LiveScript, MockGptLive};
use std::sync::Arc;
use tokio::sync::mpsc;

/// The connection asks for seeds, so every session's `reason` is on the lane;
/// the adapter itself waits for none (its own `seed_wait_ms` stays `0`).
const SEED_WAIT_MS: u64 = 400;

/// The forced abort of the cell test: configuration, never a verdict.
const FAULT_CLOSE_AT_MS: u64 = 200;

/// The cell's params: the usage lane is the end marker.
fn cell_params() -> Value {
    duplex_params(json!({"default_mode": "auto", "emit_usage": true}))
}

/// A `gpt_live` block against `mock`, with `extra` keys on top.
fn gpt_live_params(mock: &MockGptLive, extra: Value) -> GptLiveParams {
    let mut raw = json!({
        "api_key": "test-key",
        "base_url": mock.base_url(),
        "instructions": "Be Egon.",
        "sample_rate": duplex_cell::RATE,
    });
    if let (Some(obj), Some(more)) = (raw.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(raw).expect("the gpt_live block parses")
}

/// Every emission up to and including the call's `final` usage.
async fn until_final(live: &mut Live) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let e = tokio::time::timeout(duplex_cell::marker(), live.emissions.recv())
            .await
            .unwrap_or_else(|_| {
                let routes: Vec<_> = seen
                    .iter()
                    .map(|e: &Value| emission_route(e).unwrap_or_default().to_string())
                    .collect();
                panic!("HANG: no final usage; saw {routes:?}")
            })
            .expect("the handler is running")
            .content;
        let last = emission_route(&e) == Some("usage") && header(&e, "final") == Some(&json!(true));
        seen.push(e);
        if last {
            return seen;
        }
    }
}

/// The `error_code`s among `emissions`, in their order.
fn error_codes(emissions: &[Value]) -> Vec<String> {
    emissions
        .iter()
        .filter(|e| emission_route(e) == Some("error"))
        .filter_map(|e| header(e, "error_code").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The `reason`s of every `seed_ask` among `emissions`.
fn seed_reasons(emissions: &[Value]) -> Vec<String> {
    emissions
        .iter()
        .filter(|e| emission_route(e) == Some("seed_ask"))
        .filter_map(|e| header(e, "reason").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// How many `duplex_warning`s among `emissions` name the forced abort.
fn fault_closes(emissions: &[Value]) -> usize {
    emissions
        .iter()
        .filter(|e| emission_route(e) == Some("error"))
        .filter(|e| header(e, "error_code").and_then(Value::as_str) == Some("duplex_warning"))
        .filter(|e| e.to_string().contains("fault_close_fired"))
        .count()
}

/// Pin 1: with `recover_max` 1, the first forced abort is recovered -- a
/// fresh session asks for its seed with `reason: recover` -- and the second,
/// the recovered session's own, is `duplex_failed` exactly once; the client
/// reads `1011`. Each abort is said once on the lane, and two sessions were
/// opened, no third.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fault_close_triggers_recover_once_and_then_duplex_failed() {
    let mock = MockGptLive::start(LiveScript::default())
        .await
        .expect("the mock socket binds");
    let provider: Arc<dyn DuplexProvider> = Arc::new(
        GptLiveDuplex::new(gpt_live_params(
            &mock,
            json!({"fault_close_at_ms": FAULT_CLOSE_AT_MS}),
        ))
        .with_timeouts(ProviderTimeouts::default()),
    );
    let mut live = boot_recovering_with(cell_params(), provider, 1, SEED_WAIT_MS).await;
    let (mut client, _hello) = live.connect("session=fx-1&mode=auto").await;

    let seen = until_final(&mut live).await;
    assert_eq!(
        seed_reasons(&seen),
        vec!["start".to_string(), "recover".to_string()],
        "the first abort is recovered, the second is not: {seen:?}"
    );
    let failed: Vec<_> = error_codes(&seen)
        .into_iter()
        .filter(|c| c == "duplex_failed")
        .collect();
    assert_eq!(failed, vec!["duplex_failed".to_string()], "{seen:?}");
    assert_eq!(
        fault_closes(&seen),
        2,
        "one ledger line per abort: {seen:?}"
    );
    let frames = client
        .collect_until(|f| f.as_close().is_some(), duplex_cell::marker())
        .await;
    let code = frames.iter().find_map(|(_, f)| f.as_close());
    assert_eq!(code, Some(1011), "a failed call: {frames:?}");
    assert_eq!(mock.connects().await, 2, "no third session");
    assert!(
        mock.closed_requested().await,
        "the adapter tells the provider the session is over"
    );
}

/// One adapter session on its own, ended by the caller: every event it
/// reported, in order, its opening frame and the frames the mock saw.
async fn caller_ended_session(extra: Value) -> (Vec<DuplexEvent>, Value, Vec<Value>) {
    let mock = MockGptLive::start(LiveScript::default())
        .await
        .expect("the mock socket binds");
    let params = gpt_live_params(&mock, extra);
    let provider = GptLiveDuplex::new(params.clone()).with_timeouts(ProviderTimeouts::default());
    let (audio_in_tx, audio_in_rx) = mpsc::channel::<Vec<u8>>(32);
    let (audio_out_tx, _audio_out_rx) = mpsc::channel(32);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, control_rx) = mpsc::channel::<DuplexControl>(16);
    let join: tokio::task::JoinHandle<Result<(), DuplexError>> =
        tokio::spawn(provider.run_session(
            provider.format(),
            DuplexSession {
                audio_in: audio_in_rx,
                audio_out: audio_out_tx,
                events: events_tx,
                control: control_rx,
            },
            IoLivenessMark::disabled(),
        ));
    let mut events = Vec::new();
    // The session started; only then does the caller hang up.
    loop {
        let e = tokio::time::timeout(duplex_cell::marker(), events_rx.recv())
            .await
            .expect("HANG: the session never started")
            .expect("the adapter is running");
        let started = matches!(e, DuplexEvent::Started { .. });
        events.push(e);
        if started {
            break;
        }
    }
    drop(audio_in_tx);
    while let Some(e) = tokio::time::timeout(duplex_cell::marker(), events_rx.recv())
        .await
        .expect("HANG: the adapter never let its events go")
    {
        events.push(e);
    }
    let verdict = tokio::time::timeout(duplex_cell::marker(), join)
        .await
        .expect("HANG: the adapter task never ended")
        .expect("the adapter task did not panic");
    assert!(
        verdict.is_ok(),
        "a caller hanging up is no failure: {verdict:?}"
    );
    let start = mock.session_start().await.expect("a session.start arrived");
    assert_eq!(
        start,
        session_start_frame(&params, provider.format(), None),
        "the opening is the adapter's own frame"
    );
    (events, start, mock.client_events().await)
}

/// Pin 2: the default `0` never fires. Absent and `0` open with the same
/// bytes, the session ends only when the caller does (`close_requested`, no
/// `fault_close_fired`), and what reached the socket is the same sequence.
/// The switch never travels on the wire: a set value opens with the same
/// `session.start`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn default_zero_never_fires() {
    let (absent_events, absent_start, absent_frames) = caller_ended_session(json!({})).await;
    let (zero_events, zero_start, zero_frames) =
        caller_ended_session(json!({"fault_close_at_ms": 0})).await;
    for events in [&absent_events, &zero_events] {
        let closes: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                DuplexEvent::Closed { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(closes, vec!["close_requested".to_string()], "{events:?}");
        let warnings = events
            .iter()
            .filter(|e| matches!(e, DuplexEvent::Warning { .. }))
            .count();
        assert_eq!(warnings, 0, "nothing fired: {events:?}");
    }
    assert_eq!(
        absent_start, zero_start,
        "the opening is byte for byte the same"
    );
    assert_eq!(absent_frames, zero_frames, "so is everything after it");
    let types: Vec<_> = absent_frames
        .iter()
        .filter_map(|f| f.get("type").and_then(Value::as_str))
        .collect();
    assert_eq!(
        types,
        vec!["session.start", "session.close"],
        "{absent_frames:?}"
    );

    let (_set_events, set_start, _set_frames) =
        caller_ended_session(json!({"fault_close_at_ms": 600_000})).await;
    assert_eq!(set_start, absent_start, "the switch is not on the wire");
}
