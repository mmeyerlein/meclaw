//! Welle Live, zell-uhr (#798, R-L7) — when the clock and the model both have
//! something to say, the model is heard first.
//!
//! The connection's `select!` is `biased`, so the order the arms are written in
//! is the order they are polled in, and that order is a decision rather than a
//! formatting detail. The clock's arm and the provider's events arm can both be
//! ready at the same wake: a tick falls due while a transcript fragment is
//! already sitting in the channel, unread. Whichever is polled first wins the
//! iteration outright.
//!
//! Put the clock first and the loser is always the caller. The tick closes the
//! turn for silence that had in fact already ended — the words were in the
//! channel, one poll away — and the fragment then opens a second turn with the
//! rest of the sentence in it. Two turns, cut in the middle, and the assistant
//! answers the first half.
//!
//! So the events arm goes first: a deadline that fires one tick later is a
//! deadline that fired, and a turn cut in half is a turn lost.
//!
//! # How the two are made to race on purpose
//!
//! Nothing about a real wake is deterministic, so the test does not wait for
//! one. It PARKS the connection task instead: the fake's audio queue is one
//! item deep, so a client that sends a few dozen frames fills the queue behind
//! it and suspends the connection inside the arm that feeds the model. While it
//! is parked the fragment is put in the event channel and the tick falls due;
//! when the queue is drained the task resumes, and both arms are ready in the
//! same poll, every time.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{FRAME_BYTES, boot_fake_clocked, duplex_params, message_text};
use meclaw_cells::voice::contract::{DuplexEvent, Speaker};
use meclaw_core::serde_json::json;
use std::time::Duration;

/// The clock of this session. Short, so the tick is overdue well before the
/// task is let go.
const TICK_MS: u64 = 200;

/// One item: the queue the fake drains into, and the reason it stops draining.
const AUDIO_CAP: usize = 1;

/// Enough frames to fill that queue, the connection's own queue behind it
/// (32 items) and one more — the one the connection parks on.
const FRAMES: usize = 64;

/// How long the connection stays parked. Longer than the shipped `turn_gap_ms`
/// of 1 000 ms, so the open turn is stale by the time the task moves again.
const PARKED: Duration = Duration::from_millis(1_500);

/// A fragment waiting in the channel is taken before a tick that fell due.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_waiting_fragment_beats_a_due_tick() {
    let mut live = boot_fake_clocked(
        duplex_params(json!({"default_mode": "auto"})),
        TICK_MS,
        AUDIO_CAP,
    )
    .await;
    let (mut client, _hello) = live.connect("session=race-1&mode=auto").await;
    let session = live.session().await;

    // The caller starts a sentence.
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "I wanted to ask".to_string(),
            start_ms: 0,
            end_ms: 600,
        })
        .await;

    // And now the connection is held still. Nobody reads `live.audio`, so the
    // fake blocks on its one-item queue, the connection's own queue fills
    // behind it, and the connection suspends feeding the model.
    let frame = vec![0u8; FRAME_BYTES];
    for _ in 0..FRAMES {
        client.send_audio(&frame).await.expect("the socket is open");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        live.audio.len(),
        AUDIO_CAP,
        "the fake's queue is full, so the connection is parked behind it"
    );

    // Parked long enough for the turn to go stale and for the tick to fall due.
    tokio::time::sleep(PARKED).await;

    // The rest of the sentence, put in the channel while nobody is reading it.
    // On the MODEL's timeline the caller paused for 950 ms, just inside the
    // shipped gap of 1 000 ms, so this fragment belongs to the turn that is
    // open -- rule 1 of R-25-9 would not cut on it. On the connection's side
    // the tick fell due while the task was parked, and it reads the clock as
    // 600 + 1 500, which IS past the gap. That is the race: two arms, both
    // ready, one of them holding the words.
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: " about my appointment".to_string(),
            start_ms: 1_550,
            end_ms: 1_900,
        })
        .await;

    // Let go, and keep letting go: every item taken off the fake's queue lets
    // one more frame through, and the connection parks again as soon as they
    // run out. From the first resume onwards the clock's arm and the events arm
    // are both ready in the same poll, and the order they are written in
    // decides what the caller said.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let emission = loop {
        while live.audio.try_recv().is_ok() {}
        match tokio::time::timeout(Duration::from_millis(25), live.emissions.recv()).await {
            Ok(Some(e)) => {
                if duplex_cell::emission_route(&e.content) == Some("turn") {
                    break e.content;
                }
            }
            Ok(None) => panic!("the emission channel closed before a `turn` arrived"),
            Err(_) => assert!(
                std::time::Instant::now() < deadline,
                "no `turn` emission within the failure marker"
            ),
        }
    };
    assert_eq!(
        message_text(&emission, 0),
        "I wanted to ask about my appointment",
        "the fragment that was already in the channel belongs to the turn the tick \
         was about to close: {emission}"
    );
}
