//! GH #728, lock 7 — the deadline is one knob.
//!
//! Ruling point 2: "the deadline is the one new parameter (per assistant, in
//! `params`)". It is `collector/assemble.late_after_ms` — a param, a
//! `contract.settings` entry and a script literal with one value (the w13 triplet) —
//! and it is set visibly, at its default, in BOTH ref markers of the assistant, so
//! "per assistant" is something a reader finds where the assistant's other
//! surface decisions stand (`memory_tier`, `brain.model`): a hive param reaches no
//! child cell, a ref marker's `override_params` does.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::*;
use serde_json::json;

const DEFAULT: i64 = 30_000;

#[test]
fn param_setting_and_literal_agree() {
    let cfg = config_of(ASSEMBLE);
    assert_eq!(cfg["params"]["late_after_ms"], DEFAULT);
    let s = &cfg["contract"]["settings"]["late_after_ms"];
    assert_eq!(s["default"], DEFAULT);
    assert_eq!(s["type"], "number");
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains(&format!("_int(\"late_after_ms\", {DEFAULT})")),
        "the script's own fallback is the shipped default"
    );
}

#[test]
fn both_surfaces_of_the_assistant_set_it() {
    for rel in [TALKY_REF, TALKY_CHAT_REF] {
        let v = &config_of(rel)["override_params"]["collector/assemble"]["late_after_ms"];
        assert_eq!(
            *v,
            json!(DEFAULT),
            "{rel}: late_after_ms is not set visibly"
        );
    }
}

#[test]
fn the_knob_moves_the_deadline() {
    let doc = || {
        lane(
            "in_delegation",
            json!({"turn_id": "s1#3"}),
            json!({"delegation_id": "dlg-1"}),
            json!([{"origin": "user", "type": "text", "text": "x"}]),
        )
    };
    let deadline = |over: &[(&str, serde_json::Value)]| -> i64 {
        let out = assemble(over, doc());
        let key = hop_str(in_phase(&out, "turn-open"), "turn_id");
        key.rsplit('~')
            .nth(1)
            .expect("deadline field")
            .parse()
            .expect("int")
    };
    let before = now_ms();
    let short = deadline(&[("late_after_ms", json!(5))]);
    let shipped = deadline(&[]);
    assert!(short >= before + 5 && short < before + DEFAULT, "{short}");
    assert!(shipped >= before + DEFAULT, "{shipped}");
}
