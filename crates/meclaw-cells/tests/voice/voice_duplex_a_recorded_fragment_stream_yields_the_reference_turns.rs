//! Welle Live, L3 — the rules hold against a recording of a real call.
//!
//! The five files beside this one are tables somebody wrote down; this one is
//! the call that produced them. The fixture is the real fragment stream of both
//! transcript channels, with the session identity removed
//! (`crates/meclaw-testing/src/fixtures/gpt_live/reference_run.json`, laid down
//! by L6 out of the wave's own measurement run).
//!
//! The expectation is written **by hand after reading the fragments**, never
//! copied out of what the function returned: a reference taken from the
//! implementation pins the implementation to itself and proves nothing.
//!
//! The fixture never reaches outside the repository (OR-L39) and it always
//! travels with it — it lives under `crates/`, and the export takes `crates/`
//! whole — so this lock has nothing to guard against and skips nothing: a
//! missing file, broken JSON or a changed shape fails loudly. The silent skip
//! of the GH #49 form is how this very lock was green under L1, reading
//! `ereignisse` while the fixture said `events`.
//!
//! Armed by L3, which also filled in the reference numbers below.

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

/// The recorded fragment stream, read strictly.
///
/// Every key is mandatory. A `?` per key (the shape this file had before the
/// fix round) turns a fixture whose SHAPE changed into a green test without a
/// line in the log — it buys nothing, because the file cannot fail to travel,
/// and it costs the whole bite of the lock.
fn recorded() -> Vec<TurnInput> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../meclaw-testing/src/fixtures/gpt_live/reference_run.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reference fixture {} unreadable: {e}", path.display()));
    let doc: meclaw_core::JsonValue = meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("reference fixture is not JSON: {e}"));
    // The fixture is a recording of the wire, so it speaks the wire's words:
    // an array under `events`, each entry a `session.*_transcript.delta` with
    // its text in `delta`. L1 wrote this reader against the key the plan named
    // (`ereignisse`, `speaker`, `text`), which is the shape of the measurement
    // file rather than of what L6 shipped (OR-L.L3.1).
    let events = doc
        .get("events")
        .and_then(|e| e.as_array())
        .unwrap_or_else(|| panic!("the fixture carries its fragments under `events`"));
    let mut out = Vec::new();
    for (i, e) in events.iter().enumerate() {
        let kind = e
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or_else(|| panic!("event {i} has no `type`"));
        let speaker = match kind {
            "session.input_transcript.delta" => Speaker::User,
            "session.output_transcript.delta" => Speaker::Assistant,
            _ => continue,
        };
        let text = e
            .get("delta")
            .and_then(|d| d.as_str())
            .unwrap_or_else(|| panic!("event {i} has no `delta`"));
        let at = |key: &str| {
            e.get(key)
                .and_then(|v| v.as_u64())
                .unwrap_or_else(|| panic!("event {i} has no `{key}`"))
        };
        out.push(frag(speaker, text, at("start_ms"), at("end_ms")));
    }
    out
}

#[test]
fn the_recording_yields_the_turns_a_reader_counted() {
    let inputs = recorded();
    assert_eq!(
        inputs.len(),
        55,
        "the recording is 55 fragments; half a fixture is not half a proof"
    );
    let (mut state, mut actions) = run(inputs);
    actions.extend(live_turns::step(
        &mut state,
        TurnInput::Close,
        GAP_MS,
        BACKCHANNEL_MAX_MS,
    ));
    let closed = turns(&actions);

    // Written by L3 after reading the fragments, event by event, never by
    // copying what the function returned. How the two turns were counted, in
    // the fixture's own event indices:
    //
    //   turn 0: user 0-7 and 9 (" Hallo" .. " so", ends 3600), assistant 8 and
    //           10-13 (" Hallo!" .. " danke.", ends 4600). It closes on the
    //           user fragment at 5400, which is 1800 ms after the caller last
    //           stopped -- more than the gap, and the model had stopped at
    //           4600, so nobody was talking over anybody.
    //   turn 1: user 14-23 and 25, assistant 24 and 26-54. The seam inside it
    //           is the OR-L10 case: 6000 to 7000 is EXACTLY the gap, and
    //           exactly the gap does not cut -- "Sag mal" and the question
    //           that follows are one sentence.
    //
    // And no barge-in anywhere, although the fixture's `scene` line promises
    // one (OR-L.L3.2): rule 3 never applies here, because no fragment of the
    // caller's begins while the model's transcript block is still running.
    // The two that come close are events 9 (3400) and 25 (10600), and each of
    // them begins EXACTLY where the block's last fragment ended -- outside it
    // by the module's own line (`model_speaks_at`, strict `<`), so both fall
    // under rule 1, 200 ms after the caller last stopped, and join the open
    // turn. The disturbance that was recorded happened in the audio, after the
    // agent's transcript block had already ended. The reference is the
    // fragments, not the prose beside them.
    let expected: Vec<(&str, Option<&str>)> = vec![
        (" Hallo Egon. Wie geh", Some(" Hallo! Mir geht's g")),
        (" Sag mal, kannst du ", Some(" Klar! Das Sonnenlic")),
    ];
    let expected_barge_ins = 0usize;
    assert_eq!(closed.len(), expected.len(), "{actions:?}");
    for (turn, (user, assistant)) in closed.iter().zip(expected) {
        let head = |s: &str| s.chars().take(20).collect::<String>();
        assert_eq!(head(&turn.user), user);
        assert_eq!(
            turn.assistant.as_deref().map(head),
            assistant.map(|a| a.to_string())
        );
    }
    assert_eq!(barge_ins(&actions), expected_barge_ins);
}
