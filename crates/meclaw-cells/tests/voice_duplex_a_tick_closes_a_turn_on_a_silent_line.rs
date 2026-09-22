//! Welle Live, zell-uhr (#798, R-L7) — the turn machine gets a clock of its own.
//!
//! Rule 1 of R-25-9 closes a user turn when `turn_gap_ms` passes with nothing
//! further said. On a line where the caller stops and the model says nothing
//! back, no fragment ever arrives to carry that verdict — so the machine has to
//! be told that time passed.
//!
//! Until this file the thing that told it was `session.usage.updated`, the
//! provider's running meter (OR-L8). That was measured and it does not hold: on
//! a session fed nothing but silence the meter arrived at no point inside 45 s
//! and twice inside 60 s over four runs (GH #798, L9, 2026-09-21), which is
//! exactly the case the sentence was written for. So the meter is a reading of
//! the context window and nothing else, and the clock is the cell's own
//! (`params.duplex.tick_ms`, R-L7).
//!
//! The lock below says the smallest complete version of that: a caller speaks,
//! the model stays silent, NO meter event is ever sent, and the turn still
//! reaches the `turn` lane.

#[path = "support/duplex_cell.rs"]
mod duplex_cell;

use duplex_cell::{boot_fake, duplex_params, header, message_text};
use meclaw_cells::voice::contract::{DuplexEvent, Speaker};
use meclaw_core::serde_json::json;

/// A turn closes on the cell's own clock, with no meter event anywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tick_closes_a_turn_on_a_silent_line() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (_client, _hello) = live.connect("session=tick-1&mode=auto").await;
    let session = live.session().await;

    session
        .say(DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "When is my appointment?".to_string(),
            start_ms: 0,
            end_ms: 900,
        })
        .await;

    // And then nothing at all. No `Usage`, no assistant fragment, no second
    // user fragment — the quiet line of GH #798. What closes the turn is the
    // interval in the connection and nothing else.
    let emission = live.emission("turn").await;
    assert_eq!(
        message_text(&emission, 0),
        "When is my appointment?",
        "the whole of what the caller said, closed by the clock: {emission}"
    );
    assert_eq!(
        header(&emission, "turn_id"),
        Some(&json!("tick-1#0")),
        "the first turn of the session: {emission}"
    );
    assert_eq!(
        header(&emission, "engine"),
        Some(&json!("duplex")),
        "a turn closed by the tick is still a duplex turn: {emission}"
    );
}

/// And the tick does not invent turns: a line nobody spoke on stays empty.
///
/// The other half of the same claim. Without it a tick that closed something
/// every time it fired would pass the test above and fill the colony's memory
/// with empty turns for the length of every call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tick_on_an_empty_line_closes_nothing() {
    let mut live = boot_fake(duplex_params(json!({"default_mode": "auto"}))).await;
    let (_client, _hello) = live.connect("session=tick-2&mode=auto").await;
    let _session = live.session().await;

    // Long enough for several ticks at the shipped `tick_ms`.
    let quiet = tokio::time::timeout(
        std::time::Duration::from_millis(3_500),
        live.emissions.recv(),
    )
    .await;
    assert!(
        quiet.is_err(),
        "nobody said anything, so nothing is a turn: got {quiet:?}"
    );
}
