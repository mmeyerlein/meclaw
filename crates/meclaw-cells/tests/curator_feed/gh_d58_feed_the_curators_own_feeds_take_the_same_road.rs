//! D58 feed (GH #1481, plan `u-feed.md` contract points 2, 6 and 7, review
//! F-H4, F-W2) -- the curator's own feeds take the same road.
//!
//! WHY this file exists: the fact from a tool (`fact_from_tool`) and the
//! early interrupt each built their own claim (`pending:fact_tool:…`,
//! `pending:interrupt:…`) and the tool fact its own mark (`fact_from_tool
//! {tool}`). They now go through the one function an order from outside goes
//! through: the claim is `pending:feed:<live>#<turn>#<once>`, the tool fact's
//! mark is `feed` (its tool in the mark's `mark` field), and both still leave
//! once per turn. The road does not depend on `feed_in`: the knob opens the
//! lane from outside, the hives here leave it shipped ("0"). For the person
//! and the voice nothing changes: the same section, head and body. Since D58
//! moveout (GH #1476, contract point 8) the interrupt is no own feed of the
//! curator any more: the lead orders it on the same road, the lane
//! `in_feed`, which its composite opens (`feed_in` "1", as talky ships it);
//! how early it went is the lead's row `interrupt:<session>#<turn>` (was the
//! curator's mark `interrupt_early`).

// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_feed::*;
use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

#[test]
fn gh_d58_feed_the_curators_own_feeds_take_the_same_road() {
    if !shipped() {
        return;
    }
    // The fact from a tool, the lane from outside shut.
    let mut h = Hive::with(&[("policy", "fact_from_tool", json!("weather"))]);
    h.check_emits = true;
    to_the_tool_result(&mut h, "t1");
    let facts = feeds_of(&h, "fact");
    assert_eq!(facts.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(facts[0].hop["fact_source"], json!("tool"));
    assert_eq!(facts[0].hop["live"], json!(OWN_LIVE));
    assert_eq!(facts[0].body["payload"], json!(SUMMARY));
    assert!(
        marks_of(&h, "fact_from_tool").is_empty(),
        "the old mark is gone"
    );
    let fm = feed_marks_of(&h, "fact");
    assert_eq!(
        fm.len(),
        1,
        "the tool fact books `feed`: {fm:?} {:?}",
        h.stderr
    );
    assert_eq!(fm[0].value["mark"]["tool"], json!("weather"), "{:?}", fm[0]);
    assert!(
        keys_with(&h, "pending:fact_tool:").is_empty(),
        "the old claim is gone"
    );
    let c = claims(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(
        c[0].starts_with(&format!("{CLAIM}{OWN_LIVE}#t1#")),
        "the claim of the one road: {c:?}"
    );
    // A second iteration of the turn: no second fact, no second mark.
    let mut hop = tool_stamp("weather");
    hop["needs"] = json!("");
    let text = json!({"summary": SUMMARY}).to_string();
    tool_seam(
        &mut h,
        "t1",
        "2",
        hop,
        json!({"messages": [tool_call("c1", "weather"), tool_result("c1", &text)]}),
    );
    assert!(
        feeds_of(&h, "fact").is_empty(),
        "once per turn: {:?}",
        h.out
    );
    assert_eq!(feed_marks_of(&h, "fact").len(), 1);
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // The early interrupt: the lead's order on the same road, the lane open.
    let mut over = crisis_knobs();
    over.push(("policy", KNOB, json!("1")));
    let mut h = Hive::with(&over);
    h.check_emits = true;
    crisis_partial(&mut h, "s1#2");
    assert_eq!(
        feeds_of(&h, "interrupt").len(),
        1,
        "{:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        keys_with(&h, "pending:interrupt:").is_empty(),
        "the old claim is gone"
    );
    let c = claims(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(
        c[0].starts_with(&format!("{CLAIM}{OWN_LIVE}#s1#2#")),
        "the claim of the one road: {c:?}"
    );
    crisis_partial(&mut h, "s1#2");
    assert!(
        feeds_of(&h, "interrupt").is_empty(),
        "once per turn: {:?}",
        h.out
    );
    crisis_turn_end(&mut h, "s1#2");
    assert!(
        feeds_of(&h, "interrupt").is_empty(),
        "the turn's end sends none"
    );
    assert!(
        marks_of(&h, "interrupt_early").is_empty(),
        "the old mark is gone"
    );
    let early: Vec<Value> = h
        .lead_rows("SELECT value FROM state WHERE key = 'interrupt:s1#s1#2'")
        .iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the row is JSON"))
        .collect();
    assert_eq!(
        early.len(),
        1,
        "the turn end is booked by the lead: {:?}",
        h.stderr
    );
    assert_eq!(
        early[0]["delta_n"],
        json!(7),
        "the words of the partial: {early:?}"
    );
    assert!(
        early[0]["ms_before_turn_end"]
            .as_i64()
            .is_some_and(|ms| ms >= 0),
        "{early:?}"
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // The lane shut, the lead's order is parked: no interrupt at all.
    let mut h = Hive::with(&crisis_knobs());
    crisis_partial(&mut h, "s1#2");
    assert!(
        feeds_of(&h, "interrupt").is_empty(),
        "an order of the lead needs the lane open: {:?}",
        h.out
    );
    assert!(claims(&h).is_empty(), "nothing claimed");
}
