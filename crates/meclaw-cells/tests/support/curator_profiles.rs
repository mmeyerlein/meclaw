//! The five value sets of the former role table of the curator, as named
//! window profiles for tests (D58 target, contract point 13, R-58-15).
//!
//! WHY this file exists: the curator keeps no roles. What a target's window
//! looks like is what its cell reports in its profile (`window_profile`,
//! lane `in_catalog`); a value the profile does not name comes from the knob
//! of the curator instance, else from its shipped value. These five sets are
//! EXAMPLES -- the values the roles `talky`, `quick`, `consult`, `coding`
//! and `research` carried until the table left (GH #1427, #1430, #1247) --
//! and the curator does not know them. A test that ran a role on the
//! instance announces the profile here for its target instead; with
//! `system: delta` the target holds its tree, so its profile is also the
//! source of the wall's values (plan part, Nachtrag 11.11.).

use meclaw_core::serde_json::{Value, json};

/// The names of the five example sets.
pub const VALUE_SETS: [&str; 5] = ["talky", "quick", "consult", "coding", "research"];

/// The window values of the example set `name`, as the role table held
/// them (a `null` is a value the set leaves to the instance).
pub fn value_set(name: &str) -> Value {
    match name {
        "talky" => json!({"compress_at": 0.5, "rebuild_to": 0.45,
              "keep_recent": 40, "keep_recent_tokens": 80000,
              "horizon": "day", "tiers": "raw,summary,none",
              "keep_rounds": 1, "gc": "1", "gc_scaffold": "1", "stub_tools_after": 20,
              "broadcast_mode": "tail", "recall_push": "1", "recall_budget": 200,
              "short_ids": "1", "identity_budget": "full", "window_target": 100000,
              "notes_share": 0.02, "state_lines": 8}),
        "consult" => json!({"compress_at": 0.5, "rebuild_to": 0.35,
                "keep_recent": 10, "keep_recent_tokens": null,
                "horizon": "task", "tiers": "raw,summary",
                "keep_rounds": 2, "gc": "0", "gc_scaffold": "1", "stub_tools_after": 10,
                "broadcast_mode": "tail", "recall_push": "", "recall_budget": 0,
                "short_ids": "1", "identity_budget": "full", "window_target": 0,
                "notes_share": 0, "state_lines": 0}),
        "coding" => json!({"compress_at": 0.4, "rebuild_to": 0.25,
               "keep_recent": 20, "keep_recent_tokens": null,
               "horizon": "task", "tiers": "raw,summary",
               "keep_rounds": 3, "gc": "0", "gc_scaffold": "1", "stub_tools_after": 20,
               "broadcast_mode": "tail", "recall_push": "", "recall_budget": 0,
               "short_ids": "1", "identity_budget": "full", "window_target": 0,
               "notes_share": 0, "state_lines": 0}),
        "research" => json!({"compress_at": 0.4, "rebuild_to": 0.25,
                 "keep_recent": 10, "keep_recent_tokens": null,
                 "horizon": "task", "tiers": "raw,none",
                 "keep_rounds": 1, "gc": "0", "gc_scaffold": "1", "stub_tools_after": 10,
                 "broadcast_mode": "tail", "recall_push": "", "recall_budget": 0,
                 "short_ids": "1", "identity_budget": "full", "window_target": 0,
                 "notes_share": 0, "state_lines": 0}),
        "quick" => json!({"compress_at": 0.5, "rebuild_to": 0.35,
              "keep_recent": 4, "keep_recent_tokens": null,
              "horizon": "day", "tiers": "raw,summary,none",
              "keep_rounds": 1, "gc": "0", "gc_scaffold": "1", "stub_tools_after": 4,
              "broadcast_mode": "tail", "recall_push": "", "recall_budget": 0,
              "short_ids": "1", "identity_budget": "full", "window_target": 12000,
              "notes_share": 0, "state_lines": 0}),
        other => panic!("no example value set {other}"),
    }
}

/// The profile of a target that runs the example set `name` the way an
/// instance ran the role: the set's values, and the target holds its tree
/// (`system: delta`), so the set rules its calls and the wall alike. The
/// `null` values are left out (the instance's knob fills them).
pub fn profile(name: &str) -> Value {
    let mut p = json!({"system": "delta"});
    for (k, v) in value_set(name).as_object().cloned().unwrap_or_default() {
        if !v.is_null() {
            p[k] = v;
        }
    }
    p
}

/// The values of the set `name` a call's cut reads (`window_target` of the
/// budget, `state_lines`, `short_ids`, `identity_budget`, `broadcast_mode`,
/// `notes_share`; contract point 3, Nachtrag 11.11. Nr. 1) -- what a target
/// beside a tree-holding one carries when it shall cut as that set did.
pub fn call_values(name: &str) -> Value {
    let set = value_set(name);
    let mut p = json!({});
    for k in [
        "window_target",
        "state_lines",
        "short_ids",
        "identity_budget",
        "broadcast_mode",
        "notes_share",
    ] {
        p[k] = set[k].clone();
    }
    p
}

/// The message a model cell sends on `package` when it reports itself: a
/// row that names only its model (no limits: the window stays the one the
/// answers report) and the profile beside it. Lane `in_catalog`, hop
/// `package_of` the cell's path under `/comp`.
pub fn package_with(target: &str, profile: Value) -> (Value, Value) {
    (
        json!({"package_of": format!("/comp/{target}")}),
        json!({"messages": [], "package": {"model": format!("test/{target}")},
               "window_profile": profile, "system_order": []}),
    )
}

/// The knobs of an instance that cuts every window as the example set
/// `name` did, `over` on top -- for a probe of the pure half of the policy
/// (`policy_scope`), which reads no package and so no profile: a target
/// that reported none is cut on the instance's values (contract point 1),
/// and those are then the set's. A knob in `over` stays over the set's
/// value, as a set knob stood over the preset of the former role; an unset
/// one (`null`, blank) leaves the set's value, as it left the preset.
pub fn set_knobs(name: &str, over: Value) -> Value {
    let mut p = json!({});
    for (k, v) in value_set(name).as_object().cloned().unwrap_or_default() {
        if !v.is_null() {
            p[k] = v;
        }
    }
    for (k, v) in over.as_object().cloned().unwrap_or_default() {
        let unset = v.is_null() || v.as_str().is_some_and(|t| t.trim().is_empty());
        if !(unset && p.get(&k).is_some()) {
            p[k] = v;
        }
    }
    p
}

/// The profile of every example set by its name, and `""` the empty one (a
/// target that reported none: the instance's values) -- the `ARGS` of a
/// probe that reads, per set, what the curator takes from the profile
/// (`knob_of(profile_params(p, bool(p)), key)` in the policy).
pub fn profiles_by_set() -> Value {
    let mut all = json!({"": {}});
    for name in VALUE_SETS {
        all[name] = profile(name);
    }
    all
}
