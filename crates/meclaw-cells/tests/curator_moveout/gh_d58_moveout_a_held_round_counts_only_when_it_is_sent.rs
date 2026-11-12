//! D58 moveout (GH #1476, plan `u-moveout.md` contract point 7; plan review
//! W4) -- a held round counts only when it is sent.
//!
//! WHY this file exists: with an awaited speculation the round's sends are
//! held in the lead until the spec answer decides. The advise and the start
//! of the round rode them (`horizon_ops`): a kept spec answer sends none of
//! it and starts no round; a dropped one sends the stored round, and only
//! then the advise goes and the round counts. The bookkeeping now lives in
//! the lead, so the same statement must hold there: nothing while held,
//! nothing on a keep, the round (advise, side call, start) on the release.
//! "Counts" is read where it happens (review A I1, B I-1): the lead's clock
//! of the turn (`horizon_turn:<session>`, started by the note `round` that
//! rides with the held sends) and its booked durations (`horizon_rounds`).
//! A note sent at once instead of held starts the clock while held and on a
//! keep -- the mutant this pin answers red.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_feed::feed_marks_of;
use curator_hive::*;
use curator_moveout::*;
use meclaw_core::serde_json::{Value, json};

const ASK: &str = "will it rain in Hamburg tomorrow?";

/// The default of `weather`: 120 s, far above any round this test runs (a
/// hang mark, never a judgement of speed) -- a next round below it was
/// advised from a booked duration, not from the default.
const DEFAULT_MS: u64 = 120_000;

fn spec_hive() -> Hive {
    let mut over = horizon_knobs(json!({"weather": DEFAULT_MS}));
    over.push(("lead", "quick_speculative", json!("1")));
    over.push(("lead", "spec_tools", json!("weather")));
    moveout_hive(&over)
}

/// The collector's early curate of the turn; returns the spec call.
fn spec_call(h: &mut Hive, turn: &str) -> Msg {
    seam(
        h,
        turn,
        "0",
        json!({"live": LIVE_H, "spec": "1"}),
        json!({"messages": [user(ASK)]}),
    );
    let spec: Vec<Msg> = h
        .out
        .iter()
        .filter(|m| m.route() == "model_call" && m.hop.get("spec") == Some(&json!("1")))
        .cloned()
        .collect();
    assert_eq!(spec.len(), 1, "one spec call: {:?} {:?}", h.out, h.stderr);
    spec[0].clone()
}

/// The spec call's end as talky's `in_spec_llm` edge hands it in.
fn spec_end(h: &mut Hive, call: &Msg, finish: &str, messages: Value) {
    h.out.clear();
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert("spec".into(), json!("1"));
    h.lane(
        "in_spec_llm",
        Value::Object(ctx),
        json!({"finish_reason": finish, "model": "test-model", "tokens_prompt": 10}),
        json!({"messages": messages}),
    );
}

#[test]
fn gh_d58_moveout_a_held_round_counts_only_when_it_is_sent() {
    if !shipped() {
        return;
    }
    // Dropped: the stored round goes, and only now its advise and its start.
    let mut h = spec_hive();
    let spec = spec_call(&mut h, "t1");
    seam(
        &mut h,
        "t1",
        "0",
        tool_stamp("weather"),
        json!({"messages": [user(ASK)]}),
    );
    assert!(
        horizons(&h).is_empty() && side_calls(&h).is_empty() && main_calls(&h).is_empty(),
        "held: nothing of the round leaves while the spec answer is awaited: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        feed_marks_of(&h, "horizon").is_empty(),
        "and nothing is booked"
    );
    assert!(
        h.lead_state("horizon_turn:s1").is_empty(),
        "held: the lead's clock has not started -- the note `round` waits with the sends: {:?}",
        h.lead_rows("SELECT key, value FROM state ORDER BY key")
    );
    spec_end(&mut h, &spec, "stop", json!([said("It will be sunny.")]));
    let feeds = horizons(&h);
    assert_eq!(
        feeds.len(),
        1,
        "released with the round: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(
        feeds[0].body["seconds"],
        json!(DEFAULT_MS / 1000),
        "the default of `weather`"
    );
    assert_eq!(
        side_calls(&h).len(),
        1,
        "the side call is part of the round: {:?}",
        h.out
    );
    assert_eq!(feed_marks_of(&h, "horizon").len(), 1);
    assert_eq!(
        table_turns(&h, "horizon_turn:s1"),
        vec!["t1".to_string()],
        "released: the note went with the sends, the clock of t1 runs: {:?}",
        h.lead_rows("SELECT key, value FROM state ORDER BY key")
    );
    assert!(
        lead_json(&h, "horizon_turn:s1")["t1"]["at_ms"].is_u64(),
        "from the time of the seam"
    );
    finish_round(&mut h, "t1", "weather");
    assert_eq!(
        booked(&h, "weather").len(),
        1,
        "the released round is booked: {:?}",
        h.lead_rows("SELECT key, value FROM state ORDER BY key")
    );
    assert!(
        h.lead_state("horizon_turn:s1").is_empty(),
        "and its clock is taken down"
    );
    // The released round counted: the next round's seconds come from its
    // booked duration, not from the default.
    seam(
        &mut h,
        "t2",
        "0",
        tool_stamp("weather"),
        json!({"messages": [user("and Sunday?")]}),
    );
    let feeds = horizons(&h);
    assert_eq!(feeds.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        feeds[0].body["seconds"]
            .as_u64()
            .is_some_and(|s| s < DEFAULT_MS / 1000),
        "the released round was booked: {:?}",
        feeds[0].body
    );

    // Kept: the stored round never goes -- no advise, no side call, no mark.
    let mut h = spec_hive();
    let spec = spec_call(&mut h, "t1");
    seam(
        &mut h,
        "t1",
        "0",
        tool_stamp("weather"),
        json!({"messages": [user(ASK)]}),
    );
    spec_end(
        &mut h,
        &spec,
        "tool_calls",
        json!([tool_call("c1", "weather")]),
    );
    assert!(
        horizons(&h).is_empty() && side_calls(&h).is_empty(),
        "a kept spec answer sends none of the stored round: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        feed_marks_of(&h, "horizon").is_empty(),
        "and books none of it"
    );
    assert!(
        h.lead_state("horizon_turn:s1").is_empty(),
        "kept: the lead never started the round's clock: {:?}",
        h.lead_rows("SELECT key, value FROM state ORDER BY key")
    );
    assert!(
        h.lead_state("horizon_rounds").is_empty(),
        "kept: and booked no duration"
    );
}
