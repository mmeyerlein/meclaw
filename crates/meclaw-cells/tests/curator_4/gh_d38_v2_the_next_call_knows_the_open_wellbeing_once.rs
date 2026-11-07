//! GH #1451 (D38 V2, T-7) -- the next call knows the open wellbeing, once.
//!
//! WHY this file exists: in the R039 acceptance call 2 did not ask after the
//! person, who had said in call 1 that they felt unwell and slept badly. The
//! topic ages since #1436, rightly; the wellbeing stood only as a fact line
//! of the text window. A call starts from the seed of `./history`
//! (`seed_fact_state`) and goes on with the live feed of `./policy`; both
//! read ONE function, `wellbeing_line(marks, day)`: the day's newest fact
//! of predicate `health` (or `needs` `unfulfilled`) at a confidence of at
//! least `wellbeing_min_confidence` is one data line `{state: "open", ...}`
//! -- no prose, the words belong to the model (R-TR-27). Knob
//! `wellbeing_line` "0" (shipped), both cells the same preset.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

pub const DAY: &str = "2026-11-07";

pub fn fact(seq: i64, at: &str, predicate: &str, claim: &str, conf: f64) -> Value {
    json!({"seq": seq, "kind": "fact", "session_id": "text-1", "at": at,
           "value": canonical(&json!({"subject": "Ada", "predicate": predicate,
                                      "claim": claim, "source": "person",
                                      "stance": "asserted", "confidence": conf}))})
}

pub fn line(cell: &str, marks: &Value) -> Value {
    let (got, err) = cell_scope(
        cell,
        json!({"wellbeing_line": "1"}),
        "wellbeing_line(ARGS['marks'], ARGS['day'])",
        json!({"marks": marks, "day": DAY}),
    );
    assert!(!err.contains("Traceback"), "{cell}: {err}");
    got
}

#[test]
fn both_cells_declare_the_same_knobs_and_presets() {
    let h = cell_config("history");
    let p = cell_config("policy");
    for knob in ["wellbeing_line", "wellbeing_min_confidence"] {
        assert!(!h["params"][knob].is_null(), "history declares {knob}");
        assert_eq!(h["params"][knob], p["params"][knob], "the preset of {knob}");
    }
    assert_eq!(h["params"]["wellbeing_line"], "0", "shipped off");
}

/// Pin: the day's open wellbeing is one line with state `open`, the same
/// in the seed and in the feed.
#[test]
fn the_next_call_knows_the_open_wellbeing_once() {
    if !shipped() {
        return;
    }
    let marks = json!([
        fact(
            3,
            "2026-11-07T08:58:00Z",
            "moves_to",
            "Elias wants to move to Hamburg.",
            0.9
        ),
        fact(
            2,
            "2026-11-07T08:55:00Z",
            "health",
            "Ada feels unwell and slept badly.",
            0.9
        )
    ]);
    let seed = line("history", &marks);
    assert_eq!(seed["state"], json!("open"), "{seed}");
    assert_eq!(seed["predicate"], json!("health"), "{seed}");
    assert_eq!(seed, line("policy", &marks), "one function, both twins");
    // Under the confidence preset it is no line.
    let weak = json!([fact(
        2,
        "2026-11-07T08:55:00Z",
        "health",
        "Maybe tired.",
        0.1
    )]);
    assert_eq!(line("history", &weak), Value::Null);
}
