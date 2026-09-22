//! Welle Live, L3 — a sentence over the model IS an interruption, and it is
//! announced exactly once.
//!
//! OR-L18. The same run of user audio that is a backchannel at 600 ms is a
//! barge-in at 2200, and the difference matters twice over: the open turn
//! closes, and the connection is told to cut the audio that is already on its
//! way to the caller (`speak_end cancelled`, which is what the telephony hive's
//! `uuid_break` hangs off).
//!
//! **Exactly once per agent block** is the half that is easy to get wrong. The
//! run arrives as eleven 200 ms fragments; ten of them are past the ceiling, and
//! a barge-in per fragment would fire ten cancels at one sentence.
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
fn a_long_run_over_the_model_barges_in_exactly_once() {
    let mut inputs = vec![
        frag(Speaker::User, "explain that to me again", 800, 2000),
        frag(
            Speaker::Assistant,
            "Sure! The sunlight hits the air molecules",
            10400,
            15800,
        ),
    ];
    // 12000..14200, as the raster delivers it: eleven fragments of 200 ms. The
    // run passes 1500 ms on the fragment 13400..13600, the eighth, where it
    // reaches 1600.
    for i in 0..11u64 {
        let start = 12000 + i * 200;
        inputs.push(frag(Speaker::User, "no ", start, start + 200));
    }
    let (mut state, mut actions) = run(inputs);
    assert_eq!(
        barge_ins(&actions),
        1,
        "one interruption is one barge-in, however many fragments carried it: \
         {actions:?}"
    );
    let closed = turns(&actions);
    assert_eq!(
        closed.len(),
        1,
        "and it closed the turn it interrupted: {actions:?}"
    );
    assert_eq!(closed[0].user, "explain that to me again");
    assert_eq!(
        closed[0].assistant.as_deref(),
        Some("Sure! The sunlight hits the air molecules")
    );

    actions.extend(live_turns::step(
        &mut state,
        TurnInput::Close,
        GAP_MS,
        BACKCHANNEL_MAX_MS,
    ));
    let closed = turns(&actions);
    assert_eq!(closed.len(), 2, "the interruption is the next turn");
    assert_eq!(
        closed[1].user,
        "no ".repeat(11),
        "and it carries the WHOLE run, from before the ceiling was crossed"
    );
    assert_eq!(closed[1].index, 1);
}

#[test]
fn a_second_agent_block_earns_a_second_barge_in() {
    // "Exactly once" counts agent BLOCKS, not turns (OR-L18). The model falls
    // silent, the caller has the floor, and then the model starts a SECOND
    // answer -- that answer is interruptible like the first one, and its
    // interruption has to reach the connection, because that is what becomes
    // the cancelled `speak_end` and the `uuid_break`. S0 measured the model
    // still speaking 4.4-11.0 s after a disturbance: a second block over a
    // caller who is already talking is the normal case here, not the exotic
    // one.
    let mut inputs = vec![
        frag(Speaker::User, "explain that to me again", 800, 2000),
        frag(
            Speaker::Assistant,
            "Sure! The sunlight hits the air molecules",
            10400,
            15800,
        ),
    ];
    for i in 0..11u64 {
        let start = 12000 + i * 200;
        inputs.push(frag(Speaker::User, "no ", start, start + 200));
    }
    // 4200 ms of silence between the two blocks -- far past `GAP_MS`, so this
    // is a new answer and not the tail of the old one still trickling in.
    inputs.push(frag(
        Speaker::Assistant,
        "So, from the top again",
        20000,
        30000,
    ));
    for i in 0..11u64 {
        let start = 22000 + i * 200;
        inputs.push(frag(Speaker::User, "yes ", start, start + 200));
    }
    let (mut state, mut actions) = run(inputs);
    assert_eq!(
        barge_ins(&actions),
        2,
        "two agent blocks, two long runs over them: two barge-ins, not one for \
         the whole call: {actions:?}"
    );

    actions.extend(live_turns::step(
        &mut state,
        TurnInput::Close,
        GAP_MS,
        BACKCHANNEL_MAX_MS,
    ));
    let closed = turns(&actions);
    assert_eq!(
        closed.len(),
        3,
        "two interruptions cut two turns: {actions:?}"
    );
    assert_eq!(closed[1].user, "no ".repeat(11));
    assert_eq!(
        closed[1].assistant.as_deref(),
        Some("So, from the top again"),
        "the second block belongs to the turn the first interruption opened"
    );
    assert_eq!(closed[2].user, "yes ".repeat(11));
    assert_eq!(closed[2].index, 2);
}
