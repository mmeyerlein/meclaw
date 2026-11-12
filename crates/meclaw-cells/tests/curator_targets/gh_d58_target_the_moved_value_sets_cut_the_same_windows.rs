//! D58 target (GH #1483, contract point 13, R-58-15) -- the moved value sets
//! cut the same windows.
//!
//! WHY this file exists: the table of roles leaves the curator, and its
//! value sets move unchanged into the profiles of the model cells (OR-58-34:
//! the values themselves do not change). The proof is in the bytes: for each
//! former role a fixed conversation was run once with the knob `role` (the
//! state before the move, recorded as a fixed expectation under
//! `tests/fixtures/d58_target/`), and the same conversation, run for a main
//! target whose profile is the same value set and holds its tree, must give
//! the same bytes.
//!
//! The conversation is chosen so that a wrongly moved value shows: it runs
//! over three days (the horizon `day`/`task`/`all` and the tiers
//! `raw,summary,none` age its rows apart), holds more turns than the
//! smallest `keep_recent` and fewer than the largest, stands under a small
//! soft line, so the rebuild has to cut and asks the summarizer, and leaves
//! a note (`notes_share`) for the window after the rebuild. Planner's
//! addendum of 11.11.: the rebuild follows the answer of a target that holds
//! no tree -- before, an answer on the fast way (path `simple`) whose tap and
//! rebuild ran on the instance's role; now the main target holds the tree
//! with the role's value set and the fast target carries only what acts per
//! call. Compared: the summarizer's requests and the plan of the rebuild
//! (`moved_<role>_rebuild.json`), and the next window of the main target --
//! system part, messages, tool scope (`moved_<role>.json`). A guard holds
//! the five recorded windows and the five rebuilds pairwise different: a
//! fixture all five sets cut alike would let a wrong set pass.
//!
//! The plan's field `role` is the name of the former role (`window_plan`
//! writes it); after R-58-15 there is no role to name, so that one field --
//! and nothing else -- is taken out of the comparison of the plan.
//!
//! Recording: under `MECLAW_D38_C0_BLESS=1` (the corpus switch, which travels
//! to a lane) a missing fixture is written from the hive as it stands, with
//! the knob `role`; an existing one is never overwritten -- once the knob is
//! gone there is nothing to record it from. The fixtures were recorded on the
//! tree before the strand.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`) on the corpus
//! clock; no model.

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
use curator_profiles::{VALUE_SETS, call_values, value_set};
use curator_target::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const FIXTURES: &str = "crates/meclaw-cells/tests/fixtures/d58_target";
const BLESS: &str = "MECLAW_D38_C0_BLESS";

const ROLES: [&str; 5] = VALUE_SETS;

fn fixture(role: &str) -> std::path::PathBuf {
    repo(&format!("{FIXTURES}/moved_{role}.json"))
}

fn rebuild_fixture(role: &str) -> std::path::PathBuf {
    repo(&format!("{FIXTURES}/moved_{role}_rebuild.json"))
}

/// One day of the corpus clock.
const DAY_US: i64 = 86_400_000_000;

/// The soft line every answer of the conversation reports: small, so the
/// rebuild has to cut the talk to its aim.
const SOFT: i64 = 4000;

/// The note the conversation leaves for the window after the rebuild.
const NOTE: &str = "bring the blue umbrella to the river";

/// The day of turn `i` of [`TURNS`] (0 = two days before the last).
fn day_of_turn(i: usize) -> i64 {
    match i {
        0..=5 => 0,
        6..=11 => 1,
        _ => 2,
    }
}

/// The number of turns of the main target before the fast one answers.
const TURNS: usize = 26;

/// The words of turn `i` and the answer of the main target: short enough
/// that the raw rows of every value set fit its aim after the rebuild (over
/// it, the plan gives the summary up), long enough to cross the line.
fn talk(i: usize) -> (String, String) {
    let (words, reply) = TALK[i % TALK.len()];
    (
        format!("{words} (turn {i})"),
        format!("{reply} (turn {i}) I keep that in mind."),
    )
}

/// The usage of an answer: `prompt` tokens under the soft line [`SOFT`].
fn usage(prompt: i64) -> Value {
    json!({"tokens_prompt": prompt, "tokens_cached": 0,
           "input_soft": SOFT, "input_hard": 10_000_000,
           "cost_in": 10, "cost_cached_in": 0,
           "cache_expires_at": "2099-01-01T00:00:00Z"})
}

