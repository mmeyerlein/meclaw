//! GH #1451 (D38 V5, review d38_r1 condition 3) -- the hard guard cuts the
//! quoted turn, the recall block stays.
//!
//! WHY this file exists: `recall_in_wall` "1" leaves out a recall block of an
//! earlier round whose every quote stands raw in the wall. It reads the wall
//! BEFORE the hard guard at `input_hard` (`cut_items`, `overflow_cut`) cuts
//! the oldest raw rounds of the same call. A window at the line whose oldest
//! raw round is the quoted one would lose the turn AND its recall: the quote
//! reaches the model not twice, not once, but never. This pins that the quote
//! survives the guard; the knob "0" case shows the setup carries (the guard
//! cuts the source, the recall alone holds the quote).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const RECALL_ID: &str = "call_recall_00000000000pins";
const SAID: &str = "My sister Anna moved to Hamburg.";
const REPLY: &str = "Hamburg is lovely.";

/// A package with a hard line of 4 000 tokens (the guard's margin 3 600).
fn package() -> Value {
    json!({"input_soft": 2000, "input_hard": 4000, "cost_in": 10.0, "cost_cached_in": 0.0,
           "cache_expires_at": "2099-01-01T00:00:00Z", "tokens_prompt": 100})
}

/// The window of `t3` with `recall_in_wall` at `knob`: the quoted round R0
/// is the oldest raw round, then about 16 000 tokens of filler, then the
/// round that set the model's package, then `t2`, an earlier round whose
/// `memory_recall` quotes R0 word for word. Returns the request as text and
/// the `overflow_cut` and `recall_in_wall` entries of the hop.
fn sent(knob: &str) -> (String, Vec<Value>) {
    clear_of_midnight();
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(2)),
        ("policy", "recall_in_wall", json!(knob)),
    ])
    .reporting_composite("talky");
    let base = backdated(50);
    h.row(base, "s", "r_said", "user", &user(SAID), 0);
    h.row(
        base + chrono::Duration::milliseconds(10),
        "s",
        "r_said",
        "assistant",
        &said(REPLY),
        1,
    );
    for i in 0..20i64 {
        let at = base + chrono::Duration::milliseconds(100 * (i + 1));
        let text = format!("{i} {}", "x".repeat(1200));
        h.row(at, "s", &format!("r{i}"), "user", &user(&text), 0);
        h.row(
            at + chrono::Duration::milliseconds(10),
            "s",
            &format!("r{i}"),
            "assistant",
            &said(&text),
            1,
        );
    }
    turn(&mut h, "s", "t1", "hello", "hi", package());
    let pair = json!([
        user("Tell me about Anna."),
        tool_call(RECALL_ID, "memory_recall"),
        tool_result(RECALL_ID, &format!("WHAT WAS SAID\n  user: \"{SAID}\""))
    ]);
    let c2 = h.curate("s", "t2", 0, pair, mode("Be brief."));
    h.tap(&c2, "stop", package(), json!([said("She likes it there.")]));
    let c = h.curate("s", "t3", 0, json!([user("And?")]), mode("Be brief."));
    let cuts = c.hop["cuts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|x| x["what"] == "overflow_cut" || x["what"] == "recall_in_wall")
        .collect();
    (sj::to_string(&c.messages()).expect("messages"), cuts)
}

/// Pin: the guard cuts the quoted round in the very call `recall_in_wall`
/// checks -- the quote still reaches the model, as the recall or the turn.
#[test]
fn the_guard_cuts_the_quoted_turn_and_the_recall_stays() {
    if !shipped() {
        return;
    }
    let (text, cuts) = sent("1");
    // Review d38_o28-R1 K4: one guard cut and no `recall_in_wall`, the
    // quote exactly once.
    assert_eq!(
        cuts.len(),
        1,
        "the guard cut in this call, no recall_in_wall: {cuts:?} {text}"
    );
    assert_eq!(cuts[0]["what"], "overflow_cut", "{cuts:?}");
    assert!(
        !text.contains(REPLY),
        "the guard took the quoted round: {text}"
    );
    assert_eq!(
        text.matches(SAID).count(),
        1,
        "the quote once -- lost when turn and recall both go, doubled when \
         both stay: {text}"
    );
}

/// Check of the setup: knob "0", the same window -- the guard cuts the
/// quoted round, the recall alone carries the quote.
#[test]
fn off_the_guard_cuts_the_quoted_turn_and_the_recall_carries_it() {
    if !shipped() {
        return;
    }
    let (text, cuts) = sent("0");
    assert_eq!(cuts.len(), 1, "{cuts:?} {text}");
    assert!(!text.contains(REPLY), "{text}");
    assert_eq!(text.matches(SAID).count(), 1, "{text}");
    assert!(text.contains(RECALL_ID), "{text}");
}
