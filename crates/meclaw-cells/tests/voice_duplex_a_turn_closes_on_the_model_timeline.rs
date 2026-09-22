//! Welle Live, L3 — a turn ends where the MODEL's clock says it does.
//!
//! Rule 1 of R-25-9, and the reason it is a rule at all: arrival time is the
//! network's opinion. A frame delayed 300 ms would cut a sentence in two, and a
//! burst that caught up would glue two sentences into one. So the gap is
//! measured between the `end_ms` of the last user fragment and the `start_ms`
//! of the next one, both stamped by the model.
//!
//! The second half is OR-L10: the comparison is `>`, not `>=`. The reference
//! run has two fragments exactly `gap_ms` apart — "Tell me" and the question
//! that follows it — and they are one sentence. At `>=` they would be two
//! turns, and the colony would answer the first half of a question.
//!
//! Armed by L3.

use meclaw_cells::voice::contract::Speaker;
use meclaw_cells::voice::live_turns::{self, FinishedTurn, TurnAction, TurnInput, TurnState};

/// The two numbers this wave ships (`params.duplex`), so every file here reads
/// the reference run the way a shipped colony would.
const GAP_MS: u64 = 1000;
const BACKCHANNEL_MAX_MS: u64 = 1500;

/// One fragment on the model's clock.
fn frag(speaker: Speaker, text: &str, start_ms: u64, end_ms: u64) -> TurnInput {
    TurnInput::Fragment {
        speaker,
        text: text.to_string(),
        start_ms,
        end_ms,
    }
}

/// Run a whole stream through the machine and keep what it decided.
fn run(inputs: Vec<TurnInput>) -> (TurnState, Vec<TurnAction>) {
    let mut state = TurnState::new();
    let mut actions = Vec::new();
    for input in inputs {
        actions.extend(live_turns::step(
            &mut state,
            input,
            GAP_MS,
            BACKCHANNEL_MAX_MS,
        ));
    }
    (state, actions)
}

/// Only the finished turns, in the order they closed.
fn turns(actions: &[TurnAction]) -> Vec<&FinishedTurn> {
    actions
        .iter()
        .filter_map(|a| match a {
            TurnAction::Emit(t) => Some(t),
            _ => None,
        })
        .collect()
}

/// How many barge-ins the run produced.
fn barge_ins(actions: &[TurnAction]) -> usize {
    actions
        .iter()
        .filter(|a| matches!(a, TurnAction::BargeIn))
        .count()
}

/// The reference run, as `meclaw-next/25-gpt-live/messungen/README.md` recorded
/// it: a 200 ms raster, segmented at the 400 ms threshold. Both transcript
/// streams, interleaved the way they actually arrived.
fn reference_run() -> Vec<TurnInput> {
    vec![
        frag(
            Speaker::User,
            "Hello there. How are you doing today",
            800,
            3600,
        ),
        frag(
            Speaker::Assistant,
            "Hi! I'm doing well, thanks.",
            3200,
            4600,
        ),
        frag(Speaker::User, "Tell me", 5400, 6000),
        frag(
            Speaker::User,
            ", can you explain why the sky is actually blue",
            7000,
            10800,
        ),
        frag(
            Speaker::Assistant,
            "Sure! Sunlight hits the air molecules",
            10400,
            15800,
        ),
    ]
}

#[test]
fn the_reference_run_is_two_turns() {
    let (mut state, mut actions) = run(reference_run());
    // The last turn is still open when the recording ends; the close is what a
    // hung-up call sends.
    actions.extend(live_turns::step(
        &mut state,
        TurnInput::Close,
        GAP_MS,
        BACKCHANNEL_MAX_MS,
    ));
    let closed = turns(&actions);
    assert_eq!(
        closed.len(),
        2,
        "the reference run is two turns, not three: {actions:?}"
    );

    assert_eq!(closed[0].index, 0);
    assert_eq!(closed[0].user, "Hello there. How are you doing today");
    assert_eq!(
        closed[0].assistant.as_deref(),
        Some("Hi! I'm doing well, thanks."),
        "the answer belongs to the question it answered"
    );
    assert_eq!(
        closed[0].happened_at_ms, 4600,
        "the turn happened when its last fragment ended"
    );

    assert_eq!(closed[1].index, 1);
    assert_eq!(
        closed[1].user, "Tell me, can you explain why the sky is actually blue",
        "6000 to 7000 is exactly the gap, and exactly the gap is not MORE than \
         the gap (OR-L10): one sentence, one turn"
    );
    assert_eq!(
        closed[1].assistant.as_deref(),
        Some("Sure! Sunlight hits the air molecules")
    );
    assert_eq!(barge_ins(&actions), 0, "nobody interrupted anybody");
}

/// Rule 8, and the half of it that is easy to lose: a `Partial` carries the
/// cumulative text of the part it belongs to, and the part it belongs to is the
/// OPEN turn. A partial that kept growing across a turn boundary would show the
/// caller the previous question again, prefixed to the one being asked.
///
/// L1 left rule 8 without a lock of its own; L3 added this one (OR-L.L3.6).
#[test]
fn every_fragment_replaces_the_partial_of_its_own_part() {
    let (_state, actions) = run(reference_run());
    let partials: Vec<(Speaker, &str)> = actions
        .iter()
        .filter_map(|a| match a {
            TurnAction::Partial { speaker, text } => Some((*speaker, text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(partials.len(), 5, "one per fragment: {actions:?}");
    assert_eq!(
        partials[0],
        (Speaker::User, "Hello there. How are you doing today")
    );
    assert_eq!(
        partials[1],
        (Speaker::Assistant, "Hi! I'm doing well, thanks.")
    );
    assert_eq!(
        partials[2],
        (Speaker::User, "Tell me"),
        "the turn before this one closed, and its words went with it"
    );
    assert_eq!(
        partials[3],
        (
            Speaker::User,
            "Tell me, can you explain why the sky is actually blue"
        )
    );
    assert_eq!(
        partials[4],
        (Speaker::Assistant, "Sure! Sunlight hits the air molecules")
    );
}

/// A gap that passes with nobody speaking closes the turn too — the session's
/// own clock is what carries it (R-L7), so a caller who stops mid-sentence is
/// not a turn held open for the rest of the call.
#[test]
fn a_tick_closes_a_turn_nobody_ended() {
    let mut state = TurnState::new();
    let mut actions = live_turns::step(
        &mut state,
        frag(Speaker::User, "are you still there", 1000, 2000),
        GAP_MS,
        BACKCHANNEL_MAX_MS,
    );
    actions.extend(live_turns::step(
        &mut state,
        TurnInput::Tick { now_ms: 2800 },
        GAP_MS,
        BACKCHANNEL_MAX_MS,
    ));
    assert!(
        turns(&actions).is_empty(),
        "800 ms of silence is a pause, not an end: {actions:?}"
    );
    actions.extend(live_turns::step(
        &mut state,
        TurnInput::Tick { now_ms: 3200 },
        GAP_MS,
        BACKCHANNEL_MAX_MS,
    ));
    let closed = turns(&actions);
    assert_eq!(closed.len(), 1, "2200 ms of it is an end: {actions:?}");
    assert_eq!(closed[0].user, "are you still there");
    assert_eq!(
        closed[0].assistant, None,
        "and the model never got a word in"
    );
}
