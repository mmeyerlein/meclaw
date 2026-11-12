//! D58 note (GH #1477, contract points 5 and 6) -- a shown note with a
//! receipt answers at the rim.
//!
//! WHY this file exists: `in_note` ended with "Nothing answers it" -- no
//! route, no edge, no field for a sender's mark (plan review B1). A note
//! that carries `receipt` and was shown leaves on `note_receipt`, once per
//! turn, with every field in the head (edges read only the head). The road is
//! built in this strand: the curator hive's out-edges from `./policy` and
//! `./history`, its `contract.emits` and the two cells', and every core
//! composition that leads `in_note` to its curator (talky, cogny,
//! simple-agent, figure) hands `note_receipt` out at its own rim. Where it
//! goes from there is wired by whoever sends notes.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_colony::cel_eval::{evaluate_condition, parse_condition};
use meclaw_core::serde_json::{Map, Value, json};
use note_once::*;

const TEXT: &str = "Ask about the garden.";

/// The shipped edges of `template` from `from`, for a hop of `route`.
fn targets(template: &str, from: &str, route: &str) -> Vec<String> {
    let cfg = read_json(&repo(&format!("templates/{template}/config.json")));
    let mut hop = Map::new();
    hop.insert("route".into(), json!(route));
    let ctx = Map::new();
    cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| e["from"] == from)
        .filter(|e| {
            e["condition"].as_str().is_none_or(|c| {
                matches!(
                    evaluate_condition(&parse_condition(c).expect("a condition"), &ctx, &hop),
                    Ok(true)
                )
            })
        })
        .map(|e| e["to"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The routes of `contract.emits` of a template (`params.contract`) or a cell
/// (`contract`): a list of `{route, ...}`, or a cell's schema block whose
/// `hop.route.values` lists them.
fn emits(cfg: &Value) -> Vec<String> {
    let c = if cfg["params"]["contract"].is_object() {
        &cfg["params"]["contract"]
    } else {
        &cfg["contract"]
    };
    if let Some(values) = c["emits"]["hop"]["route"]["values"].as_array() {
        return values
            .iter()
            .map(|x| x.as_str().unwrap_or("").to_string())
            .collect();
    }
    c["emits"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| x["route"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_shown_note_with_a_receipt_answers_at_the_rim() {
    if !shipped() {
        return;
    }
    let mut h = once_hive(&[]);
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    post(
        &mut h,
        json!([
            once("n1", TEXT, Some("r-1")),
            once("n2", "No receipt asked.", None)
        ]),
    );

    // A talk turn at its stamp: the call to the brain shows both notes.
    seam(&mut h, "t2", "talk", "c");
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(shows(&calls[0], TEXT), "{:?}", texts(&calls[0]));
    assert!(shows(&calls[0], "No receipt asked."));

    // One receipt, for the note that asked for one, every field in the head.
    let r = receipts(&h);
    assert_eq!(r.len(), 1, "{:?} {:?}", h.out, h.stderr);
    for k in RECEIPT_KEYS {
        assert!(r[0].hop.contains_key(k), "head field {k}: {:?}", r[0].hop);
    }
    assert_eq!(field(&r[0], "note_source"), "probe");
    assert_eq!(field(&r[0], "note_id"), "n1");
    assert_eq!(field(&r[0], "receipt"), "r-1");
    assert_eq!(field(&r[0], "session_id"), "s1");
    assert_eq!(field(&r[0], "turn_id"), "t2");
    assert_eq!(field(&r[0], "target"), field(&calls[0], "target"));
    assert!(!field(&r[0], "target").is_empty(), "{:?}", r[0].hop);
    assert!(!field(&r[0], "at").is_empty(), "{:?}", r[0].hop);
    assert_eq!(h.emit_violations, Vec::<String>::new());

    // A tool round of the same turn answers nothing more.
    h.out.clear();
    h.tap(
        &calls[0],
        "tool_calls",
        json!({}),
        json!([tool_call("x1", "look")]),
    );
    let c1 = h.curate("s1", "t2", 1, json!([]), mode("Be brief."));
    assert!(shows(&c1, TEXT));
    assert!(receipts(&h).is_empty(), "once per turn: {:?}", h.out);

    // The curator hive: out-edges from policy and history, declared emits.
    let curator = read_json(&repo("templates/curator/config.json"));
    for cell in ["./policy", "./history"] {
        assert_eq!(
            targets("curator", cell, RECEIPT_ROUTE),
            ["."],
            "curator {cell} hands note_receipt out"
        );
    }
    assert!(emits(&curator).contains(&RECEIPT_ROUTE.to_string()));
    for cell in ["policy", "history"] {
        assert!(
            emits(&cell_config(cell)).contains(&RECEIPT_ROUTE.to_string()),
            "curator/{cell} declares note_receipt"
        );
    }
    let accepts = curator["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts")
        .iter()
        .find(|a| a["route"] == "in_note")
        .cloned()
        .expect("the curator accepts in_note");
    assert!(
        !accepts.to_string().contains("Nothing answers it"),
        "the contract of in_note says what answers it: {accepts}"
    );

    // The four core compositions hand it out at their rims.
    for (template, inner) in [
        ("talky", "./curator"),
        ("cogny", "./curator"),
        ("simple-agent", "./curator"),
        ("figure", "./simple-agent"),
    ] {
        assert_eq!(
            targets(template, inner, RECEIPT_ROUTE),
            ["."],
            "{template}: {inner} -> . carries note_receipt"
        );
        let cfg = read_json(&repo(&format!("templates/{template}/config.json")));
        assert!(
            emits(&cfg).contains(&RECEIPT_ROUTE.to_string()),
            "{template} declares note_receipt"
        );
    }
}
