//! GH #1272 (K1) -- a role rebuilds at its own window target.
//!
//! WHY this file exists: `curate_mark` knew `hard`, `soft` (the model's
//! `input_soft`, luna 250 000) and `cost`. A conversation's window reached
//! the soft line practically never, so talky's `tiers`, `keep_rounds` and
//! day horizon never applied: whole days lay raw in the window, and a
//! character's day cost 0.08-0.15 $ instead of <= 0.03 $. The role now names
//! the window it rebuilds at (`window_target`, talky 40 000 tokens then,
//! 100 000 since GH #1427 -- the owner's guide value, R-38-2, OR-38-12): at
//! or over it and under soft the mark is `target`, the aim the same
//! proportion a soft rebuild keeps (`rebuild_to` / `compress_at`; talky's
//! `rebuild_to` is 0.45 since GH #1427, OR-38-18, so the aim is 90 000).
//! Every instance may set its own number, 0 turns it off. Without a package
//! the mark is read all the same -- the window, not the model, drives cost
//! and latency.
//!
//! The windows below are read off the preset (`WINDOW_TARGET`,
//! `target_aim_of`), so the test follows the role's numbers; the one place
//! that names them is `every_role_names_its_window_target`.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The talky instance as it ships (the marker's overrides), plus `more`.
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
    Hive::with(&all).reporting_composite("talky")
}

/// A round answered with a measured window of `window` tokens: its user turn
/// holds 3 x `window` + 3 000 characters, so the policy's own projection of
/// the call is above `window` and the measured `tokens_prompt` is the window
/// (`window_of`).
fn turn_of(h: &mut Hive, window: i64) {
    let chars = (3 * window + 3000) as usize;
    let call = h.curate(
        "s",
        "t1",
        0,
        json!([user(&"x".repeat(chars))]),
        mode("Be brief."),
    );
    h.tap(&call, "stop", usage(window), json!([said("a")]));
}

/// The package of the answering model: luna's soft line.
fn usage(tokens_prompt: i64) -> Value {
    json!({"tokens_prompt": tokens_prompt, "input_soft": 250_000, "input_hard": 1_000_000})
}

/// talky's preset: `(window_target, aim)` of a `target` rebuild without a
/// package.
fn preset() -> (i64, i64) {
    let (got, err) = policy_scope(
        curator_profiles::set_knobs("talky", json!({})),
        "[WINDOW_TARGET, target_aim_of({})]",
        Value::Null,
    );
    let t = got[0].as_i64().unwrap_or_else(|| panic!("{got} {err}"));
    let a = got[1].as_i64().unwrap_or_else(|| panic!("{got} {err}"));
    assert!(
        t > 0 && a > 0 && a < t,
        "a target and an aim under it: {got}"
    );
    (t, a)
}

/// The turn's stamp the policy keeps for the round of the call.
fn stamp(h: &Hive) -> Value {
    meclaw_core::serde_json::from_str(&h.state_in("curate", ROUND_E)).unwrap_or(Value::Null)
}

#[test]
fn a_role_rebuilds_at_its_window_target() {
    if !shipped() {
        return;
    }
    // 1 000 tokens over talky's target, far under the package's soft line.
    let (target, aim) = preset();
    let mut h = talky(&[]);
    turn_of(&mut h, target + 1000);
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "target", "{s} {:?}", h.stderr);
    assert_eq!(
        s["window_tokens"],
        target + 1000,
        "the measured window: {s}"
    );
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the target orders a rebuild");
    assert_eq!(order.body["emit_body"]["reason"], "target");
    assert_eq!(
        h.state_in("aim", ROUND_E),
        aim.to_string(),
        "window_target x rebuild_to / compress_at"
    );
    // The rebuild plans the window by the main target's profile (the value
    // set of the former role talky): its tiers and keep_recent. R-58-15: the
    // plan names no role any more.
    h.fire(&order);
    let plan = h.plan_in(ROUND_E);
    assert!(
        plan.get("role").is_none(),
        "a plan names no role: {plan} {:?}",
        h.stderr
    );
    // The round is inside talky's keep_recent (40) and of today's tier
    // (`raw`): every row of it stays above the cover, none is condensed.
    let rows: Vec<i64> = h
        .rows("SELECT seq FROM wall WHERE turn_id = 't1' ORDER BY seq")
        .into_iter()
        .map(|r| r[0].as_i64().unwrap())
        .collect();
    let cover = plan["cover"].as_i64().expect("a cover");
    assert!(
        !rows.is_empty() && rows.iter().all(|s| *s > cover),
        "keep_recent holds the round raw: {plan} {rows:?}"
    );
    assert_eq!(plan["sum_from"], 0, "nothing goes to a tier: {plan}");

    // 1 000 under the target -- nothing is due, no rebuild is ordered.
    let mut h = talky(&[]);
    turn_of(&mut h, target - 1000);
    assert_eq!(stamp(&h)["curate_mark"], "none");
    assert!(
        h.clock_order_for(ROUND_E).is_none(),
        "no order under the target"
    );

    // The instance sets its own number: 0 is off.
    let mut h = talky(&[("policy", "window_target", json!(0))]);
    turn_of(&mut h, target + 1000);
    assert_eq!(stamp(&h)["curate_mark"], "none");
}

