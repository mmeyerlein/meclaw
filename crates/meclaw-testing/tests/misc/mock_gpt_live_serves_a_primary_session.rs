//! Wave live (L6) — the mock of the primary GPT-Live socket, tested against a
//! plain WebSocket client.
//!
//! These tests are the mock's own contract: what a strand that builds the
//! `gpt_live` adapter (L2a) may rely on. They speak the protocol by hand
//! rather than through the adapter, so a change in the adapter can never make
//! the mock look right.

use futures_util::{SinkExt, StreamExt};
use meclaw_core::serde_json::{Value as JsonValue, json};
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive, b64_encode};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// Opens the socket the way the adapter will: `base_url` plus the path of the
/// primary session endpoint, with a `Bearer` credential in the header.
async fn connect(mock: &MockGptLive) -> WebSocketStream<TcpStream> {
    let url = format!("{}/v1/live/sessions", mock.base_url());
    let mut req = url.as_str().into_client_request().expect("request");
    req.headers_mut()
        .insert("authorization", "Bearer test-token".parse().expect("hv"));
    let stream = TcpStream::connect(mock.base_url().trim_start_matches("ws://"))
        .await
        .expect("connect");
    let (ws, _resp) = tokio_tungstenite::client_async(req, stream)
        .await
        .expect("handshake");
    ws
}

/// The first client event of every session, as the reference client sends it.
fn session_start_frame() -> WsMessage {
    WsMessage::Text(
        json!({
            "type": "session.start",
            "event_id": "event_start",
            "session": {
                "model": "gpt-live-1",
                "instructions": "Be concise.",
                "audio": {
                    "format": { "type": "audio/pcm", "rate": 16000 },
                    "output": { "voice": "marin" }
                },
                "delegation": { "type": "client" }
            }
        })
        .to_string()
        .into(),
    )
}

/// Reads frames until one carries `type`, and returns it.
async fn read_until(
    read: &mut futures_util::stream::SplitStream<WebSocketStream<TcpStream>>,
    kind: &str,
) -> JsonValue {
    while let Some(Ok(msg)) = read.next().await {
        if let WsMessage::Text(txt) = msg {
            let v: JsonValue = meclaw_core::serde_json::from_str(&txt).expect("json");
            if v["type"] == kind {
                return v;
            }
        }
    }
    panic!("the socket ended before a {kind} frame arrived");
}

#[tokio::test]
async fn the_session_start_is_answered_with_session_started() {
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_test_1".to_string(),
        ..LiveScript::default()
    })
    .await
    .expect("bind");

    let (mut write, mut read) = connect(&mock).await.split();
    write.send(session_start_frame()).await.expect("start");

    let started = read_until(&mut read, "session.started").await;
    assert_eq!(started["session"]["id"], "sess_test_1");
    assert_eq!(started["session"]["model"], "gpt-live-1");

    let seen = mock.session_start().await.expect("the start frame");
    assert_eq!(seen["session"]["audio"]["format"]["rate"], 16000);
    assert!(mock.authorization_is_bearer().await);
    assert_eq!(mock.connects().await, 1);
}

