//! Welle Live, L1/L2b — a duplex cell never falls through to the cascade's echo
//! branch.
//!
//! The placeholder is the risk this file exists for (OR-L23). A duplex cell
//! carries an `EchoStt` in its recogniser slot — not because anything
//! recognises, but because `VoiceIo::new` is unchanged and twenty hand-built
//! fixtures would otherwise have to move for a field this mode never reads. The
//! price of that decision is exactly one failure mode: some reader asks
//! `shared.stt` before it asks `shared.duplex`, sees the name `echo`, and takes
//! the CASCADE's loopback branch — where a `hold` frame is answered
//! `wrong_mode` with "the echo provider has no turns", and a live model that
//! was listening is never told to unmute.
//!
//! So the assertion is the negative one: with a duplex provider configured, the
//! echo branch is not what answers.
//!
//! **Armed by L2b.** `run_connection` now branches on `shared.duplex` before it
//! reads anything off the cascade pair, so a `hold` here reaches `run_duplex`
//! and opens the model's ear instead of being refused by the loopback branch.

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::voice::contract::{DuplexProvider, ProviderTimeouts};
use meclaw_cells::voice::io::{VoiceIo, run_io};
use meclaw_cells::voice::params::DuplexParams;
use meclaw_cells::voice::providers::{build_duplex, echo::EchoStt};
use meclaw_cells::voice::wire::Mode;
use meclaw_core::Path;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::surface_listener;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Failure-marker timeout: generous, per the 30 s convention.
const MARKER: Duration = Duration::from_secs(30);

/// The mount this fixture registers.
const MOUNT: &str = "voice";

fn loopback() -> Arc<dyn DuplexProvider> {
    let params: DuplexParams = meclaw_core::serde_json::from_value(json!({"provider": "echo"}))
        .expect("the loopback block parses");
    build_duplex(&params, ProviderTimeouts::default()).expect("the loopback adapter")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_duplex_cell_answers_as_a_duplex_cell() {
    let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
    // The test stands in for the handler, and the one thing a handler owes a
    // connection before its `hello` is to take the session (GH #836).
    let (events_tx, mut events_rx) = mpsc::channel::<meclaw_cells::voice::cell::VoiceEvent>(64);
    tokio::spawn(async move {
        while let Some(mut event) = events_rx.recv().await {
            event.acknowledge();
        }
    });
    let (_reconfig_tx, reconfig_rx) = mpsc::channel(64);
    let mut io = VoiceIo::new(
        MOUNT.to_string(),
        // The placeholder this file is about: an inert `EchoStt` beside a real
        // duplex provider, exactly as the factory builds it (OR-L23).
        Arc::new(EchoStt::new()),
        None,
        Mode::Hold,
        Duration::from_secs(5),
        Duration::from_secs(5),
        events_tx,
    );
    io.duplex = Some(loopback());
    io.cell_path = Path::new("/main/members/tester/channels/voice");
    io.surfaces = Arc::clone(&surfaces);
    let _task = tokio::spawn(run_io(io, reconfig_rx));
    let (addr, _listener) = surface_listener(Arc::clone(&surfaces)).await;
    tokio::time::timeout(MARKER, async {
        while surfaces.table().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the cell registers its mount within the failure marker");

    let info: Value = reqwest::get(format!("http://{addr}/{MOUNT}/info"))
        .await
        .expect("GET /info")
        .json()
        .await
        .expect("the declaration is JSON");
    assert_eq!(
        info["duplex"], "echo",
        "the engine is named without opening a session (R-V6): {info}"
    );
    assert_eq!(
        info["stt"], "echo",
        "one provider answers for both directions, so both names are its: {info}"
    );
    assert_eq!(info["tts"], "echo");

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/{MOUNT}/ws"))
        .await
        .expect("the cell accepts a websocket");
    let hello: Value = meclaw_core::serde_json::from_str(
        tokio::time::timeout(MARKER, ws.next())
            .await
            .expect("the hello arrives")
            .expect("the socket is open")
            .expect("a frame")
            .to_text()
            .expect("the first frame is text"),
    )
    .expect("the hello is JSON");
    assert_eq!(hello["duplex"], true, "got {hello}");

    // The measurement point. A `hold` on a duplex connection unmutes the
    // model's ear (OR-L24); it must NOT reach the cascade's loopback branch,
    // which answers this frame with a refusal that names the echo provider.
    ws.send(WsMessage::Text(r#"{"type":"hold"}"#.into()))
        .await
        .expect("the socket takes a control frame");
    let answered = tokio::time::timeout(Duration::from_millis(500), ws.next()).await;
    if let Ok(Some(Ok(msg))) = answered
        && let Ok(text) = msg.to_text()
    {
        assert!(
            !text.contains("the echo provider has no turns"),
            "a duplex connection fell through to the cascade's echo branch: {text}"
        );
        assert!(
            !text.contains("wrong_mode"),
            "and a `hold` in hold mode is not a wrong-mode frame: {text}"
        );
    }
}