/// The mark as a pure function: the values of every example set (the
/// former presets of the roles, R-58-15: profile values now, read as the
/// curator reads a profile -- talky 100 000 since GH #1427, the owner's
/// guide value), the aim (talky 90 000: 100 000 x `rebuild_to` 0.45 /
/// `compress_at` 0.5, OR-38-18), and the window without a package.
#[test]
fn every_role_names_its_window_target() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({}),
        "dict((r, wall_values(p)[10]) for r, p in ARGS.items())",
        curator_profiles::profiles_by_set(),
    );
    assert_eq!(
        got,
        json!({"talky": 100_000, "quick": 12_000, "consult": 0, "coding": 0,
               "research": 0, "": 0}),
        "{err}"
    );
    let (got, err) = policy_scope(
        json!({}),
        "[wall_values(ARGS)[i] for i in (3, 2, 0)]",
        curator_profiles::profile("talky"),
    );
    assert_eq!(got, json!([0.45, 0.5, 40]), "talky's presets: {err}");
    // Without a package (no `input_soft`) the target is read all the same.
    let (got, err) = policy_scope(
        curator_profiles::set_knobs("talky", json!({})),
        "[curate_mark(101000, {}), curate_mark(99000, {})]",
        Value::Null,
    );
    assert_eq!(got[0]["mark"], "target", "{got} {err}");
    assert_eq!(got[0]["rebuild_to"], 90_000, "{got}");
    assert_eq!(got[1]["mark"], "none", "{got}");
    // Over soft the soft line rules, as before.
    let (got, _) = policy_scope(
        curator_profiles::set_knobs("talky", json!({})),
        "curate_mark(60000, {'input_soft': 50000})",
        Value::Null,
    );
    assert_eq!(got["mark"], "soft", "{got}");
}

/// The clock orders of the round whose reason is `target`.
fn target_orders(h: &Hive) -> usize {
    h.clock
        .iter()
        .filter(|m| m.body.get("op") == Some(&json!("add")))
        .filter(|m| m.body["emit_body"]["reason"] == "target")
        .count()
}

/// Review I-1: a rebuild that cannot bring the window under the target --
/// talky keeps its 40 newest rounds raw -- orders no rebuild on every
/// following turn. `target` fires again only once the window has grown by
/// `window_target - aim` since the lowest window after its last order -- the
/// one the rebuild left, review I-3 -- (or after it fell under the target): before that a rebuild would condense next to nothing and only
/// break the prefix cache.
#[test]
fn a_target_rebuild_that_cannot_reach_its_aim_is_not_repeated() {
    if !shipped() {
        return;
    }
    // The hold: `window_target` - aim (talky 10 000 since GH #1427; 12 000
    // with the presets of before).
    let (target, aim) = preset();
    let hold = target - aim;
    let mut h = talky(&[]);
    turn_of(&mut h, target + 1000);
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the target orders a rebuild");
    h.fire(&order);
    assert_eq!(target_orders(&h), 1, "{:?}", h.stderr);
    // The next turn: still over the target, the rest is the raw kept rounds.
    turn_of(&mut h, target + 2000);
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "none", "{s} {:?}", h.stderr);
    assert_eq!(s["target_hold"], target + 1000, "the hold says why: {s}");
    assert_eq!(target_orders(&h), 1, "exactly one rebuild");
    // Grown by more than the hold since the order (a longer turn: the window
    // is the smaller of measure and projection): load again.
    let grown = target + 1000 + hold + 1000;
    turn_of(&mut h, grown);
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "target", "{s} {:?}", h.stderr);
    assert_eq!(target_orders(&h), 2);
    // Review I-3: the hold counts from the window the rebuild left, not from
    // the order. Back on the floor of the raw kept rounds (target + 5 000),
    // held ...
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the second target orders a rebuild");
    h.fire(&order);
    let floor = target + 5000;
    turn_of(&mut h, floor);
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "none", "{s} {:?}", h.stderr);
    assert_eq!(target_orders(&h), 2);
    assert_eq!(
        h.state_in("target_at", ROUND_E),
        floor.to_string(),
        "lowered to the floor"
    );
    // ... and due again at floor + hold, not at `grown` + hold: the line does
    // not climb by window_target - aim per order.
    turn_of(&mut h, floor + hold + 1000);
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "target", "{s} {:?}", h.stderr);
    assert_eq!(target_orders(&h), 3);
}

/// Review I-2: without the model's package (no `input_soft`, only its
/// window) the hive still marks `target` and names the aim of the rebuild.
#[test]
fn without_a_package_the_target_names_its_aim() {
    if !shipped() {
        return;
    }
    let (target, aim) = preset();
    let mut h = talky(&[]);
    let call = h.curate(
        "s",
        "t1",
        0,
        json!([user(&"x".repeat((3 * target + 6000) as usize))]),
        mode("Be brief."),
    );
    h.tap(
        &call,
        "stop",
        json!({"tokens_prompt": target + 1000, "context_window": 400_000}),
        json!([said("a")]),
    );
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "target", "{s} {:?}", h.stderr);
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the target orders a rebuild");
    assert_eq!(order.body["emit_body"]["reason"], "target");
    assert_eq!(
        h.state_in("aim", ROUND_E),
        aim.to_string(),
        "window_target x rebuild_to / compress_at, without a package"
    );
}
