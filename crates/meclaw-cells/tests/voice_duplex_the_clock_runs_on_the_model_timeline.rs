//! Welle Live, zell-uhr (#798, R-L7) — the session's clock is stamped on the
//! MODEL's timeline, not on the connection's wall clock.
//!
//! Rule 1 of R-25-9 closes a user turn `turn_gap_ms` after the caller fell
//! quiet, and it decides that by comparing two numbers: where the clock stands
//! now, and where the caller's last fragment ended. The second one is the
//! model's own stamp — `start_ms`/`end_ms` of a transcript fragment, measured
//! by the provider against the session it opened. So the first one has to be on
//! that same timeline, or the comparison is between two clocks that were never
//! set to each other.
//!
//! They are not. The connection's own elapsed time starts when `run_duplex`
//! does, which is before the handshake, before `session.start` and before the
//! provider ever answers; S0's `a-pacing.json` measured the difference at 626
//! to 750 ms, one-sidedly ahead. Against a shipped gap of 1 000 ms that is a
//! third to three quarters of the silence the rule is supposed to wait for —
//! turns close early and the assistant answers half a sentence.
//!
//! So the tick carries the model's clock: the last offset the provider stamped,
//! plus the time that has passed since it arrived. Before the first stamp there
//! is nothing to be ahead of, and the connection's own elapsed time is all
//! there is.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{boot_fake_clocked, duplex_params, message_text};
use meclaw_cells::voice::contract::{DuplexEvent, Speaker};
use meclaw_core::serde_json::json;
use std::time::Duration;

/// A tick fine enough that the deadline is what the test measures, not the
/// raster. The shipped one is 1 000 ms.
const TICK_MS: u64 = 50;

/// The ordinary audio queue: nothing is parked here.
const AUDIO_CAP: usize = 256;

/// The shipped `turn_gap_ms`, which this fixture cannot override (its duplex
/// block is `echo`, and `echo` has no knobs).
const GAP: Duration = Duration::from_millis(1_000);

/// How far the connection's wall clock is ahead of the model's when the
/// fragment arrives. The measured offset was 626 to 750 ms.
const SKEW: Duration = Duration::from_millis(700);

/// A turn does not close before `turn_gap_ms` of the MODEL's silence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_waits_out_the_gap_on_the_models_clock() {
    let mut live = boot_fake_clocked(
        duplex_params(json!({"default_mode": "auto"})),
        TICK_MS,
        AUDIO_CAP,
    )
    .await;
    let (_client, _hello) = live.connect("session=skew-1&mode=auto").await;
    let session = live.session().await;

    // The session has been up for a while before the model says anything about
    // it — the handshake, the greeting, the first audio. This is the offset,
    // and it is the whole difference between the two clocks.
    tokio::time::sleep(SKEW).await;
    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "When is my appointment?".to_string(),
            start_ms: 0,
            end_ms: 50,
        })
        .await;

    // And then silence. On the model's clock the caller fell quiet at 50 ms, so
    // nothing may close before 1 050 ms of it — a connection counting its own
    // elapsed time is already past that number when the fragment arrives, and
    // would cut the turn within a fraction of the gap.
    let early = tokio::time::timeout(GAP - Duration::from_millis(250), live.emission("turn")).await;
    assert!(
        early.is_err(),
        "a turn closed before the model's own gap had passed: {early:?}"
    );

    // It still closes, and it closes with everything the caller said.
    let emission = live.emission("turn").await;
    assert_eq!(
        message_text(&emission, 0),
        "When is my appointment?",
        "the whole of what the caller said, closed by the clock: {emission}"
    );
}
