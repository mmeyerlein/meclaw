//! Welle Live, L2a -- voice_duplex_gpt_live_refuses_bad_credentials.
//!
//! A 401 or 403 on the upgrade is `DuplexError::Auth` and everything else is
//! `Connect`; the credential appears in no message, no log and no error.
//!
//! The distinction is not cosmetic: the connection task reports an `Auth` to
//! the error lane of a colony whose operator has to change a value, and a
//! `Connect` to one whose network had a bad minute. Reading the second as the
//! first sends somebody to rotate a working key.

use meclaw_cells::voice::contract::{
    DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
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

/// The credential this test hands the adapter. Nothing it produces may contain
/// it -- that is half of what this file checks.
const CREDENTIAL: &str = "sk-not-a-real-key-0123456789";

fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": CREDENTIAL,
        "base_url": base_url,
        "instructions": "Be Egon.",
    }))
    .expect("params parse")
}

/// Runs one session to its end against a fake that refuses the upgrade with
/// `status`, and returns the verdict.
async fn refused_with(status: u16) -> DuplexError {
    let mock = MockGptLive::start(LiveScript {
        upgrade_status: Some(status),
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");
    verdict_against(&mock).await
}

/// Runs one session against `mock` to the end it never reaches, and returns
/// the verdict it failed with.
async fn verdict_against(mock: &MockGptLive) -> DuplexError {
    let (_audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, _audio_out_rx) = mpsc::channel(32);
    let (events_tx, mut events_rx) = mpsc::channel::<DuplexEvent>(64);
    let (_control_tx, control_rx) = mpsc::channel(16);
    let provider = GptLiveDuplex::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_millis(5_000),
        idle: Duration::from_millis(30_000),
    });
    let verdict = tokio::time::timeout(
        FAILURE_TIMEOUT,
        provider.run_session(
            provider.format(),
            DuplexSession {
                audio_in: audio_in_rx,
                audio_out: audio_out_tx,
                events: events_tx,
                control: control_rx,
            },
            IoLivenessMark::disabled(),
        ),
    )
    .await
    .expect("the session gave up rather than hanging")
    .expect_err("a session that was refused is never a session");

    assert!(
        events_rx.try_recv().is_err(),
        "a session that never opened reports no event, least of all `Started`"
    );
    verdict
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_refuses_bad_credentials() {
    for status in [401u16, 403] {
        let verdict = refused_with(status).await;
        let DuplexError::Auth(detail) = &verdict else {
            panic!("http {status} is a verdict on the credential, not {verdict:?}");
        };
        assert!(
            detail.contains(&status.to_string()),
            "the status is what the operator needs: {detail}"
        );
        assert!(
            !detail.contains(CREDENTIAL) && !verdict.to_string().contains(CREDENTIAL),
            "a credential has no business in an error text"
        );
    }

    // Everything else is the transport's problem, not the key's.
    for status in [400u16, 429, 500] {
        let verdict = refused_with(status).await;
        assert!(
            matches!(verdict, DuplexError::Connect(_)),
            "http {status} says nothing about the credential: {verdict:?}"
        );
        assert!(
            !verdict.to_string().contains(CREDENTIAL),
            "a credential has no business in an error text"
        );
    }
}

/// The third class of refusal, and the only one that happens on an open
/// socket: a peer that takes the `session.start` and answers it with `error`.
///
/// That event is the verdict on the configuration just sent -- an unsupported
/// rate, a model that is not there -- so it is `Protocol` and the session is
/// over. It is also the reason `map_event` needs no `Fatal` (OR-L.L2a.1):
/// after `session.started` the same event is one item's problem, not the
/// session's, and the two live in different functions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_before_started_is_a_protocol_verdict() {
    let mock = MockGptLive::start(LiveScript {
        // No `session.started` -- the script speaks instead of the handshake.
        started: false,
        actions: vec![LiveAction::Send(json!({
            "type": "error",
            "event_id": "event_mock_error",
            "error": {
                "type": "invalid_request_error",
                "message": "unsupported sample rate"
            }
        }))],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let verdict = verdict_against(&mock).await;
    let DuplexError::Protocol(detail) = &verdict else {
        panic!("an error on the session.start is a protocol verdict, not {verdict:?}");
    };
    assert!(
        detail.contains("unsupported sample rate"),
        "the provider's own words are what the operator needs: {detail}"
    );
    assert!(
        !detail.contains(CREDENTIAL) && !verdict.to_string().contains(CREDENTIAL),
        "a credential has no business in an error text"
    );
}