/// [`TURNS`] turns of the main target over three days, then one of the
/// fast one whose answer crosses `compress_at` of the soft line; the
/// rebuild that order starts, the summarizer answered; a note; the next
/// call of the main target. `main`/`side` name the targets (none: the route
/// stamp `talk`/`simple` of before chooses the way). Returns the rebuild
/// (`summarizer` requests, `plan`) and the next window (`system`,
/// `messages`, `tool_scope`).
fn fixed_talk(
    over: &[(&str, &str, Value)],
    main: Option<&str>,
    side: Option<&str>,
    before: impl FnOnce(&mut Hive),
) -> (Value, Value) {
    let mut h = Hive::with_prelude(over, PRELUDE);
    h.set_clock(T0_US);
    before(&mut h);
    let mut step = 0i64;
    let mut at = |h: &mut Hive, day: i64| {
        step += 1;
        h.set_clock(T0_US + day * DAY_US + 5_000_000 * step);
    };
    let main_stamp = json!({"path": if main.is_some() { "" } else { "talk" }});
    for i in 0..TURNS {
        let (words, reply) = talk(i);
        at(&mut h, day_of_turn(i));
        let calls = curate_for(
            &mut h,
            main,
            &format!("t{i:02}"),
            0,
            json!([user(&words)]),
            main_stamp.clone(),
        );
        let c = the_call(&h, &calls);
        at(&mut h, day_of_turn(i));
        answer(&mut h, &c, "stop", usage(100), json!([said(&reply)]));
    }
    at(&mut h, 2);
    let calls = curate_for(
        &mut h,
        side,
        "t90",
        0,
        json!([user("What should we cook tonight?")]),
        json!({"path": if side.is_some() { "" } else { "simple" }}),
    );
    let c = the_call(&h, &calls);
    at(&mut h, 2);
    answer(&mut h, &c, "stop", usage(3800), json!([said("Soup.")]));
    at(&mut h, 2);
    let order = last_add(&h);
    h.fire(&order);
    let mut requests = Vec::new();
    while let Some(req) = h.summ.front().cloned() {
        assert!(requests.len() < 16, "the summarizer never rests");
        requests.push(Value::Object(req.body.clone()));
        h.answer("A summary of the talk by the river.", "stop");
    }
    let rebuild = json!({"summarizer": requests, "plan": h.rebuild_in(ROUND_E)});
    at(&mut h, 2);
    h.lane(
        "in_note",
        json!({"session_id": "s1"}),
        json!({}),
        json!({"notes": [{"id": "n1", "source": "probe", "text": NOTE, "scope": "turn"}]}),
    );
    at(&mut h, 2);
    let calls = curate_for(
        &mut h,
        main,
        "t91",
        0,
        json!([user("And what about tomorrow?")]),
        main_stamp,
    );
    let c = the_call(&h, &calls);
    let window = json!({"system": c.body.get("system").cloned().unwrap_or(Value::Null),
                        "messages": c.body.get("messages").cloned().unwrap_or(Value::Null),
                        "tool_scope": c.body.get("tool_scope").cloned().unwrap_or(Value::Null)});
    (window, rebuild)
}

/// What a fast target beside a main target running `role` carries after
/// the move (OR-d58-2.target.60): every value its call's cut reads, with the
/// value that acted -- the short window and no note of the former fast way,
/// the rest of the call's values from the main target's set, which the
/// instance's role gave it -- and no value of the wall.
fn fast_profile(role: &str) -> Value {
    let mut p = call_values(role);
    p["keep_recent"] = json!(4);
    p["notes_share"] = json!(0);
    p["system"] = json!("full");
    p
}

/// The model cells of the compositions of this tree and the example set each
/// one's profile carries (contract point 13, OR-d58-2.target.60).
const MAIN_TARGETS: [(&str, &str); 4] = [
    ("talky/brain", "talky"),
    ("cogny/brain", "consult"),
    ("simple-agent/brain", "talky"),
    ("simple-agent/strong", "talky"),
];
const FAST_TARGETS: [&str; 3] = ["talky/quick", "talky/quick_spec", "talky/quick_bridge"];
const WALL_ONLY: [&str; 11] = [
    "keep_recent_tokens",
    "compress_at",
    "rebuild_to",
    "horizon",
    "tiers",
    "keep_rounds",
    "gc",
    "gc_scaffold",
    "stub_tools_after",
    "recall_push",
    "recall_budget",
];

/// What the compositions carry: each main target the whole set, each fast
/// target the values of its call (main set's call values, the short window,
/// no note) and no value of the wall, and no curator instance a value of a
/// set as its knob (OR-d58-2.target.60, which lifts .55).
fn composition_findings() -> Vec<String> {
    let mut out = Vec::new();
    let profile_of = |cell: &str| {
        read_json(&repo(&format!("templates/{cell}/config.json")))["params"]["window_profile"]
            .clone()
    };
    for (cell, set) in MAIN_TARGETS {
        let got = profile_of(cell);
        for (k, v) in value_set(set).as_object().cloned().unwrap_or_default() {
            if !v.is_null() && got[&k] != v {
                out.push(format!(
                    "{cell}: `{k}` is {} in its profile, the set {set} says {v}",
                    got[&k]
                ));
            }
        }
    }
    for cell in FAST_TARGETS {
        let got = profile_of(cell);
        for (k, v) in fast_profile("talky")
            .as_object()
            .cloned()
            .unwrap_or_default()
        {
            if k != "system" && got[&k] != v {
                out.push(format!(
                    "{cell}: `{k}` is {} in its profile, its call read {v}",
                    got[&k]
                ));
            }
        }
        for k in WALL_ONLY {
            if got.get(k).is_some() {
                out.push(format!("{cell}: carries the wall's value `{k}`"));
            }
        }
    }
    let keys: Vec<String> = value_set("talky")
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    for comp in ["talky", "cogny", "simple-agent"] {
        let policy = read_json(&repo(&format!("templates/{comp}/curator/config.json")))
            ["override_params"]["policy"]
            .clone();
        for k in keys.iter().map(String::as_str).chain(["role"]) {
            if policy.get(k).is_some() {
                out.push(format!("{comp}: the curator instance sets `{k}`"));
            }
        }
    }
    out
}

