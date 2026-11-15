//! GH #1456 (D58 F1) -- the `fact` mark keeps a registered class only with
//! `fact_mark_kind` "1".
//!
//! WHY this file exists: a reader of the `fact` mark the intake writes from a
//! checked sidecar item (`fact_marks`) may key on a class the model set on
//! the fact. The mark dropped every key but subject, predicate, claim,
//! source and stance (plus `fact_kind`, `valid_from`, `confidence` since D38
//! V1), so a class the model set never reached a reader. Since D58 followup
//! (GH #1479, OR-d58-2.followup.3) the curator knows no class by name: the
//! composition registers the classes of a section (`sections.<name>.classes`,
//! a talky's curator `["wellbeing"]`). With `fact_mark_kind` "1" the mark
//! keeps a registered class as `true` where the item carries it; a false,
//! missing or non-bool class leaves the mark without the key (missing reads
//! as false, one form), and so does a class the section does not register.
//! Off ("0"), the mark of before, byte for byte. The item is fact (b) of
//! `gh_d58_f1_the_wellbeing_line_keys_on_the_class_not_the_predicate.rs`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// Text window S8, "a slight twinge in the back today", as the sidecar
/// delivered it and the check passed it.
fn back_twinge(class: Option<Value>) -> Value {
    let mut item = json!({"subject": "Rita", "predicate": "has_symptom",
                          "claim": "Rita has a slight twinge in her back today.",
                          "source": "person", "stance": "asserted",
                          "fact_kind": "experience", "valid_from": "2026-11-08",
                          "verified": ["claim"]});
    if let Some(c) = class {
        item["wellbeing"] = c;
    }
    item
}

/// The value of the one `fact` mark the intake writes for `item`, raw, with
/// the section registered as a talky's curator registers it.
fn mark_raw(knob: &str, item: Value) -> String {
    mark_raw_with(knob, json!(["wellbeing"]), item)
}

/// The same with the section's `classes` given.
fn mark_raw_with(knob: &str, classes: Value, item: Value) -> String {
    let probe = "fact_marks('memory', {'items': [ARGS]}, 'items')";
    let (got, err) = cell_scope(
        "intake",
        json!({"fact_mark_kind": knob,
               "sections": {"memory": {"lane": "sidecar", "mark": "topic", "classes": classes}}}),
        probe,
        item,
    );
    assert!(!err.contains("Traceback"), "{err}");
    got[0][1]
        .as_str()
        .unwrap_or_else(|| panic!("a mark: {got} {err}"))
        .to_string()
}

fn mark(knob: &str, item: Value) -> Value {
    meclaw_core::serde_json::from_str(&mark_raw(knob, item)).expect("the mark value is JSON")
}

/// Pin: with "1" a classed item's mark carries the registered `wellbeing: true`.
#[test]
fn on_the_mark_keeps_a_true_class() {
    if !shipped() {
        return;
    }
    let got = mark("1", back_twinge(Some(json!(true))));
    assert_eq!(got["wellbeing"], json!(true), "{got}");
    assert_eq!(got["predicate"], json!("has_symptom"), "{got}");
}

/// Pin: false, missing or a non-bool class -- no key; the mark equals the
/// mark of an item without the field, byte for byte.
#[test]
fn on_anything_but_true_leaves_the_mark_of_an_unclassed_item() {
    if !shipped() {
        return;
    }
    let bare = mark_raw("1", back_twinge(None));
    assert!(
        mark("1", back_twinge(None)).get("wellbeing").is_none(),
        "{bare}"
    );
    for class in [json!(false), json!("yes"), json!(1)] {
        assert_eq!(
            mark_raw("1", back_twinge(Some(class.clone()))),
            bare,
            "class {class}"
        );
    }
}

/// Pin: off, the mark of before -- the class changes nothing, byte for byte.
#[test]
fn off_the_mark_is_byte_identical() {
    if !shipped() {
        return;
    }
    let before = mark_raw("0", back_twinge(None));
    assert_eq!(mark_raw("0", back_twinge(Some(json!(true)))), before);
    assert!(
        mark("0", back_twinge(Some(json!(true))))
            .get("wellbeing")
            .is_none()
    );
}

/// Pin: a class the section does not register stays off the mark -- the
/// mark equals the mark of an unclassed item, byte for byte.
#[test]
fn on_an_unregistered_class_leaves_the_mark_of_an_unclassed_item() {
    if !shipped() {
        return;
    }
    let bare = mark_raw_with("1", json!([]), back_twinge(None));
    assert_eq!(
        mark_raw_with("1", json!([]), back_twinge(Some(json!(true)))),
        bare
    );
    assert_eq!(
        mark_raw_with("1", json!(["mood"]), back_twinge(Some(json!(true)))),
        bare
    );
}
