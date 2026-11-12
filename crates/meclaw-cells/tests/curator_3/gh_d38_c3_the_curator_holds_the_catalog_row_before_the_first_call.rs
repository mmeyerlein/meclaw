//! GH #1450 (D38 C3b, curator side of GH #1449) -- the catalogue row is
//! here before the first call.
//!
//! WHY this file exists: until GH #1449 the curator learned a model's
//! package only from the header of an answer, so every first call ran on
//! fallback params. Now a model cell with `package_line` puts its row on the
//! curator's `in_catalog` lane (`hop.package_of` = the cell's path). The
//! curator side, claimed here:
//!
//! 1. `./policy` keeps the row per model cell (state `catalog:<cell>`, the
//!    last segment of `package_of`) and cuts the FIRST call by it: a row with
//!    a hard line under the call stands on that call as an `overflow_cut`.
//! 2. No row: with the knob `catalog_ask` on, the curator asks the target's
//!    cell once at a session's start -- one message `catalog_ask` beside the
//!    call (never instead of it), the body the empty params slot
//!    `{"system": {}, "params": {}}` that the cell answers with its row (one
//!    round trip, no polling). A held row asks nothing.
//! 3. Off (the default) and without a row: the call of before, nothing asked
//!    (the c0 corpus holds the bytes).
//! 4. The trees: every composite whose model cell carries `package_line`
//!    leads the ask from `./curator` to that cell, and the curator leads it
//!    from `./policy` out of the hive.
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

fn row(model: &str, soft: i64, hard: i64) -> Value {
    json!({"model": model, "input_soft": soft, "input_hard": hard, "max_output": 4096,
           "price_tiers": null, "cache_mode": "breakpoints", "cache_ttl_s": 300,
           "wire_dialect": "chat_completions", "supported_params": [],
           "chars_per_token": 3.5})
}

/// The cell `cell` of talky reports its package: `row`, and the profile its
/// template writes beside it (D58 target, contract point 13).
fn catalog(h: &mut Hive, cell: &str, row: Value) {
    h.out.clear();
    h.report_row("talky", cell, row);
}

fn has_overflow_cut(call: &Msg) -> bool {
    call.hop
        .get("cuts")
        .and_then(Value::as_array)
        .is_some_and(|c| c.iter().any(|x| x["what"] == "overflow_cut"))
}

#[test]
fn the_policy_keeps_the_row_of_its_model_cell() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    catalog(&mut h, "brain", row("test/kept", 90000, 200000));
    let kept: Value = sj::from_str(&h.state("catalog:brain")).unwrap_or(Value::Null);
    assert_eq!(
        kept["model"],
        json!("test/kept"),
        "the row of /talky/brain is kept under catalog:brain: {kept} {:?}",
        h.stderr
    );
    assert_eq!(kept["input_soft"], json!(90000), "{kept}");
    assert_eq!(kept["input_hard"], json!(200000), "{kept}");
    assert!(
        !h.stderr.iter().any(|l| l.contains("in_catalog")),
        "the lane is no longer dropped: {:?}",
        h.stderr
    );
}

#[test]
fn the_first_call_is_cut_by_the_row() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    catalog(&mut h, "brain", row("test/tiny", 20, 30));
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("hello there")]),
        mode("Be brief."),
    );
    assert!(
        has_overflow_cut(&call),
        "the first call already stands under the row's hard line: {:?}",
        call.hop
    );
}

#[test]
fn without_a_row_and_the_knob_off_the_first_call_is_as_before() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("hello there")]),
        mode("Be brief."),
    );
    assert!(
        !has_overflow_cut(&call),
        "fallback as before: {:?}",
        call.hop
    );
    assert!(
        h.routed("catalog_ask").is_empty(),
        "nothing asked: {:?}",
        h.out
    );
}

#[test]
fn a_missing_row_is_asked_once_at_the_session_start() {
    if !shipped() {
        return;
    }
    // OR-d58-2.target.15: the ask is no knob any more -- it always goes,
    // once per session, to every cell when no cell has reported yet.
    let cfg = cell_config("policy");
    assert!(cfg["params"].get("catalog_ask").is_none(), "no knob");
    assert!(
        cfg["contract"]["settings"].get("catalog_ask").is_none(),
        "no setting"
    );
    // The composite names the target of its curates; its cell has not
    // reported yet.
    let mut h = Hive::new();
    h.target = Some(MAIN_TARGET.to_string());
    let c1 = h.curate("s1", "t1", 0, json!([user("q1")]), mode("Be brief."));
    let asks = h.routed("catalog_ask");
    assert_eq!(
        asks.len(),
        1,
        "one ask at the start: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        asks[0].hop.get("catalog_for").is_none(),
        "the first ask of the session goes to every cell: {:?}",
        asks[0].hop
    );
    assert_eq!(
        Value::Object(asks[0].body.clone()),
        json!({"system": {}, "params": {}}),
        "the ask is the empty params slot"
    );
    h.tap(&c1, "stop", json!({}), json!([said("a1")]));
    let c2 = h.curate("s1", "t2", 0, json!([user("q2")]), mode("Be brief."));
    assert!(
        h.routed("catalog_ask").is_empty(),
        "the session asked once -- no second ask, no polling: {:?}",
        h.out
    );
    h.tap(&c2, "stop", json!({}), json!([said("a2")]));
    h.curate("s2", "t1", 0, json!([user("q3")]), mode("Be brief."));
    assert_eq!(
        h.routed("catalog_ask").len(),
        1,
        "a new session without a row asks once again: {:?}",
        h.out
    );
}

#[test]
fn a_held_row_asks_nothing() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    catalog(&mut h, "brain", row("test/held", 90000, 200000));
    h.curate("s1", "t1", 0, json!([user("q1")]), mode("Be brief."));
    assert!(h.routed("catalog_ask").is_empty(), "{:?}", h.out);
}

fn edges_of(cfg: &Value) -> Vec<Value> {
    cfg["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn asks_on(edges: &[Value], from: &str, to: &str) -> bool {
    edges.iter().any(|e| {
        e["from"] == from
            && e["to"] == to
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("'catalog_ask'"))
    })
}

#[test]
fn every_model_cell_with_a_package_line_is_asked_through_an_edge() {
    if !shipped() {
        return;
    }
    let cur = read_json(&repo("templates/curator/config.json"));
    assert!(
        asks_on(&edges_of(&cur), "./policy", "."),
        "the curator leads the ask out of the hive"
    );
    let mut seen = 0;
    for t in ["talky", "cogny"] {
        let cfg = read_json(&repo(&format!("templates/{t}/config.json")));
        let edges = edges_of(&cfg);
        let dir = repo(&format!("templates/{t}"));
        for e in std::fs::read_dir(&dir).expect("template dir").flatten() {
            let child = e.path().join("config.json");
            if !child.is_file() {
                continue;
            }
            let c = read_json(&child);
            if c["params"]["package_line"] != json!(true) {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            seen += 1;
            assert!(
                asks_on(&edges, "./curator", &format!("./{name}")),
                "{t}: no edge leads the curator's catalog_ask to ./{name}"
            );
        }
    }
    assert!(seen >= 5, "the shipped model cells with the line: {seen}");
}
