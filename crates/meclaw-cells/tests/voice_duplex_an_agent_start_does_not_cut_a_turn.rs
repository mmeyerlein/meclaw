//! Welle Live, L3 — the model talking over the caller does not end the
//! caller's turn.
//!
//! Rule 4 of R-25-9, and it is not a corner case: the two transcript streams
//! of this model OVERLAP by about 400 ms, measured
//! (`meclaw-next/25-gpt-live/messungen/README.md`). The model starts answering
//! while the last words of the question are still being transcribed, so an
//! assistant fragment whose `start_ms` lies before the caller's last `end_ms`
//! is the normal shape of a conversation.
//!
//! Treating that as a boundary would cut the end off every question in the
//! call.
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

#[test]
fn an_overlapping_answer_leaves_the_question_whole() {
    let (mut state, mut actions) = run(vec![
        frag(Speaker::User, "why is the sky", 800, 3000),
        // Starts 400 ms BEFORE the caller finished — the measured overlap.
        frag(Speaker::Assistant, "Because the light", 3200, 4000),
        frag(Speaker::User, " actually blue", 3400, 3600),
    ]);
    assert!(
        turns(&actions).is_empty(),
        "nothing closed: the answer overlapping the question is not a boundary: \
         {actions:?}"
    );
    actions.extend(live_turns::step(
        &mut state,
        TurnInput::Close,
        GAP_MS,
        BACKCHANNEL_MAX_MS,
    ));
    let closed = turns(&actions);
    assert_eq!(closed.len(), 1, "one question, one turn: {actions:?}");
    assert_eq!(
        closed[0].user, "why is the sky actually blue",
        "the whole question, ending included"
    );
    assert_eq!(closed[0].assistant.as_deref(), Some("Because the light"));
}
