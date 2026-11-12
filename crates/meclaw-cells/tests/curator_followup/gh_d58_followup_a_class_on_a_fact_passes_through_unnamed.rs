//! D58 followup (GH #1479, contract point 2) -- a class the model sets on a
//! fact passes the curator unnamed.
//!
//! WHY this file exists: the class of a fact belongs to the domain that
//! defines it (the memory-hive's schema offers `wellbeing`, GH #1456); the
//! curator only carries it. Until this strand `./intake` copied the class onto
//! the `fact` mark by name (`item.get("wellbeing")`). Now:
//! (1) the forwarded section carries every key of a fact item as the model
//! wrote it -- the class a domain hears there, `wellbeing` or any other --
//! byte for byte (`registered_out`; the check only adds `verified`);
//! (2) the `fact` mark keeps the classes the composition REGISTERS with the
//! section (`sections.<name>.classes`, a list of item keys, OR-d58-2.followup.3),
//! each only as the boolean true and only with `fact_mark_kind` "1"; a key not
//! registered stays off the mark, whatever its name -- `wellbeing` too. The
//! probe uses a class no script knows (`mood`), so a name in code cannot pass.
//!
//! Red until `./intake` reads the registration (strand followup, Bauer 2).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const CLAIM: &str = "Rita has a slight twinge in her back today.";

fn item(extra: Value) -> Value {
    let mut v = json!({"subject": "Rita", "predicate": "has_symptom", "claim": CLAIM,
                       "source": "person", "stance": "asserted",
                       "fact_kind": "experience", "valid_from": DAY,
                       "verified": ["claim"]});
    for (k, x) in obj(extra) {
        v[k] = x;
    }
    v
}

/// The value of the one `fact` mark `./intake` writes for `it`, parsed.
fn mark(knob: &str, classes: Value, it: Value) -> Value {
    let (got, err) = cell_scope(
        "intake",
        json!({"fact_mark_kind": knob,
               "sections": {"memory": {"lane": "sidecar", "mark": "topic",
                                       "classes": classes}}}),
        "fact_marks('memory', {'facts': [ARGS]}, 'facts')",
        it,
    );
    assert!(!err.contains("Traceback"), "{err}");
    sj::from_str(
        got[0][1]
            .as_str()
            .unwrap_or_else(|| panic!("a mark: {got} {err}")),
    )
    .expect("the mark value is JSON")
}

#[test]
fn gh_d58_followup_a_class_on_a_fact_passes_through_unnamed() {
    if !shipped() {
        return;
    }
    // (1) The forwarded section: the item as the model wrote it.
    let mut h = Hive::with(&[(
        "intake",
        "sections",
        json!({"memory": {"lane": "sidecar", "mark": "topic"}}),
    )]);
    let call = h.curate("s1", "t1", 0, json!([user("my back")]), Value::Null);
    h.tap(&call, "stop", json!({}), json!([said("Oh dear.")]));
    h.out.clear();
    let facts = json!([
        {"subject": "Rita", "predicate": "has_symptom", "claim": CLAIM, "quote": CLAIM,
         "source": "person", "stance": "asserted", "mood": true, CLASS: true,
         "confidence": 0.8}
    ]);
    h.section(
        &call,
        "memory",
        json!({"nothing_new": false, "facts": facts}),
    );
    let fwd = h.routed("sidecar");
    assert_eq!(fwd.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        fwd[0].body["payload"]["facts"], facts,
        "the section leaves with every key of the item"
    );

    // (2) The mark: registered classes only, by the registration.
    let both = json!({"mood": true, CLASS: true});
    let m = mark("1", json!(["mood"]), item(both.clone()));
    assert_eq!(m["mood"], json!(true), "a registered class rides: {m}");
    assert!(
        m.get(CLASS).is_none(),
        "an unregistered key stays off the mark, whatever its name: {m}"
    );
    let bare = mark("1", json!(["mood"]), item(json!({})));
    for class in [json!(false), json!("yes"), json!(1)] {
        assert_eq!(
            mark("1", json!(["mood"]), item(json!({"mood": class.clone()}))),
            bare,
            "only the boolean true rides: {class}"
        );
    }
    assert_eq!(
        mark("1", json!([]), item(both.clone())),
        bare,
        "nothing registered, no class on the mark"
    );
    assert_eq!(
        mark("0", json!(["mood"]), item(both)),
        mark("0", json!(["mood"]), item(json!({}))),
        "fact_mark_kind off: the mark of before"
    );
}
