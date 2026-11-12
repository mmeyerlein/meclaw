//! D58 judge (GH #1480, contract point 7; plan review H4) -- off sends no
//! ask.
//!
//! WHY this file exists: shipped, the question stays out (`judge_context`
//! "0", `context_turns` 0, `screen_judge.enabled` "0"): the turn goes to
//! `./decide` at once, and the standard way does not grow by the six messages
//! of a view. Switched on, each judge orders ONE view for its target -- the
//! name the param `view_target` carries, which talky sets to `decide` for
//! the route and the screen judge alike (one family for the judgement of the
//! composite, OR-d58-2.judge.13) -- and nothing on the old lane `turns_ask`. The on half is what makes the off half mean something:
//! an ask that is never sent at all would pass the off half too.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::{lane, run_cell};
use serde_json::{Value, json};

const ROUTE: &str = "templates/talky/route/config.json";
const JUDGE: &str = "templates/talky/screen_judge/config.json";

fn turn() -> Value {
    lane(
        "in_turn",
        json!({}),
        json!({"turn_id": "c-1", "channel": "tg:1", "audience_set": "[\"m-1\"]"}),
        json!([{"origin": "user", "type": "text", "text": "How is the weather tomorrow?"}]),
    )
}

fn routes(out: &[Value]) -> Vec<String> {
    out.iter()
        .map(|m| m["header"]["route"].as_str().unwrap_or("").to_string())
        .collect()
}

fn asks(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "view_ask" || m["header"]["route"] == "turns_ask")
        .collect()
}

#[test]
fn gh_d58_judge_off_sends_no_ask() {
    // Shipped: no ask, the decide call at once; the screen judge silent.
    let off = run_cell(ROUTE, &[], turn()).0;
    assert!(asks(&off).is_empty(), "{:?}", routes(&off));
    assert_eq!(routes(&off), vec!["decide".to_string()]);
    assert!(run_cell(JUDGE, &[], turn()).0.is_empty());

    // On: one view for the judge's own target, never the old lane.
    let on = run_cell(ROUTE, &[("judge_context", json!("1"))], turn()).0;
    assert_eq!(routes(&on), vec!["view_ask".to_string()], "{on:?}");
    assert_eq!(on[0]["view_ask"]["target"], json!("decide"), "{on:?}");
    let screen = run_cell(JUDGE, &[("enabled", json!("1"))], turn()).0;
    assert_eq!(routes(&screen), vec!["view_ask".to_string()], "{screen:?}");
    assert_eq!(
        screen[0]["view_ask"]["target"],
        json!("decide"),
        "{screen:?}"
    );

    // The person's words alone (`context_turns` above 0, the judge off): a
    // view WITHOUT a target -- the plain read of before.
    let plain = run_cell(ROUTE, &[("context_turns", json!(4))], turn()).0;
    assert_eq!(routes(&plain), vec!["view_ask".to_string()], "{plain:?}");
    assert!(plain[0]["view_ask"].get("target").is_none(), "{plain:?}");
}
