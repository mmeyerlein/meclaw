//! D58 target (GH #1483, contract point 3) -- each field of the profile
//! shapes the window.
//!
//! WHY this file exists: the profile a composite writes on a model cell is
//! the only place that says what the cell uses its model for; the curator
//! keeps no table of roles. The fields are closed: the window values a role
//! preset held, `window`, `system`, `tools`, `must_call`, `speaks`,
//! `instructions`, `frame`, `closes_turn` and `recall`. A field that moves no
//! window is a field nobody needs, and one the curator ignores is a promise
//! the profile cannot keep. One row per field: the same conversation runs
//! twice, once for a target whose profile sets the field to one value and
//! once to the other (beside a base both rows share), and the part of the
//! call the field is for differs -- nothing else in the row is set.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

/// The text of the note the conversation leaves for the next window.
const NOTE: &str = "remember the umbrella before the walk";

/// The id of the memory's own lookup an earlier turn of the conversation
/// made (`memory_recall`): its rows are the memory lines of the wall.
const RECALL_ID: &str = "call_recall_0000000000target1";

/// The text of the target's own instruction slot the composite fills.
const SLOT_TEXT: &str = "Speak warmly and slowly.";

/// The text a profile names for its slot while the slot is empty.
const FALLBACK_TEXT: &str = "Answer in one short sentence.";

/// A leaf of the family `instructions` that no session holds.
const STYLE_TEXT: &str = "Use plain words.";

/// The answer the last call of the conversation gets.
const LAST_ANSWER: &str = "It stays dry.";

/// The mark of a required tool round whose verdict names no fallback: its
/// answer is the answer of the turn (OR-d58-2.target.71).
const UNBACKED_MARK: &str = "tool_round_answer";

/// The system part of [`system_tree`] with the slot `voice_rules` filled.
fn system_with_slot() -> Value {
    let mut s = system_tree();
    s["instructions"]["voice_rules"] = json!({"text": SLOT_TEXT});
    s
}

/// What one run shows of the call it ends with, the system part `system`
/// on that call.
///
/// The instance sets a place: the leaf `frame` needs both, the target's
/// word (`frame` in its profile) and a moment to tell (`place` or `timezone`
/// on the instance; D58 frame, GH #1478, OR-d58-2.frame.2). The place is the
/// same for both runs of a row, so it moves nothing but the field `frame`.
fn observed_in(profile: Value, system: Value) -> Value {
    let mut h = Hive::with(&[("policy", "place", json!("Springfield"))]);
    announce_profile(&mut h, "seat", profile);
    for (i, (words, reply)) in TALK.iter().take(3).enumerate() {
        turn_for(&mut h, "seat", &format!("t{i}"), words, reply);
    }
    // A turn with the memory's lookup and its result: the memory lines the
    // field `recall` lets into the window or keeps out of it.
    let looked = curate_for(
        &mut h,
        Some("seat"),
        "t8",
        0,
        json!([
            user("What did I say about my sister?"),
            tool_call(RECALL_ID, "memory_recall"),
            tool_result(
                RECALL_ID,
                "WHAT WAS SAID\n  user: \"My sister moved to Hamburg.\""
            )
        ]),
        json!({}),
    );
    let lc = the_call(&h, &looked);
    answer(
        &mut h,
        &lc,
        "stop",
        warm_usage(),
        json!([said("She lives in Hamburg.")]),
    );
    h.lane(
        "in_note",
        json!({"session_id": "s1"}),
        json!({}),
        json!({"notes": [{"id": "n1", "source": "probe", "text": NOTE, "scope": "turn"}]}),
    );
    let calls = curate_with_system(
        &mut h,
        Some("seat"),
        "t9",
        0,
        json!([user("Will it rain on the walk? Look it up.")]),
        json!({"path": "", "needs": "tool", "families": "web"}),
        system,
    );
    let c = the_call(&h, &calls);
    answer(&mut h, &c, "stop", warm_usage(), json!([said(LAST_ANSWER)]));
    let answered = h
        .rows(
            "SELECT b.body FROM wall w JOIN blocks b ON b.hash = w.hash \
             WHERE w.turn_id = 't9' AND w.kind = 'assistant'",
        )
        .iter()
        .any(|r| r[0].to_string().contains(LAST_ANSWER));
    let turn_rows = h
        .rows("SELECT key FROM state WHERE key LIKE 'turn_system:s1:t9%'")
        .len();
    let recalled = c
        .body
        .get("messages")
        .is_some_and(|m| m.to_string().contains(RECALL_ID));
    json!({
        "system": c.body.get("system").cloned().unwrap_or(Value::Null),
        "messages": c.body.get("messages").cloned().unwrap_or(Value::Null),
        "tool_scope": c.body.get("tool_scope").cloned().unwrap_or(Value::Null),
        "interim": c.hop.get("interim").cloned().unwrap_or(Value::Null),
        // The memory lines of the earlier lookup in the window of the call
        // (the field acts in the cut of the call, not on the route
        // `recall`, which only the push's question on `in_recall_ask` takes).
        "recall_in_window": recalled,
        "turn_row_left": turn_rows,
        "answered": answered,
        "unbacked_marks": marks_of_kind(&h, UNBACKED_MARK),
        "stderr": h.stderr.join("\n"),
    })
}

