//! d58-3 treue (T1, evidence L5b) -- the Jena answer of an earlier session
//! keeps its frame.
//!
//! WHY this file exists: in step 2 of d54g run L5b (shipped composition, a
//! fresh member) the window showed the call's history behind the boundary of
//! an earlier session ("[earlier session", GH #1335), the model's own right
//! answer about Jena bare beside it, and the model took that answer back as
//! unchecked. With the shipped composition's preset the history keeps its
//! boundary and every own answer of it opens with the frame of an own answer
//! said in an earlier session.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const HOME: &str = "I live in Jena.";
const JENA: &str = "Jena lies on the Saale, in Thuringia.";
const FAR: &str = "How far is it from there to Weimar?";
const WEIMAR: &str = "From Jena to Weimar it is about 20 kilometres.";
const NOW: &str = "Where did I say I live?";

fn text(m: &Value) -> &str {
    m["text"].as_str().unwrap_or("")
}

#[test]
fn gh_d583_treue_the_jena_answer_of_an_earlier_session_keeps_its_frame() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    // The role whose preset the shipped composition reports (GH #1303).
    let mut h = Hive::with(&[]).reporting_composite("talky");
    let base = backdated(50);
    let at = |ms: i64| base + chrono::Duration::milliseconds(ms);
    h.row(at(0), "s1", "t1", "user", &user(HOME), 0);
    h.row(at(10), "s1", "t1", "assistant", &said(JENA), 1);
    h.row(at(20), "s1", "t2", "user", &user(FAR), 0);
    h.row(at(30), "s1", "t2", "assistant", &said(WEIMAR), 1);
    let call = h.curate("s2", "t1", 0, json!([user(NOW)]), mode("Be brief."));
    let msgs = call.messages();
    let since = base.format("%Y-%m-%d %H:%M UTC").to_string();
    let at = |line: &str| -> usize {
        msgs.iter()
            .position(|m| text(m).contains(line))
            .unwrap_or_else(|| panic!("{line:?} stays in the window: {msgs:#?}"))
    };
    let find = |line: &str| -> Value { msgs[at(line)].clone() };
    for line in [HOME, FAR] {
        let m = find(line);
        assert!(
            text(&m).starts_with(&format!(
                "[earlier session, since {since} -- from before this session, not said now]\n"
            )),
            "the history of the call stands as an earlier session: {:?}",
            text(&m)
        );
    }
    for line in [JENA, WEIMAR] {
        let m = find(line);
        assert_eq!(
            m["origin"], "assistant",
            "{line:?} stays the model's own: {m:#?}"
        );
        // GH #1566: the frame is the last line of the element before the
        // answer; the answer keeps its own words.
        assert_eq!(text(&m), line, "no frame in the model's own text");
        let before = &msgs[at(line) - 1];
        assert!(
            before["origin"] != "assistant"
                && text(before).ends_with(&format!(
                    "\n[your answer right after this: your own, in an earlier session, since {since} -- said then, not now]"
                )),
            "the own answer of the earlier session keeps its frame: {:?}",
            text(before)
        );
    }
}
