//! POC gpt-live-1 (A45) -- voice_duplex_gpt_live_notes_a_superseded_responses_delegation.
//!
//! Under a Responses delegation the provider runs the backend, and nothing
//! cancels work it already started. With `responses_superseded_note` set, a
//! new Responses delegation that overtakes an open one sends the note once as
//! `session.thinking.append`, so the model knows the old result is outdated.
//! A delegation that finished before the new one opened gets no note.

use meclaw_cells::voice::contract::{
    DuplexControl, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;

/// Failure-marker timeout: generous on purpose, it never discriminates timing.
const FAILURE_TIMEOUT: Duration = Duration::from_secs(30);

const NOTE: &str = "The earlier request is outdated; ignore its result.";

fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be brief.",
        "delegation": {"type": "responses", "responses": {"model": "gpt-6-sol"}},
        "responses_superseded_note": NOTE,
    }))
    .expect("params parse")
}

fn created(id: &str) -> Value {
    json!({
        "type": "session.delegation.created",
        "event_id": format!("ev_{id}"),
        "offset_ms": 100,
        "delegation": {"id": id, "type": "delegation", "target": "responses"}
    })
}

fn completed(id: &str) -> Value {
    json!({
        "type": "response.event",
        "event_id": format!("ev_done_{id}"),
        "delegation_id": id,
        "event": {"type": "response.completed", "response": {"id": "resp_x"}}
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_notes_a_superseded_responses_delegation() {
    // dlg_a finishes before dlg_b opens: no note. dlg_b is still open when
    // dlg_c opens: one note. The append is acknowledged before the close.
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_superseded".to_string(),
        actions: vec![
            LiveAction::Send(created("dlg_a")),
            LiveAction::Send(completed("dlg_a")),
            LiveAction::Send(created("dlg_b")),
            LiveAction::Send(created("dlg_c")),
            LiveAction::ExpectAppend {
                kind: "thinking",
                contains: NOTE,
            },
            LiveAction::Delay(Duration::from_secs(60)),
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let (_audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, _audio_out_rx) = mpsc::channel(32);
    let (events_tx, mut events_rx) = mpsc::channel::<DuplexEvent>(64);
    let (control_tx, control_rx) = mpsc::channel::<DuplexControl>(16);
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

    // The acknowledgement of the note is the marker that it went out.
    let acked = tokio::time::timeout(FAILURE_TIMEOUT, async {
        while let Some(event) = events_rx.recv().await {
            if let DuplexEvent::Appended { event_id, .. } = &event
                && event_id == "superseded:dlg_b"
            {
                return true;
            }
        }
        false
    })
    .await
    .expect("timed out waiting for the note's acknowledgement");
    assert!(acked, "the note for dlg_b was acknowledged");

    control_tx
        .send(DuplexControl::Close)
        .await
        .expect("the session was still taking guidance");
    let verdict = tokio::time::timeout(FAILURE_TIMEOUT, join)
        .await
        .expect("the session ended")
        .expect("the session task did not panic");
    assert!(verdict.is_ok(), "a requested close: {verdict:?}");

    let notes: Vec<Value> = mock
        .client_events()
        .await
        .into_iter()
        .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("session.thinking.append"))
        .collect();
    assert_eq!(notes.len(), 1, "exactly one note, for dlg_b: {notes:?}");
    assert_eq!(notes[0]["event_id"], "superseded:dlg_b");
    assert_eq!(notes[0]["content"], NOTE);
    assert_eq!(notes[0]["delegation_id"], Value::Null);
}
