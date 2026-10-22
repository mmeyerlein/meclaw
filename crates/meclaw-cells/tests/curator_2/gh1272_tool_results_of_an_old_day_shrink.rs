//! GH #1272 (K2) -- the tool results of an old day leave the raw window.
//!
//! WHY this file exists: a `memory_recall` result stays in every later call
//! of its round, raw, until a rebuild moves it -- and a conversation's
//! window ordered none (the model's soft line). A day of 40 lookups was
//! paid again on every turn of the next days. With the day mark the first
//! turn of the next day rebuilds: the old day's results go with their day
//! into its tier form (talky: the summary, OR-SA.K.2), the next call shows
//! none of them, and the plan names how far its cover reaches.
//!
//! The days are the turns' own time (a replay channel's `happened_at`); the
//! lookups are wall rows of that day, written the way `./intake` writes them.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const LOOKUPS: usize = 40;

/// One round on the replay channel, said at `at`, answered on the tap with
/// luna's package and a window far under the target.
fn round_at(h: &mut Hive, turn: &str, at: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s", "turn_id": turn, "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s", "turn_id": turn, "iter": "0", "channel": "replay-1",
                     "audience_set": ROUND_E, "happened_at": at});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(&format!("tell me about {turn}"))]}),
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
        json!({"tokens_prompt": 2_000, "input_soft": 250_000, "input_hard": 1_000_000}),
        json!([said(&format!("answered {turn}"))]),
    );
    call
}

#[test]
fn tool_results_of_an_old_day_shrink() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("intake", "replay_channel_prefix", json!("replay-")),
    ]);
    round_at(&mut h, "d1-open", "2026-03-01T08:00:00Z");
    // The day's lookups: 40 recalls in one round, each a call and its result.
    let base = chrono::DateTime::parse_from_rfc3339("2026-03-01T09:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut results = Vec::new();
    h.row(
        base,
        "s",
        "d1-look",
        "user",
        &user("what do you remember?"),
        0,
    );
    for i in 0..LOOKUPS {
        let at = base + chrono::Duration::seconds(2 * i as i64 + 1);
        let id = format!("r{i:02}");
        h.row(
            at,
            "s",
            "d1-look",
            "tool_call",
            &tool_call(&id, "memory_recall"),
            0,
        );
        let el = tool_result(&id, &format!("recalled fact {i}: {}", "m".repeat(400)));
        let (seq, _) = h.row(
            at + chrono::Duration::milliseconds(500),
            "s",
            "d1-look",
            "tool_result",
            &el,
            0,
        );
        results.push((seq, short_id(&el)));
    }
    h.row(
        base + chrono::Duration::seconds(200),
        "s",
        "d1-look",
        "assistant",
        &said("that is what I remember"),
        1,
    );

    // The next day: the day mark rebuilds, the summary takes the old day.
    round_at(&mut h, "d2-a", "2026-03-02T08:00:00Z");
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the day orders a rebuild");
    assert_eq!(order.body["emit_body"]["reason"], "day", "{:?}", h.stderr);
    h.fire(&order);
    h.answer("The day before: forty recalled facts.", "stop");
    let plan = h.plan_in(ROUND_E);
    let cover = plan["cover"].as_i64().expect("a cover");
    let under = results.iter().filter(|(s, _)| *s <= cover).count();
    assert_eq!(
        under, LOOKUPS,
        "every lookup of the old day is under the cover: {plan}"
    );
    assert!(
        plan["sum_to"].as_i64().unwrap_or(0) >= results.iter().map(|(s, _)| *s).max().unwrap(),
        "the summary covers them: {plan}"
    );

    // The next call shows none of them.
    let call = h.curate(
        "s",
        "d2-b",
        0,
        json!([user("and today?")]),
        mode("Be brief."),
    );
    let shown = call.messages();
    for (_, id) in &results {
        assert!(
            !shown.iter().any(|m| m.to_string().contains(id.as_str())),
            "the lookup {id} left the window: {shown:?}"
        );
    }
    assert!(
        !shown
            .iter()
            .any(|m| m["text"].as_str().unwrap_or("").contains("recalled fact")),
        "no result text of the old day is raw: {shown:?}"
    );
}
