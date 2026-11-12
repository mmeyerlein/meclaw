//! D58 moveout (GH #1476, plan `u-moveout.md` contract points 3 and 4) --
//! the kind of a call travels in a head field.
//!
//! WHY this file exists: the curator, its intake, the collector and talky's
//! edges told a bridge by the ending `-bridge` of its call id (and the
//! collector an escalated call by `-esc`), and the head field `bridge` named
//! the kind of a bridge. The id may keep its ending; nobody reads it any
//! more. The kind of a call stands in ONE neutral head field, `call_kind`
//! (OR-d58-2.moveout.1), and a head field travels only where an edge puts it
//! into the context: talky's edges from the curator to its model cells set
//! it from the head, and every `delete_context` list of talky and of the
//! curator hive that drops `curator_call` drops it too, so it never leaks
//! into the next call.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_moveout.rs"]
#[allow(unused_imports)]
use crate::curator_moveout; // harness_fold: mod curator_moveout;

use curator_hive::*;
use curator_moveout::*;
use meclaw_core::serde_json::{Value, json};

fn edges(template: &str) -> Vec<Value> {
    read_json(&repo(&format!("templates/{template}/config.json")))["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn lists(e: &Value, key: &str) -> bool {
    e["modifier"]["delete_context"]
        .as_array()
        .is_some_and(|l| l.iter().any(|k| k == key))
}

#[test]
fn gh_d58_moveout_the_kind_of_a_call_travels_in_a_head_field() {
    if !shipped() {
        return;
    }
    // On the call: the kind in its head field, no `bridge`.
    let mut h = moveout_hive(&[]);
    seam(
        &mut h,
        "t1",
        "0",
        tool_stamp("weather"),
        json!({"messages": [user("will it rain in Hamburg tomorrow?")]}),
    );
    let side = side_calls(&h);
    assert_eq!(
        side.len(),
        1,
        "the side call says its kind in `{KIND}`: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(side[0].hop.get("bridge").is_none(), "{:?}", side[0].hop);
    for m in main_calls(&h) {
        assert!(m.hop.get(KIND) != Some(&json!(SIDE)), "{:?}", m.hop);
    }

    // On the edges: talky lifts it to every model cell and drops it with
    // `curator_call`; it lifts no `bridge`.
    for template in ["talky", "curator"] {
        for (i, e) in edges(template).iter().enumerate() {
            let set = &e["modifier"]["set_context"];
            if template == "talky"
                && e["from"] == json!("./curator")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'model_call'"))
            {
                assert!(
                    set.get(KIND).is_some(),
                    "talky edge {i} ({} -> {}) lifts `{KIND}`",
                    e["from"],
                    e["to"]
                );
            }
            assert!(
                set.get("bridge").is_none() && !lists(e, "bridge"),
                "{template} edge {i}: no `bridge` in the context any more"
            );
            if lists(e, "curator_call") {
                assert!(
                    lists(e, KIND),
                    "{template} edge {i} ({} -> {}) drops `curator_call` and so `{KIND}`",
                    e["from"],
                    e["to"]
                );
            }
            let cond = e["condition"].as_str().unwrap_or_default();
            assert!(
                !cond.contains("context.bridge") && !cond.contains("endsWith('-"),
                "{template} edge {i} reads no kind but `{KIND}`: {cond}"
            );
        }
    }

    // In the scripts: no ending is read, no `bridge` field.
    for (path, needles) in [
        (
            "curator/policy",
            &["endswith(\"-bridge\")", "b_head[\"bridge\"]"][..],
        ),
        ("curator/intake", &["ctx.get(\"bridge\")"][..]),
        (
            "collector/assemble",
            &["\"-esc\"", "ctx.get(\"bridge\""][..],
        ),
    ] {
        let s = script_at(path);
        assert!(!s.is_empty(), "{path} has a program");
        for n in needles {
            assert!(!s.contains(n), "{path} still reads `{n}`");
        }
    }
}
