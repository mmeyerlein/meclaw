//! GH #1528 (d58-3 ballast) -- the judge's mark moves nothing in the window
//! sent before the next rebuild.
//!
//! WHY this file exists: the judge answers between two calls. A mark that
//! changed the window at once would break the cached prefix of every call
//! after a marked round -- the cost the rebuild exists to bound. The mark
//! waits for the next rebuild; until then the window is byte for byte the
//! window a judge that keeps every round leaves, and the watch of the prefix
//! (`snapshot_window` "1", `prefix_break` in the curate stamp) counts no
//! break.
//!
//! Pinned on the wall `phone_asked`, the judge on, two turns after the wall
//! that order no rebuild (`curator_bank::quiet_turns`): the judge was asked
//! about every marked round and said `asked`; both calls send the same texts
//! as under a judge that says `keep` to everything; no message is in the
//! one-line form of the ballast; the stamp after the second turn names
//! `prefix_break` 0.

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_bank::*;
use curator_hive::{Msg, texts};
use meclaw_core::serde_json::json;

fn keeps_everything(_: &Wall, _: &Msg) -> Judged {
    Judged::Keep
}

#[test]
fn a_mark_moves_nothing_before_the_rebuild() {
    let (wall, _) = phone_asked::sown();
    let wall = wall.knob("policy", "snapshot_window", json!("1"));
    let run = Run::of("quick").with(JUDGE_SWITCH, json!("1"));

    let (marked, [m1, m2]) = quiet_turns(&wall, &run);
    let (_, [k1, k2]) = quiet_turns(&wall.clone().judged_by(keeps_everything), &run);

    for (key, line) in &wall.asked {
        let reqs: Vec<&Msg> = marked
            .judged
            .iter()
            .filter(|r| request_text(r).contains(key.as_str()))
            .collect();
        assert_eq!(
            reqs.len(),
            1,
            "the judge is asked about the round {key} once"
        );
        assert_eq!(
            verdict_of_the_wall(&wall, reqs[0]),
            Judged::Asked(line.clone()),
            "the stub marks {key}"
        );
    }
    assert_eq!(
        texts(&m1),
        texts(&k1),
        "the first call after the marks sends what a keeping judge leaves"
    );
    assert_eq!(
        texts(&m2),
        texts(&k2),
        "the second call after the marks sends what a keeping judge leaves"
    );
    for t in texts(&m1).iter().chain(texts(&m2).iter()) {
        assert!(
            asked_ids(t).is_empty(),
            "no round is shown as asked before the rebuild: {t}"
        );
    }
    let stamp = curate_stamp(&marked);
    assert_eq!(
        stamp["prefix_break"],
        json!(0),
        "the watch of the prefix counts no break: {stamp}"
    );
}
