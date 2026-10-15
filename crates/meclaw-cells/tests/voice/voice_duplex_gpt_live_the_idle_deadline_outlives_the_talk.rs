//! Welle Live, L2a -- voice_duplex_gpt_live_the_idle_deadline_outlives_the_talk.
//!
//! The idle deadline lives OUTSIDE the streaming loop and is reset only by a
//! frame that arrived. This file is the lock on that sentence: the caller talks
//! without a pause, the model says nothing at all, and the session still ends
//! on the deadline.
//!
//! It is the one shape of the guard that a plausible refactor breaks in
//! silence. A `timeout(idle, read.next())` inside the `select!` arm reads the
//! same and does nothing: every time the audio arm wins -- three thousand
//! times a minute against a talking caller -- the timeout future is dropped
//! and built again, so against a dead socket and a live caller it never fires.
//! That is precisely the case it exists for: a connection task waiting on a
//! provider that stopped answering holds an open line and a paying session.
//!
//! The test is in the interleaving class (AGENTS.md rule 11): the fake is a
//! loopback socket, so the clock cannot be paused, and nothing here is judged
//! by the wall clock. The order is made of events: the fake's last word is a
//! transcript fragment, and the adapter hands it on only AFTER it reset its
//! deadline on it -- so once the test holds that fragment, the line is dead.
//! Only then does the caller speak: a burst of frames first, then a frame
//! clock -- a stimulus, the way a phone line hands over a frame every 20 ms --
//! until the adapter lets go of it; the fake stays silent far beyond every
//! hang mark. How much talk reached the dead line is counted from that event
//! on, and what ended the session is told by its reason, never by how long it
//! took.

