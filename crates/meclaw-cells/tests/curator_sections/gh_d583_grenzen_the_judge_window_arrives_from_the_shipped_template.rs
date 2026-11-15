//! GH #1526 (d58-3 grenzen, N6, OR-583.grenzen.11) -- the window of the
//! judge arrives from the shipped template, and only as the size of its
//! view.
//!
//! WHY this file exists: `./route` and `./screen_judge` of talky ask the
//! curator for a view of the turn for the target `decide`, sized by that
//! target's window (`view_share` x `input_soft` of its row). The pins of N6
//! put the row on the curator's lane `in_catalog` by hand (`report_row`).
//! In the shipped composite no edge carried a row of `./decide` anywhere:
//! only `./brain`, `./quick`, `./quick_spec` and `./quick_bridge` tell their
//! package. `./history` never learned `limits:decide`, and the view fell
//! back to the window of the wall holder -- 0.1875 x 200 000 = 37 500
//! tokens for a model whose row says 32 000 at most (review of the strand,
//! Important 1). A `catalog:` row is no way out either: `form_view` of
//! `./policy` compares the rows of every target, so a row of `decide` that
//! arrives after the first cut orders a change of plan of the wall holder's
//! window (fix round 1 of the strand). The rule: the model cell reports its
//! package line on a lane of its own; `./policy` keeps no row of it and only
//! tells `./history` the limits. Here the line runs across the shipped edges
//! of talky, and the change of plan a `catalog:` row would order is shown
//! beside it, so its absence is a measured one.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The judge's row: a small window, as a decider has.
const JUDGE_SOFT: u64 = 8000;
const RATE: u64 = 3;
const TURNS: usize = 8;

fn judge_row() -> Value {
    json!({"model": "test/judge", "input_soft": JUDGE_SOFT, "input_hard": 32000,
           "chars_per_token": RATE})
}

/// The view of `decide` asked without a token number.
fn view(h: &mut Hive) -> Msg {
    h.out.clear();
    h.lane(
        "in_view_ask",
        json!({"session_id": "s1", "turn_id": "t99", "audience_set": ROUND_E}),
        json!({}),
        json!({"messages": [], "view_ask": {"target": "decide", "turns": TURNS}}),
    );
    let views = h.routed("view");
    assert_eq!(views.len(), 1, "one view: {:?} {:?}", h.out, h.stderr);
    views[0].clone()
}

#[test]
fn gh_d583_grenzen_the_judge_window_arrives_from_the_shipped_template() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    for i in 1..=TURNS {
        let text = format!("{}{}", i, "j".repeat(3000));
        let call = h.curate(
            "s1",
            &format!("t{i}"),
            0,
            json!([user(&text)]),
            mode("Be brief."),
        );
        h.tap(
            &call,
            "stop",
            json!({"input_soft": 250000}),
            json!([said("ok")]),
        );
    }
    let went = h.package_line_through("talky", "decide", judge_row());
    assert!(
        went.iter().any(|(to, _)| to == "./curator"),
        "the package line of ./decide reaches ./curator over a shipped edge: {went:?}"
    );
    assert!(
        !went.iter().any(|(to, _)| to == "./route"),
        "the package line of ./decide never lands at ./route as a decision: {went:?}"
    );
    let limits: Value = meclaw_core::serde_json::from_str(&h.state("limits:decide"))
        .unwrap_or_else(|_| panic!("./history learned `limits:decide`: {went:?} {:?}", h.stderr));
    assert_eq!(limits["input_soft"], json!(JUDGE_SOFT), "{limits}");
    assert!(
        h.state("catalog:decide").is_empty(),
        "./policy keeps no catalogue row of a cell that is only the target of a view"
    );

    let share = cell_config("history")["params"]["view_share"]
        .as_f64()
        .expect("the shipped view_share");
    let budget = (share * JUDGE_SOFT as f64) as u64;
    let v = view(&mut h);
    assert_eq!(
        v.body.get("budget_tokens"),
        Some(&json!(budget)),
        "the view is view_share x the judge's window: {:?}",
        v.body
    );
    let chars: usize = v
        .messages()
        .iter()
        .map(|m| m["text"].as_str().map(str::len).unwrap_or(0))
        .sum();
    assert!(
        chars > 0 && chars <= (budget * RATE) as usize,
        "the view holds the conversation within {budget} tokens: {chars} chars"
    );
}

/// The reason of the order the clock holds for the round after a second
/// round with a tool, when the row of `decide` came home by `deliver`
/// between the first and the second call.
fn reason_after(deliver: impl Fn(&mut Hive)) -> Option<Value> {
    clear_of_midnight();
    let mut t = Talk::new(&[
        ("policy", "recall_in_wall", json!("1")),
        ("policy", "snapshot_window", json!("1")),
    ])
    .reporting_composite("talky");
    t.turn("t1", "Good day. A quiet day today.", "Received.");
    deliver(&mut t.h);
    let c2 = t.call(
        "t2",
        0,
        json!([user("Read the paper of yesterday, please.")]),
    );
    t.answer(
        &c2,
        "tool_calls",
        json!([tool_call("call_tool_judge_0001", "read_file")]),
    );
    t.h.clock_order_for(ROUND_E)
        .map(|o| o.body["emit_body"]["reason"].clone())
}

#[test]
fn gh_d583_grenzen_the_judge_row_orders_no_change_of_plan() {
    if !shipped() {
        return;
    }
    // Beside it: a `catalog:` row of the judge orders the change of plan --
    // the effect the own lane keeps away, measured in the same setup.
    let by_catalog = reason_after(|h| h.report_row("talky", "decide", judge_row()));
    assert_eq!(
        by_catalog,
        Some(json!("catalog")),
        "check of the setup: a catalogue row of decide moves the plan"
    );
    let by_template = reason_after(|h| {
        h.package_line_through("talky", "decide", judge_row());
        assert!(
            !h.state("limits:decide").is_empty(),
            "check of the setup: the row came home: {:?}",
            h.stderr
        );
    });
    assert_ne!(
        by_template,
        Some(json!("catalog")),
        "the judge's row over the shipped edges orders no change of plan"
    );
}
