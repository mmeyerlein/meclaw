//! Welle Live, L3 — a question the model never answered is still a turn.
//!
//! Rule 5 of R-25-9. The obvious implementation waits for both halves, because
//! a turn "is" a question and an answer — and then a caller who says two things
//! in a row, or who hangs up mid-sentence, loses the first one. What reaches the
//! topology has to be what was said, not what was said and answered.
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
fn two_questions_in_a_row_are_two_turns() {
    let (mut state, mut actions) = run(vec![
        frag(Speaker::User, "are you there", 800, 1600),
        frag(Speaker::User, "hello", 4000, 4400),
    ]);
    actions.extend(live_turns::step(
        &mut state,
        TurnInput::Close,
        GAP_MS,
        BACKCHANNEL_MAX_MS,
    ));
    let closed = turns(&actions);
    assert_eq!(closed.len(), 2, "both of them travelled: {actions:?}");
    assert_eq!(closed[0].user, "are you there");
    assert_eq!(
        closed[0].assistant, None,
        "a turn the model never answered passes as it is"
    );
    assert_eq!(closed[1].user, "hello");
    assert_eq!(closed[1].assistant, None);
    assert_eq!(closed[0].happened_at_ms, 1600);
    assert_eq!(closed[1].happened_at_ms, 4400);
}

/// And what a delegation reads: the open turn, or the last one that closed.
#[test]
fn the_open_turn_is_what_a_delegation_carries() {
    let mut state = TurnState::new();
    assert_eq!(
        live_turns::open_user_text(&state),
        None,
        "nothing has been said yet"
    );
    live_turns::step(
        &mut state,
        frag(Speaker::User, "how warm will it be tomorrow", 800, 2400),
        GAP_MS,
        BACKCHANNEL_MAX_MS,
    );
    assert_eq!(
        live_turns::open_user_text(&state).as_deref(),
        Some("how warm will it be tomorrow"),
        "a delegation created mid-sentence carries the sentence"
    );
    live_turns::step(&mut state, TurnInput::Close, GAP_MS, BACKCHANNEL_MAX_MS);
    assert_eq!(
        live_turns::open_user_text(&state).as_deref(),
        Some("how warm will it be tomorrow"),
        "and with nothing open it falls back to the last turn that closed -- a \
         delegation with no question attached is work nobody can place"
    );
}
