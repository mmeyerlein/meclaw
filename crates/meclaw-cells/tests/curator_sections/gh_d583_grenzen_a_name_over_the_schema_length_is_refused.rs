//! GH #1526 (d58-3 grenzen, O5) -- the length of a thing's name and kind is
//! a form rule of the offer, in one place.
//!
//! WHY this file exists: a name is no paragraph, so `objects/gate` refused a
//! name over 80 characters and emptied a kind over 32 (`NAME_MAX`,
//! `KIND_MAX`). The numbers stood only in the gate: the model that wrote the
//! section never saw them, and nothing in the contract named them. The rule
//! (contract point 3): both lengths are `maxLength` of the offer's schema
//! (`objects/schemas`, `THINGS_OFFER`), and the gate checks against that one
//! place -- a name over it is refused, one at it passes.

// harness_fold: #[path = "support/objects_hive.rs"]
#[allow(unused_imports)]
use crate::objects_hive; // harness_fold: mod objects_hive;

use meclaw_core::serde_json::json;
use objects_hive as obj;

#[test]
fn a_name_over_the_schema_length_is_refused() {
    if !obj::shipped() {
        return;
    }
    let props = obj::pure(
        "schemas",
        "THINGS_OFFER['schema']['properties']['items']['items']['properties']",
        json!(null),
    );
    let name_max = props["name"]["maxLength"]
        .as_u64()
        .unwrap_or_else(|| panic!("the offer names the length of a name: {props}"));
    let kind_max = props["kind"]["maxLength"]
        .as_u64()
        .unwrap_or_else(|| panic!("the offer names the length of a kind: {props}"));
    assert_eq!((name_max, kind_max), (80, 32), "{props}");
    // One place: the gate carries the offer word for word (a code cell has
    // no library to share) and reads both lengths from it.
    assert_eq!(
        obj::block("gate", "things-offer"),
        obj::block("schemas", "things-offer"),
        "the gate's copy of the offer is the offer"
    );

    let at = "a".repeat(name_max as usize);
    let over = "b".repeat(name_max as usize + 1);
    let checked = obj::pure(
        "gate",
        "clean_items(ARGS)",
        json!([{"name": at, "kind": "k".repeat(kind_max as usize)},
               {"name": over, "kind": "device"}]),
    );
    let names: Vec<&str> = checked
        .as_array()
        .expect("a list")
        .iter()
        .map(|c| c["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        names,
        vec![at.as_str()],
        "a name at the length passes, one over it is refused"
    );
    assert_eq!(
        checked[0]["kind"].as_str().map(str::len),
        Some(kind_max as usize),
        "a kind at its length is kept"
    );
}