/// The rebuild as compared: the plan without its field `role`, the name of
/// a former role (R-58-15: no role is left to name); every other byte stays.
fn compared(rebuild: &Value) -> Value {
    let mut r = rebuild.clone();
    if let Some(plan) = r.get_mut("plan").and_then(Value::as_object_mut) {
        plan.remove("role");
    }
    r
}

/// Under the switch, the windows and rebuilds of the roles as they stand,
/// for every fixture that is missing. True when the switch is set.
fn record() -> bool {
    if std::env::var(BLESS).as_deref() != Ok("1") {
        return false;
    }
    for role in ROLES {
        let (path, rpath) = (fixture(role), rebuild_fixture(role));
        if path.is_file() && rpath.is_file() {
            continue;
        }
        let (w, r) = fixed_talk(&[("policy", "role", json!(role))], None, None, |_| {});
        std::fs::create_dir_all(path.parent().expect("a dir")).expect("fixture dir");
        if !path.is_file() {
            std::fs::write(&path, sj::to_string_pretty(&w).expect("json") + "\n")
                .expect("the fixture is written");
        }
        if !rpath.is_file() {
            std::fs::write(&rpath, sj::to_string_pretty(&r).expect("json") + "\n")
                .expect("the fixture is written");
        }
    }
    true
}

#[test]
fn gh_d58_target_the_moved_value_sets_cut_the_same_windows() {
    if !shipped() || record() {
        return;
    }
    let mut failures = Vec::new();
    let mut windows: Vec<(&str, String)> = Vec::new();
    let mut rebuilds: Vec<(&str, String)> = Vec::new();
    for role in ROLES {
        let (path, rpath) = (fixture(role), rebuild_fixture(role));
        for p in [&path, &rpath] {
            assert!(
                p.is_file(),
                "{role}: no fixture at {} -- recorded once with {BLESS}=1 before the move",
                p.display()
            );
        }
        let want = read_json(&path);
        let want_rebuild = compared(&read_json(&rpath));
        windows.push((role, canonical(&want)));
        rebuilds.push((role, canonical(&want_rebuild)));
        // The sets with a `summary` tier and a floor below the talk condense:
        // talky's token floor (`keep_recent_tokens` 80000) keeps the whole
        // talk raw, research has no `summary` tier.
        let asks = !want_rebuild["summarizer"]
            .as_array()
            .is_none_or(|a| a.is_empty());
        if asks != ["quick", "consult", "coding"].contains(&role) {
            failures.push(format!(
                "{role}: the recorded rebuild asks the summarizer: {asks} -- the talk does not \
                 reach the value set as it should"
            ));
        }
        let main_profile = {
            let mut p = value_set(role);
            p["system"] = json!("delta");
            p
        };
        let (got, got_rebuild) = fixed_talk(&[], Some("main"), Some("side"), |h| {
            announce_profile(h, "main", main_profile);
            announce_profile(h, "side", fast_profile(role));
        });
        let got_rebuild = compared(&got_rebuild);
        for part in ["system", "messages", "tool_scope"] {
            if canonical(&want[part]) != canonical(&got[part]) {
                failures.push(format!(
                    "{role}: `{part}` differs\n  role:    {}\n  profile: {}",
                    canonical(&want[part]),
                    canonical(&got[part])
                ));
            }
        }
        for part in ["summarizer", "plan"] {
            if canonical(&want_rebuild[part]) != canonical(&got_rebuild[part]) {
                failures.push(format!(
                    "{role}: the rebuild's `{part}` differs\n  role:    {}\n  profile: {}",
                    canonical(&want_rebuild[part]),
                    canonical(&got_rebuild[part])
                ));
            }
        }
    }
    // The guard: a value set that moved wrongly must change a byte, so the
    // five recorded windows and the five rebuilds are pairwise different.
    for (what, seen) in [("window", &windows), ("rebuild", &rebuilds)] {
        for (i, (a, x)) in seen.iter().enumerate() {
            for (b, y) in seen.iter().skip(i + 1) {
                if x == y {
                    failures.push(format!(
                        "guard: the recorded {what} of {a} and {b} are the same bytes"
                    ));
                }
            }
        }
    }
    failures.extend(composition_findings());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
