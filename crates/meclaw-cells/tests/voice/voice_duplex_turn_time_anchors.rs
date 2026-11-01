//! live-2 LC2 fix round, M3 — the two time marks of a closed turn.
//!
//! `t_person_end_ms` is where the caller's last words before the answer ended,
//! `t_first_audio_ms` when the first audio frame of the model's answer to them
//! went out to the client (BAT R1e, C3: no longer the answer's transcript),
//! both on the call's clock. A backchannel inside the answer is not "last words before
//! the answer", the run a barge-in cuts off belongs to the turn it opens, and
//! the tail of an interrupted block is not the answer to the interruption.
//! In each case the old anchors put the person's end behind the first answer.

use meclaw_cells::voice::contract::Speaker;
use meclaw_cells::voice::live_turns::{self, FinishedTurn, TurnAction, TurnInput, TurnState};

/// The shipped numbers (`params.duplex`).
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

/// The connection sent model audio to the client at `at_ms`.
fn audio(at_ms: u64) -> TurnInput {
    TurnInput::AudioOut { at_ms }
}

/// The finished turns of a whole stream, in the order they closed.
fn turns(inputs: Vec<TurnInput>) -> Vec<FinishedTurn> {
    let mut state = TurnState::new();
    let mut out = Vec::new();
    for input in inputs {
        for action in live_turns::step(&mut state, input, GAP_MS, BACKCHANNEL_MAX_MS) {
            if let TurnAction::Emit(t) = action {
                out.push(t);
            }
        }
    }
    out
}

#[test]
fn a_backchannel_inside_the_answer_moves_no_anchor() {
    let t = turns(vec![
        frag(Speaker::User, "what is the time", 0, 900),
        audio(1340),
        frag(Speaker::Assistant, "it is ", 1300, 3000),
        frag(Speaker::Assistant, "ten past five", 3000, 5000),
        frag(Speaker::User, "mhm", 3200, 3500),
        audio(3530),
        TurnInput::Tick { now_ms: 6200 },
    ]);
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!(t[0].person_end_ms, Some(900), "{t:?}");
    assert_eq!(t[0].first_audio_ms, Some(1340), "{t:?}");
}

#[test]
fn a_barge_in_keeps_both_turns_on_their_own_anchors() {
    let t = turns(vec![
        frag(Speaker::User, "tell me a story", 0, 900),
        audio(1340),
        frag(Speaker::Assistant, "once ", 1300, 3000),
        frag(Speaker::Assistant, "upon a time", 3000, 6000),
        frag(Speaker::User, "wait ", 3200, 4000),
        frag(Speaker::User, "stop please", 4000, 5000),
        // The model needs a moment to stop: the tail of the old block, and
        // its audio, is no answer to the interruption.
        audio(5050),
        frag(Speaker::Assistant, " there", 5100, 6000),
        audio(6010),
        // Its answer to the interruption, a new block.
        frag(Speaker::Assistant, "sure", 7500, 8500),
        audio(7630),
        TurnInput::Tick { now_ms: 10_000 },
    ]);
    assert_eq!(t.len(), 2, "{t:?}");
    assert_eq!(t[0].person_end_ms, Some(900), "closed turn: {t:?}");
    assert_eq!(t[0].first_audio_ms, Some(1340), "closed turn: {t:?}");
    assert_eq!(t[1].person_end_ms, Some(5000), "barge-in turn: {t:?}");
    assert_eq!(t[1].first_audio_ms, Some(7630), "barge-in turn: {t:?}");
}

#[test]
fn an_answer_overlapping_the_last_words_never_lies_before_them() {
    let t = turns(vec![
        frag(Speaker::User, "is it raining", 0, 1400),
        audio(1000),
        frag(Speaker::Assistant, "no", 1000, 2500),
        TurnInput::Tick { now_ms: 4000 },
    ]);
    assert_eq!(t.len(), 1, "{t:?}");
    assert_eq!(t[0].person_end_ms, Some(1400), "{t:?}");
    assert_eq!(t[0].first_audio_ms, Some(1400), "{t:?}");
}
