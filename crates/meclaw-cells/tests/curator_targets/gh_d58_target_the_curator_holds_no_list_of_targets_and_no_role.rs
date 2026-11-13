//! D58 target (GH #1483, contract points 2 and 13) -- the curator holds no
//! list of targets and no role.
//!
//! WHY this file exists: R-58-15 and R-58-17. The curator knows its targets
//! only from what they report and keeps nothing of its own about them: no
//! table of roles (`ROLES`), no knob `role`, no list of the cells it cuts
//! for (`CUT_CELLS`, `cut_cell`), no list of ways (`WAYS`). The proof that
//! no list hides elsewhere: a target whose name no composite of this tree
//! uses is held and cut for like any other.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use curator_profiles::{VALUE_SETS, value_set};
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

/// `v` with every word that is a name of the lock (a run of `[a-z0-9_-]`)
/// swapped for `<name>_probe`, strings inside lists and objects as well.
fn neutral(v: &Value) -> Value {
    match v {
        Value::String(s) => {
            let mut out = String::new();
            let mut word = String::new();
            let flush = |word: &mut String, out: &mut String| {
                out.push_str(word);
                if LOCKED_NAMES.contains(&word.as_str()) {
                    out.push_str("_probe");
                }
                word.clear();
            };
            for c in s.chars() {
                if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                    word.push(c);
                } else {
                    flush(&mut word, &mut out);
                    out.push(c);
                }
            }
            flush(&mut word, &mut out);
            Value::String(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(neutral).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), neutral(x))).collect()),
        other => other.clone(),
    }
}

/// The policy's shipped params, every name of the lock in them swapped
/// ([`neutral`]); `script_inline` is no knob.
fn neutral_params() -> Value {
    let mut params = cell_config("policy")["params"].clone();
    if let Some(o) = params.as_object_mut() {
        o.remove("script_inline");
    }
    neutral(&params)
}

#[test]
fn gh_d58_target_the_curator_holds_no_list_of_targets_and_no_role() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    assert!(
        policy["params"].get("role").is_none(),
        "the knob `role` is gone from the params"
    );
    assert!(
        policy["contract"]["settings"].get("role").is_none(),
        "the knob `role` is gone from the contract's settings"
    );
    // The knobs whose only work was the table of the former fast way go
    // with it: the profile says `tools`/`allow` and `instructions.only`.
    for knob in ["quick_tools", "quick_instructions_slot"] {
        assert!(
            policy["params"].get(knob).is_none()
                && policy["contract"]["settings"].get(knob).is_none(),
            "the knob `{knob}` is gone with the table of the former fast way"
        );
    }
    let (left, err) = policy_scope(
        json!({}),
        "sorted(n for n in ('ROLES', 'CUT_CELLS', 'WAYS', 'QUICK', 'cut_cell', 'role_named', \
         'ROLE', 'legacy_profile', 'QUICK_TOOLS', 'QUICK_SLOT', 'QUICK_TEXT') if n in globals())",
        Value::Null,
    );
    assert_eq!(
        left,
        json!([]),
        "the policy keeps no list of targets or roles: {left} {err}"
    );
    // No value set under another name either (R-58-15): no dict anywhere in
    // the policy's scope carries one of the five former sets whole.
    let mut sets = json!({});
    for name in VALUE_SETS {
        let mut set = json!({});
        for (k, v) in value_set(name).as_object().cloned().unwrap_or_default() {
            if !v.is_null() {
                set[k] = v;
            }
        }
        sets[name] = set;
    }
    let (kept, err) = policy_scope(
        json!({}),
        "(lambda f: sorted(n for n, s in ARGS.items() if any(isinstance(d, dict) and \
         all(k in d and d[k] == v for k, v in s.items()) for d in f(f, [v for k, v in \
         globals().items() if k not in ('__builtins__', 'ARGS')]))))(lambda f, xs: [y for x in xs for y in \
         ([x] + (f(f, list(x.values())) if isinstance(x, dict) else f(f, list(x)) \
         if isinstance(x, (list, tuple)) else []))])",
        sets,
    );
    assert_eq!(
        kept,
        json!([]),
        "the policy keeps a former value set under another name: {kept} {err}"
    );
    // No table of names under any name either (D58 close, review 1 m1 of
    // the target strand: the names above are fixed, a renamed table would
    // pass them). Every list, tuple, set or dict in the policy's scope is
    // read whole, keys and values and what they hold: none holds a name of
    // the lock (`support/curator_locked_names.txt`) -- also not one built at
    // load time from a string the lock reads no name in (`"brain quick"
    // .split()`). The knobs are the instance's: a shipped knob value that
    // names a name of the lock (`errand_slot` "consult", rulings .44/.45 of
    // the target strand) reaches the scope through the knob, not through a
    // table of the code, so the policy loads here with every such name in
    // its params swapped for a probe word.
    let (tables, err) = policy_scope(
        neutral_params(),
        "(lambda f: sorted(set('%s: %s' % (k, s) for k, v in globals().items() if k not in \
         ('__builtins__', 'ARGS') and isinstance(v, (dict, list, tuple, set, frozenset)) \
         for s in f(f, v) if s in ARGS)))(lambda f, x: [x] if isinstance(x, str) else \
         [y for kk, vv in x.items() for y in f(f, kk) + f(f, vv)] if isinstance(x, dict) else \
         [y for e in x for y in f(f, e)] if isinstance(x, (list, tuple, set, frozenset)) \
         else [])",
        json!(LOCKED_NAMES.as_slice()),
    );
    assert_eq!(
        tables,
        json!([]),
        "a table in the policy's scope holds a name of the lock: {tables} {err}"
    );

    let mut h = Hive::new();
    announce(
        &mut h,
        "zebra_7",
        row("test/zebra", 20, 30),
        // It holds its tree: the one target is also the wall's source, so no
        // mark of a missing wall (`target_unknown`, missing "wall",
        // Nachtrag 11.11. Nr. 2) stands beside the claim below.
        json!({"short_ids": "1", "system": "delta"}),
        json!([]),
    );
    assert_eq!(
        held(&h, "zebra_7")["model"],
        json!("test/zebra"),
        "any name is held: {}",
        held(&h, "zebra_7")
    );
    let c = say(&mut h, Some("zebra_7"), "t1", "Hello there.");
    assert_eq!(c.hop.get("target"), Some(&json!("zebra_7")), "{:?}", c.hop);
    assert!(has_overflow_cut(&c), "and cut for by its row: {:?}", c.hop);
    assert!(
        marks_of_kind(&h, TARGET_MARK).is_empty(),
        "a reported name is a known target"
    );
}
