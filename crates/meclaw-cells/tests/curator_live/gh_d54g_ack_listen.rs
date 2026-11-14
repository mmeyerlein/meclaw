//! D54g ack (GH #1504, contract points 4 to 7) -- the turn of a running
//! session is only heard, and its delegation goes straight to the target
//! the composition names.
//!
//! WHY this file exists: a running session whose model speaks itself
//! reports every closed turn with both voices, and the curator treated it
//! like any turn: stamp, lead, a call of a target, an answer into the
//! session. Only beside an open delegation of that very turn the turn path
//! parked (GH #1407). Without one two answered: the session's own model and
//! the target. With the lead's `live_turn` `listen` the policy sends no
//! call for such a turn -- the same place and the same course as the park
//! beside a delegation: `pending:<call>` falls, mark `turn_parked` with
//! `{reason: "listen", iter}`; the turn stays in the history with both
//! voices. The live feed keeps its beat: a heard turn has no answer that
//! closes it, so the park itself starts the phase `live`, once per turn,
//! when `live_feed` is on -- in both forms the delegation park is written
//! in (`errand_outcome` "0" and "1"). The question to the lead names the
//! kind of an event round (`event`, read off the round's structure: its
//! event key and the `delegation_id` at the seam, never off its words), so
//! the lead's `event_routes` sends a delegation to its target without a
//! stamp; one utterance runs one tool round, the delegation's.
//!
//! Every knob is set as the override of an instance. The hive in one
//! process (`support/curator_hive.rs`, `support/curator_live.rs`); no model.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;
// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_feed::{
    KNOB, OWN_LIVE, QUESTION, SESSION, crisis_knobs, crisis_partial, feed_marks_of, feeds_of,
};
use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const OPEN_UNTIL: u64 = 4_102_444_800_000;
/// The member turn the pins hear, and the label of its delegation round.
const LABEL: &str = "voice-turn-1";
/// The session the delegation round runs under, and the one of the turn path.
const DELEGATION_SESSION: &str = "deleg-session-5";
const TURN_SESSION: &str = "phone-2026-11-12T09:06:50Z";
/// What the person said and what the session's own model said in the turn.
const ASKED: &str = "is it going to rain in the afternoon";
const SPOKEN: &str = "one moment while the forecast comes in";

/// The round key of the delegation of turn `label`.
fn event_key(label: &str) -> String {
    format!("{label}~{OPEN_UNTIL}~ab12cd34")
}

/// One `in_curate` of `turn` in `session` at iteration `iter`: `keys` in
/// hop and context, `extra` in the hop, `messages` the round's elements.
#[allow(clippy::too_many_arguments)]
fn curate(
    h: &mut Hive,
    session: &str,
    turn: &str,
    iter: &str,
    path: &str,
    keys: &Value,
    extra: Value,
    messages: Value,
) {
    h.out.clear();
    let mut hop = json!({"session_id": session, "turn_id": turn, "iter": iter, "phase": "",
                         "path": path, "needs": "", "families": ""});
    let mut ctx = json!({"session_id": session, "turn_id": turn, "iter": iter,
                         "channel": "test", "audience_set": ROUND_E, "path": path});
    for (k, v) in obj(keys.clone()) {
        hop[k.as_str()] = v.clone();
        ctx[k.as_str()] = v;
    }
    for (k, v) in obj(extra) {
        hop[k.as_str()] = v;
    }
    h.lane("in_curate", ctx, hop, json!({"messages": messages}));
}

/// The turn path of the running session: turn `turn`, stamped `path`.
fn turn_path(h: &mut Hive, turn: &str, path: &str, extra: Value) {
    curate(
        h,
        TURN_SESSION,
        turn,
        "0",
        path,
        &live_keys(LIVE),
        extra,
        json!([user(ASKED)]),
    );
}

/// The delegation round of turn [`LABEL`] opens (its first iteration).
fn delegation(h: &mut Hive, path: &str) {
    curate(
        h,
        DELEGATION_SESSION,
        &event_key(LABEL),
        "0",
        path,
        &live_keys(LIVE),
        json!({"delegation_id": "item_d1", "needs": "tool", "families": "web"}),
        json!([user(ASKED)]),
    );
}

/// Every call that left since `out` was cleared: a model call (a side call
/// included) or an errand.
fn calls(h: &Hive) -> Vec<Msg> {
    h.out
        .iter()
        .filter(|m| m.route() == "model_call" || m.route() == "errand")
        .cloned()
        .collect()
}

/// The values of every mark of `kind`, oldest first.
fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null))
    .collect()
}

