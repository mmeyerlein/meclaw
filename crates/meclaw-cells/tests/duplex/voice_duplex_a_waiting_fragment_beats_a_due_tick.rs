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
//! The clock is paused once the cell is booted (the boot binds a socket and
//! waits for the mount, so the test cannot carry `start_paused = true`); from
//! there on everything is tokio timers and channels, and no wake is a matter of
//! the machine's load. The test PARKS the connection task instead: the client stops reading its link
//! and the model says more than the link holds, so the connection suspends
//! inside the arm that sends the model's audio. While it is parked the fragment
//! is put in the event channel and the tick falls due; when the client reads on
//! the task resumes, and both arms are ready in the same poll, every time. (A
//! model that takes no audio parks nothing any more, GH #1634.)

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{FRAME_BYTES, boot_fake_clocked, duplex_params, message_text};
use meclaw_cells::voice::contract::{DuplexEvent, Speaker};
use meclaw_core::serde_json::json;
use meclaw_testing::wait;
use std::time::Duration;

/// The clock of this session. Short, so the tick is overdue well before the
/// task is let go.
const TICK_MS: u64 = 200;

/// The frames of the chunk the connection parks on, far more than a link holds.
const FRAMES: usize = 1_000;

/// How long the connection stays parked. Longer than the shipped `turn_gap_ms`
/// of 1 000 ms, so the open turn is stale by the time the task moves again.
const PARKED: Duration = Duration::from_millis(1_500);

/// A fragment waiting in the channel is taken before a tick that fell due.
#[tokio::test(flavor = "current_thread")]
async fn a_waiting_fragment_beats_a_due_tick() {
    let mut live =
        boot_fake_clocked(duplex_params(json!({"default_mode": "auto"})), TICK_MS, 256).await;
    tokio::time::pause();
    let (mut client, _hello) = live.link("race-1", "auto").await;
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

    // And now the connection is held still. The client reads nothing, so the
    // link fills behind one long chunk of the model's silence, and the
    // connection suspends sending it: the chunk after it is never taken.
    for frames in [FRAMES, 1] {
        session
            .audio_out
            .send(vec![0u8; FRAME_BYTES * frames])
            .await
            .expect("the connection is there");
    }
    wait::until("the connection is parked on the first chunk", || {
        let waiting = session.audio_out.max_capacity() - session.audio_out.capacity();
        async move { waiting == 1 }
    })
    .await;
    // Parked long enough for the turn to go stale and for the tick to fall due.
    tokio::time::advance(PARKED).await;

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

    // Let go: with the last frame of the chunk the task resumes, the clock's arm
    // and the events arm are both ready in the same poll, and the order they are
    // written in decides what the caller said.
    for _ in 0..FRAMES {
        client.next_audio().await;
    }
    let emission = live.emission("turn").await;
    assert_eq!(
        message_text(&emission, 0),
        "I wanted to ask about my appointment",
        "the fragment that was already in the channel belongs to the turn the tick \
         was about to close: {emission}"
    );
}
