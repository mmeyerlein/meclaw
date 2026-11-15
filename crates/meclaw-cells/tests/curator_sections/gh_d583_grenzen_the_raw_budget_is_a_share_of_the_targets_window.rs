//! GH #1526 (d58-3 grenzen, OR-583.grenzen.12) -- the raw token budget is a
//! share of the target's window, and a token value set beside it wins.
//!
//! WHY this file exists: talky's profile kept 80000 tokens raw through every
//! rebuild (`keep_recent_tokens`, GH #1427), whatever model it ran on -- the
//! number was a third of luna's window (`input_soft` 250000) and would be
//! four windows of a model of 17500. The profile now names a share
//! (`keep_recent_share` 0.32 = 80000 / 250000) and the budget is that share
//! of the window the target's catalog row names: two targets with different
//! windows get different budgets out of the same profile. A token value
//! (`keep_recent_tokens`, `window_target`) that is set wins over every share
//! on every level -- the override for tests and instances; an instance's
//! token value beats a profile's share too (OR-583.grenzen.16) -- and the
//! instance's share sizes a target whose profile names neither.
//!
//! Runs the pure half of the shipped policy (`policy_scope`): the reported
//! rows are the state `ARGS`, the profile is the shipped `talky/brain` one.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn talky_profile() -> Value {
    read_json(&repo("templates/talky/brain/config.json"))["params"]["window_profile"].clone()
}

/// `share` of `window` in tokens, as the policy sizes it (`sized`).
fn of(share: &Value, window: u64) -> Value {
    let s = share.as_f64().expect("a numeric share");
    json!((s * window as f64).round() as u64)
}

/// The state with one reported row per `(target, input_soft, profile)`.
fn rows(targets: &[(&str, u64, Value)]) -> Value {
    let mut st = meclaw_core::serde_json::Map::new();
    for (t, soft, profile) in targets {
        let row = json!({"model": format!("test/{t}"), "input_soft": soft,
                         "input_hard": soft * 4, "window_profile": profile});
        st.insert(format!("catalog:{t}"), Value::String(row.to_string()));
    }
    Value::Object(st)
}

/// The raw token budget of the wall held by each target of `st`.
fn budgets(over: Value, st: Value, targets: &[&str]) -> Vec<Value> {
    let probe = format!(
        "[wall_values(profile_held(t, ARGS))[1] for t in {}]",
        json!(targets)
    );
    policy_scope(over, &probe, st)
        .0
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn gh_d583_grenzen_the_raw_budget_is_a_share_of_the_targets_window() {
    if !shipped() {
        return;
    }
    let p = talky_profile();
    assert_eq!(
        p["keep_recent_share"],
        json!(0.32),
        "talky's profile names the share of the window it keeps raw: {p}"
    );
    let share = &p["keep_recent_share"];
    let st = rows(&[("a", 250000, p.clone()), ("b", 200000, p.clone())]);
    assert_eq!(
        budgets(json!({}), st, &["a", "b"]),
        vec![of(share, 250000), of(share, 200000)],
        "the share of 250000 and of 200000: the budget follows the target's window"
    );
}

#[test]
fn gh_d583_grenzen_a_token_override_beats_any_share() {
    if !shipped() {
        return;
    }
    // On the profile: the token value set beside the share wins.
    let mut p = talky_profile();
    p["keep_recent_tokens"] = json!(5000);
    let st = rows(&[("a", 250000, p)]);
    assert_eq!(budgets(json!({}), st, &["a"]), vec![json!(5000)]);

    // A profile that names neither: the instance's share, of this target's window.
    let mut bare = talky_profile();
    let o = bare.as_object_mut().expect("an object");
    o.remove("keep_recent_share");
    o.remove("keep_recent_tokens");
    let st = rows(&[("a", 250000, bare.clone()), ("b", 100000, bare.clone())]);
    assert_eq!(
        budgets(json!({"keep_recent_share": 0.1}), st.clone(), &["a", "b"]),
        vec![json!(25000), json!(10000)]
    );
    // And on the instance too the token value wins over the share.
    assert_eq!(
        budgets(
            json!({"keep_recent_share": 0.1, "keep_recent_tokens": 3000}),
            st.clone(),
            &["a", "b"]
        ),
        vec![json!(3000), json!(3000)]
    );
    // Nothing set anywhere: off, as shipped.
    assert_eq!(
        budgets(json!({}), st, &["a"]),
        vec![json!(0)],
        "no share and no token value: no raw token budget"
    );

    // Across the levels (OR-583.grenzen.16): the instance's token value beats
    // the share the shipped profile names, for both knobs, on two targets of
    // different windows. Without it each target holds its share of its own
    // window -- two models, two windows, two budgets; with it both hold the
    // instance's number. Before .16 the profile's share won.
    let p = talky_profile();
    let keep = p["keep_recent_share"].clone();
    let aim = p["window_target_share"].clone();
    let st = rows(&[("a", 250000, p.clone()), ("b", 200000, p)]);
    assert_eq!(
        budgets(json!({}), st.clone(), &["a", "b"]),
        vec![of(&keep, 250000), of(&keep, 200000)],
        "without an override each target keeps its share of its own window"
    );
    assert_ne!(
        of(&keep, 250000),
        of(&keep, 200000),
        "two windows, two budgets"
    );
    assert_eq!(
        budgets(json!({"keep_recent_tokens": 3000}), st.clone(), &["a", "b"]),
        vec![json!(3000), json!(3000)],
        "an instance's token value beats the profile's keep_recent_share"
    );
    let probe = "[[call_profile(t, 'in', ARGS)['window_target'], \
                  wall_values(profile_held(t, ARGS))[10]] for t in ['a', 'b']]";
    assert_eq!(
        policy_scope(json!({}), probe, st.clone()).0,
        json!([
            [of(&aim, 250000), of(&aim, 250000)],
            [of(&aim, 200000), of(&aim, 200000)]
        ]),
        "without an override each target aims at its share of its own window"
    );
    assert_eq!(
        policy_scope(json!({"window_target": 7000}), probe, st).0,
        json!([[7000, 7000], [7000, 7000]]),
        "an instance's window_target beats the profile's window_target_share"
    );
}