#[tokio::test]
async fn five_appends_arrive_as_five_decoded_entries() {
    let mock = MockGptLive::start(LiveScript {
        actions: vec![
            LiveAction::RequireAudioBytes(20),
            LiveAction::Send(json!({ "type": "info", "event_id": "event_marker" })),
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind");

    let (mut write, mut read) = connect(&mock).await.split();
    write.send(session_start_frame()).await.expect("start");
    read_until(&mut read, "session.started").await;

    for n in 0u8..5 {
        let audio = b64_encode(&[n, n, n, n]);
        write
            .send(WsMessage::Text(
                json!({ "type": "session.input_audio.append", "audio": audio })
                    .to_string()
                    .into(),
            ))
            .await
            .expect("append");
    }
    read_until(&mut read, "info").await;

    let frames = mock.received_audio().await;
    assert_eq!(frames.len(), 5, "one entry per append, never merged");
    assert_eq!(frames[3], vec![3u8, 3, 3, 3]);
    assert_eq!(mock.received_audio_bytes().await, 20);
    assert!(
        mock.client_events()
            .await
            .iter()
            .all(|e| e["type"] != "session.input_audio.append"),
        "audio is counted, not collected as a client event"
    );
}

#[tokio::test]
async fn expect_append_blocks_until_the_append_arrives() {
    let mock = MockGptLive::start(LiveScript {
        actions: vec![
            LiveAction::ExpectAppend {
                kind: "commentary",
                contains: "invoice",
            },
            LiveAction::Send(json!({ "type": "info", "event_id": "event_marker" })),
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind");

    let (mut write, mut read) = connect(&mock).await.split();
    write.send(session_start_frame()).await.expect("start");
    read_until(&mut read, "session.started").await;

    // The script must not run past the expectation on its own. A short window
    // is enough: the next action sends immediately once it is unblocked.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), read.next())
            .await
            .is_err(),
        "the script ran past ExpectAppend without an append"
    );

    write
        .send(WsMessage::Text(
            json!({
                "type": "session.commentary.append",
                "event_id": "c1",
                "delegation_id": null,
                "content": "The invoice was paid on Tuesday."
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("append");

    let appended = read_until(&mut read, "session.commentary.appended").await;
    assert_eq!(appended["client_event_id"], "c1");
    assert!(appended["start_ms"].is_number());
    assert!(appended["end_ms"].is_number());
    read_until(&mut read, "info").await;

    let appends = mock.appends().await;
    assert_eq!(appends.len(), 1);
    assert_eq!(appends[0]["type"], "session.commentary.append");
}

#[tokio::test]
async fn expect_mute_answers_muted_and_unmute_answers_unmuted() {
    let mock = MockGptLive::start(LiveScript {
        actions: vec![LiveAction::ExpectMute, LiveAction::ExpectUnmute],
        ..LiveScript::default()
    })
    .await
    .expect("bind");

    let (mut write, mut read) = connect(&mock).await.split();
    write.send(session_start_frame()).await.expect("start");
    read_until(&mut read, "session.started").await;

    write
        .send(WsMessage::Text(
            json!({ "type": "session.input_audio.mute", "event_id": "m1" })
                .to_string()
                .into(),
        ))
        .await
        .expect("mute");
    let muted = read_until(&mut read, "session.input_audio.muted").await;
    assert_eq!(muted["client_event_id"], "m1");

    write
        .send(WsMessage::Text(
            json!({ "type": "session.input_audio.unmute", "event_id": "u1" })
                .to_string()
                .into(),
        ))
        .await
        .expect("unmute");
    let unmuted = read_until(&mut read, "session.input_audio.unmuted").await;
    assert_eq!(unmuted["client_event_id"], "u1");
}

#[tokio::test]
async fn close_with_delivers_session_closed() {
    let mock = MockGptLive::start(LiveScript {
        actions: vec![LiveAction::CloseWith {
            reason: "close_requested",
            usage_seconds: 12.0,
        }],
        ..LiveScript::default()
    })
    .await
    .expect("bind");

    let (mut write, mut read) = connect(&mock).await.split();
    write.send(session_start_frame()).await.expect("start");
    read_until(&mut read, "session.started").await;

    let closed = read_until(&mut read, "session.closed").await;
    assert_eq!(closed["reason"], "close_requested");
    assert_eq!(closed["usage"]["seconds"], 12.0);
}

#[tokio::test]
async fn a_client_close_is_answered_and_recorded() {
    let mock = MockGptLive::start(LiveScript {
        actions: vec![LiveAction::Delay(Duration::from_secs(30))],
        ..LiveScript::default()
    })
    .await
    .expect("bind");

    let (mut write, mut read) = connect(&mock).await.split();
    write.send(session_start_frame()).await.expect("start");
    read_until(&mut read, "session.started").await;

    write
        .send(WsMessage::Text(
            json!({ "type": "session.close", "event_id": "x1" })
                .to_string()
                .into(),
        ))
        .await
        .expect("close");

    let closed = read_until(&mut read, "session.closed").await;
    assert_eq!(closed["reason"], "close_requested");
    assert!(mock.closed_requested().await);
}

#[tokio::test]
async fn the_scripted_events_carry_the_measured_shapes() {
    let mock = MockGptLive::start(LiveScript {
        actions: vec![
            LiveAction::Transcript {
                speaker: "user",
                delta: " Hallo",
                start_ms: 800,
                end_ms: 1000,
            },
            LiveAction::Transcript {
                speaker: "assistant",
                delta: " Hallo!",
                start_ms: 3200,
                end_ms: 3400,
            },
            LiveAction::SendAudio(vec![1u8, 2, 3, 4]),
            LiveAction::Delegation {
                id: "item_fixture_delegation",
                offset_ms: 11600,
            },
            LiveAction::Usage {
                seconds: 14.0,
                ratio: Some(0.42),
            },
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind");

    let (mut write, mut read) = connect(&mock).await.split();
    write.send(session_start_frame()).await.expect("start");
    read_until(&mut read, "session.started").await;

    let input = read_until(&mut read, "session.input_transcript.delta").await;
    assert_eq!(input["delta"], " Hallo");
    assert_eq!(input["start_ms"], 800);
    assert_eq!(input["end_ms"], 1000);

    let output = read_until(&mut read, "session.output_transcript.delta").await;
    assert_eq!(output["delta"], " Hallo!");

    let audio = read_until(&mut read, "session.output_audio.delta").await;
    assert_eq!(audio["delta"], b64_encode(&[1u8, 2, 3, 4]));

    let delegation = read_until(&mut read, "session.delegation.created").await;
    assert_eq!(delegation["delegation"]["id"], "item_fixture_delegation");
    assert_eq!(delegation["delegation"]["target"], "client");
    assert_eq!(delegation["offset_ms"], 11600);

    let usage = read_until(&mut read, "session.usage.updated").await;
    assert_eq!(usage["usage"]["seconds"], 14.0);
    assert_eq!(usage["context_window"]["usage_ratio"], 0.42);
}

#[tokio::test]
async fn a_scripted_upgrade_status_refuses_the_handshake() {
    let mock = MockGptLive::start(LiveScript {
        upgrade_status: Some(401),
        ..LiveScript::default()
    })
    .await
    .expect("bind");

    let url = format!("{}/v1/live/sessions", mock.base_url());
    let req = url.as_str().into_client_request().expect("request");
    let stream = TcpStream::connect(mock.base_url().trim_start_matches("ws://"))
        .await
        .expect("connect");
    let err = match tokio_tungstenite::client_async(req, stream).await {
        Ok(_) => panic!("the scripted 401 must refuse the handshake"),
        Err(e) => e,
    };
    assert!(
        format!("{err}").contains("401"),
        "expected a 401 in {err}, got none"
    );
}

/// Every fixture under `src/fixtures/gpt_live/` is a server event of the
/// PRIMARY socket — no SIP transport event (OR-L50), and nothing that only
/// looks like JSON. L2a and L3 read these files instead of inventing a shape.
#[test]
fn every_fixture_is_a_primary_socket_event() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fixtures/gpt_live");
    let mut seen = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("the fixture directory") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a file name")
            .to_string();
        let text = std::fs::read_to_string(&path).expect("readable");
        let value: JsonValue = meclaw_core::serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{name} is not JSON: {e}"));
        if name == "reference_run.json" {
            assert_eq!(value["session_id"], "sess_fixture");
            let events = value["events"].as_array().expect("events");
            assert!(events.len() > 20, "the reference run is a whole session");
            for event in events {
                let kind = event["type"].as_str().expect("a type");
                assert!(
                    kind == "session.input_transcript.delta"
                        || kind == "session.output_transcript.delta",
                    "the reference run holds transcript fragments only, found {kind}"
                );
                assert!(event["delta"].is_string());
                assert!(event["start_ms"].is_number());
                assert!(event["end_ms"].is_number());
            }
        } else {
            let kind = value["type"].as_str().expect("a type").to_string();
            assert!(
                !kind.starts_with("transport."),
                "{name} is a SIP transport event, not a primary-socket one"
            );
            seen.push(kind);
        }
    }
    seen.sort();
    assert_eq!(
        seen,
        vec![
            "error",
            "info",
            "response.event",
            "session.closed",
            "session.commentary.appended",
            "session.delegation.created",
            "session.input_audio.muted",
            "session.input_audio.unmuted",
            "session.input_transcript.delta",
            "session.instructions.appended",
            "session.output_audio.delta",
            "session.output_transcript.delta",
            "session.started",
            "session.thinking.appended",
            "session.updated",
            "session.usage.updated",
        ]
    );
}
