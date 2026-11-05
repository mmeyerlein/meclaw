//! GH #1431 (D38 K5, R-38-2 Nr. 2/4) -- a window over the price step rebuilds
//! sooner, because the cost rule prices each side at its own step.
//!
//! WHY this file exists: the llm cell stamps `cost_in` / `cost_cached_in` of
//! the step the answer's prompt falls in (GH #1318). The rule of before read
//! those two for BOTH sides of the bargain: the held window (right, it is
//! read at the step) and the rebuilt one (wrong: it lands below the step and
//! is read and written at the base price). So a rebuild from 120k to 90k was
//! priced at 90 000 x 50 instead of 90 000 x 12.5, and the rule never fired.
//! With the ladder on the tap (`price_tiers`, base step first) one function,
//! `turn_cost(window, cached, limits)`, prices a turn at the step its window
//! falls in; `amortise_after` and `cost_cents` both use it. Without a ladder
//! the numbers are the numbers of before, byte for byte.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`) and
//! the hive, as the tap feeds it.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The ladder as the llm cell stamps it for the Haiku 5.5 row since
/// GH #1431: base step (read 10, cached 1, 5m write 12.5), the step above
/// `above` (read 50, cached 5, write 62.5), cents per million tokens.
fn ladder(above: u64) -> Value {
    json!([
        {"above_input": 0, "cost_in": 10, "cost_cached_in": 1, "cost_cache_write": 12.5},
        {"above_input": above, "cost_in": 50, "cost_out": 250,
         "cost_cached_in": 5, "cost_cache_write": 62.5}
    ])
}

/// The limits of a 120k prompt as the tap stamps them: `cost_in` /
/// `cost_cached_in` already at the step (`priced_for`), the soft line at the
/// owner's hard stop, and -- with `stepped` -- the ladder beside them.
fn limits(stepped: bool) -> String {
    let tiers = if stepped {
        format!(
            ", 'price_tiers': {}, 'cost_cache_write': 12.5",
            ladder(100_000)
        )
    } else {
        String::new()
    };
    format!(
        "{{'input_soft': 200000, 'input_hard': 950000, 'cost_in': 50, 'cost_cached_in': 5{tiers}}}"
    )
}

/// 120k held at the step: keeping it costs 120 000 x 5 = 0.6 c a turn, the
/// rebuilt 90k (`cost_aim_of`, 200 000 x 0.45) 90 000 x 1 = 0.09 c; the
/// rebuild writes 90 000 x 12.5 = 1.125 c once -- paid back on the third
/// turn, `cost` at ten. The rule of before priced the rebuild at the step
/// (90 000 x 50 = 4.5 c): paid back on the 31st turn, `none` at ten.
#[test]
fn a_window_over_the_price_step_rebuilds_sooner() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"role": "talky", "window_target": 0}),
        &format!(
            "[curate_mark(120000, {}), curate_mark(120000, {})]",
            limits(true),
            limits(false)
        ),
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(got[0]["mark"], "cost", "with the ladder: {got}");
    assert_eq!(got[0]["rebuild_to"], 90000, "the cost aim: {got}");
    assert_eq!(got[1]["mark"], "none", "without a ladder, as before: {got}");
}

/// One function prices a turn: the fresh part at the step's input price,
/// the cached part at the step's cached price, the step the window falls in.
/// Without a ladder it is `cost_cents` of before.
#[test]
fn the_turn_is_priced_at_the_step_of_its_window() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"role": "talky"}),
        &format!(
            "[turn_cost(120000, 120000, {l}), turn_cost(90000, 90000, {l}), \
             turn_cost(120000, 100000, {l}), \
             turn_cost(4000, 3900, {{'cost_in': 10, 'cost_cached_in': 1}}), \
             cost_cents(4000, 3900, {{'cost_in': 10, 'cost_cached_in': 1}}), \
             cost_cents(120000, 100000, {l})]",
            l = limits(true)
        ),
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    let want = [0.6, 0.09, 1.5, 0.0049, 0.0049, 1.5];
    for (i, w) in want.iter().enumerate() {
        let g = got[i]
            .as_f64()
            .unwrap_or_else(|| panic!("{i}: a number: {got}"));
        assert!((g - w).abs() < 1e-9, "{i}: {g} != {w}: {got}");
    }
}

/// The talky instance as it ships, plus `more` (the overlay of
/// `gh1038_the_curator_knows_what_its_window_costs.rs`).
fn talky(more: &[(&str, &str, Value)]) -> Hive {
    let marker = read_json(&repo("templates/talky/curator/config.json"));
    let mut over: Vec<(String, String, Value)> = Vec::new();
    for (cell, params) in marker["override_params"]
        .as_object()
        .cloned()
        .unwrap_or_default()
    {
        for (k, v) in params.as_object().cloned().unwrap_or_default() {
            over.push((cell.clone(), k, v));
        }
    }
    let mut all: Vec<(&str, &str, Value)> = over
        .iter()
        .map(|(c, k, v)| (c.as_str(), k.as_str(), v.clone()))
        .collect();
    all.extend(more.iter().cloned());
    Hive::with(&all)
}

fn stamp(h: &Hive) -> Value {
    let raw = h.state_in("curate", ROUND_E);
    meclaw_core::serde_json::from_str(&raw).unwrap_or(Value::Null)
}