/// The parked calls (`pending:<call>`), without the delegation parks.
fn pending_calls(h: &Hive) -> Vec<String> {
    state_keys(h, "pending:")
        .into_iter()
        .filter(|k| !k.starts_with("pending:delegation:"))
        .collect()
}

/// How often the ledger was asked in phase `phase` since step `since`.
fn steps_in(h: &Hive, since: usize, phase: &str) -> usize {
    h.ledger_steps[since..]
        .iter()
        .filter(|(_, p)| p == phase)
        .count()
}

/// The hive with the lead told to only hear the turn of a running session.
fn listening(over: &[(&str, &str, Value)]) -> Hive {
    let mut all: Vec<(&str, &str, Value)> = vec![("lead", "live_turn", json!("listen"))];
    all.extend(over.iter().cloned());
    live_hive(&all)
}

#[test]
fn gh_d54g_ack_the_question_names_the_event_of_its_round() {
    if !shipped() {
        return;
    }
    // The lead sends a round whose question names the event `delegation` to
    // `quick`; every round below is stamped `talk`, which goes to `brain`.
    let over = [("lead", "event_routes", json!("delegation:quick"))];
    let to = |h: &Hive| (h.routed("quick").len(), h.routed("brain").len());

    // An event key and a delegation id at the seam: the delegation round.
    let mut h = live_hive(&over);
    curate(
        &mut h,
        DELEGATION_SESSION,
        &event_key(LABEL),
        "0",
        "talk",
        &live_keys(LIVE),
        json!({"delegation_id": "item_d1"}),
        json!([user(ASKED)]),
    );
    assert_eq!(
        to(&h),
        (1, 0),
        "the question of a delegation round names its event: {:?} {:?}",
        h.out,
        h.stderr
    );

    // The turn's own round names none.
    let mut h = live_hive(&over);
    turn_path(&mut h, LABEL, "talk", json!({}));
    assert_eq!(to(&h), (0, 1), "a turn's own round: {:?}", h.stderr);

    // Read off the structure: an event key without a delegation id is no
    // delegation, nor is a delegation id on a turn's own key.
    let mut h = live_hive(&over);
    curate(
        &mut h,
        DELEGATION_SESSION,
        &event_key(LABEL),
        "0",
        "talk",
        &live_keys(LIVE),
        json!({}),
        json!([user(ASKED)]),
    );
    assert_eq!(to(&h), (0, 1), "an event key alone: {:?}", h.stderr);
    let mut h = live_hive(&over);
    turn_path(&mut h, LABEL, "talk", json!({"delegation_id": "item_d1"}));
    assert_eq!(to(&h), (0, 1), "a delegation id alone: {:?}", h.stderr);

    // Never off the words of the round.
    let mut h = live_hive(&over);
    curate(
        &mut h,
        TURN_SESSION,
        LABEL,
        "0",
        "talk",
        &live_keys(LIVE),
        json!({}),
        json!([user(&format!(
            "[delegation item_d1 from the voice channel] {ASKED}"
        ))]),
    );
    assert_eq!(to(&h), (0, 1), "the words name no event: {:?}", h.stderr);
}

