//! GH #1451 (D38 V2, T-7) -- the seed of the next call carries the open
//! wellbeing once, and books it closed.
//!
//! WHY this file exists: the pins beside it hold `wellbeing_line` itself;
//! this one holds the road of `./history` a call starts on. The person said
//! in a text session that they felt unwell (a `fact` mark as the intake
//! writes it, without a confidence, the class `wellbeing` set, GH #1456); the seed of call 2 carries ONE data line
//! `Wellbeing: {...,"state":"open"}` and, in the same bundle, a `wellbeing`
//! mark `{state: closed}` (OR-d38-3.v.2); the seed of call 3 carries none.
//! Off (`wellbeing_line` "0", shipped) no line and no mark.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/history_judge.rs"]
#[allow(unused_imports)]
use crate::history_judge; // harness_fold: mod history_judge;

use curator_hive::*;
use history_judge::{history_params, only, seed_ask};
use meclaw_core::serde_json::{Value, json};

fn wellbeing_lines(seed: &Msg) -> Vec<String> {
    seed.messages()
        .iter()
        .filter_map(|m| m["text"].as_str())
        .filter(|t| t.starts_with("Wellbeing: "))
        .map(str::to_string)
        .collect()
}

fn unwell_today() -> Hive {
    clear_of_midnight();
    let mut h = Hive::new();
    let at = backdated(30);
    h.row(
        at,
        "text-1",
        "t1",
        "user",
        &user("I feel unwell, slept badly"),
        1,
    );
    h.mark(
        at + chrono::Duration::seconds(1),
        "text-1",
        "t1",
        "fact",
        &canonical(&json!({"subject": "Ada", "predicate": "has_symptom",
                           "claim": "Ada feels unwell and slept badly.",
                           "source": "person", "stance": "asserted",
                           "wellbeing": true})),
    );
    h
}

fn closed_marks(h: &Hive) -> Vec<Vec<Value>> {
    h.rows("SELECT kind, value FROM marks WHERE kind = 'wellbeing' ORDER BY seq")
}

/// Pin: call 2 carries the line once and books it closed; call 3 none.
#[test]
fn the_seed_carries_the_open_wellbeing_once() {
    if !shipped() {
        return;
    }
    let h = unwell_today();
    let on = history_params(&[("wellbeing_line", json!("1"))], &["now_utc"]);
    let seed2 = only(&seed_ask(&h, &on, "call-2"), "seed");
    let lines = wellbeing_lines(&seed2);
    assert_eq!(lines.len(), 1, "{:?}", seed2.messages());
    assert!(lines[0].contains("\"state\":\"open\""), "{}", lines[0]);
    assert!(
        lines[0].contains("\"predicate\":\"has_symptom\""),
        "{}",
        lines[0]
    );
    let closed = closed_marks(&h);
    assert_eq!(closed.len(), 1, "one closing mark: {closed:?}");
    assert!(
        closed[0][1]
            .as_str()
            .unwrap_or("")
            .contains("\"state\":\"closed\""),
        "{closed:?}"
    );
    let seed3 = only(&seed_ask(&h, &on, "call-3"), "seed");
    assert!(
        wellbeing_lines(&seed3).is_empty(),
        "asked after once: {:?}",
        seed3.messages()
    );
    assert_eq!(closed_marks(&h).len(), 1, "no second closing mark");
}

/// Pin: off (shipped) the seed carries no line and books no mark.
#[test]
fn off_the_seed_carries_no_wellbeing() {
    if !shipped() {
        return;
    }
    let h = unwell_today();
    let off = history_params(&[], &["now_utc"]);
    let seed = only(&seed_ask(&h, &off, "call-2"), "seed");
    assert!(wellbeing_lines(&seed).is_empty(), "{:?}", seed.messages());
    assert!(closed_marks(&h).is_empty());
}
