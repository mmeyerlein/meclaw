//! GH #1448 (D38 C2, OR-38-55 (a)) -- an open line survives in the brief.
//!
//! WHY this file exists: talky ages its window by day (`tiers` raw, summary,
//! none): what fell out of the raw budget takes the tier of its age -- the
//! day before is condensed, the day before that is gone, and older things
//! come back through recall and `history_read` (OR-38-55, on purpose). With
//! the handover brief (`summary_form` "brief") the open kinds of line are
//! different: a promise (`promised`), an open matter (`open`), an intent
//! (`intent`) still carry the exchange on, whatever their age. So they stay
//! in the brief when their day goes to `none`; a statement line of the same
//! day goes with its day.
//!
//! The days are the turns' own time (a replay channel's `happened_at`, as in
//! GH #1272); the summarizer is the test's (`Hive::answer`), no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::json;

const PROMISE: &str = "Call Hannah on Sunday.";
const STATEMENT: &str = "The garden is in bloom.";
const RAIN: &str = "It rained all morning.";

fn ask_of(at: &str) -> String {
    format!("what happened at {at}?")
}

fn round_at(h: &mut Hive, turn: &str, at: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s", "turn_id": turn, "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s", "turn_id": turn, "iter": "0", "channel": "replay-1",
                     "audience_set": ROUND_E, "happened_at": at});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(&ask_of(at))]}),
    );
    let call = h
        .routed("brain")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("one call: {:?} {:?}", h.out, h.stderr));
    h.out.clear();
    h.tap(
        &call,
        "stop",
        json!({"tokens_prompt": 900, "input_soft": 250_000, "input_hard": 1_000_000}),
        json!([said(&format!("answered {turn}"))]),
    );
    call
}

/// The day rebuild the turn ordered, answered with `brief`.
fn day_rebuild(h: &mut Hive, brief: &str) {
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the new day orders a rebuild");
    h.fire(&order);
    assert!(
        !h.summ.is_empty(),
        "the day before is condensed: {:?}",
        h.stderr
    );
    h.answer(brief, "stop");
}

#[test]
fn an_open_line_outlives_its_day_in_the_brief() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "keep_recent_tokens", json!(0)),
        ("policy", "summary_form", json!("brief")),
        ("intake", "replay_channel_prefix", json!("replay-")),
    ])
    .reporting_composite("talky");
    let d1 = "2026-03-01T09:00:00Z";
    round_at(&mut h, "d1-a", d1);
    round_at(&mut h, "d2-a", "2026-03-02T08:00:00Z");
    let src = short_id(&user(&ask_of(d1)));
    day_rebuild(
        &mut h,
        &format!(
            "- promised (2026-03-01) call/hannah: {PROMISE} [#{src}]\n\
             - person (2026-03-01) garden/state: {STATEMENT} [asserted] [#{src}]"
        ),
    );
    round_at(&mut h, "d3-a", "2026-03-03T08:00:00Z");
    let src2 = short_id(&user(&ask_of("2026-03-02T08:00:00Z")));
    day_rebuild(
        &mut h,
        &format!("- person (2026-03-02) weather/today: {RAIN} [asserted] [#{src2}]"),
    );
    let call = round_at(&mut h, "d3-b", "2026-03-03T09:00:00Z");
    let window = canonical(&json!(call.body));
    assert!(
        window.contains(RAIN),
        "the day before stands condensed: {window}"
    );
    assert!(
        window.contains(PROMISE),
        "the promise of two days ago stays in the brief: {window}"
    );
    assert!(
        !window.contains(STATEMENT),
        "a statement of two days ago goes with its day: {window}"
    );
}
