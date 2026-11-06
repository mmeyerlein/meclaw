//! GH #1434 (D38 K6b, review m-2 of the second fix round) -- the rebuild
//! measures a final answer cut at its sidecar block at the text the window
//! sends.
//!
//! WHY this file exists: `render` sends an earlier answer whose sidecar block
//! is over the window's share of it (`SIDECAR_SHARE` of `input_soft`) cut to
//! the answer and the mark where the block stood. The `curate` stamp counts
//! it so (pinned in `gh_d38_k6b_the_curate_stamp_measures_like_the_budget`);
//! the rebuild reads the same answer through `enrich` -> `with_cut`, and that
//! side had no pin: an `enrich` that dropped the cut counted the whole block
//! in the budget, the cap at the aim, the plan's `estimate` and the `gc`
//! mark, and every test stayed green. The `gc` mark's `tool_share` is the
//! tool results' characters over all the estimate counts -- with the answer
//! counted whole (60 000 characters of sidecar) the share of a 6 000-character
//! result falls under a tenth; counted as sent it is most of the window.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); deterministic,
//! no model anywhere.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::{Hive, last_add, mode, said, shipped, texts, tool_call, tool_result, user};
use meclaw_core::serde_json;
use meclaw_core::serde_json::{Value, json};

/// The window the taps report, in tokens: the sidecar share of it is 10 000
/// tokens (30 000 characters), so a block of 60 000 is cut.
const INPUT_SOFT: i64 = 100_000;

#[test]
fn the_rebuild_counts_a_cut_sidecar_answer_as_sent() {
    if !shipped() {
        return;
    }
    let big = format!(
        "long.\n\n```sidecar\n{{\"a\": \"{}\"}}\n```",
        "x".repeat(60_000)
    );
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "gc", json!("0")),
    ]);
    let call = h.curate(
        "s",
        "t1",
        1,
        json!([
            user("look it up"),
            tool_call("c1", "lookup"),
            tool_result("c1", &format!("found: {}", "r".repeat(6_000)))
        ]),
        mode("x"),
    );
    h.tap(
        &call,
        "stop",
        json!({"input_soft": INPUT_SOFT}),
        json!([said(&big)]),
    );
    let call = h.curate("s", "t2", 0, json!([user("and now")]), mode("x"));
    assert!(
        texts(&call).iter().any(|t| t.contains("[dropped: sidecar")),
        "the window sends the answer cut: {:?}",
        texts(&call)
    );
    h.tap(
        &call,
        "stop",
        json!({"input_soft": INPUT_SOFT, "cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("?")]),
    );
    h.fire(&last_add(&h));
    let marks: Vec<Value> = h
        .rows("SELECT value FROM marks WHERE kind = 'gc' ORDER BY seq")
        .into_iter()
        .map(|r| serde_json::from_str(r[0].as_str().unwrap()).unwrap())
        .collect();
    let mark = marks
        .last()
        .unwrap_or_else(|| panic!("the rebuild leaves a gc mark: {:?}", h.stderr));
    let share = mark["tool_share"].as_f64().expect("tool_share");
    assert!(
        share > 0.5,
        "the result is most of the window as sent -- the cut answer counts at the \
         characters sent, not its 60 000 of sidecar: {mark}"
    );
}
