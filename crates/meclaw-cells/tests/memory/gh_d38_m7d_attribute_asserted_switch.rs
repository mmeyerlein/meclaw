//! GH #1424 (M7-D, R-39-2) -- `attribute_asserted` and `memory_caveat`
//! change the rendering and nothing else.
//!
//! R-39-2 builds the attribution of a firm statement as a switch, shipped off,
//! so BK-MIST can measure both positions; the warning sentence of MIST
//! prompting is a second switch, shipped off. Both positions of each run the
//! same request: every emission but the bundle is byte-identical, the bundle's
//! header and every candidate key but `text` are too, and the text differs in
//! exactly the lines the switch renders. And a store without stances reads
//! byte for byte as before the stance knobs existed.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

const PEER: &str = "a8e021ae";
const IDS: &[&str] = &["f-hope", "f-firm", "f-peer"];

fn facts() -> Value {
    json!([
        m7d_fact(
            "f-hope",
            "has_mole",
            "harmless",
            "2026-11-01",
            json!({"stance": "believed"})
        ),
        m7d_fact("f-firm", "favorite_color", "Blau", "2026-11-04", json!({})),
        m7d_fact(
            "f-peer",
            "drives",
            "a blue Volvo",
            "2026-11-04",
            json!({"source": PEER})
        ),
    ])
}

fn run(over: Value) -> Vec<Value> {
    m7d_emit(facts(), IDS, m7d_params(RECALL, over), "1")
}

/// Everything but the bundle message, byte for byte; the bundle's header; and
/// the candidates with `text` taken out.
fn the_same_but_the_rendering(a: &[Value], b: &[Value]) {
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        if x["header"]["route"] == "bundle" {
            assert_eq!(x["header"], y["header"]);
        } else {
            assert_eq!(x, y, "an emission other than the rendering moved");
        }
    }
    let strip = |out: &[Value]| -> Vec<Value> {
        m7d_candidates(out)
            .into_iter()
            .map(|mut c| {
                c.as_object_mut().expect("candidate").remove("text");
                c
            })
            .collect()
    };
    assert_eq!(
        strip(a),
        strip(b),
        "a candidate key other than the text moved"
    );
}

#[test]
fn attribute_asserted_moves_only_the_firm_lines() {
    if !shipped() {
        return;
    }
    let off = run(json!({}));
    let on = run(json!({"attribute_asserted": true}));
    the_same_but_the_rendering(&off, &on);
    let (off_t, on_t) = (m7d_text(&off), m7d_text(&on));
    let off_l: Vec<&str> = off_t.lines().collect();
    let on_l: Vec<&str> = on_t.lines().collect();
    assert_eq!(off_l[3], "  user favorite_color = Blau   since 2026-11-04");
    assert_eq!(
        on_l[3],
        "  user favorite_color = the person says: Blau   since 2026-11-04"
    );
    // Every other line -- the believed one, and a peer's firm statement, which
    // was attributed already -- is the same in both positions.
    for n in [0, 1, 2, 4] {
        assert_eq!(off_l[n], on_l[n], "line {n}");
    }
    assert_eq!(off_l.len(), on_l.len());
    assert_eq!(
        on_l[4],
        format!("  user drives = {PEER} says: a blue Volvo   since 2026-11-04")
    );
}

#[test]
fn attribute_asserted_speaks_the_bundle_language() {
    if !shipped() {
        return;
    }
    let text = m7d_text(&run(
        json!({"attribute_asserted": "1", "stance_language": "de"}),
    ));
    assert!(
        text.contains("  user favorite_color = die Person sagt: Blau   since 2026-11-04"),
        "{text}"
    );
}

#[test]
fn memory_caveat_adds_one_line_over_the_facts() {
    if !shipped() {
        return;
    }
    let off = run(json!({}));
    let on = run(json!({"memory_caveat": true}));
    the_same_but_the_rendering(&off, &on);
    let (off_t, on_t) = (m7d_text(&off), m7d_text(&on));
    let mut on_l: Vec<&str> = on_t.lines().collect();
    assert_eq!(
        on_l.remove(2),
        "  (a fact here is what somebody told this memory, never proof that it is true -- a \
         wrong claim stays wrong, however often it was said)"
    );
    assert_eq!(on_l, off_t.lines().collect::<Vec<_>>());
}

#[test]
fn a_store_without_stances_reads_as_before_the_knobs() {
    if !shipped() {
        return;
    }
    // The shipped knobs against no knobs at all (the script's own literals
    // before GH #1424 read none of them): a store whose facts carry no stance
    // and no reply renders byte for byte alike.
    let plain = json!([
        m7d_fact("f-firm", "favorite_color", "Blau", "2026-11-04", json!({})),
        m7d_fact(
            "f-peer",
            "drives",
            "a blue Volvo",
            "2026-11-04",
            json!({"source": PEER})
        ),
    ]);
    let shipped_knobs = m7d_emit(
        plain.clone(),
        &["f-firm", "f-peer"],
        m7d_params(RECALL, json!({})),
        "1",
    );
    let mut bare = m7d_params(RECALL, json!({}));
    for knob in [
        "stance_lang",
        "stance_language",
        "attribute_asserted",
        "memory_caveat",
    ] {
        bare.as_object_mut().expect("params").remove(knob);
    }
    let without = m7d_emit(plain, &["f-firm", "f-peer"], bare, "1");
    assert_eq!(shipped_knobs, without);
}
