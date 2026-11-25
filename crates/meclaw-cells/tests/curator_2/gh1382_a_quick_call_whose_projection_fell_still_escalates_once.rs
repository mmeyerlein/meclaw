//! GH #1382 -- a quick call whose projection fell still answers once.
//!
//! WHY this file exists: a quick call stores a copy of its turn
//! (`pending:<call>-esc`) so that a call ending without an answer can go to
//! the brain once (GH #1352). Its projection `window_est:<call>` falls when
//! the session's next turn builds its call (`f-est-old`). Before #1382 the
//! policy's `tap` read the way of the call from that projection: a late
//! `error` after the next turn found no way, and the turn ended in silence.
//! Since #1382 `tap` takes the way from the copy. This pin holds the plain
//! quick call to that: a late `error` goes to the brain once (mark
//! `escalate`, reason `no_answer`); a late answer stays quick and the copy
//! goes. GH #1352's own pin covers the call whose projection still stands.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The normal curate of a turn at the route stamp.
fn seam(h: &mut Hive, turn: &str, path: &str, text: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": "", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(text)]}));
}

fn escalations(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'escalate' ORDER BY seq")
        .iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("JSON"))
        .collect()
}

/// Every model call the hive sent after the tap.
fn answers(h: &Hive) -> usize {
    ["quick", "brain"].iter().map(|r| h.routed(r).len()).sum()
}

const TIMEOUT: &str = r#"{"error_code": "timeout", "error_source": "wire"}"#;

#[test]
fn a_quick_call_whose_projection_fell_still_escalates_once() {
    if !shipped() {
        return;
    }
    for (finish, extra, said_text) in [
        ("error", sj::from_str::<Value>(TIMEOUT).unwrap(), None),
        ("stop", json!({}), Some("Who should I call?")),
    ] {
        let mut h = Hive::with(&[]).reporting_composite("talky");
        h.check_emits = true;
        seam(&mut h, "t1", "simple", "please call my daughter");
        let quick = h.routed("quick");
        assert_eq!(quick.len(), 1, "one quick call: {:?} {:?}", h.out, h.stderr);
        let first = quick[0].clone();
        let call = first.hop["curator_call"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        seam(&mut h, "t2", "simple", "what is the weather");
        assert_eq!(
            h.state(&format!("window_est:{call}")),
            "",
            "the next call took the projection: {:?} {:?}",
            h.out,
            h.stderr
        );
        assert!(
            !h.state(&format!("pending:{call}-esc")).is_empty(),
            "the copy outlives the projection"
        );
        h.out.clear();
        let messages = match said_text {
            Some(t) => json!([said(t)]),
            None => json!([]),
        };
        h.tap(&first, finish, extra, messages);
        let brain = h.routed("brain");
        if finish == "error" {
            assert_eq!(
                brain.len(),
                1,
                "the late error goes to the brain: {:?} {:?}",
                h.out,
                h.stderr
            );
            assert_eq!(answers(&h), 1, "exactly one: {:?}", h.out);
            assert_eq!(brain[0].hop["turn_id"], json!("t1"), "its own turn");
            let shown = sj::to_string(&brain[0].body["messages"]).unwrap();
            assert!(shown.contains("call my daughter"), "{shown}");
            let marks = escalations(&h);
            assert_eq!(marks.len(), 1, "{marks:?}");
            assert_eq!(marks[0]["reason"], json!("no_answer"));
        } else {
            assert_eq!(answers(&h), 0, "nothing asks again: {:?}", h.out);
            assert!(escalations(&h).is_empty(), "{:?}", escalations(&h));
        }
        assert_eq!(h.state(&format!("pending:{call}-esc")), "", "the copy goes");
        assert_eq!(h.emit_violations, Vec::<String>::new());
    }
}