fn with(base: &Value, field: &str, v: &Value) -> Value {
    let mut p = base.clone();
    if !v.is_null() {
        p[field] = v.clone();
    }
    p
}

/// Every run in `runs`, each on one of a few threads, keyed by its input.
/// A run is a hive of its own (its own store, its own warm python children,
/// which `support/warm_python.rs` keeps per thread), so the runs share
/// nothing.
fn observe_all(runs: &[(Value, Value)]) -> Vec<((Value, Value), Value)> {
    let n = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 6);
    std::thread::scope(|sc| {
        let handles: Vec<_> = (0..n)
            .map(|i| {
                sc.spawn(move || {
                    runs.iter()
                        .skip(i)
                        .step_by(n)
                        .map(|(p, s)| ((p.clone(), s.clone()), observed_in(p.clone(), s.clone())))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a run of the table"))
            .collect()
    })
}

/// WHY two passes (D58 close, GH #1494): the checks below read 38 runs, 11
/// of them twice, one after the other -- 88 s under the strand gate's load
/// (SLOW-LOCK, gate a5d99377 of the target strand), 40 s alone on a lane.
/// A first pass over the checks only names the runs they read; each
/// distinct run then goes once, on several threads, and the second pass
/// reads the same checks over the results. Every claim and every run stays.
#[test]
fn gh_d58_target_each_field_of_the_profile_shapes_the_window() {
    if !shipped() {
        return;
    }
    let mut wanted: Vec<(Value, Value)> = Vec::new();
    checks(&mut |p, s| {
        if !wanted.contains(&(p.clone(), s.clone())) {
            wanted.push((p, s));
        }
        Value::Null
    });
    let seen = observe_all(&wanted);
    assert_eq!(
        seen.len(),
        wanted.len(),
        "check of the setup: every run ran"
    );
    let failures = checks(&mut |p, s| {
        let key = (p, s);
        seen.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("check of the setup: the run {key:?} was named"))
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The claims of the test over the runs `obs` gives for a profile and a
/// system part; the failures, one line each.
fn checks(obs: &mut dyn FnMut(Value, Value) -> Value) -> Vec<String> {
    let obs = std::cell::RefCell::new(obs);
    let observed = |p: Value| (*obs.borrow_mut())(p, system_tree());
    let observed_in = |p: Value, s: Value| (*obs.borrow_mut())(p, s);
    let none = json!({});
    // (field, one value, the other (null: unset), base of both, the part
    // of the observation the field moves)
    let table: Vec<(&str, Value, Value, Value, &str)> = vec![
        // The window values of a former role preset, one for all of them.
        ("keep_recent", json!(2), json!(40), none.clone(), "messages"),
        (
            "window",
            json!("turn"),
            json!("wall"),
            none.clone(),
            "messages",
        ),
        (
            "system",
            json!("none"),
            json!("full"),
            none.clone(),
            "system",
        ),
        (
            "tools",
            json!("none"),
            json!("all"),
            none.clone(),
            "tool_scope",
        ),
        // A required round: the scope requires a call. Without a fallback
        // in the verdict nothing hands the round on (contract point 4), so
        // the call carries no `interim`.
        (
            "must_call",
            json!(true),
            json!(false),
            none.clone(),
            "tool_scope",
        ),
        (
            "speaks",
            json!(true),
            json!(false),
            json!({"notes_share": 0.02}),
            "messages",
        ),
        // `tools: scoped` with a list narrower than the instance's class.
        (
            "allow",
            json!(["weather"]),
            json!(["web", "weather"]),
            json!({"tools": "scoped"}),
            "tool_scope",
        ),
        (
            "instructions",
            json!({"slot": "voice_rules", "text": FALLBACK_TEXT}),
            Value::Null,
            none.clone(),
            "system",
        ),
        ("frame", json!(true), json!(false), none.clone(), "system"),
        (
            "closes_turn",
            json!(true),
            json!(false),
            json!({"system": "delta"}),
            "turn_row_left",
        ),
        (
            "recall",
            json!(true),
            json!(false),
            none.clone(),
            "recall_in_window",
        ),
    ];
    let mut failures = Vec::new();
    for (field, a, b, base, part) in &table {
        let one = observed(with(base, field, a));
        let other = observed(with(base, field, b));
        if one[*part] == other[*part] {
            failures.push(format!(
                "{field}: {a} and {b} give the same `{part}`: {} ({})",
                one[*part], one["stderr"]
            ));
        }
    }
    // The rows say what the field is for, not merely that something moved.
    // `must_call` without a fallback in the verdict (OR-d58-2.target.71):
    // the round requires its call; an answer without one is the answer of
    // the turn -- it stands in the wall, nothing is parked, and a mark says
    // so. With the field off there is no such round and no mark.
    let required = observed(json!({"must_call": true}));
    let free = observed(json!({"must_call": false}));
    if required["tool_scope"]["require"] != json!(true) {
        failures.push(format!(
            "must_call: the scope does not require a call: {}",
            required["tool_scope"]
        ));
    }
    if required["interim"] != Value::Null {
        failures.push(format!(
            "must_call without a fallback: the call is marked interim ({}), so its answer \
             would not count",
            required["interim"]
        ));
    }
    if required["answered"] != json!(true) || free["answered"] != json!(true) {
        failures.push(format!(
            "must_call without a fallback: the answer is not the turn's answer ({} / {}): {}",
            required["answered"], free["answered"], required["stderr"]
        ));
    }
    if required["stderr"]
        .as_str()
        .unwrap_or("")
        .contains("lost its parked turn")
    {
        failures.push(format!(
            "must_call without a fallback: the round was parked: {}",
            required["stderr"]
        ));
    }
    let marked = required["unbacked_marks"].as_array().map_or(0, Vec::len);
    let unmarked = free["unbacked_marks"].as_array().map_or(0, Vec::len);
    if marked != 1 || unmarked != 0 {
        failures.push(format!(
            "must_call without a fallback: one `{UNBACKED_MARK}` mark for the required round, \
             none without it: {} / {}",
            required["unbacked_marks"], free["unbacked_marks"]
        ));
    }
    let speaking = observed(json!({"notes_share": 0.02, "speaks": true}));
    let silent = observed(json!({"notes_share": 0.02, "speaks": false}));
    if !speaking["messages"].to_string().contains(NOTE)
        || silent["messages"].to_string().contains(NOTE)
    {
        failures
            .push("speaks: the note stands in the window of the target that speaks only".into());
    }
    let recalling = observed(json!({"recall": true}));
    let forgetting = observed(json!({"recall": false}));
    if recalling["recall_in_window"] != json!(true)
        || forgetting["recall_in_window"] != json!(false)
    {
        failures.push(
            "recall: the memory lines stand in the window of the target with `recall` only".into(),
        );
    }
    let turn_only = observed(json!({"window": "turn"}));
    if turn_only["messages"].to_string().contains(TALK[0].1) {
        failures.push("window turn: an answer of an earlier turn stands in the window".into());
    }
    let no_system = observed(json!({"system": "none"}));
    if no_system["system"]
        .as_object()
        .is_some_and(|s| !s.is_empty())
    {
        failures.push(format!(
            "system none: a system part left: {}",
            no_system["system"]
        ));
    }
    // `tools: scoped` narrows the scope to exactly its list: the list stands
    // at the ops stage, a stamped family outside it only at the hint stage,
    // whatever class of tools the instance holds.
    let ops = |o: &Value| -> Vec<String> {
        let names = |k: &str| -> Vec<String> {
            o["tool_scope"][k]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let hint = names("hint");
        names("allow")
            .into_iter()
            .filter(|n| !hint.contains(n))
            .collect()
    };
    let narrow = observed(json!({"tools": "scoped", "allow": ["weather"]}));
    let wide = observed(json!({"tools": "scoped", "allow": ["web", "weather"]}));
    if ops(&narrow) != vec!["weather".to_string()] {
        failures.push(format!(
            "tools scoped: the list [weather] is not the scope's ops stage: {}",
            narrow["tool_scope"]
        ));
    }
    if ops(&wide).len() != 2 {
        failures.push(format!(
            "tools scoped: the list [web, weather] is not the scope's ops stage: {}",
            wide["tool_scope"]
        ));
    }
    let own_profile = json!({"instructions": {"slot": "voice_rules", "text": FALLBACK_TEXT}});
    let empty = observed(own_profile.clone());
    let empty_system = empty["system"].to_string();
    if !empty_system.contains(FALLBACK_TEXT) || !empty_system.contains("Be brief.") {
        failures.push(format!(
            "instructions: with its slot empty the text of the profile does not stand beside \
             the family: {}",
            empty["system"]
        ));
    }
    let filled = observed_in(own_profile, system_with_slot());
    let filled_system = filled["system"].to_string();
    if !filled_system.contains(SLOT_TEXT)
        || filled_system.contains(FALLBACK_TEXT)
        || !filled_system.contains("Be brief.")
    {
        failures.push(format!(
            "instructions: a filled slot does not hold, or the text of the profile stands \
             beside it: {}",
            filled["system"]
        ));
    }
    // `instructions.only` (OR-d58-2.target.53): with it the filled slot
    // takes the place of the rest of the family (`instructions.mode` is a
    // slot of the session and stays, so the family's witness here is a leaf
    // of its own, `instructions.style`); without it (the line above) the
    // family stays beside the slot.
    let only_profile =
        json!({"instructions": {"slot": "voice_rules", "text": FALLBACK_TEXT, "only": true}});
    let styled = |mut s: Value| {
        s["instructions"]["style"] = json!({"text": STYLE_TEXT});
        s
    };
    let only = observed_in(only_profile.clone(), styled(system_with_slot()));
    let only_system = only["system"].to_string();
    if !only_system.contains(SLOT_TEXT)
        || only_system.contains(STYLE_TEXT)
        || only_system.contains(FALLBACK_TEXT)
    {
        failures.push(format!(
            "instructions only: the filled slot does not take the place of the family: {}",
            only["system"]
        ));
    }
    let beside = observed_in(
        json!({"instructions": {"slot": "voice_rules", "text": FALLBACK_TEXT}}),
        styled(system_with_slot()),
    );
    if !beside["system"].to_string().contains(STYLE_TEXT) {
        failures.push(format!(
            "instructions without only: the family does not stay beside the slot: {}",
            beside["system"]
        ));
    }
    // As the switch `quick_instructions_slot` "1" did: an empty slot holds
    // the profile's text, and that text takes the family's place.
    let only_empty = observed_in(only_profile, styled(system_tree()));
    let only_empty_system = only_empty["system"].to_string();
    if !only_empty_system.contains(FALLBACK_TEXT) || only_empty_system.contains(STYLE_TEXT) {
        failures.push(format!(
            "instructions only: with its slot empty the profile's text does not take the \
             family's place: {}",
            only_empty["system"]
        ));
    }
    // A slot no reported profile names is part of the family: once a target
    // reported its profile, no table of former ways takes a slot away (the
    // slot of the former fast way, `instructions.quick`, among them).
    let mut quick_filled = system_tree();
    quick_filled["instructions"]["quick"] = json!({"text": SLOT_TEXT});
    // An empty profile is no profile (OR-d58-2.target.25): the target here
    // reports one that names no slot.
    let unnamed = observed_in(json!({"frame": false}), quick_filled);
    if !unnamed["system"].to_string().contains(SLOT_TEXT) {
        failures.push(format!(
            "instructions: a slot no reported profile names left the family: {}",
            unnamed["system"]
        ));
    }
    failures
}
