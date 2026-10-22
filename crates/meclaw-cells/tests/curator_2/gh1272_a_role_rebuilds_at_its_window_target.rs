//! GH #1272 (K1) -- a role rebuilds at its own window target.
//!
//! WHY this file exists: `curate_mark` knew `hard`, `soft` (the model's
//! `input_soft`, luna 250 000) and `cost`. A conversation's window reached
//! the soft line practically never, so talky's `tiers`, `keep_rounds` and
//! day horizon never applied: whole days lay raw in the window, and a
//! character's day cost 0.08-0.15 $ instead of <= 0.03 $. The role now names
//! the window it rebuilds at (`window_target`, talky 40 000 tokens): at or
//! over it and under soft the mark is `target`, the aim the same proportion
//! a soft rebuild keeps (`rebuild_to` / `compress_at`). Every instance may
//! set its own number, 0 turns it off. Without a package the mark is read
//! all the same -- the window, not the model, drives cost and latency.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

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
    Hive::with(&all)
}

/// A round whose user turn holds `chars` characters -- the policy's own
/// projection of the call is then at least `chars / 3` tokens, so the
/// measured `tokens_prompt` below it is the window -- answered with `usage`.
fn turn_of(h: &mut Hive, chars: usize, usage: Value) {
    let call = h.curate(
        "s",
        "t1",
        0,
        json!([user(&"x".repeat(chars))]),
        mode("Be brief."),
    );
    h.tap(&call, "stop", usage, json!([said("a")]));
}

/// The package of the answering model: luna's soft line.
fn usage(tokens_prompt: i64) -> Value {
    json!({"tokens_prompt": tokens_prompt, "input_soft": 250_000, "input_hard": 1_000_000})
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
    // 41 000 tokens: over talky's 40 000, far under the package's soft line.
    let mut h = talky(&[]);
    turn_of(&mut h, 130_000, usage(41_000));
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "target", "{s} {:?}", h.stderr);
    assert_eq!(s["window_tokens"], 41_000, "the measured window: {s}");
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the target orders a rebuild");
    assert_eq!(order.body["emit_body"]["reason"], "target");
    assert_eq!(
        h.state_in("aim", ROUND_E),
        "28000",
        "40 000 x rebuild_to 0.35 / compress_at 0.5"
    );
    // The rebuild plans the window by the role: its tiers and keep_recent.
    h.fire(&order);
    let plan = h.plan_in(ROUND_E);
    assert_eq!(
        plan["role"], "talky",
        "a plan of the role: {plan} {:?}",
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

    // 39 000: under the target -- nothing is due, no rebuild is ordered.
    let mut h = talky(&[]);
    turn_of(&mut h, 130_000, usage(39_000));
    assert_eq!(stamp(&h)["curate_mark"], "none");
    assert!(
        h.clock_order_for(ROUND_E).is_none(),
        "no order under the target"
    );

    // The instance sets its own number: 0 is off.
    let mut h = talky(&[("policy", "window_target", json!(0))]);
    turn_of(&mut h, 130_000, usage(41_000));
    assert_eq!(stamp(&h)["curate_mark"], "none");
}

/// The mark as a pure function: the presets of every role, the aim, and
/// the window without a package.
#[test]
fn every_role_names_its_window_target() {
    if !shipped() {
        return;
    }
    for (role, target) in [
        ("talky", 40_000),
        ("quick", 12_000),
        ("consult", 0),
        ("coding", 0),
        ("research", 0),
        ("", 0),
    ] {
        let (got, err) = policy_scope(json!({"role": role}), "WINDOW_TARGET", Value::Null);
        assert_eq!(got, json!(target), "{role:?}: {err}");
    }
    // Without a package (no `input_soft`) the target is read all the same.
    let (got, err) = policy_scope(
        json!({"role": "talky"}),
        "[curate_mark(41000, {}), curate_mark(39000, {})]",
        Value::Null,
    );
    assert_eq!(got[0]["mark"], "target", "{got} {err}");
    assert_eq!(got[0]["rebuild_to"], 28_000, "{got}");
    assert_eq!(got[1]["mark"], "none", "{got}");
    // Over soft the soft line rules, as before.
    let (got, _) = policy_scope(
        json!({"role": "talky"}),
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
    let mut h = talky(&[]);
    turn_of(&mut h, 130_000, usage(41_000));
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the target orders a rebuild");
    h.fire(&order);
    assert_eq!(target_orders(&h), 1, "{:?}", h.stderr);
    // The next turn: still over the target, the rest is the raw kept rounds.
    turn_of(&mut h, 130_000, usage(42_000));
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "none", "{s} {:?}", h.stderr);
    assert_eq!(s["target_hold"], 41_000, "the hold says why: {s}");
    assert_eq!(target_orders(&h), 1, "exactly one rebuild");
    // Grown by more than 40 000 - 28 000 since the order (a longer turn: the
    // window is the smaller of measure and projection): load again.
    turn_of(&mut h, 200_000, usage(60_000));
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "target", "{s} {:?}", h.stderr);
    assert_eq!(target_orders(&h), 2);
    // Review I-3: the hold counts from the window the rebuild left, not from
    // the order. Back on the floor of the raw kept rounds (45 000), held ...
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the second target orders a rebuild");
    h.fire(&order);
    turn_of(&mut h, 130_000, usage(45_000));
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "none", "{s} {:?}", h.stderr);
    assert_eq!(target_orders(&h), 2);
    assert_eq!(
        h.state_in("target_at", ROUND_E),
        "45000",
        "lowered to the floor"
    );
    // ... and due again at floor + 12 000, not at 60 000 + 12 000: the line
    // does not climb by window_target - aim per order.
    turn_of(&mut h, 200_000, usage(58_000));
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
    let mut h = talky(&[]);
    turn_of(
        &mut h,
        130_000,
        json!({"tokens_prompt": 41_000, "context_window": 400_000}),
    );
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "target", "{s} {:?}", h.stderr);
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the target orders a rebuild");
    assert_eq!(order.body["emit_body"]["reason"], "target");
    assert_eq!(
        h.state_in("aim", ROUND_E),
        "28000",
        "40 000 x rebuild_to 0.35 / compress_at 0.5, without a package"
    );
}