/// A round of 15 000 characters (a projection of at least 5 000 tokens) and
/// the tap's usage: the measured 4 000 is the window.
fn turn_of(h: &mut Hive, usage: Value) {
    let call = h.curate(
        "s",
        "t1",
        0,
        json!([user(&"x".repeat(15_000))]),
        mode("Be brief."),
    );
    h.tap(&call, "stop", usage, json!([said("a")]));
}

/// The usage of a 4k window at a step above 3k (the Haiku numbers scaled
/// down): soft 5 000, `rebuild_to` 0.35 -> the cost aim 1 750.
fn usage(stepped: bool) -> Value {
    let mut u = json!({
        "tokens_prompt": 4000, "tokens_cached": 3900,
        "input_soft": 5000, "input_hard": 100000,
        "cost_in": 50, "cost_cached_in": 5,
    });
    if stepped {
        u["price_tiers"] = ladder(3000);
        u["cost_cache_write"] = json!(12.5);
    }
    u
}

/// Through the hive: the tap carries the ladder to the cost rule. Keeping
/// 4 000 at the step costs 0.02 c a turn, the rebuilt 1 750 at the base
/// 0.00175 c; the rebuild writes 1 750 x 12.5 = 0.021875 c: paid back on the
/// second turn, `cost` at five. The rule of before (rebuild at 1 750 x 50)
/// paid back on the eighth: `none` at five -- and that is what the same
/// usage without the ladder still orders.
#[test]
fn the_tap_carries_the_ladder_to_the_cost_rule() {
    if !shipped() {
        return;
    }
    let knobs: &[(&str, &str, Value)] = &[
        ("policy", "rebuild_to", json!(0.35)),
        ("policy", "amortise_turns", json!(5)),
        ("policy", "window_target", json!(0)),
    ];
    let mut h = talky(knobs);
    turn_of(&mut h, usage(true));
    assert_eq!(
        last_add(&h).body["emit_body"]["reason"],
        "cost",
        "{:?}",
        h.stderr
    );
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "cost", "{s}");
    assert_eq!(
        h.state_in("aim", ROUND_E),
        "1750",
        "input_soft * rebuild_to"
    );
    // 100 fresh at 50 c/M + 3 900 cached at 5 c/M, the step's prices.
    assert!(
        (s["cost_cents"].as_f64().unwrap() - 0.0245).abs() < 1e-12,
        "{s}"
    );

    let mut h = talky(knobs);
    turn_of(&mut h, usage(false));
    assert!(
        h.clock.is_empty(),
        "without a ladder, as before: {:?}",
        h.clock
    );
    assert_eq!(stamp(&h)["curate_mark"], "none");
}

/// GH #1431, fix round 1 (R-38-2 "UX before price"): with a price ladder the
/// price never rebuilds under the role's target. A talky window of 105k held
/// over its target (`target_at` 101 000: the last `target` rebuild left it
/// there, the hold band runs to 101 000 + 100 000 - 90 000) is not rebuilt
/// for its price on the next turn -- without the rule `cost` would order the
/// same 90k aim on every turn, the rebuild would not get under the target and
/// break the cache each time (review I-1).
#[test]
fn a_held_window_is_not_rebuilt_for_its_price() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"role": "talky", "window_target": 100000}),
        &format!(
            "[curate_mark(105000, {}, target_at=101000), curate_mark(105000, {}, target_at=101000)]",
            limits(true),
            limits(false)
        ),
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(got[0]["mark"], "none", "with the ladder, held: {got}");
    assert_eq!(got[0]["hold"], 101000, "the hold stands: {got}");
    assert_eq!(got[1]["mark"], "none", "without a ladder, as before: {got}");
    assert_eq!(got[1]["hold"], 101000, "{got}");
}

/// The window's size is the target's to set, not the price's: a role whose
/// target is 150 000 keeps a 105k window over the 100k price step -- the
/// step does not become a cap of its own (review I-2).
#[test]
fn the_price_step_is_no_cap_under_the_target() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"role": "talky", "window_target": 150000}),
        &format!("curate_mark(105000, {})", limits(true)),
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(got["mark"], "none", "{got}");
}

/// Over the target and not held, the target rebuilds -- the price does not
/// get there first, and its aim is the target's.
#[test]
fn over_the_target_the_target_rebuilds() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"role": "talky", "window_target": 100000}),
        &format!("curate_mark(120000, {})", limits(true)),
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(got["mark"], "target", "{got}");
}

/// Without a ladder on the tap the cost rule is the rule of before, byte for
/// byte: `amortise_after` with the limits equals the four-argument form, and
/// `curate_mark` gives the marks of before (talky without a target, soft
/// 200 000, step prices 50 / 5: the rebuild to 90k pays back on the 61st
/// turn at 105k, on the 10th at 190k -- `none`, then `cost` at ten).
#[test]
fn without_a_ladder_the_cost_rule_is_the_rule_of_before() {
    if !shipped() {
        return;
    }
    let l = limits(false);
    let (got, err) = policy_scope(
        json!({"role": "talky", "window_target": 0}),
        &format!(
            "[amortise_after(105000, 90000, 50, 5, {l}), amortise_after(105000, 90000, 50, 5), \
             amortise_after(190000, 90000, 50, 5, {l}), amortise_after(190000, 90000, 50, 5), \
             amortise_after(500000, 100000, 10, 1, {{'cost_in': 10, 'cost_cached_in': 1}}), \
             curate_mark(105000, {l})['mark'], curate_mark(190000, {l})['mark'], \
             curate_mark(190000, {l})['rebuild_to']]"
        ),
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(
        got,
        json!([61, 61, 10, 10, 3, "none", "cost", 90000]),
        "{got}"
    );
}
