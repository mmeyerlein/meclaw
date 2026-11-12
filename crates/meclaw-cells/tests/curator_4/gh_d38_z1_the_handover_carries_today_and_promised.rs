//! GH #1439 (D38 Z1) -- with the handover in the window the block says less.
//!
//! WHY this file exists: the first call of a new session carries the leaf
//! `history.handover` (GH #896), and the handover already names the open
//! topic and the open promises (`./handover` `handover_text`: topic, pins,
//! commitments, summary, turns). The state block of such a call leaves
//! `today` and `promised` out -- two copies of the same promise make the
//! model say it twice -- and keeps `open` and `set today`, which the handover
//! does not carry. `./handover` itself is untouched (A10).
//!
//! Runs the shipped hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::json;

const TOPIC: &str = "the garden";
const PROMISE: &str = "Call Hannah on Sunday.";

#[test]
fn the_handover_carries_today_and_promised() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = Hive::with(&[("policy", "state_block", json!("1"))]).reporting_composite("talky");
    turn(
        &mut h,
        "s1",
        "t1",
        "We planted tomatoes.",
        "Lovely.",
        json!({}),
    );
    let at = backdated(5);
    h.mark(
        at,
        "s1",
        "t1",
        "topic",
        &canonical(&json!({"movement": "start", "name": TOPIC})),
    );
    h.mark(
        at + chrono::Duration::microseconds(1),
        "s1",
        "t1",
        "fact",
        &canonical(&json!({"subject": "Hannah", "predicate": "lives_in",
                           "claim": "Hannah lives in Porto.", "source": "person",
                           "stance": "asserted"})),
    );
    h.lane(
        "in_pin",
        json!({"audience_set": ROUND_E}),
        json!({}),
        json!({"pins": [{"text": PROMISE, "source": "commitment"}]}),
    );
    let c = h.curate(
        "s-next",
        "n1",
        0,
        json!([user("Where were we?")]),
        mode("Be brief."),
    );
    let leaf = c.body["system"]["history"]["handover"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !leaf.is_empty(),
        "the first call of the new session carries the handover: {:?}",
        h.stderr
    );
    let text = texts(&c).last().cloned().unwrap_or_default();
    let line = text
        .lines()
        .find(|l| l.starts_with("- [state -- "))
        .unwrap_or("")
        .to_string();
    assert_eq!(
        line, "- [state -- set today: Hannah lives in Porto.]",
        "no `today`, no `promised` beside the handover: {text}"
    );
}
