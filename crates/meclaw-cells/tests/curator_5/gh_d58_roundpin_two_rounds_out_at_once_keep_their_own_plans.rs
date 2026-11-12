//! GH #1471 (D58 roundpin, contract point 1) -- two tool rounds out at once
//! keep their own plans.
//!
//! WHY this file exists: what a tool round keeps is keyed by the round of
//! its turn, not by the audience. One place per audience named one call
//! (GH #1463), and with two calls of the audience out at once the younger
//! one decided for both. Two tool rounds that run at the same time and
//! began on two sides of a rebuild each go on with the plan their own
//! first call was cut by, whichever of them ends first.
//!
//! The same holds inside one turn: two calls that are both the first of
//! their turn at once leave by one cut -- the one that lost reads the row
//! the other laid and is read once more by it: its two reads (the hashes,
//! the window) a second time, and nothing else.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model. The
//! two tool rounds are the turns of two sessions of one audience.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_roundpin_two_rounds_out_at_once_keep_their_own_plans() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut t = Talk::told(6);

    // The older tool round begins before the rebuild.
    let (a1, a2, b1) = (
        "call_tool_rptwo_a001",
        "call_tool_rptwo_a002",
        "call_tool_rptwo_b001",
    );
    let older0 = t.call("t7", 0, json!([user("Read the file, please.")]));
    let began = cut_epoch(&t.h, &older0);
    t.answer(&older0, "tool_calls", json!([tool_call(a1, "read_file")]));
    t.strike("- The person told items 1 to 4.");
    assert!(
        t.cover() > 0,
        "the plan of the rebuild is written while the older tool round runs: {:?}",
        t.h.stderr
    );
    let newer = standing_epoch(&t.h, ROUND_E);
    assert_ne!(newer, began, "check of the setup: a newer plan stands");

    // The younger tool round begins after the rebuild, in another session
    // of the same audience: its first call is cut by the plan that stands.
    let younger0 = t.h.curate(
        "s2",
        "u1",
        0,
        json!([user("Hello from the other session.")]),
        mode("Be brief."),
    );
    assert_eq!(
        cut_epoch(&t.h, &younger0),
        newer,
        "a tool round that begins after the rebuild is cut by its plan: {:?}",
        t.h.stderr
    );
    t.h.tap(
        &younger0,
        "tool_calls",
        warm_usage(),
        json!([tool_call(b1, "read_file")]),
    );

    // Both are out. Each goes on with its own plan.
    let older1 = t.call(
        "t7",
        1,
        json!([tool_call(a1, "read_file"), tool_result(a1, "file 1: abc")]),
    );
    assert_eq!(
        cut_epoch(&t.h, &older1),
        began,
        "the older tool round goes on with the plan it began with: {:?}",
        t.h.stderr
    );
    let younger1 = t.h.curate(
        "s2",
        "u1",
        1,
        json!([tool_call(b1, "read_file"), tool_result(b1, "file 2: def")]),
        mode("Be brief."),
    );
    assert_eq!(
        cut_epoch(&t.h, &younger1),
        newer,
        "the younger tool round goes on with the plan it began with: {:?}",
        t.h.stderr
    );

    // The younger one ends first; the older one keeps its plan all the same.
    t.h.tap(&younger1, "stop", warm_usage(), json!([said("Hello.")]));
    t.answer(&older1, "tool_calls", json!([tool_call(a2, "read_file")]));
    let older2 = t.call(
        "t7",
        2,
        json!([tool_call(a2, "read_file"), tool_result(a2, "file 3: ghi")]),
    );
    assert_eq!(
        cut_epoch(&t.h, &older2),
        began,
        "the end of the younger tool round leaves the older one its plan: {:?}",
        t.h.stderr
    );
    assert!(
        older2.messages().contains(&said("Noted item 1.")),
        "the older tool round keeps the window it began with: {:?}",
        older2.messages()
    );
    assert_eq!(
        t.breaks(),
        Vec::<String>::new(),
        "every call of the older tool round begins with the whole window before it"
    );
}

/// The messages the store answers while the first call of the turn `t7`
/// is built alone, `(sender, step)` each.
fn steps_of_a_lone_first_call() -> Vec<(String, String)> {
    let mut t = Talk::told(6);
    t.strike("- The person told items 1 to 4.");
    let at = t.h.ledger_steps.len();
    t.call("t7", 0, json!([user("Read the file, please.")]));
    t.h.ledger_steps[at..].to_vec()
}

