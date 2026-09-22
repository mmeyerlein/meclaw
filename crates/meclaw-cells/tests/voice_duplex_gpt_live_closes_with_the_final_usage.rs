//! Welle Live, L2a -- voice_duplex_gpt_live_closes_with_the_final_usage.
//!
//! `audio_in` closing sends `session.close`, the adapter waits at most
//! `close_grace_ms` for `session.closed` and reports `Closed` with the final
//! meter reading; a socket that dies first is still `Ok(())`.
//!
//! Both halves say the same thing from two sides: the end of a call is not a
//! failure. The `Err` of this adapter is reserved for a disturbance -- a
//! refused credential, a protocol it does not speak -- because the connection
//! task turns an `Err` into an `error duplex_failed` on the colony's lane, and
//! a caller hanging up is not an incident anybody should be paged about.

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

/// The provider's own close is worth waiting for, so the grace period is a
/// parameter of the test: the two calls that get an answer keep the default,
/// the one that never gets one cannot.
fn params(base_url: &str, close_grace_ms: u64) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
        "close_grace_ms": close_grace_ms,
    }))
    .expect("params parse")
}

struct Session {
    audio_in: mpsc::Sender<Vec<u8>>,
    #[allow(dead_code)]
    audio_out: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    #[allow(dead_code)]
    control: mpsc::Sender<DuplexControl>,
    join: tokio::task::JoinHandle<Result<(), DuplexError>>,
}

fn start(mock: &MockGptLive, close_grace_ms: u64) -> Session {
    let (audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, audio_out_rx) = mpsc::channel(32);
    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(16);
    let provider = GptLiveDuplex::new(params(&mock.base_url(), close_grace_ms)).with_timeouts(
        ProviderTimeouts {
            external: Duration::from_millis(5_000),
            idle: Duration::from_millis(30_000),
        },
    );
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
    Session {
        audio_in: audio_in_tx,
        audio_out: audio_out_rx,
        events: events_rx,
        control: control_tx,
        join: tokio::spawn(future),
    }
}

/// Drains the event channel and returns the last event, which is the `Closed`
/// this file is about.
async fn last_event(events: &mut mpsc::Receiver<DuplexEvent>) -> DuplexEvent {
    let mut last = None;
    while let Ok(Some(event)) = tokio::time::timeout(FAILURE_TIMEOUT, events.recv()).await {
        let done = matches!(event, DuplexEvent::Closed { .. });
        last = Some(event);
        if done {
            break;
        }
    }
    last.expect("the session said something before it ended")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_closes_with_the_final_usage() {
    // The script waits out the test: the close this file is about comes from
    // the CLIENT going away, and the fake answers it wherever its script
    // happens to stand -- with the meter reading the script would have
    // reported itself.
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_close".to_string(),
        actions: vec![
            LiveAction::Usage {
                seconds: 12.5,
                ratio: Some(0.3),
            },
            LiveAction::Delay(Duration::from_secs(60)),
            LiveAction::CloseWith {
                reason: "never played",
                usage_seconds: 12.5,
            },
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let mut session = start(&mock, 15_000);
    assert!(matches!(
        tokio::time::timeout(FAILURE_TIMEOUT, session.events.recv())
            .await
            .expect("timed out waiting for `Started`"),
        Some(DuplexEvent::Started { .. })
    ));

    // The caller hung up.
    drop(session.audio_in);

    assert_eq!(
        last_event(&mut session.events).await,
        DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 12.5,
        },
        "the final meter reading is the provider's own, taken from `session.closed`"
    );
    assert!(
        mock.closed_requested().await,
        "the adapter asked for the close instead of dropping the socket"
    );
    let verdict = tokio::time::timeout(FAILURE_TIMEOUT, session.join)
        .await
        .expect("the session ended")
        .expect("the session task did not panic");
    assert!(
        verdict.is_ok(),
        "the end of a call is not a failure: {verdict:?}"
    );
}

/// The other side of the same sentence: a provider that vanishes mid-call ends
/// the session with what is known, and still not with an `Err`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_socket_that_dies_first_is_still_an_orderly_end() {
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_lost".to_string(),
        actions: vec![
            LiveAction::Usage {
                seconds: 4.0,
                ratio: None,
            },
            LiveAction::DropSocket,
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let mut session = start(&mock, 15_000);
    assert_eq!(
        last_event(&mut session.events).await,
        DuplexEvent::Closed {
            reason: "connection_lost".to_string(),
            usage_seconds: 4.0,
        },
        "what the meter last said is what the close reports"
    );
    let verdict = tokio::time::timeout(FAILURE_TIMEOUT, session.join)
        .await
        .expect("the session ended")
        .expect("the session task did not panic");
    assert!(
        verdict.is_ok(),
        "a socket the far side dropped is not this adapter's failure: {verdict:?}"
    );
}

/// The third way out, and the only one with a clock on it: a provider that
/// takes the `session.close` and never answers it.
///
/// `close_grace_ms` ends the wait, the reason says which wait it was -- a
/// `connection_lost` here would blame the socket for a silence that came from
/// the far side -- and the meter reading is the last one that was reported,
/// because no better one is coming.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_nobody_answers_ends_on_the_grace_period() {
    // `DropSocket` at the end of the script is what switches the fake's
    // automatic answer to `session.close` off; it is never reached, because
    // the minute in front of it outlasts the test. What the adapter sees is a
    // socket that stays open and says nothing.
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_grace".to_string(),
        actions: vec![
            LiveAction::Usage {
                seconds: 7.5,
                ratio: None,
            },
            LiveAction::Delay(Duration::from_secs(60)),
            LiveAction::DropSocket,
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let mut session = start(&mock, 500);
    // Waiting for the meter reading rather than for `Started` is what makes
    // the final number this test's number and not a race with the close.
    loop {
        match tokio::time::timeout(FAILURE_TIMEOUT, session.events.recv())
            .await
            .expect("timed out waiting for the meter")
        {
            Some(DuplexEvent::Usage { seconds, .. }) => {
                assert_eq!(seconds, 7.5);
                break;
            }
            Some(_) => continue,
            None => panic!("the session ended before it reported anything"),
        }
    }

    let began = std::time::Instant::now();
    drop(session.audio_in);
    assert_eq!(
        last_event(&mut session.events).await,
        DuplexEvent::Closed {
            reason: "finalization_timeout".to_string(),
            usage_seconds: 7.5,
        },
        "a close nobody answered says so, with the last reading it has"
    );
    let took = began.elapsed();
    assert!(
        took < Duration::from_secs(10),
        "the grace period ended the wait, not the fake's minute: {took:?}"
    );
    assert!(
        mock.closed_requested().await,
        "the adapter did ask before it gave up on the answer"
    );
    let verdict = tokio::time::timeout(FAILURE_TIMEOUT, session.join)
        .await
        .expect("the session ended")
        .expect("the session task did not panic");
    assert!(
        verdict.is_ok(),
        "a far side that stays silent about its own close is still not this adapter's failure: {verdict:?}"
    );
}
