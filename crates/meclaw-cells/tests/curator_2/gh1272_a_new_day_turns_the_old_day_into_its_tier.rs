//! GH #1272 (K2) -- a new day turns the old day into its tier.
//!
//! WHY this file exists: talky's day horizon (`tiers` raw, summary, none)
//! was applied only when a rebuild ran, and a conversation's window ordered
//! one practically never (the model's soft line, luna 250 000). Yesterday
//! lay raw beside today, every turn, at full price. Now a role with a day
//! horizon orders a rebuild when its turn is on another UTC day than the
//! newest round of its last rebuild (mark `day`, state `last_rebuild_day`)
//! -- under its window target too. After it the day before stands in its
//! tier form (talky: the summary) and today stays raw; a second turn on the
//! same day orders nothing.
//!
//! The days are the turns' own time: a replay channel's `happened_at`, the
//! clock the policy ages its rows by (GH #1225, K-1) -- no second clock, and
//! no test waits for a midnight.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const DAY_1: &str = "2026-03-01";
const DAY_2: &str = "2026-03-02";

/// talky's policy on an instance whose replay channels carry their time.
fn talky() -> Hive {
    Hive::with(&[
        ("policy", "role", json!("talky")),
        ("intake", "replay_channel_prefix", json!("replay-")),
    ])
}

/// One round on the replay channel, said at `at`, and its answer on the
/// tap with luna's package and a small window (far under the target).
fn round_at(h: &mut Hive, turn: &str, at: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s", "turn_id": turn, "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s", "turn_id": turn, "iter": "0", "channel": "replay-1",
                     "audience_set": ROUND_E, "happened_at": at});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(&format!("what happened at {at}?"))]}),
    );
    let calls = h.routed("brain");
    assert_eq!(
        calls.len(),
        1,
        "one round, one call: {:?} {:?}",
        h.out,
        h.stderr
    );
    let call = calls[0].clone();
    h.out.clear();
    h.tap(
        &call,
        "stop",
        json!({"tokens_prompt": 900, "input_soft": 250_000, "input_hard": 1_000_000}),
        json!([said(&format!("answered {turn}"))]),
    );
    call
}

fn mark(h: &Hive) -> Value {
    let s: Value =
        meclaw_core::serde_json::from_str(&h.state_in("curate", ROUND_E)).unwrap_or(Value::Null);
    s["curate_mark"].clone()
}

/// The wall rows of `turn` as seqs.
fn seqs(h: &Hive, turn: &str) -> Vec<i64> {
    h.rows(&format!(
        "SELECT seq FROM wall WHERE turn_id = '{turn}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| r[0].as_i64().unwrap())
    .collect()
}

#[test]
fn a_new_day_turns_the_old_day_into_its_tier() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    round_at(&mut h, "d1-a", &format!("{DAY_1}T09:00:00Z"));
    assert_eq!(mark(&h), "none", "{:?}", h.stderr);
    round_at(&mut h, "d1-b", &format!("{DAY_1}T18:00:00Z"));
    assert_eq!(mark(&h), "none", "the same day orders nothing");

    // The first turn of the next day: under the target, the day is due.
    round_at(&mut h, "d2-a", &format!("{DAY_2}T08:00:00Z"));
    assert_eq!(mark(&h), "day", "{:?}", h.stderr);
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the day orders a rebuild");
    assert_eq!(order.body["emit_body"]["reason"], "day");
    h.fire(&order);
    h.answer("Yesterday: two questions, two answers.", "stop");

    // Yesterday is the summary's, today is raw.
    let plan = h.plan_in(ROUND_E);
    let day1: Vec<i64> = [seqs(&h, "d1-a"), seqs(&h, "d1-b")].concat();
    let today = seqs(&h, "d2-a");
    assert!(!day1.is_empty() && !today.is_empty());
    let cover = plan["cover"].as_i64().expect("a cover");
    assert!(
        day1.iter().all(|s| *s <= cover),
        "every row of the day before is under the cover: {plan} {day1:?}"
    );
    assert!(
        today.iter().all(|s| *s > cover),
        "today stays raw: {plan} {today:?}"
    );
    assert_eq!(
        plan["sum_from"].as_i64(),
        day1.iter().min().copied(),
        "{plan}"
    );
    assert_eq!(
        plan["sum_to"].as_i64(),
        day1.iter().max().copied(),
        "{plan}"
    );
    assert_ne!(plan["summary"], "", "the day before is condensed: {plan}");

    // A second turn on the same day: no further rebuild.
    round_at(&mut h, "d2-b", &format!("{DAY_2}T09:00:00Z"));
    assert_eq!(mark(&h), "none", "{:?}", h.stderr);
    assert_eq!(day_orders(&h), 1, "one day, one day order");
}

/// The clock orders of a `day` rebuild the hive armed.
fn day_orders(h: &Hive) -> usize {
    h.clock
        .iter()
        .filter(|m| m.body["op"] == "add" && m.body["emit_body"]["reason"] == "day")
        .count()
}

/// A role without a day horizon never orders a `day` rebuild.
#[test]
fn a_task_horizon_has_no_day_mark() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("consult")),
        ("intake", "replay_channel_prefix", json!("replay-")),
    ]);
    round_at(&mut h, "d1-a", &format!("{DAY_1}T09:00:00Z"));
    round_at(&mut h, "d2-a", &format!("{DAY_2}T09:00:00Z"));
    assert_eq!(mark(&h), "none", "{:?}", h.stderr);
}