/// Pin: two first calls of one turn at once. The one read the plan of
/// before, the other the plan a rebuild wrote in between; the other lays
/// the row of the turn first. Both leave cut by that row, and the call that
/// lost was read exactly once more than a first call alone: one repeat, the
/// two reads of a call (`win-hashes`, `win`) a second time.
#[test]
fn two_first_calls_of_a_turn_at_once_leave_by_one_cut() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let alone = steps_of_a_lone_first_call();
    let again = ("policy".to_string(), "win-hashes".to_string());
    assert!(
        !alone.contains(&again),
        "check of the setup: a first call alone is read once: {alone:?}"
    );

    let mut t = Talk::told(6);
    t.strike("- The person told items 1 to 4.");
    let before = standing_epoch(&t.h, ROUND_E);
    // A plan of a later state, as a rebuild writes it: it lands after the
    // one call read the plan of before and before the other call reads.
    let mut plan = t.h.plan_in(ROUND_E);
    let newer_ms = plan["as_of_ms"].as_i64().expect("a plan of a rebuild") + 1;
    plan["as_of_ms"] = json!(newer_ms);
    let newer = format!("{newer_ms}:{}", plan["cover"].as_i64().unwrap_or(0));
    t.h.sql_at_phase = Some((
        "win-hashes".to_string(),
        format!(
            "UPDATE state SET value = '{}' WHERE key = 'window_plan:{}'",
            canonical(&plan).replace('\'', "''"),
            round_key(ROUND_E)
        ),
    ));
    // The other call of the turn comes in right before the store takes the
    // row of the one, and runs to rest there: it lays the row first.
    let ctx = json!({"session_id": "s1", "turn_id": "t7", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E});
    let hop = json!({"session_id": "s1", "turn_id": "t7", "iter": "0", "phase": ""});
    let body = json!({"messages": [user("Read the file, please.")],
                      "system": mode("Be brief.")});
    t.h.lane_at_phase("win", "in_curate", ctx.clone(), hop.clone(), body.clone());
    t.h.out.clear();
    let at = t.h.ledger_steps.len();
    t.h.lane("in_curate", ctx, hop, body);
    let both = t.h.ledger_steps[at..].to_vec();
    assert!(
        !t.h.waits_at_phase(),
        "check of the setup: the other call came in while the one was built"
    );
    let calls = t.h.routed("brain");
    assert_eq!(
        calls.len(),
        2,
        "both calls of the turn leave: {:?} {:?}",
        t.h.out,
        t.h.stderr
    );

    // One cut: the row of the turn holds the plan the winner read, and both
    // calls name it -- the one that lost read the plan of before.
    let kept = turn_cut_row(&t.h, ROUND_E, "s1", "t7");
    assert_eq!(
        kept["plan"],
        json!(canonical(&plan)),
        "the row of the turn is the one the first of the two laid"
    );
    assert_ne!(newer, before, "check of the setup: two plans");
    for (i, call) in calls.iter().enumerate() {
        assert_eq!(
            cut_epoch(&t.h, call),
            newer,
            "call {i} of the two is cut by the row of the turn: {:?}",
            t.h.stderr
        );
    }
    assert_eq!(
        calls[0].messages(),
        calls[1].messages(),
        "the two calls leave with one window"
    );

    // The call that lost was read once more, and that is all it cost: the
    // one repeat is the hash read and the window read a second time.
    let mut rest = both.clone();
    for step in [again, ("policy".to_string(), "win".to_string())] {
        let lost = rest.iter().position(|s| *s == step).unwrap_or_else(|| {
            panic!("the call that lost is read once more by the row ({step:?}): {both:?}")
        });
        rest.remove(lost);
    }
    assert_eq!(
        both.len(),
        2 * alone.len() + 2,
        "one repeat, two messages to the store: {both:?}"
    );
    let mut twice = [alone.clone(), alone].concat();
    rest.sort();
    twice.sort();
    assert_eq!(
        rest, twice,
        "beside that one repeat the two calls take the steps of two first calls: {both:?}"
    );
}
