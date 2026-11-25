//! GH #1630 -- a send to the provider never holds the reading back.
//!
//! The session loop awaited every send to the provider inline and read the
//! provider's socket last, so as long as a send hung (measured up to 38 ms on
//! the real endpoint) none of the model's audio was read. The writing half now
//! belongs to a task of its own behind one bounded queue.

use meclaw_cells::voice::contract::{
    AppendKind, DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession,
    ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_testing::deadline::hang;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// How many frames of the caller wait for a provider that takes none, and how
/// long each is: more than the socket's buffers and the send queue together
/// take, so the last of them cannot have left the caller's channel.
const CALLER_FRAMES: usize = 64;
const CALLER_FRAME_BYTES: usize = 512 * 1024;

/// What the model says while the caller's audio stands.
const MODEL_CHUNKS: u8 = 8;
/// How many chunks of the model the connection's outlet holds.
const OUTLET_ROOM: usize = 64;
/// The chunk that says the fake received every frame of the caller.
const ALL_RECEIVED: u8 = 0xEE;

fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
        "keepalive_ms": 600_000,
    }))
    .expect("params parse")
}

/// One session on `mock`, and the ends of its channels.
struct Call {
    audio_in: mpsc::Sender<Vec<u8>>,
    audio_out: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Receiver<DuplexEvent>,
    control: mpsc::Sender<DuplexControl>,
    join: JoinHandle<Result<(), DuplexError>>,
}

/// Opens a session on `mock` whose two inlets hold `room` items, with
/// `spoken` already in the caller's one.
fn call(mock: &MockGptLive, room: usize, spoken: &[Vec<u8>]) -> Call {
    let (audio_in, audio_in_rx) = mpsc::channel(room);
    for frame in spoken {
        audio_in.try_send(frame.clone()).expect("room was counted");
    }
    let (audio_out_tx, audio_out) = mpsc::channel::<Vec<u8>>(OUTLET_ROOM);
    let (events_tx, events) = mpsc::channel::<DuplexEvent>(64);
    let (control, control_rx) = mpsc::channel::<DuplexControl>(room);
    let provider = GptLiveDuplex::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        external: Duration::from_secs(600),
        idle: Duration::from_secs(600),
    });
    let join = tokio::spawn(provider.run_session(
        provider.format(),
        DuplexSession {
            audio_in: audio_in_rx,
            audio_out: audio_out_tx,
            events: events_tx,
            control: control_rx,
        },
        IoLivenessMark::disabled(),
    ));
    Call {
        audio_in,
        audio_out,
        events,
        control,
        join,
    }
}

impl Call {
    async fn next_chunk(&mut self, what: &str) -> Vec<u8> {
        hang(what, self.audio_out.recv())
            .await
            .unwrap_or_else(|| panic!("the session ended before {what}"))
    }

    /// Waits until the session took what stands in the caller's channel.
    async fn audio_taken(&self) {
        hang(
            "the session takes the caller's audio",
            self.audio_in.reserve_many(self.audio_in.max_capacity()),
        )
        .await
        .expect("the session was still taking audio");
    }

    /// Waits until the session took what stands in the guidance channel.
    async fn control_taken(&self) {
        hang(
            "the session takes the guidance",
            self.control.reserve_many(self.control.max_capacity()),
        )
        .await
        .expect("the session was still taking guidance");
    }

    /// Closes the session and expects an orderly end.
    async fn close(mut self) {
        self.control
            .send(DuplexControl::Close)
            .await
            .expect("the session was still taking guidance");
        loop {
            match hang("`Closed` after the close", self.events.recv())
                .await
                .expect("the session ended before `Closed`")
            {
                DuplexEvent::Closed { reason, .. } => {
                    assert_eq!(reason, "close_requested");
                    break;
                }
                _ => continue,
            }
        }
        let verdict = hang("the end of the session after its close", self.join)
            .await
            .expect("the session task did not panic");
        assert!(
            verdict.is_ok(),
            "a requested close is no failure: {verdict:?}"
        );
    }
}

