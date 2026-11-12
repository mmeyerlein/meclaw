//! GH #1451 (D38 V4, T-13, OR-38-79, soul2me #208) -- a persona set after the
//! birth reaches the window.
//!
//! WHY this file exists: on POC C the weather delegation asked for the city:
//! its window held no `persona.*` leaf. The pack slots were written at the
//! birth (05:05:50Z), the persona (`persona.self`/`persona.member`, the place
//! of the person) came after it (05:07:31Z). This file pins the curator's
//! half: a second pack after the first call carries the new family, and the
//! next call sends it. Whether the second
//! pack is ever pushed is the pusher's half (`affinity/push`, see the
//! strand report, OR-d38-3.v.4).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const PLACE: &str = "Lives in Berlin.";

fn pack(h: &mut Hive, system: Value) {
    h.out.clear();
    h.lane("in_pack", json!({}), json!({}), json!({ "system": system }));
    let ack = h.routed("pack_ack");
    assert_eq!(ack.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(ack[0].hop["error_code"], "", "{:?}", ack[0].hop);
}

fn system_of(c: &Msg) -> String {
    sj::to_string(&c.body.get("system").cloned().unwrap_or(Value::Null)).expect("system")
}

#[test]
fn a_persona_after_the_birth_reaches_the_next_call() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    pack(
        &mut h,
        json!({"identity": {"text": "You have no name yet."}}),
    );
    let first = turn(&mut h, "s1", "t1", "Hello.", "Hi.", json!({}));
    assert!(!system_of(&first).contains(PLACE));
    pack(
        &mut h,
        json!({"identity": {"text": "You have no name yet."},
               "persona": {"self": {"text": "A companion."}, "member": {"text": PLACE}}}),
    );
    let next = h.curate(
        "s1",
        "t2",
        0,
        json!([user("Weather tomorrow?")]),
        mode("Be brief."),
    );
    assert!(system_of(&next).contains(PLACE), "{}", system_of(&next));
}
