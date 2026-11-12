//! D58 moveout (GH #1476, plan `u-moveout.md` contract point 2) -- the
//! answer of a side call never ends the turn.
//!
//! WHY this file exists: what a side call gets is fixed in the curator and
//! the same for every side call, and its answer never ends the turn: it
//! frees no turn system, books no round, and an answer without words (an
//! error, a content filter) does not hand the round to the fallback -- the
//! main call answers the turn. Before the strand the policy knew this by the
//! ending `-bridge` of the call id at eight places; the side call is now
//! known by its kind (`call_kind` "side", lifted into the context by the
//! composite's edge like `curator_call`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_hive::*;
use curator_moveout::*;
use meclaw_core::serde_json::json;

fn escalations(h: &Hive) -> usize {
    h.rows("SELECT value FROM marks WHERE kind = 'escalate'")
        .len()
}

/// The ends of a turn a side answer must not cause (review A, Minor), each
/// counted since `ledger`/`lead` (lengths of `ledger_ops`/`ledger_steps` and
/// `lead_ops` before the answer): the delete of the turn's system row
/// (`t-tsys`), the curator's step that decides the turn's live feed (phase
/// `live`), the lead's read that books the round (`round_end` on `turn_end`:
/// the turn table and the durations in one read) and the lead's drop of a
/// shown spec sentence (`shown_end` on `turn_end`).
struct Ends {
    system_dropped: usize,
    live_decided: usize,
    round_booked: usize,
    shown_dropped: usize,
}

fn ends_since(h: &Hive, ops: usize, steps: usize, lead: usize) -> Ends {
    let lead_ops = &h.lead_ops[lead..];
    Ends {
        system_dropped: h.ledger_ops[ops..]
            .iter()
            .filter(|(_, op)| {
                op["operation"] == json!("delete")
                    && op["where"]["key"]
                        .as_str()
                        .is_some_and(|k| k.starts_with("turn_system:"))
            })
            .count(),
        live_decided: h.ledger_steps[steps..]
            .iter()
            .filter(|(_, phase)| phase == "live")
            .count(),
        round_booked: lead_ops
            .iter()
            .filter(|(_, op)| {
                let keys = op["where"]["key"]["in"].to_string();
                op["operation"] == json!("select")
                    && keys.contains("horizon_turn:")
                    && keys.contains("horizon_rounds")
            })
            .count(),
        shown_dropped: lead_ops
            .iter()
            .filter(|(_, op)| {
                op["operation"] == json!("delete")
                    && op["where"]["key"]
                        .as_str()
                        .is_some_and(|k| k.starts_with("spec_shown:"))
            })
            .count(),
    }
}

fn marks_now(h: &Hive) -> (usize, usize, usize) {
    (h.ledger_ops.len(), h.ledger_steps.len(), h.lead_ops.len())
}

#[test]
fn gh_d58_moveout_a_side_call_never_ends_the_turn() {
    if !shipped() {
        return;
    }
    // The live feed and the waiting-time bookkeeping on, so their turn-end
    // steps can be seen (or not).
    let mut over = horizon_knobs(json!({"weather": 2000}));
    over.push(("policy", "live_feed", json!("1")));
    let mut h = moveout_hive(&over);

    // A side call beside an errand: its answer, whatever it is, starts nothing.
    for (turn, finish) in [("t1", "stop"), ("t2", "error")] {
        seam(
            &mut h,
            turn,
            "0",
            errand_stamp("complex"),
            json!({"messages": [user("plan my week around the move")]}),
        );
        let side = side_calls(&h);
        assert_eq!(side.len(), 1, "{turn}: {:?} {:?}", h.out, h.stderr);
        let side = side[0].clone();
        let extra = if finish == "error" {
            json!({"error_code": "timeout", "error_source": "wire"})
        } else {
            json!({})
        };
        let said_it = if finish == "stop" {
            json!([said("I am on it.")])
        } else {
            json!([])
        };
        let (ops, steps, lead) = marks_now(&h);
        answer_call(&mut h, &side, finish, extra, said_it);
        assert!(
            h.routed("brain").is_empty() && main_calls(&h).is_empty(),
            "{turn}: a side answer ({finish}) starts no call: {:?} {:?}",
            h.out,
            h.stderr
        );
        let ends = ends_since(&h, ops, steps, lead);
        assert_eq!(
            (ends.system_dropped, ends.live_decided, ends.round_booked),
            (0, 0, 0),
            "{turn}: a side answer ({finish}) ends nothing of the turn -- system row, live \
             feed, the round's booking: {:?}",
            h.stderr
        );
        assert_eq!(
            escalations(&h),
            0,
            "{turn}: and hands nothing to the fallback"
        );
    }

    // A side call of a tool round: its answer leaves the round open -- the
    // main call goes on to the tool result and answers the turn.
    seam(
        &mut h,
        "t3",
        "0",
        tool_stamp("weather"),
        json!({"messages": [user("will it rain in Hamburg tomorrow?")]}),
    );
    let side = side_calls(&h);
    assert_eq!(side.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let side = side[0].clone();
    let main: Vec<Msg> = main_calls(&h);
    assert_eq!(main.len(), 1, "{:?}", h.out);
    let main_call = main[0].clone();
    let (ops, steps, lead) = marks_now(&h);
    answer_call(
        &mut h,
        &side,
        "stop",
        json!({}),
        json!([said("One moment.")]),
    );
    let ends = ends_since(&h, ops, steps, lead);
    assert_eq!(
        (ends.system_dropped, ends.live_decided, ends.round_booked),
        (0, 0, 0),
        "the side answer of the tool round ends nothing of the turn: {:?}",
        h.stderr
    );
    assert!(
        h.routed("brain").is_empty() && main_calls(&h).is_empty(),
        "the side answer of the tool round starts nothing: {:?}",
        h.out
    );
    assert_eq!(escalations(&h), 0);
    // The main call still owns the round: `finish_round` answers it to the end.
    h.out.push(main_call);
    finish_round(&mut h, "t3", "weather");
    assert_eq!(
        escalations(&h),
        0,
        "the round ended on its main call: {:?}",
        h.stderr
    );
    // The control: the round's closing answer does what the side answer did
    // not -- so the counts above see it.
    let ends = ends_since(&h, ops, steps, lead);
    assert!(
        ends.live_decided >= 1 && ends.round_booked >= 1,
        "the closing answer decides the live feed and books the round: {:?}",
        h.stderr
    );

    // `shown_end` (the lead drops the turn's shown spec sentence): with the
    // speculation on, a side answer never says it; the closing answer does.
    let mut h = moveout_hive(&[("lead", "quick_speculative", json!("1"))]);
    seam(
        &mut h,
        "t1",
        "0",
        tool_stamp("weather"),
        json!({"messages": [user("will it rain in Hamburg tomorrow?")]}),
    );
    let side = side_calls(&h);
    assert_eq!(side.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let side = side[0].clone();
    let main = main_calls(&h);
    assert_eq!(main.len(), 1, "{:?}", h.out);
    let main_call = main[0].clone();
    let (ops, steps, lead) = marks_now(&h);
    answer_call(
        &mut h,
        &side,
        "stop",
        json!({}),
        json!([said("One moment.")]),
    );
    assert_eq!(
        ends_since(&h, ops, steps, lead).shown_dropped,
        0,
        "a side answer says no `shown_end`: {:?}",
        h.lead_ops
    );
    h.out.push(main_call);
    finish_round(&mut h, "t1", "weather");
    assert!(
        ends_since(&h, ops, steps, lead).shown_dropped >= 1,
        "the closing answer does: {:?}",
        h.lead_ops
    );
}
