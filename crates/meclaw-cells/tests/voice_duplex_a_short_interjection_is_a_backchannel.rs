//! Welle Live, L3 — "mhm" is not a turn.
//!
//! Rule 3 of R-25-9. A caller says "mhm", "uh-huh", "right" while the model is
//! explaining something, and none of it is a new question. Emitting a turn for
//! each would put three empty prompts into the topology and, worse, cut the
//! model's answer into three pieces that were never three thoughts.
//!
//! The threshold is a param (`backchannel_max_ms`, 1500 ms by default), because
//! it was measured on one model in one language.
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

#[test]
fn a_hum_inside_an_answer_joins_the_open_turn() {
    let (mut state, mut actions) = run(vec![
        frag(Speaker::User, "explain that to me again", 800, 2000),
        frag(
            Speaker::Assistant,
            "Sure! The sunlight hits the air molecules",
            10400,
            15800,
        ),
        // 600 ms, well under the ceiling: a listener, not a speaker.
        frag(Speaker::User, "mhm", 12000, 12600),
    ]);
    assert_eq!(
        barge_ins(&actions),
        0,
        "600 ms is a backchannel, and a backchannel interrupts nothing: {actions:?}"
    );
    assert!(
        turns(&actions).is_empty(),
        "and it opens no turn of its own: {actions:?}"
    );
    actions.extend(live_turns::step(
        &mut state,
        TurnInput::Close,
        GAP_MS,
        BACKCHANNEL_MAX_MS,
    ));
    let closed = turns(&actions);
    assert_eq!(closed.len(), 1, "one exchange, one turn: {actions:?}");
    assert_eq!(
        closed[0].user, "explain that to me againmhm",
        "the hum joins the caller's part rather than becoming a turn of its own"
    );
}