use meclaw_cells::voice::contract::{
    DuplexControl, DuplexError, DuplexEvent, DuplexProvider, DuplexSession, ProviderTimeouts,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_colony::IoLivenessMark;
use meclaw_testing::deadline;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use meclaw_testing::wait;
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

/// How long the socket may stay quiet: the product parameter under test, at
/// a time factor of one. It is a configuration of the adapter, not a wait of
/// the test -- the test awaits the session's end under its hang mark. It is
/// scaled with the host's time factor (see [`idle`]) because the burst below
/// has to land inside it: a host that stands still longer than the whole
/// deadline between two events is a host the claim cannot be made on.
const IDLE: Duration = Duration::from_millis(500);

/// The frames the caller hands over at once, the moment the line is dead --
/// more than the twenty the claim asks for, fewer than the 32 the audio
/// channel holds, so not one of them waits for room.
const BURST: usize = 24;

/// One 20 ms frame of 16 kHz PCM16 -- the size the connection really sends.
const FRAME_BYTES: usize = 640;

/// The caller's frame clock: a tick every 5 ms, four times the line's 20 ms
/// on purpose -- the more often the audio arm wins, the sharper the claim. A
/// stimulus, never a guess at how long the system needs.
const FRAME: Duration = Duration::from_millis(5);

/// How long the fake keeps silent: an hour, far beyond every hang mark, so the
/// end of its silence can never be what ends the session.
const SILENCE: Duration = Duration::from_secs(3600);

/// [`IDLE`] times this process's time factor.
fn idle() -> Duration {
    IDLE.mul_f64(deadline::time_factor())
}

fn params(base_url: &str) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": base_url,
        "instructions": "Be Egon.",
    }))
    .expect("params parse")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn voice_duplex_gpt_live_the_idle_deadline_outlives_the_talk() {
    // The fake upgrades, answers `session.start`, says one last fragment and
    // then nothing for an hour: a socket that is open and dead.
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_idle".to_string(),
        actions: vec![
            LiveAction::Transcript {
                speaker: "assistant",
                delta: "last word",
                start_ms: 0,
                end_ms: 100,
            },
            LiveAction::Delay(SILENCE),
        ],
        ..LiveScript::default()
    })
    .await
    .expect("bind fake");

    let (audio_in_tx, audio_in_rx) = mpsc::channel(32);
    let (audio_out_tx, _audio_out_rx) = mpsc::channel(32);
    let (events_tx, mut events_rx) = mpsc::channel::<DuplexEvent>(64);
    let (_control_tx, control_rx) = mpsc::channel::<DuplexControl>(16);
    let provider = GptLiveDuplex::new(params(&mock.base_url())).with_timeouts(ProviderTimeouts {
        // The send budget of a frame is the product wait of the test (TV-Z9):
        // a frame that is slow to leave under load is not what this is about.
        external: Duration::from_millis(deadline::product_wait_ms()),
        idle: idle(),
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

    // The caller waits for the line to go dead, then talks for as long as
    // anybody listens: a burst first, handed over without a single await so
    // the whole of it sits in the channel at once, then one frame per tick of
    // its frame clock. The only way out of the loop is the adapter dropping
    // its receiver. A clock that fell behind under load catches up in a burst
    // (the interval's default), so the caller never falls silent for longer
    // than the host stood still. It reports how much it got in.
    let (dead_tx, dead_rx) = tokio::sync::oneshot::channel::<()>();
    let feeder = tokio::spawn(async move {
        if dead_rx.await.is_err() {
            return 0;
        }
        let mut frames = 0usize;
        for _ in 0..BURST {
            if audio_in_tx.try_send(vec![7u8; FRAME_BYTES]).is_err() {
                return frames;
            }
            frames += 1;
        }
        let mut clock = tokio::time::interval(FRAME);
        loop {
            clock.tick().await;
            if audio_in_tx.send(vec![7u8; FRAME_BYTES]).await.is_err() {
                return frames;
            }
            frames += 1;
        }
    });

    assert!(
        matches!(
            deadline::hang("`Started`", events_rx.recv()).await,
            Some(DuplexEvent::Started { .. })
        ),
        "the session opened before it went quiet"
    );
    // The fake's last word. The adapter resets its deadline on a session frame
    // BEFORE it hands the frame on, so holding this fragment means: from here
    // on the line is dead, and every frame the caller sends now is talk into a
    // silence the deadline is already counting.
    assert!(
        matches!(
            deadline::hang("the fake's last word", events_rx.recv()).await,
            Some(DuplexEvent::Transcript { ref delta, .. }) if delta == "last word"
        ),
        "the fake's last fragment reached the adapter before the silence"
    );
    dead_tx
        .send(())
        .expect("the caller is still waiting for the line to go dead");

    let verdict = deadline::hang("the idle deadline to end the session", join)
        .await
        .expect("the session task did not panic")
        .expect_err("a socket that says nothing for half a second is not a session");

    // What ended it is told by the reason: the guard named itself. The fake's
    // hour of silence lies beyond every hang mark, and a fake that gave up or
    // tore the socket ends in `Finalise` or a lost connection, never here.
    let DuplexError::Closed(detail) = &verdict else {
        panic!("a dead socket under a talking caller is `Closed`, not {verdict:?}");
    };
    assert!(
        detail.contains("idle"),
        "the reason names the guard that fired: {detail}"
    );

    // The load-bearing half: the audio arm really was winning. The caller's
    // only exit is the adapter letting go, so the feeder ending at all says it
    // talked until the very end; the frames that reached the socket say the
    // adapter took them and sent them on while the deadline ran down. Both
    // counts are bounded by events, not by time: every frame was sent after
    // the line went dead, the burst sat in the channel whole, and the
    // adapter's `biased` select drains every frame it holds before the
    // deadline arm is even read -- so all of the burst reaches the socket
    // inside the silence. Without this the test would also pass against a
    // guard that any frame in either direction keeps alive.
    let frames = deadline::hang("the feeder to stop when the adapter let go", feeder)
        .await
        .expect("the feeder did not panic");
    assert!(
        frames >= BURST,
        "the caller was talking while the deadline ran down: {frames} frames"
    );
    // The fake reads its socket on a task of its own, so the frames that left
    // the adapter are awaited until the fake has taken them off the wire --
    // never read once and hoped for. A burst that never arrives is a hang.
    wait::until(&format!("{BURST} appends to reach the socket"), || async {
        mock.received_audio().await.len() >= BURST
    })
    .await;
}