#[test]
fn gh_d54g_ack_a_heard_turn_sends_no_call_and_is_marked() {
    if !shipped() {
        return;
    }
    let mut h = listening(&[]);
    let pending_before = pending_calls(&h);

    // A turn that would get a tool round on the fast target.
    turn_path(
        &mut h,
        LABEL,
        "simple",
        json!({"needs": "tool", "families": "web"}),
    );
    assert!(
        calls(&h).is_empty(),
        "a heard turn sends no call and no side call: {:?} {:?}",
        calls(&h),
        h.stderr
    );
    assert_eq!(
        marks(&h, "turn_parked"),
        vec![json!({"reason": "listen", "iter": 0})],
        "the heard turn is marked"
    );
    assert_eq!(
        pending_calls(&h),
        pending_before,
        "the parked call of the heard turn falls"
    );

    // A turn whose call would leave the composition: nothing leaves either.
    turn_path(
        &mut h,
        "voice-turn-2",
        "complex",
        json!({"families": "web"}),
    );
    assert!(calls(&h).is_empty(), "{:?} {:?}", calls(&h), h.stderr);
    assert_eq!(marks(&h, "turn_parked").len(), 2);

    // A turn without a handle is no running session's: called as before.
    curate(
        &mut h,
        "text-session-1",
        "text-turn-1",
        "0",
        "talk",
        &json!({}),
        json!({}),
        json!([user(ASKED)]),
    );
    assert_eq!(
        h.routed("brain").len(),
        1,
        "a turn without a handle gets its call: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(marks(&h, "turn_parked").len(), 2);

    // The heard turn stays in the history, with both voices.
    let mut h = listening(&[]);
    curate(
        &mut h,
        TURN_SESSION,
        "voice-turn-3",
        "0",
        "talk",
        &live_keys(LIVE),
        json!({}),
        json!([user(ASKED), said(SPOKEN)]),
    );
    assert!(calls(&h).is_empty(), "{:?} {:?}", calls(&h), h.stderr);
    let history: Vec<String> = h
        .rows(
            "SELECT b.body FROM wall w JOIN blocks b ON b.hash = w.hash \
             WHERE w.turn_id = 'voice-turn-3' ORDER BY w.seq",
        )
        .iter()
        .map(|r| r[0].as_str().unwrap_or("").to_string())
        .collect();
    for (who, words) in [("the person", ASKED), ("the session's model", SPOKEN)] {
        assert!(
            history.iter().any(|b| b.contains(words)),
            "the words of {who} stay in the history: {history:?}"
        );
    }
}

#[test]
fn gh_d54g_ack_a_heard_turn_still_gets_its_live_tick_once() {
    if !shipped() {
        return;
    }
    // Both forms the delegation park is written in.
    for errand_outcome in ["0", "1"] {
        let form = [("policy", "errand_outcome", json!(errand_outcome))];

        // The heard turn alone: a speculative curate of it first, which is
        // no tick, then the turn's own -- one tick, and it finds the handle
        // (the turn's row was written ahead of it: the facts are read).
        let mut h = listening(&form);
        let since = h.ledger_steps.len();
        turn_path(&mut h, LABEL, "simple", json!({"spec": "1"}));
        assert_eq!(
            steps_in(&h, since, "live"),
            0,
            "errand_outcome {errand_outcome}: a speculation is no tick"
        );
        turn_path(&mut h, LABEL, "simple", json!({}));
        assert!(calls(&h).is_empty(), "{:?} {:?}", calls(&h), h.stderr);
        assert_eq!(
            (
                steps_in(&h, since, "live"),
                steps_in(&h, since, "live-facts")
            ),
            (1, 1),
            "errand_outcome {errand_outcome}: the heard turn gets its live tick once, \
             and the tick reads the facts of its handle: {:?}",
            h.stderr
        );
        assert!(
            state_keys(&h, "live_turn:").is_empty(),
            "errand_outcome {errand_outcome}: the tick took the turn's row: {:?}",
            state_keys(&h, "live_turn:")
        );

        // Beside an open delegation of the turn: the park of the delegation
        // stands as it was written, the heard turn gets its one tick.
        let mut h = listening(&form);
        delegation(&mut h, "");
        assert_eq!(
            calls(&h).len(),
            1,
            "errand_outcome {errand_outcome}: the delegation makes its call: {:?}",
            h.stderr
        );
        assert_eq!(
            state_keys(&h, "pending:delegation:"),
            vec![format!("pending:delegation:{LIVE}#{LABEL}")],
            "errand_outcome {errand_outcome}: the delegation park is written as before"
        );
        let since = h.ledger_steps.len();
        turn_path(&mut h, LABEL, "simple", json!({}));
        assert!(calls(&h).is_empty(), "{:?} {:?}", calls(&h), h.stderr);
        assert_eq!(
            steps_in(&h, since, "live"),
            1,
            "errand_outcome {errand_outcome}: one tick beside the delegation: {:?}",
            h.stderr
        );
    }

    // With the live feed off there is no tick, and the turn is heard all
    // the same.
    let mut h = Hive::with(&[
        ("policy", "live_feed", json!("0")),
        ("lead", "live_turn", json!("listen")),
    ])
    .reporting_composite("talky");
    let since = h.ledger_steps.len();
    turn_path(&mut h, LABEL, "simple", json!({}));
    assert!(calls(&h).is_empty(), "{:?} {:?}", calls(&h), h.stderr);
    assert_eq!(steps_in(&h, since, "live"), 0, "no live feed, no tick");
    assert_eq!(marks(&h, "turn_parked").len(), 1);
}

#[test]
fn gh_d54g_ack_a_delegation_beside_a_heard_turn_runs_one_tool_round() {
    if !shipped() {
        return;
    }
    let over = [
        ("lead", "live_turn", json!("listen")),
        ("lead", "event_routes", json!("delegation:quick")),
    ];
    for turn_first in [true, false] {
        let mut h = live_hive(&over);
        if turn_first {
            turn_path(
                &mut h,
                LABEL,
                "simple",
                json!({"needs": "tool", "families": "web"}),
            );
            assert!(calls(&h).is_empty(), "{:?} {:?}", calls(&h), h.stderr);
        }

        // The delegation is never only heard: it makes its call, on the
        // target its route names, with no side call beside it.
        delegation(&mut h, "");
        let first = calls(&h);
        assert_eq!(
            first.len(),
            1,
            "turn first {turn_first}: the delegation makes one call: {:?} {:?}",
            h.out,
            h.stderr
        );
        assert_eq!(
            h.routed("quick").len(),
            1,
            "turn first {turn_first}: on the target of its route: {:?}",
            first[0].hop
        );
        assert_eq!(
            state_keys(&h, "pending:delegation:"),
            vec![format!("pending:delegation:{LIVE}#{LABEL}")],
            "turn first {turn_first}: the delegation park is written as before"
        );

        if !turn_first {
            turn_path(
                &mut h,
                LABEL,
                "simple",
                json!({"needs": "tool", "families": "web"}),
            );
            assert!(calls(&h).is_empty(), "{:?} {:?}", calls(&h), h.stderr);
        }
        assert_eq!(
            marks(&h, "turn_parked"),
            vec![json!({"reason": "listen", "iter": 0})],
            "turn first {turn_first}: the turn path is heard, whichever came first"
        );

        // The one tool round of the utterance: the delegation asks for the
        // tool, the result comes back on iteration 1, the target answers.
        h.out.clear();
        h.tap(
            &first[0],
            "tool_calls",
            json!({}),
            json!([tool_call("c1", "web_search")]),
        );
        curate(
            &mut h,
            DELEGATION_SESSION,
            &event_key(LABEL),
            "1",
            "",
            &live_keys(LIVE),
            json!({"delegation_id": "item_d1"}),
            json!([
                tool_call("c1", "web_search"),
                tool_result("c1", "dry until the evening")
            ]),
        );
        let second = calls(&h);
        assert_eq!(
            second.len(),
            1,
            "turn first {turn_first}: the round's answer call: {:?} {:?}",
            h.out,
            h.stderr
        );
        h.out.clear();
        h.tap(
            &second[0],
            "stop",
            json!({}),
            json!([said("It stays dry until the evening.")]),
        );
        assert!(calls(&h).is_empty(), "{:?} {:?}", calls(&h), h.stderr);

        // Every call of the utterance is the delegation's: two calls, one
        // tool round, none under the turn's own id.
        let called: Vec<String> = h
            .rows("SELECT turn_id FROM calls ORDER BY started_at")
            .iter()
            .map(|r| r[0].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            called,
            vec![event_key(LABEL), event_key(LABEL)],
            "turn first {turn_first}: one tool round for the utterance, the delegation's"
        );
    }
}

/// The hive in which a crisis heard on a partial interrupts at once
/// (`support/curator_feed.rs`), `over` on top.
fn crisis_hive(over: &[(&'static str, &'static str, Value)]) -> Hive {
    let mut all = crisis_knobs();
    all.push(("policy", KNOB, json!("1")));
    all.extend(over.iter().cloned());
    Hive::with(&all).reporting_composite("talky")
}

/// The end of the crisis turn `turn`: its own curate on the turn path of
/// the running session the partial belongs to.
fn crisis_turn(h: &mut Hive, turn: &str) {
    curate(
        h,
        SESSION,
        turn,
        "0",
        "talk",
        &live_keys(OWN_LIVE),
        json!({}),
        json!([user("i do not want to go on")]),
    );
}

#[test]
fn gh_d54g_ack_a_heard_crisis_turn_still_sends_its_early_interrupt_once() {
    if !shipped() {
        return;
    }
    // The control, as shipped (`call`): the partial interrupts once, and
    // the turn gets its call.
    let mut h = crisis_hive(&[]);
    crisis_partial(&mut h, "t1");
    assert_eq!(
        feeds_of(&h, "interrupt").len(),
        1,
        "the control interrupts: {:?} {:?}",
        h.out,
        h.stderr
    );
    crisis_turn(&mut h, "t1");
    assert!(
        !calls(&h).is_empty(),
        "the control's turn is called: {:?} {:?}",
        h.out,
        h.stderr
    );

    // Told to only hear the turn: the guard's interrupt is no call of a
    // target -- it leaves all the same, exactly once.
    let mut h = crisis_hive(&[("lead", "live_turn", json!("listen"))]);
    crisis_partial(&mut h, "t1");
    let sent = feeds_of(&h, "interrupt");
    assert_eq!(
        sent.len(),
        1,
        "the early interrupt of a heard crisis turn leaves: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(sent[0].messages()[0]["text"], json!(QUESTION));
    assert!(calls(&h).is_empty(), "{:?} {:?}", calls(&h), h.stderr);
    crisis_partial(&mut h, "t1");
    assert!(
        feeds_of(&h, "interrupt").is_empty(),
        "a second partial of the turn: none"
    );

    // The turn itself is heard: no call of a target, and no second
    // interrupt at its end.
    crisis_turn(&mut h, "t1");
    assert!(
        calls(&h).is_empty(),
        "the heard crisis turn sends no call: {:?} {:?}",
        calls(&h),
        h.stderr
    );
    assert!(
        feeds_of(&h, "interrupt").is_empty(),
        "no second interrupt at the turn's end: {:?}",
        h.out
    );
    assert_eq!(
        marks(&h, "turn_parked"),
        vec![json!({"reason": "listen", "iter": 0})]
    );
    assert_eq!(
        feed_marks_of(&h, "interrupt").len(),
        1,
        "one interrupt for the turn, partials and end together"
    );

    // The next turn of the session may interrupt again.
    crisis_partial(&mut h, "t2");
    assert_eq!(feeds_of(&h, "interrupt").len(), 1, "{:?}", h.stderr);
}

/// A partial of `turn` of the running session: the stamp of its delta, as
/// the route sends it.
fn partial(h: &mut Hive, turn: &str) {
    h.out.clear();
    let hop = json!({"turn_id": turn, "path": "talk"});
    let ctx = json!({"session_id": TURN_SESSION, "turn_id": turn, "delta": "1", "live": LIVE,
                     "audience_set": ROUND_E});
    h.lane(
        "in_delta_stamp",
        ctx,
        hop,
        json!({"messages": [], "stamp": {"turn_id": turn, "path": "talk", "p": 0.9,
                                         "intent": "", "needs": [], "families": [],
                                         "duration": 5, "text_norm": "is it going to rain",
                                         "choices": {}, "turn_notes": []}}),
    );
}

/// The ledger's writes on the row the turn `turn` is cut by: how many
/// inserts and how many deletes since the hive was born.
fn cut_row_writes(h: &Hive, turn: &str) -> (usize, usize) {
    let key = format!(
        "pending:turn_cut:{}:{TURN_SESSION}:{turn}",
        round_key(ROUND_E)
    );
    let count = |operation: &str, at: &dyn Fn(&Value) -> Value| {
        h.ledger_ops
            .iter()
            .filter(|(_, op)| op["operation"] == json!(operation) && at(op) == json!(key))
            .count()
    };
    (
        count("insert", &|op| op["row"]["key"].clone()),
        count("delete", &|op| op["where"]["key"].clone()),
    )
}

#[test]
fn gh_d54g_ack_a_heard_turn_leaves_no_turn_state_behind() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    // Review 1, M-5: two rows of a turn fall with the answer that closes
    // it -- the row the turn is cut by (the curator's
    // `pending:turn_cut:<round>:<session>:<turn>`, laid by the turn's first
    // curate with `recall_in_wall` on) and the stamp of the turn's last
    // delta (the lead's `delta:<turn>`, with `delta_speculation` on). A
    // heard turn has no such answer: the cut row stayed until the sweep of
    // the parked rows, which runs only beside a rebuild probe, and the
    // delta row for good (the lead's store sweeps nothing). In a session of
    // heard turns that is two rows per turn. The park of a heard turn takes
    // both down.
    let knobs = [
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
        ("lead", "delta_speculation", json!("1")),
    ];
    let delta_row = |h: &Hive, turn: &str| h.lead_state(&format!("delta:{turn}"));

    // The control, a called turn: both rows stand while the turn runs and
    // fall with the answer that closes it.
    let mut h = live_hive(&knobs);
    partial(&mut h, LABEL);
    assert!(
        !delta_row(&h, LABEL).is_empty(),
        "check of the setup: the partial's stamp is kept: {:?}",
        h.stderr
    );
    turn_path(&mut h, LABEL, "talk", json!({}));
    let brain = h.routed("brain");
    assert_eq!(
        brain.len(),
        1,
        "the control's turn is called: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        turn_cut_row(&h, ROUND_E, TURN_SESSION, LABEL).is_object(),
        "check of the setup: the called turn lays the row it is cut by: {:?}",
        h.stderr
    );
    assert!(
        !delta_row(&h, LABEL).is_empty(),
        "the stamp stands while the turn runs"
    );
    h.out.clear();
    h.tap(&brain[0], "stop", json!({}), json!([said("It stays dry.")]));
    assert_eq!(
        turn_cut_row(&h, ROUND_E, TURN_SESSION, LABEL),
        Value::Null,
        "the control: the closing answer takes the cut row down"
    );
    assert_eq!(
        delta_row(&h, LABEL),
        "",
        "the control: the closing answer takes the delta row down"
    );

    // Heard: the same turn, told to only listen. No call, and nothing of
    // the turn stays -- over a run of turns.
    let mut h = listening(&knobs);
    let pending_before = pending_calls(&h);
    for turn in [LABEL, "voice-turn-2", "voice-turn-3"] {
        partial(&mut h, turn);
        assert!(
            !delta_row(&h, turn).is_empty(),
            "{turn}: check of the setup: the partial's stamp is kept: {:?}",
            h.stderr
        );
        turn_path(&mut h, turn, "talk", json!({}));
        assert!(
            calls(&h).is_empty(),
            "{turn}: {:?} {:?}",
            calls(&h),
            h.stderr
        );
        let (laid, taken) = cut_row_writes(&h, turn);
        assert!(
            laid >= 1 && taken == 1,
            "{turn}: the heard turn's curate lays the row it is cut by ({laid}), and its \
             park takes it down once ({taken}): {:?}",
            h.stderr
        );
        assert_eq!(
            turn_cut_row(&h, ROUND_E, TURN_SESSION, turn),
            Value::Null,
            "{turn}: no cut row stays behind a heard turn"
        );
        assert_eq!(
            delta_row(&h, turn),
            "",
            "{turn}: no delta row stays behind a heard turn: {:?}",
            h.stderr
        );
    }
    assert_eq!(
        marks(&h, "turn_parked").len(),
        3,
        "every turn of the run was heard"
    );
    assert_eq!(
        pending_calls(&h),
        pending_before,
        "no parked row of any heard turn stays: {:?}",
        pending_calls(&h)
    );
    assert!(
        state_keys(&h, "live_turn:").is_empty(),
        "nor a handle row: {:?}",
        state_keys(&h, "live_turn:")
    );
    assert!(
        h.lead_rows("SELECT key FROM state WHERE key LIKE 'delta:%'")
            .is_empty(),
        "nor a delta row in the lead's store"
    );
}

// -- OR-54g-31: the history of a delegation reaches the curator -------------
//
// WHY these pins exist: the voice cell can send the call so far with a
// delegation (`messages[]`: the closed turns in order, the task last). The
// collector read the text of the last message only and dropped the rest, so
// the curator never saw the call -- a cut of content outside the curator.
// Now every entry travels on the seam as `history`, beside the round's own
// `messages`, and the curator files each as a round of its own on the wall:
// under the turn the entry names, else under its place in the session. A
// round of the wall is what the cut of a call works on (whole rounds leave,
// oldest first, each named), so the target's window is cut by its profile
// like any other; a turn the wall already holds -- heard on the turn path
// -- is not filed again, told by its turn id and never by its words.

/// What the caller is in the middle of when the model delegates: the task.
const TASK: &str = "and what about the day after";
/// The turn of the call the hive heard before the delegation.
const HEARD: &str = "voice-turn-0";
/// A turn of the call the hive never heard.
const UNHEARD: &str = "voice-turn-before";
const UNHEARD_ASKED: &str = "do i need a coat for the morning";

/// `pairs` closed turns of a call as the voice cell sends them with a
/// delegation: the caller's part, then the model's, `chars` letters each.
fn call_history(pairs: usize, chars: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for n in 0..pairs {
        out.push(user(&format!("Question {n}: {}", "q".repeat(chars))));
        out.push(said(&format!("Answer {n}: {}", "a".repeat(chars))));
    }
    out
}

/// One entry of a history that names the turn it belongs to.
fn of_turn(turn: &str, mut entry: Value) -> Value {
    entry["turn_id"] = json!(turn);
    entry
}

/// A delegation with `messages` through the shipped collector, hop by hop:
/// the arrival, the turn-open reply, the fan-in. Returns the turn-open
/// bundle of the arrival and the seam (`curate`).
fn collected(messages: Value) -> (Value, Value) {
    use assemble_cell as ac;
    let out = ac::assemble(
        &[],
        ac::lane(
            "in_delegation",
            json!({"turn_id": LABEL}),
            json!({"delegation_id": "item_d1", "engine": "duplex"}),
            messages,
        ),
    );
    let open = ac::in_phase(&out, "turn-open").clone();
    let key = ac::hop_str(&open, "turn_id");
    let own = ac::call(&open, "c-open-turn")["row"].clone();
    let opened = ac::bundle_reply(
        "turn-open",
        &key,
        &[
            ("c-open-round", json!([])),
            ("c-open-win", json!([own])),
            ("c-open-scope", json!([])),
            ("c-open-roster", json!([])),
        ],
    );
    let out = ac::assemble(&[], opened);
    let collect = out
        .iter()
        .find(|m| m["header"]["phase"].as_str() == Some("collect"))
        .unwrap_or_else(|| panic!("the round opens: {out:?}"))
        .clone();
    // The rows of the round as the store hands them back at the fan-in: the
    // window leg of this hop and whatever the arrival parked beside its row.
    let stored = |row: &Value| {
        json!({"turn_id": key, "iter": 0, "role": row["role"], "turn": row["turn"],
               "fired": 0, "recorded_at": row["recorded_at"]})
    };
    let mut rows = vec![stored(&ac::call(&collect, "c-collect-row")["row"])];
    for (id, args) in ac::calls_of(&open) {
        if id == "c-open-history" {
            rows.push(stored(&args["row"]));
        }
    }
    let out = ac::assemble(
        &[],
        ac::bundle_reply("collect", &key, &[("c-collect-read", Value::Array(rows))]),
    );
    (open, ac::on_route(&out, "curate").clone())
}

/// The delegation round of turn [`LABEL`] opens with `messages` and, when
/// given, the `history` the collector's seam carries.
fn delegation_with(h: &mut Hive, messages: Value, history: Option<Value>) {
    h.out.clear();
    let key = event_key(LABEL);
    let mut hop = json!({"session_id": DELEGATION_SESSION, "turn_id": key, "iter": "0",
                         "phase": "", "path": "talk", "needs": "", "families": "",
                         "delegation_id": "item_d1"});
    let mut ctx = json!({"session_id": DELEGATION_SESSION, "turn_id": key, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    for (k, v) in obj(live_keys(LIVE)) {
        hop[k.as_str()] = v.clone();
        ctx[k.as_str()] = v;
    }
    let mut body = json!({"messages": messages});
    if let Some(history) = history {
        body["history"] = history;
    }
    h.lane("in_curate", ctx, hop, body);
}

/// `(turn_id, kind)` of every wall row outside the delegation round and the
/// turn path's session, oldest first: the rounds a history was filed as.
fn filed(h: &Hive) -> Vec<(String, String)> {
    h.rows(&format!(
        "SELECT turn_id, kind FROM wall WHERE session_id = '{DELEGATION_SESSION}' \
         AND turn_id <> '{}' ORDER BY seq",
        event_key(LABEL)
    ))
    .into_iter()
    .map(|r| {
        (
            r[0].as_str().unwrap_or("").to_string(),
            r[1].as_str().unwrap_or("").to_string(),
        )
    })
    .collect()
}

/// How many messages of the calls that left carry `needle`.
fn shown(h: &Hive, needle: &str) -> usize {
    calls(h)
        .iter()
        .filter(|m| m.route() == "model_call")
        .take(1)
        .flat_map(texts)
        .filter(|t| t.contains(needle))
        .count()
}

#[test]
fn gh_d54g_ack_a_delegation_history_over_50_kb_reaches_the_curator_whole() {
    if !shipped() {
        return;
    }
    let history = call_history(40, 900);
    let bytes: usize = history
        .iter()
        .map(|e| e["text"].as_str().unwrap_or("").len())
        .sum();
    assert!(bytes > 50_000, "the call so far is over 50 KB: {bytes}");
    let mut messages = history.clone();
    messages.push(user(TASK));

    // The collector: every entry in front of the task leaves on the seam,
    // whole and in order, beside the round's own message.
    let (_, seam) = collected(Value::Array(messages));
    assert_eq!(
        seam["history"],
        Value::Array(history.clone()),
        "every entry of the call rides the seam as it came"
    );
    let own = seam["messages"].as_array().cloned().unwrap_or_default();
    assert_eq!(own.len(), 1, "the round itself is the delegation: {own:?}");
    assert!(
        own[0]["text"].as_str().unwrap_or("").contains(TASK),
        "the task is what was last heard: {own:?}"
    );

    // The curator: every entry stands on the wall, one round per turn.
    let mut h = live_hive(&[]);
    delegation_with(&mut h, json!(own), Some(seam["history"].clone()));
    let rows = filed(&h);
    assert_eq!(
        rows.len(),
        history.len(),
        "no entry is lost: {}",
        rows.len()
    );
    let kinds: Vec<&str> = rows.iter().map(|(_, k)| k.as_str()).collect();
    let expected: Vec<&str> = history
        .iter()
        .map(|e| e["origin"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        kinds, expected,
        "in the order of the call, each voice its own"
    );
}

#[test]
fn gh_d54g_ack_the_history_goes_through_the_curator_on_its_way_to_the_target() {
    if !shipped() {
        return;
    }
    let history = call_history(3, 40);
    let mut h = live_hive(&[]);
    delegation_with(
        &mut h,
        json!([user(TASK)]),
        Some(Value::Array(history.clone())),
    );
    // What the target is shown is the curator's window: the call so far in
    // its order, then the delegation.
    let call = calls(&h)
        .into_iter()
        .find(|m| m.route() == "model_call")
        .expect("the delegation round calls its target");
    let seen = texts(&call);
    let at = |needle: &str| {
        seen.iter()
            .position(|t| t.contains(needle))
            .unwrap_or_else(|| panic!("the target is shown `{needle}`: {seen:?}"))
    };
    let mut last = 0;
    for n in 0..3 {
        for needle in [format!("Question {n}:"), format!("Answer {n}:")] {
            let i = at(&needle);
            assert!(i >= last, "in the order of the call: {seen:?}");
            last = i;
        }
    }
    assert!(at(TASK) > last, "the task stands behind the call: {seen:?}");
    // Each entry is a round of its own on the wall, never a part of the
    // running round: the cut of a call takes whole rounds out, oldest first,
    // and never the running one -- so the target's profile decides how much
    // of the call it is shown, as for any other round of the wall.
    let rows = filed(&h);
    assert_eq!(rows.len(), history.len(), "{rows:?}");
    let mut turns: Vec<&str> = rows.iter().map(|(t, _)| t.as_str()).collect();
    turns.dedup();
    assert_eq!(turns.len(), history.len(), "one round per entry: {rows:?}");
}

#[test]
fn gh_d54g_ack_a_heard_turn_stands_once_in_the_window_of_a_delegation() {
    if !shipped() {
        return;
    }
    let mut h = listening(&[]);
    // The turn path heard the turn: it stands on the wall with its words.
    turn_path(&mut h, HEARD, "talk", json!({}));
    assert!(calls(&h).is_empty(), "a heard turn calls nobody");
    // The delegation brings the call so far: a turn nobody heard, and the
    // heard one again, each entry naming its turn.
    let history = json!([
        of_turn(UNHEARD, user(UNHEARD_ASKED)),
        of_turn(HEARD, user(ASKED)),
        of_turn(HEARD, said(SPOKEN)),
    ]);
    delegation_with(&mut h, json!([user(TASK)]), Some(history));
    assert_eq!(
        filed(&h),
        vec![(UNHEARD.to_string(), "user".to_string())],
        "the turn nobody heard is filed, the heard one is not filed again"
    );
    assert_eq!(shown(&h, UNHEARD_ASKED), 1, "the unheard turn is shown");
    assert_eq!(shown(&h, ASKED), 1, "the heard turn stands once");
    // The same history with the next delegation of the call files nothing
    // twice: the turns stand already.
    delegation_with(
        &mut h,
        json!([user(TASK)]),
        Some(json!([
            of_turn(UNHEARD, user(UNHEARD_ASKED)),
            of_turn(HEARD, user(ASKED))
        ])),
    );
    assert_eq!(
        filed(&h).len(),
        1,
        "a turn of the wall is never filed again"
    );
}

#[test]
fn gh_d54g_ack_without_history_a_delegation_window_is_unchanged() {
    if !shipped() {
        return;
    }
    // The collector: a delegation that carries the task alone parks nothing
    // beside its row, and its seam has no `history`.
    let (open, seam) = collected(json!([user(TASK)]));
    assert!(
        assemble_cell::calls_of(&open)
            .iter()
            .all(|(id, _)| id != "c-open-history"),
        "nothing is parked for a history that is not there: {open}"
    );
    assert!(seam.get("history").is_none(), "no key on the seam: {seam}");
    // The curator: the round files its own rows and no other, and the
    // target is shown what it was shown before.
    let mut plain = live_hive(&[]);
    delegation_with(&mut plain, json!([user(TASK)]), None);
    assert!(filed(&plain).is_empty(), "{:?}", filed(&plain));
    let mut empty = live_hive(&[]);
    delegation_with(&mut empty, json!([user(TASK)]), Some(json!([])));
    assert!(filed(&empty).is_empty(), "{:?}", filed(&empty));
    let window = |h: &Hive| {
        calls(h)
            .into_iter()
            .find(|m| m.route() == "model_call")
            .map(|m| texts(&m))
            .expect("the delegation round calls its target")
    };
    assert_eq!(
        window(&plain),
        window(&empty),
        "an empty history changes nothing"
    );
}