/// The provider takes none of the caller's audio any more and keeps talking:
/// everything it says reaches the connection while the caller's audio stands.
/// Once it takes frames again, every one of them arrives, in order, and the
/// close comes behind them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_uplink_does_not_hold_the_downlink() {
    let mut actions = vec![LiveAction::HoldReading];
    actions.extend((1..=MODEL_CHUNKS).map(|b| LiveAction::SendAudio(vec![b; 640])));
    actions.push(LiveAction::RequireAudioBytes(
        CALLER_FRAMES * CALLER_FRAME_BYTES,
    ));
    actions.push(LiveAction::SendAudio(vec![ALL_RECEIVED; 640]));
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_stalled_uplink".to_string(),
        actions,
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");
    let spoken: Vec<Vec<u8>> = (0..CALLER_FRAMES)
        .map(|n| vec![n as u8; CALLER_FRAME_BYTES])
        .collect();
    let mut call = call(&mock, CALLER_FRAMES, &spoken);

    for want in 1..=MODEL_CHUNKS {
        let chunk = call
            .next_chunk("audio of the model on a stalled uplink")
            .await;
        assert!(
            chunk.iter().all(|b| *b == want),
            "the model's audio arrives in its order, nothing dropped"
        );
    }
    assert!(
        call.audio_in.capacity() < CALLER_FRAMES,
        "the uplink was not stalled: the session took every frame of the caller, so this run \
         says nothing about a send that hangs"
    );

    mock.resume_reading().await;
    let last = call
        .next_chunk("sign that the fake received every frame")
        .await;
    assert!(last.iter().all(|b| *b == ALL_RECEIVED));
    let received = mock.received_audio().await;
    assert_eq!(received.len(), CALLER_FRAMES, "one item, one append");
    for (n, (got, want)) in received.iter().zip(&spoken).enumerate() {
        assert!(got == want, "frame {n} arrived out of order or changed");
    }
    call.close().await;
    assert_eq!(
        mock.received_order().await.last().map(String::as_str),
        Some("session.close"),
        "the close goes behind everything handed over before it"
    );
}

/// Every end of the connection is gone while the provider takes nothing and
/// the model keeps talking: the `session.close` still goes out, behind
/// everything handed over before it, and the session ends in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_close_outlives_every_end_of_the_connection() {
    // More than the outlet and the session's hand hold, so the session still
    // has audio of the model to read once the outlet is gone.
    let said = OUTLET_ROOM + usize::from(MODEL_CHUNKS);
    let mut actions = vec![LiveAction::HoldReading];
    actions.extend((0..said).map(|_| LiveAction::SendAudio(vec![1; 640])));
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_every_end_gone".to_string(),
        actions,
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");
    let spoken: Vec<Vec<u8>> = (0..CALLER_FRAMES)
        .map(|n| vec![n as u8; CALLER_FRAME_BYTES])
        .collect();
    let mut call = call(&mock, CALLER_FRAMES, &spoken);
    call.next_chunk("the first audio of the model").await;

    let Call {
        audio_in,
        audio_out,
        events,
        control,
        join,
    } = call;
    drop((audio_in, audio_out, events, control));
    mock.resume_reading().await;
    let verdict = hang("the end of the session", join)
        .await
        .expect("the session task did not panic");
    assert!(
        verdict.is_ok(),
        "a connection that went away is no failure: {verdict:?}"
    );
    hang("the fake to receive the close", async {
        while !mock.closed_requested().await {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(
        mock.received_order().await.last().map(String::as_str),
        Some("session.close"),
        "the close goes behind everything handed over before it"
    );
}

/// Audio and guidance share one queue: they reach the provider in the order
/// the session took them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn control_frames_keep_their_order_with_audio() {
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_one_order".to_string(),
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");
    let call = call(&mock, 1, &[]);

    // Channels of one: an item is taken before the next is handed over, and
    // that is the whole wait -- nothing here waits for the wire.
    let mut want = vec!["session.start"];
    for round in 0..30u8 {
        call.audio_in
            .send(vec![round; 640])
            .await
            .expect("the session was taking audio");
        call.audio_taken().await;
        want.push("session.input_audio.append");
        let (command, kind) = match round % 3 {
            0 => (DuplexControl::Mute, "session.input_audio.mute"),
            1 => (DuplexControl::Unmute, "session.input_audio.unmute"),
            _ => (
                DuplexControl::Append {
                    kind: AppendKind::Thinking,
                    event_id: format!("ev_{round}"),
                    delegation_id: None,
                    content: "a note".to_string(),
                },
                "session.thinking.append",
            ),
        };
        call.control
            .send(command)
            .await
            .expect("the session was taking guidance");
        call.control_taken().await;
        want.push(kind);
    }
    want.push("session.close");
    call.close().await;
    assert_eq!(mock.received_order().await, want);
    let heard: Vec<u8> = mock.received_audio().await.iter().map(|a| a[0]).collect();
    assert_eq!(heard, (0..30).collect::<Vec<u8>>());
}
