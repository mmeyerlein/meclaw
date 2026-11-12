//! GH #1473 (D58 sections) -- the hive that keeps the things offers the
//! section `things` and checks its entries.
//!
//! WHY this file exists: `things` belongs to the objects hive (owner ruling
//! R-58-13: the objects hive is the one responsible for things). Until GH #1473 the
//! curator offered the section (`./schemas`, knob `things_section`) and
//! checked its entries (`./intake`, `thing_of`: `name` a non-empty string,
//! `kind` and `note` strings, an entry out of its form left out whole, a
//! name once). Now the objects hive offers it in its own half of the menu
//! answer (`sidecar[]`) and checks what arrives; the curator only registers
//! the section on `thing_seen` and hands it on. This test drives the real
//! path: the curator hive with `things` registered, its `thing_seen` fed into
//! the objects hive, and the rows the objects hive writes.
//!
//! Both hives run in one process (`support/curator_hive.rs`,
//! `support/objects_hive.rs`). Guarded like every template-reading test
//! (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/objects_hive.rs"]
#[allow(unused_imports)]
use crate::objects_hive; // harness_fold: mod objects_hive;

use curator_hive as cur;
use meclaw_core::serde_json::{Value, json};
use objects_hive as obj;

#[test]
fn the_receiving_hive_offers_and_checks_things() {
    if !cur::shipped() || !obj::shipped() {
        return;
    }
    // The menu: the objects hive's own half carries `things`.
    let mut o = obj::Hive::new();
    o.lane("in_schemas", json!({}), json!({}), json!({"tools": ["*"]}));
    let answers = o.routed("tool_schemas");
    assert_eq!(answers.len(), 1, "one menu answer: {:?}", o.out);
    let offer = answers[0].body["sidecar"]
        .as_array()
        .and_then(|s| s.iter().find(|e| e["section"] == json!("things")))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "the objects hive offers the section `things` in its menu answer (GH #1473): {}",
                answers[0].body["sidecar"]
            )
        });
    assert_eq!(offer["required"], json!(false), "{offer}");
    let fields = &offer["schema"]["properties"]["items"]["items"]["properties"];
    for f in ["name", "kind", "note"] {
        assert!(fields[f].is_object(), "the entry names `{f}`: {offer}");
    }
    assert!(
        offer["instruction"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty()),
        "the offer carries its instruction: {offer}"
    );

    // The check: through the curator, which only hands the section on.
    assert!(
        cur::cell_config("intake")["params"]["sections"].is_object(),
        "curator/intake declares the param `sections` (GH #1473)"
    );
    let mut c = cur::Hive::with(&[(
        "intake",
        "sections",
        json!({"things": {"lane": "thing_seen"}}),
    )]);
    let call = c.curate(
        "s1",
        "t1",
        0,
        json!([cur::user("The garden shed needs a new lock.")]),
        cur::mode("Be brief."),
    );
    let mut ctx = call.context.clone();
    for k in [
        "curator_call",
        "turn_id",
        "session_id",
        "iter",
        "episode_turn_id",
    ] {
        if let Some(v) = call.hop.get(k) {
            ctx.insert(k.into(), v.clone());
        }
    }
    ctx.insert("audience_set".into(), json!(cur::ROUND_E));
    c.out.clear();
    c.lane(
        "in_section",
        Value::Object(ctx),
        json!({"section": "things"}),
        json!({"messages": [], "section": "things", "payload": {"items": [
            {"name": "Garden shed", "kind": "place", "note": "needs a new lock"},
            {"name": "Router", "kind": 7},
            {"kind": "device"},
            {"name": "   "},
            {"name": "garden SHED"},
            "not an entry"
        ]}}),
    );
    let seen = c.routed("thing_seen");
    assert_eq!(
        seen.len(),
        1,
        "things leave once on thing_seen: {:?} {:?}",
        c.out,
        c.stderr
    );

    let mut o = obj::Hive::new();
    let mut hop = seen[0].hop.clone();
    hop.remove("route");
    o.lane(
        "thing_seen",
        Value::Object(seen[0].context.clone()),
        Value::Object(hop),
        Value::Object(seen[0].body.clone()),
    );
    let names: Vec<Value> = o.heads().iter().map(|r| obj::col(r, "aliases")).collect();
    assert_eq!(
        names,
        vec![json!(["Garden shed"])],
        "one row, for the one entry in its form; `Router` with a kind that is no string is \
         left out whole, as is an entry without a name; a name counts once: {:?}",
        o.stderr
    );
}
