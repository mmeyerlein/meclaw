//! Welle Live, L2a -- voice_duplex_gpt_live_warns_when_the_socket_tears.
//!
//! A socket that tears mid-session ends the call the same way a polite one
//! does -- `Closed { connection_lost }` and `Ok(())`, OR-L.L2a.3 -- and says
//! once, on the event channel, that it tore.
//!
//! The ruling stands: reading the same cause two different ways depending on
//! whether the far side said goodbye would be worse. But an `Ok(())` is the
//! one verdict the cell does not turn into an `error duplex_failed`
//! (`vertraege.md` § 1.4), so without this warning a provider that resets
//! every session after ten seconds would look, on the colony's error lane,
//! like a run of short, ordinary calls. The warning is what makes the
//! difference between "the caller hung up" and "the socket broke" measurable;
//! L2b puts it on the lane as `duplex_warning`.

use meclaw_cells::voice::contract::{
    DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
    }))
    .expect("params parse")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_warns_when_the_socket_tears() {
    // The fake reports a meter reading and then drops the socket without a
    // close frame -- what a provider that fell over looks like from here.
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_tear".to_string(),
        actions: vec![
            LiveAction::Usage {
                seconds: 9.5,
                ratio: None,
            },
            LiveAction::DropSocket,
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let (_audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, _audio_out_rx) = mpsc::channel(32);
    let (events_tx, mut events_rx) = mpsc::channel::<DuplexEvent>(64);
    let (_control_tx, control_rx) = mpsc::channel::<DuplexControl>(16);
    let provider = GptLiveDuplex::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(5_000),
        idle: Duration::from_secs(600),
    });
    let future = provider.run_session(
        provider.format(),
        DuplexSession {
            audio_in: audio_in_rx,
            audio_out: audio_out_tx,
            events: events_tx,
            control: control_rx,
        },
        IoLivenessMark::disabled(),
    );
    let join = tokio::spawn(future);

    let mut seen = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(FAILURE_TIMEOUT, events_rx.recv()).await {
        let done = matches!(event, DuplexEvent::Closed { .. });
        seen.push(event);
        if done {
            break;
        }
    }

    let warning = seen
        .iter()
        .position(|e| matches!(e, DuplexEvent::Warning { .. }))
        .unwrap_or_else(|| {
            panic!("a torn socket is visible on the event channel, not only in the log: {seen:?}")
        });
    let closed = seen
        .iter()
        .position(|e| matches!(e, DuplexEvent::Closed { .. }))
        .expect("the session ended");
    assert!(
        warning < closed,
        "the warning is part of this session, not a postscript to it: {seen:?}"
    );
    let DuplexEvent::Warning { detail } = &seen[warning] else {
        unreachable!("checked above")
    };
    assert!(
        !detail.is_empty(),
        "the warning says what happened, so the error lane is readable"
    );

    // And the ruling it hangs on is unchanged: this is still the end of a
    // call, with the last meter reading, and still not an `Err`.
    assert_eq!(
        seen[closed],
        DuplexEvent::Closed {
            reason: "connection_lost".to_string(),
            usage_seconds: 9.5,
        },
        "what the meter last said is what the close reports"
    );
    let verdict: Result<(), DuplexError> = tokio::time::timeout(FAILURE_TIMEOUT, join)
        .await
        .expect("the session ended")
        .expect("the session task did not panic");
    assert!(
        verdict.is_ok(),
        "a socket the far side dropped is not this adapter's failure: {verdict:?}"
    );
}
