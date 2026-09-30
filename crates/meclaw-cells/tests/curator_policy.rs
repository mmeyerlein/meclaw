//! GH #892 -- the curator's window follows the policy of its role, and the
//! model trims it through the sidecar of its own answer.
//!
//! `curator@1.0.0` built one window for every model: the wall after the last
//! summary, `keep_recent` rounds raw. R-27-2 makes it a policy of knobs with a
//! preset per ROLE (`talky`, `consult`, `coding`, `research`), lets the model
//! release and pin blocks of its window by their short ids (the `window`
//! section, cut by the parent's splitter and handed back on `in_section`),
//! lets other hives pin through a door (`in_pin`), and has the curator answer
//! the collector's menu question with its own offer (`in_schemas`). What is
//! pinned here, one group per task of plan T:
//!
//! 1. the knobs and the roles -- the presets, a set knob wins, the 1.0.0 name;
//! 2. the rebuild per role -- tiers over segments, the usable window, the
//!    order in which a plan over its aim gives something up, the expiry;
//! 3. the short ids and TRIM -- every foreign block shows its id, a release is
//!    one line from the next rebuild on and never before, what stays;
//! 4. the sections, the pin door and the menu answer;
//! 5. the splitter a cogny grew for it, and the wiring on both composites.
//!
//! The hive runs in one process as `curator_cells.rs` runs it
//! (`support/curator_hive.rs`); the colony case is
//! `gh892_the_window_follows_its_role_and_the_models_trim.rs`.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};

/// The overrides a composite puts on its curator (`override_params`), as
/// `(cell, param, value)` for `Hive::with` -- so a lock measures the instance
/// that ships, not a hand-set copy of it.
fn instance(composite: &str) -> Vec<(String, String, Value)> {
    let marker = read_json(&repo(&format!("templates/{composite}/curator/config.json")));
    let mut out = Vec::new();
    for (cell, params) in marker["override_params"]
        .as_object()
        .cloned()
        .unwrap_or_default()
    {
        for (k, v) in params.as_object().cloned().unwrap_or_default() {
            out.push((cell.clone(), k, v));
        }
    }
    out
}

fn hive_of(composite: &str, more: &[(&str, &str, Value)]) -> Hive {
    let base = instance(composite);
    let mut over: Vec<(&str, &str, Value)> = base
        .iter()
        .map(|(c, k, v)| (c.as_str(), k.as_str(), v.clone()))
        .collect();
    over.extend(more.iter().cloned());
    Hive::with(&over)
}

fn id(el: &Value) -> String {
    short_id(el)
}

fn released(i: &str) -> String {
    format!("[#{i} released \u{2014} history_read(\"#{i}\")]")
}

// ================================================ 1. the knobs and the roles

const KNOBS: &str = "[KEEP_RECENT, COMPRESS_AT, REBUILD_TO, QUALITY_CAP, HORIZON, TIERS, \
                     SUMMARY_BUDGET, KEEP_ROUNDS, STUB_TOOLS_AFTER, BROADCAST_MODE, RECALL_PUSH, \
                     RECALL_BUDGET, SHORT_IDS]";

#[test]
fn each_role_has_its_presets() {
    if !shipped() {
        return;
    }
    // R-27-2 Pkt. 3 (OR-KY-T2); the role "" is the window of 1.0.0.
    let want = [
        (
            "",
            json!([
                12,
                0.5,
                0.0,
                0,
                "all",
                ["raw", "summary"],
                4000,
                0,
                0,
                "tail",
                false,
                0,
                false
            ]),
        ),
        (
            "talky",
            json!([
                40,
                0.5,
                0.35,
                120000,
                "day",
                ["raw", "summary", "none"],
                4000,
                2,
                20,
                "tail",
                true,
                200,
                true
            ]),
        ),
        (
            "consult",
            json!([
                10,
                0.5,
                0.35,
                80000,
                "task",
                ["raw", "summary"],
                3000,
                2,
                10,
                "tail",
                false,
                0,
                true
            ]),
        ),
        (
            "coding",
            json!([
                20,
                0.4,
                0.25,
                70000,
                "task",
                ["raw", "summary"],
                3000,
                3,
                20,
                "tail",
                false,
                0,
                true
            ]),
        ),
        (
            "research",
            json!([
                10,
                0.4,
                0.25,
                80000,
                "task",
                ["raw", "none"],
                2000,
                1,
                10,
                "tail",
                false,
                0,
                true
            ]),
        ),
    ];
    for (role, knobs) in want {
        let (got, err) = policy_scope(json!({"role": role}), KNOBS, Value::Null);
        assert_eq!(got, knobs, "the presets of role {role:?}");
        assert!(err.is_empty(), "{role:?}: {err}");
    }
    // The shipped instances run their roles.
    assert_eq!(
        instance("talky")
            .iter()
            .find(|(c, k, _)| c == "policy" && k == "role")
            .map(|(_, _, v)| v.clone()),
        Some(json!("talky"))
    );
    assert_eq!(
        instance("cogny")
            .iter()
            .find(|(c, k, _)| c == "policy" && k == "role")
            .map(|(_, _, v)| v.clone()),
        Some(json!("consult"))
    );
    // A name nobody knows runs as none, and says so.
    let (got, err) = policy_scope(json!({"role": "nobody"}), KNOBS, Value::Null);
    assert_eq!(got[0], json!(12));
    assert!(err.contains("unknown role 'nobody'"), "{err}");
}

#[test]
fn a_set_knob_beats_the_preset() {
    if !shipped() {
        return;
    }
    let (got, _) = policy_scope(
        json!({"role": "talky", "keep_recent": 5, "compress_at": "0.6", "tiers": "raw,none",
               "short_ids": "0", "horizon": ""}),
        KNOBS,
        Value::Null,
    );
    assert_eq!(got[0], json!(5), "a set number wins");
    assert_eq!(got[1], json!(0.6), "a numeric string is a number");
    assert_eq!(got[5], json!(["raw", "none"]), "a set tier list wins");
    assert_eq!(got[12], json!(false), "\"0\" switches a flag off");
    assert_eq!(got[4], json!("day"), "empty is not set: the preset stands");
    assert_eq!(got[3], json!(120000), "what is not set is the role's");
    // A form nobody knows keeps the preset, and says so.
    let (got, err) = policy_scope(
        json!({"role": "consult", "tiers": "raw,shredded"}),
        "TIERS",
        Value::Null,
    );
    assert_eq!(got, json!(["raw", "summary"]));
    assert!(err.contains("not understood"), "{err}");
}

#[test]
fn summary_chars_reads_as_summary_budget() {
    if !shipped() {
        return;
    }
    let probe = "[SUMMARY_BUDGET, SUMMARY_CHARS]";
    let (got, _) = policy_scope(json!({"summary_chars": 1234}), probe, Value::Null);
    assert_eq!(
        got,
        json!([1234, 1234]),
        "the 1.0.0 name is read (OR-KY-T1)"
    );
    let (got, _) = policy_scope(
        json!({"summary_budget": 900, "summary_chars": 1234}),
        probe,
        Value::Null,
    );
    assert_eq!(got, json!([900, 900]), "the new name wins");
    let (got, _) = policy_scope(json!({"role": "research"}), probe, Value::Null);
    assert_eq!(got, json!([2000, 2000]), "neither set: the role's");
    // And it is the bound a summary is held to.
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_chars", json!(10)),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    turn(
        &mut h,
        "s1",
        "t2",
        "q2",
        "a2",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let req = h.answer("far more than ten characters", "stop");
    assert!(
        req.body["system"]["instructions"]["text"]
            .as_str()
            .unwrap()
            .contains("at most 10 characters")
    );
    assert_eq!(h.rows("SELECT COUNT(*) FROM summaries")[0][0], json!(0));
}

// ============================================== 2. the rebuild per role

#[test]
fn talky_keeps_today_raw_yesterday_summarised_older_dropped() {
    if !shipped() {
        return;
    }
    clear_of_midnight(120);
    let mut h = hive_of("talky", &[("policy", "keep_recent", json!(1))]);
    h.row(
        days_ago(3, 12),
        "s",
        "d3",
        "user",
        &user("three days ago"),
        0,
    );
    h.row(
        days_ago(3, 13),
        "s",
        "d3",
        "assistant",
        &said("an old answer"),
        1,
    );
    h.row(
        days_ago(1, 9),
        "s",
        "y1",
        "user",
        &user("yesterday morning"),
        0,
    );
    h.row(
        days_ago(1, 10),
        "s",
        "y1",
        "assistant",
        &said("yes, morning"),
        1,
    );
    let call = h.curate("s", "t1", 0, json!([user("today")]), mode("Be brief."));
    h.tap(
        &call,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("today's answer")]),
    );
    h.fire(&last_add(&h));
    let req = h.answer("Yesterday: a morning chat.", "stop");
    assert_eq!(
        req.messages()[0]["text"],
        "user: yesterday morning\nassistant: yes, morning",
        "yesterday is condensed, and only yesterday"
    );
    let call = h.curate("s", "t2", 0, json!([user("again")]), mode("Be brief."));
    assert_eq!(
        texts(&call),
        vec![
            format!("[#{}] today", id(&user("today"))),
            "today's answer".to_string(),
            format!("[#{}] again", id(&user("again"))),
        ],
        "today raw; yesterday is the summary; three days ago is nothing"
    );
    let summary = call.body["system"]["history"]["summary"]["text"]
        .as_str()
        .expect("the summary rides as system.history.summary");
    assert!(summary.ends_with("Yesterday: a morning chat."), "{summary}");
    // The ledger forgets nothing: the old rows and blocks stand.
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM wall WHERE turn_id = 'd3'")[0][0],
        json!(2)
    );
    // A second rebuild the same day carries the summary on, unchanged.
    h.tap(
        &call,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
        json!([said("again, answered")]),
    );
    h.fire(&last_add(&h));
    assert!(h.summ.is_empty(), "nothing new to condense");
    let call = h.curate("s", "t3", 0, json!([user("third")]), mode("Be brief."));
    assert!(
        call.body.get("system").is_none(),
        "nothing moved in the system part: {:?}",
        call.body
    );
    assert_eq!(
        h.rows("SELECT path FROM slots WHERE path = 'history.summary'")
            .len(),
        1
    );
}

/// The day, measured without a clock: the pure planner with a chosen
/// `as_of`, a wall of three days and no summary yet.
#[test]
fn the_day_horizon_counts_calendar_days() {
    if !shipped() {
        return;
    }
    // 2026-09-29T08:00:00Z; the rows at noon of the 26th, the 28th, and 07:00
    // of the 29th.
    let as_of: i64 = 1_790_668_800_000;
    let day = 86_400_000_000i64;
    let noon26 = (as_of - 3 * 86_400_000 + 4 * 3_600_000) * 1000;
    let rows = json!([
        {"seq": noon26, "session_id": "s", "turn_id": "a", "iter": 0, "kind": "user",
         "hash": "a".repeat(64), "at": "2026-09-26T12:00:00.000000Z", "chars": 10},
        {"seq": noon26 + 2 * day, "session_id": "s", "turn_id": "b", "iter": 0, "kind": "user",
         "hash": "b".repeat(64), "at": "2026-09-28T12:00:00.000000Z", "chars": 10},
        {"seq": noon26 + 3 * day - 5 * 3_600_000_000, "session_id": "s", "turn_id": "c",
         "iter": 0, "kind": "user", "hash": "c".repeat(64), "at": "2026-09-29T07:00:00.000000Z",
         "chars": 10}
    ]);
    let probe = "window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'])";
    let (got, _) = policy_scope(
        json!({"role": "talky", "keep_recent": 1}),
        probe,
        json!({"rows": rows, "as_of": as_of}),
    );
    let todo: Vec<Value> = got["todo"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["turn_id"].clone())
        .collect();
    assert_eq!(todo, vec![json!("b")], "the 28th is yesterday: summarised");
    assert_eq!(
        got["plan"]["cover"],
        json!(rows[2]["seq"].as_i64().unwrap() - 1),
        "the 29th is today: raw from its first row on; the 26th is gone"
    );
}

/// Review focus 3 in the `task` horizon (OR-KY.T.11): a day without a single
/// `topic` mark still ends its segment at the day change. Measured without a
/// clock, like the day horizon above.
#[test]
fn a_task_segment_ends_at_the_day_change_without_marks() {
    if !shipped() {
        return;
    }
    // 2026-09-29T08:00:00Z; two rounds at 10:00 and 12:00 of the 28th, one at
    // 07:00 of the 29th, and no `topic` mark at all.
    let as_of: i64 = 1_790_668_800_000;
    let hour = 3_600_000_000i64;
    let seven29 = (as_of - 3_600_000) * 1000;
    let ten28 = seven29 - 21 * hour;
    let rows = json!([
        {"seq": ten28, "session_id": "s", "turn_id": "a", "iter": 0, "kind": "user",
         "hash": "a".repeat(64), "at": "2026-09-28T10:00:00.000000Z", "chars": 10},
        {"seq": ten28 + 2 * hour, "session_id": "s", "turn_id": "b", "iter": 0, "kind": "user",
         "hash": "b".repeat(64), "at": "2026-09-28T12:00:00.000000Z", "chars": 10},
        {"seq": seven29, "session_id": "s", "turn_id": "c", "iter": 0, "kind": "user",
         "hash": "c".repeat(64), "at": "2026-09-29T07:00:00.000000Z", "chars": 10}
    ]);
    let probe = "window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'])";
    let (got, _) = policy_scope(
        json!({"role": "consult", "keep_recent": 1}),
        probe,
        json!({"rows": rows, "as_of": as_of}),
    );
    let todo: Vec<Value> = got["todo"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["turn_id"].clone())
        .collect();
    assert_eq!(
        todo,
        vec![json!("a"), json!("b")],
        "yesterday's task is an older segment: summarised, though nobody marked its end"
    );
    assert_eq!(
        got["plan"]["cover"],
        json!(seven29 - 1),
        "today's round is the running segment: raw from its first row on"
    );
}

#[test]
fn consult_keeps_the_task_raw() {
    if !shipped() {
        return;
    }
    clear_of_midnight(120);
    let mut h = hive_of("cogny", &[("policy", "keep_recent", json!(1))]);
    let base = chrono::Utc::now() - chrono::Duration::seconds(60);
    let at = |s: i64| base + chrono::Duration::seconds(s);
    h.row(at(1), "s", "k1", "user", &user("task one"), 0);
    h.row(
        at(2),
        "s",
        "k1",
        "assistant",
        &said("task one, answered"),
        1,
    );
    h.mark(
        at(2),
        "s",
        "k1",
        "topic",
        r#"{"movement":"start","name":"task one"}"#,
    );
    h.row(at(3), "s", "k2", "user", &user("task one, more"), 0);
    h.row(at(4), "s", "k2", "assistant", &said("more, answered"), 1);
    h.row(at(5), "s", "k3", "user", &user("task two"), 0);
    h.row(
        at(6),
        "s",
        "k3",
        "assistant",
        &said("task two, answered"),
        1,
    );
    h.mark(
        at(6),
        "s",
        "k3",
        "topic",
        r#"{"movement":"start","name":"task two"}"#,
    );
    h.row(at(7), "s", "k4", "user", &user("task two, more"), 0);
    h.row(at(8), "s", "k4", "assistant", &said("more of two"), 1);
    let call = h.curate("s", "k5", 0, json!([user("task two again")]), mode("x"));
    h.tap(
        &call,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("ok")]),
    );
    h.fire(&last_add(&h));
    let req = h.answer("Task one: asked and answered.", "stop");
    assert_eq!(
        req.messages()[0]["text"],
        "user: task one\nassistant: task one, answered\nuser: task one, more\n\
         assistant: more, answered",
        "the older task is condensed"
    );
    let call = h.curate("s", "k6", 0, json!([user("more")]), mode("x"));
    let shown = texts(&call);
    assert_eq!(
        shown.first().map(String::as_str),
        Some(format!("[#{}] task two", id(&user("task two"))).as_str()),
        "the running task raw from its start, far beyond keep_recent: {shown:?}"
    );
    assert_eq!(shown.len(), 7, "{shown:?}");
}

#[test]
fn quality_cap_bounds_below_the_model_window() {
    if !shipped() {
        return;
    }
    // 70 000 of a million is nothing for the model, and more than half of
    // what a talky may use (R-27-2: vague from ~200-300 k on).
    let mut h = hive_of("talky", &[]);
    turn(
        &mut h,
        "s",
        "t1",
        "q",
        "a",
        json!({"tokens_prompt": 70000, "context_window": 1000000}),
    );
    assert_eq!(last_add(&h).body["emit_body"]["reason"], "compress");
    let mut h = hive_of("talky", &[]);
    turn(
        &mut h,
        "s",
        "t1",
        "q",
        "a",
        json!({"tokens_prompt": 50000, "context_window": 1000000}),
    );
    assert!(h.clock.is_empty(), "under the line: nothing ordered");
    // A cap above the model's own window: the model's wins (review focus 4).
    let mut h = hive_of("talky", &[]);
    turn(
        &mut h,
        "s",
        "t1",
        "q",
        "a",
        json!({"tokens_prompt": 17000, "context_window": 32000}),
    );
    assert_eq!(last_add(&h).body["emit_body"]["reason"], "compress");
    // The window the model reported is kept for the aim of the rebuild.
    assert_eq!(h.state("context_window"), "32000");
}

/// A talky over its aim: first the old tool results shrink, and only when
/// that is not enough does the summary go.
fn under_pressure(cap: i64) -> (Hive, String) {
    let mut h = hive_of(
        "talky",
        &[
            ("policy", "keep_recent", json!(2)),
            ("policy", "quality_cap", json!(cap)),
            ("policy", "keep_rounds", json!(1)),
        ],
    );
    h.row(days_ago(1, 9), "s", "y1", "user", &user("yesterday"), 0);
    h.row(days_ago(1, 10), "s", "y1", "assistant", &said("yes"), 1);
    let base = chrono::Utc::now() - chrono::Duration::seconds(30);
    let at = |s: i64| base + chrono::Duration::seconds(s);
    let big = format!("{}\nsecond line", "A".repeat(3000));
    h.row(at(1), "s", "r1", "user", &user("look twice"), 0);
    h.row(at(2), "s", "r1", "tool_call", &tool_call("c1", "look"), 0);
    let (_, first) = h.row(at(3), "s", "r1", "tool_result", &tool_result("c1", &big), 0);
    h.row(at(4), "s", "r1", "assistant", &said("seen one"), 1);
    h.row(at(5), "s", "r2", "user", &user("again"), 0);
    h.row(at(6), "s", "r2", "tool_call", &tool_call("c2", "look"), 0);
    h.row(
        at(7),
        "s",
        "r2",
        "tool_result",
        &tool_result("c2", &"B".repeat(3000)),
        0,
    );
    h.row(at(8), "s", "r2", "assistant", &said("seen two"), 1);
    let call = h.curate("s", "r3", 0, json!([user("now")]), mode("x"));
    h.tap(
        &call,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("ok")]),
    );
    h.fire(&last_add(&h));
    if !h.summ.is_empty() {
        h.answer("Yesterday, briefly.", "stop");
    }
    (h, first)
}

#[test]
fn old_tool_results_shrink_before_segments() {
    if !shipped() {
        return;
    }
    clear_of_midnight(120);
    let r1 = id(&tool_result(
        "c1",
        &format!("{}\nsecond line", "A".repeat(3000)),
    ));
    let r2 = id(&tool_result("c2", &"B".repeat(3000)));
    // Room enough: nothing shrinks.
    let (h, _) = under_pressure(100_000);
    assert_eq!(h.plan()["shrunk"], json!([]));
    // Over the aim by one tool result: the older one shrinks, the summary stays.
    let (mut h, first) = under_pressure(5800);
    assert_eq!(h.plan()["shrunk"], json!([r1.clone()]));
    assert_ne!(h.plan()["summary"], json!(""), "the summary is kept");
    let call = h.curate("s", "r4", 0, json!([user("next")]), mode("x"));
    let shown = texts(&call);
    let one = format!(
        "[#{r1}] {} ... [shortened \u{2014} history_read(\"#{r1}\")]",
        "A".repeat(120)
    );
    assert!(shown.contains(&one), "{shown:?}");
    assert!(
        shown.contains(&format!("[#{r2}] {}", "B".repeat(3000))),
        "the newest tool iteration stays raw"
    );
    assert_eq!(
        h.rows(&format!("SELECT body FROM blocks WHERE hash = '{first}'"))[0][0],
        json!(canonical(&tool_result(
            "c1",
            &format!("{}\nsecond line", "A".repeat(3000))
        ))),
        "the block itself is untouched"
    );
    assert_eq!(listed_messages(&h, &call), call.messages());
    // Still over after that: the summary is given up, and said.
    let (h, _) = under_pressure(3000);
    assert_eq!(h.plan()["shrunk"], json!([r1]));
    assert_eq!(h.plan()["summary"], json!(""));
    assert!(
        h.stderr
            .iter()
            .any(|l| l.contains("the summary is given up")),
        "{:?}",
        h.stderr
    );
}

#[test]
fn unused_tools_become_stubs_with_an_empty_schema() {
    if !shipped() {
        return;
    }
    let look = json!({"type": "function", "function": {"name": "look",
        "description": "Look at a thing.\nIn detail.",
        "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}}});
    let unused = json!({"type": "function", "function": {"name": "unused",
        "description": format!("Never called. {}", "z".repeat(3000)),
        "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}}});
    let mut h = hive_of(
        "talky",
        &[
            ("policy", "keep_recent", json!(1)),
            ("policy", "stub_tools_after", json!(2)),
            ("policy", "quality_cap", json!(1000)),
        ],
    );
    h.lane(
        "in_slots",
        json!({}),
        json!({}),
        json!({"system": {"tools": {"$replace": true,
                                    "look": {"text": look.to_string()},
                                    "unused": {"text": unused.to_string()}}}}),
    );
    turn(&mut h, "s", "t1", "a", "b", json!({}));
    turn(&mut h, "s", "t2", "c", "d", json!({}));
    let call = h.curate(
        "s",
        "t3",
        1,
        json!([user("e"), tool_call("k", "look"), tool_result("k", "r")]),
        mode("x"),
    );
    h.tap(
        &call,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("f")]),
    );
    h.fire(&last_add(&h));
    assert_eq!(h.plan()["stubs"], json!(["unused"]));
    let call = h.curate("s", "t4", 0, json!([user("g")]), mode("x"));
    let tools = &call.body["system"]["tools"];
    let stub: Value =
        meclaw_core::serde_json::from_str(tools["unused"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        stub["function"]["parameters"],
        json!({"type": "object", "properties": {}}),
        "an empty object schema: a provider refuses a declaration without one"
    );
    let described = stub["function"]["description"].as_str().unwrap();
    assert!(
        described.starts_with("Never called. zzz")
            && described.ends_with(" ...")
            && described.len() < 130,
        "{described}"
    );
    assert_eq!(
        tools["look"]["text"].as_str().unwrap(),
        look.to_string(),
        "a tool called in the newest rounds keeps its declaration"
    );
    // The stub is a block the record names.
    let id = call.hop["curator_call"].as_str().unwrap();
    let listed = h.rows(&format!(
        "SELECT b.body FROM call_blocks cb JOIN blocks b ON b.hash = cb.hash \
         WHERE cb.call_id = '{id}' AND b.kind = 'system'"
    ));
    assert!(
        listed.iter().any(
            |r| r[0].as_str().unwrap().contains("\"path\":\"tools.unused\"")
                && r[0].as_str().unwrap().contains("properties\\\": {}")
        ),
        "{listed:?}"
    );
}

#[test]
fn an_expired_block_becomes_its_id() {
    if !shipped() {
        return;
    }
    let past = (chrono::Utc::now() - chrono::Duration::minutes(5))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let result = {
        let mut r = tool_result("c1", "the weather now: sunny");
        r["valid_until"] = json!(past);
        r
    };
    let rid = id(&result);
    let mut h = hive_of("talky", &[]);
    let call = h.curate(
        "s",
        "t1",
        1,
        json!([user("weather"), tool_call("c1", "weather"), result.clone()]),
        mode("x"),
    );
    h.tap(&call, "stop", json!({}), json!([said("sunny")]));
    let call = h.curate("s", "t2", 0, json!([user("and now")]), mode("x"));
    assert!(
        texts(&call).contains(&format!("[#{rid}] the weather now: sunny")),
        "not before the rebuild: {:?}",
        texts(&call)
    );
    h.tap(
        &call,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("?")]),
    );
    h.fire(&last_add(&h));
    let call = h.curate("s", "t3", 0, json!([user("x")]), mode("x"));
    let shown = call.messages();
    let at = shown
        .iter()
        .position(|m| m["type"] == "tool_result")
        .expect("the result, with its call");
    assert_eq!(shown[at]["text"], format!("[#{rid} expired]"));
    assert!(shown[at].get("valid_until").is_none());
    assert_eq!(shown[at]["id"], "c1", "still the answer to its call");
    assert_eq!(listed_messages(&h, &call), shown);
}

// ============================================ 3. the short ids and TRIM

#[test]
fn every_message_block_shows_its_short_id() {
    if !shipped() {
        return;
    }
    let mut h = hive_of("talky", &[]);
    let first = turn(&mut h, "s1", "t1", "hello", "hi", json!({}));
    let episodes: Vec<String> = h
        .routed("turn_write")
        .iter()
        .map(|m| m.messages()[0]["text"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(episodes, vec!["hello", "hi"], "an episode carries no id");
    let call = h.curate(
        "s1",
        "t2",
        1,
        json!([
            user("look"),
            tool_call("c1", "look"),
            tool_result("c1", "seen")
        ]),
        mode("Be brief."),
    );
    let shown = call.messages();
    assert_eq!(shown[0]["text"], format!("[#{}] hello", id(&user("hello"))));
    assert_eq!(shown[1]["text"], "hi", "the model's own answer shows no id");
    assert_eq!(shown[2]["text"], format!("[#{}] look", id(&user("look"))));
    assert_eq!(
        shown[3],
        tool_call("c1", "look"),
        "a tool call is the call itself"
    );
    assert_eq!(
        shown[4]["text"],
        format!("[#{}] seen", id(&tool_result("c1", "seen")))
    );
    // The ledger rebuilds what left, byte for byte, from blocks it holds --
    // each block once.
    assert_eq!(listed_messages(&h, &call), shown);
    assert!(
        h.rows("SELECT hash FROM blocks GROUP BY hash HAVING COUNT(*) > 1")
            .is_empty()
    );
    assert_eq!(
        h.rows(&format!(
            "SELECT body FROM blocks WHERE hash LIKE '{}%' AND kind = 'user'",
            id(&user("hello"))
        ))[0][0],
        json!(canonical(&user("hello"))),
        "the id is the wall block's"
    );
    // A warm cache: the second request begins with the first one's messages.
    let before = first.messages();
    assert_eq!(&shown[..before.len()], before.as_slice());
    // Without a role: no ids (the window of 1.0.0).
    let mut h = Hive::new();
    turn(&mut h, "s1", "t1", "hello", "hi", json!({}));
    let call = h.curate("s1", "t2", 0, json!([user("x")]), Value::Null);
    assert_eq!(texts(&call), vec!["hello", "hi", "x"]);
}

#[test]
fn a_released_id_is_one_line_at_the_next_rebuild_not_before() {
    if !shipped() {
        return;
    }
    let big = user("the big result, please");
    let bid = id(&big);
    let mut h = hive_of("talky", &[]);
    let c1 = h.curate("s1", "t1", 0, json!([big.clone()]), mode("x"));
    h.tap(&c1, "stop", json!({}), json!([said("here")]));
    let c2 = h.curate("s1", "t2", 0, json!([user("next")]), mode("x"));
    h.tap(
        &c2,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("done")]),
    );
    h.section(&c2, "window", json!({"release": [format!("#{bid}")]}));
    // Not before: the window between two rebuilds only grows at its end.
    let c3 = h.curate("s1", "t3", 0, json!([user("third")]), mode("x"));
    assert_eq!(texts(&c3)[0], format!("[#{bid}] the big result, please"));
    assert_eq!(
        &c3.messages()[..c2.messages().len()],
        c2.messages().as_slice()
    );
    h.tap(
        &c3,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
        json!([said("three")]),
    );
    h.fire(&last_add(&h));
    // At the next rebuild: one line, where the block stood.
    let c4 = h.curate("s1", "t4", 0, json!([user("fourth")]), mode("x"));
    assert_eq!(texts(&c4)[0], released(&bid));
    assert_eq!(c4.messages()[0]["origin"], "user");
    assert_eq!(texts(&c4)[1], "here", "the answer beside it stays");
    let full = h.rows(&format!("SELECT hash FROM wall WHERE hash LIKE '{bid}%'"))[0][0]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        h.rows(&format!("SELECT body FROM blocks WHERE hash = '{full}'"))[0][0],
        json!(canonical(&big)),
        "the ledger keeps the block byte for byte"
    );
    assert_eq!(listed_messages(&h, &c4), c4.messages());
    let actions = h.rows(&format!(
        "SELECT actions FROM calls WHERE call_id = '{}'",
        c4.hop["curator_call"].as_str().unwrap()
    ));
    assert!(
        actions[0][0].as_str().unwrap().contains("rebuild:cache"),
        "{actions:?}"
    );
}

#[test]
fn identity_cannot_be_released() {
    if !shipped() {
        return;
    }
    let mut h = hive_of("talky", &[]);
    h.lane(
        "in_pack",
        json!({}),
        json!({}),
        json!({"system": {"identity": {"text": "You are T."}}}),
    );
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    let ident = id(&json!({"path": "identity", "text": "You are T."}));
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("x"));
    h.tap(
        &c,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("d")]),
    );
    // The identity, and a block of the round the section was said in
    // (review focus 1).
    h.section(&c, "window", json!({"release": [ident, id(&user("c"))]}));
    h.fire(&last_add(&h));
    assert_eq!(h.plan()["released"], json!([]));
    let said = h.stderr.join("");
    assert!(
        said.contains("identity, persona and instructions stay"),
        "{said}"
    );
    assert!(said.contains("the round it was said in stays"), "{said}");
    let c = h.curate("s1", "t3", 0, json!([user("e")]), mode("x"));
    assert!(texts(&c).contains(&format!("[#{}] c", id(&user("c")))));
    assert_eq!(
        h.rows("SELECT path FROM slots WHERE path = 'identity'")
            .len(),
        1
    );
}

#[test]
fn a_pinned_block_survives_the_tier_cut() {
    if !shipped() {
        return;
    }
    clear_of_midnight(120);
    let keep = user("keep me, the owner's rule");
    let mut h = hive_of("talky", &[("policy", "keep_recent", json!(1))]);
    h.row(days_ago(3, 12), "s", "d3", "user", &keep, 0);
    h.row(
        days_ago(3, 12) + chrono::Duration::minutes(1),
        "s",
        "d3",
        "user",
        &user("drop me"),
        0,
    );
    let c = h.curate("s", "t1", 0, json!([user("today")]), mode("x"));
    assert_eq!(
        texts(&c)[0],
        format!("[#{}] keep me, the owner's rule", id(&keep))
    );
    h.tap(
        &c,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("ok")]),
    );
    h.section(&c, "window", json!({"pin": [id(&keep)]}));
    h.fire(&last_add(&h));
    let c = h.curate("s", "t2", 0, json!([user("later")]), mode("x"));
    let shown = texts(&c);
    assert_eq!(
        shown[0],
        format!("[#{}] keep me, the owner's rule", id(&keep)),
        "a pin beats the tier: three days old and still raw"
    );
    assert!(!shown.iter().any(|t| t.contains("drop me")), "{shown:?}");
    assert_eq!(
        h.rows("SELECT source, until FROM pins"),
        vec![vec![json!("model"), json!("")]]
    );
    // Released, it is gone at the next rebuild: its tier says nothing.
    h.tap(
        &c,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
        json!([said("ok")]),
    );
    h.section(&c, "window", json!({"release": [id(&keep)]}));
    h.fire(&last_add(&h));
    let c = h.curate("s", "t3", 0, json!([user("then")]), mode("x"));
    assert!(!texts(&c).iter().any(|t| t.contains(&id(&keep))));
    assert!(h.rows("SELECT hash FROM pins").is_empty());
}

#[test]
fn an_unknown_id_is_ignored() {
    if !shipped() {
        return;
    }
    let mut h = hive_of("talky", &[]);
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("x"));
    h.tap(
        &c,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("d")]),
    );
    h.section(
        &c,
        "window",
        json!({"release": ["#ffffffffffff", "not an id"], "pin": ["#000000000000"]}),
    );
    h.fire(&last_add(&h));
    assert_eq!(h.plan()["released"], json!([]));
    assert!(h.rows("SELECT hash FROM pins").is_empty());
    let said = h.stderr.join("");
    assert!(said.contains("#ffffffffffff: no such block"), "{said}");
    assert!(said.contains("#000000000000: no such block"), "{said}");
    assert!(said.contains("'not an id' is not a block id"), "{said}");
    let next = h.curate("s1", "t3", 0, json!([user("e")]), mode("x"));
    assert_eq!(
        texts(&next)[..4],
        [
            format!("[#{}] a", id(&user("a"))),
            "b".to_string(),
            format!("[#{}] c", id(&user("c"))),
            "d".to_string()
        ]
    );
}

// ============================ 4. the sections, the pin door, the menu

#[test]
fn memory_section_leaves_unchanged_and_marks_its_topic() {
    if !shipped() {
        return;
    }
    let payload = json!({"nothing_new": false, "facts": [{"subject": "user", "claim": "Berlin"}],
                         "topic": {"movement": "start", "name": "where the user lives"}});
    let mut h = hive_of("talky", &[]);
    let c = h.curate(
        "s1",
        "t1",
        0,
        json!([user("ich wohne in Berlin")]),
        mode("x"),
    );
    h.out.clear();
    h.section(&c, "memory", payload.clone());
    let out = h.routed("sidecar");
    assert_eq!(out.len(), 1, "one message out: {:?}", h.out);
    assert_eq!(
        Value::Object(out[0].body.clone()),
        json!({"messages": [], "section": "memory", "payload": payload}),
        "the splitter's body, unchanged"
    );
    assert_eq!(out[0].hop["section"], "memory");
    assert_eq!(
        out[0].context["session_id"], "s1",
        "the answer's context rides on"
    );
    // OR-KY-71: the `topic` mark is the canonical JSON of movement and name
    // (keys sorted, no whitespace) -- the form of the `gap` mark -- so a reader
    // of the ledger learns WHICH topic is open, not only that one moved.
    let started = r#"{"movement":"start","name":"where the user lives"}"#;
    assert_eq!(
        h.rows("SELECT kind, value, session_id, turn_id FROM marks"),
        vec![vec![
            json!("topic"),
            json!(started),
            json!("s1"),
            json!("t1")
        ]]
    );
    // A core keeps no memory: the mark, and nothing out.
    let mut h = hive_of("cogny", &[]);
    let c = h.curate("s1", "t1", 0, json!([user("q")]), mode("x"));
    h.out.clear();
    h.section(&c, "memory", payload);
    assert!(h.routed("sidecar").is_empty(), "{:?}", h.out);
    // A topic the section does not name is marked with an empty name.
    h.section(
        &c,
        "memory",
        json!({"nothing_new": true, "facts": [], "topic": {"movement": "end"}}),
    );
    assert_eq!(
        h.rows("SELECT kind, value FROM marks ORDER BY seq"),
        vec![
            vec![json!("topic"), json!(started)],
            vec![json!("topic"), json!(r#"{"movement":"end","name":""}"#)]
        ]
    );
    // A section that is none of the curator's is dropped, and said.
    h.section(&c, "fact", json!({"payload": "x"}));
    assert!(h.stderr.join("").contains("none of the curator's"));
}

#[test]
fn window_section_writes_marks() {
    if !shipped() {
        return;
    }
    let mut h = hive_of("talky", &[]);
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("x"));
    let a = id(&user("a"));
    let section = json!({"release": [format!("[#{a}]"), "#GGGGGGGGGGGG"], "pin": [a.clone()]});
    h.section(&c, "window", section.clone());
    assert_eq!(
        h.rows("SELECT kind, value, session_id, turn_id FROM marks ORDER BY seq"),
        vec![
            vec![json!("release"), json!(a.clone()), json!("s1"), json!("t2")],
            vec![json!("pin"), json!(a.clone()), json!("s1"), json!("t2")]
        ],
        "releases before pins: in one block, the pin is the later word"
    );
    assert!(h.stderr.join("").contains("is not a block id"));
    // The same section twice writes nothing twice.
    h.section(&c, "window", section);
    assert_eq!(h.rows("SELECT COUNT(*) FROM marks")[0][0], json!(2));
    // A gap is its own cell's since GH #895: `./push` marks it, once, in its
    // own form -- the intake's door does not take the section.
    h.section(&c, "gap", json!({"query": "the owner's birthday"}));
    let gaps = h.rows("SELECT value FROM marks WHERE kind = 'gap'");
    assert_eq!(gaps.len(), 1, "one gap, one mark: {gaps:?}");
    let gap: Value = meclaw_core::serde_json::from_str(gaps[0][0].as_str().unwrap()).unwrap();
    assert_eq!(gap["text"], "the owner's birthday");
    // And pin + release of one id in one block: kept (review focus 2).
    h.tap(
        &c,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("d")]),
    );
    h.fire(&last_add(&h));
    assert_eq!(h.plan()["released"], json!([]));
    assert_eq!(h.rows("SELECT source FROM pins").len(), 1);
}

#[test]
fn in_pin_lands_under_pinned_source() {
    if !shipped() {
        return;
    }
    let mut h = hive_of("talky", &[]);
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    h.lane(
        "in_pin",
        json!({}),
        json!({}),
        json!({"pins": [
            {"text": "The owner is on holiday until Friday.", "source": "orga"},
            {"text": "x", "source": "model"},
            {"text": "", "source": "orga"},
            {"text": "y", "source": "orga", "until": "next week"}
        ]}),
    );
    let pin = json!({"type": "pin", "source": "orga",
                     "text": "The owner is on holiday until Friday."});
    let pid = id(&pin);
    assert_eq!(
        h.rows("SELECT source, until FROM pins"),
        vec![vec![json!("orga"), json!("")]]
    );
    assert_eq!(
        h.rows(&format!("SELECT kind FROM blocks WHERE hash LIKE '{pid}%'")),
        vec![vec![json!("pin")]]
    );
    let refused = h.stderr.join("");
    assert!(refused.contains("source 'model'"), "{refused}");
    assert!(refused.contains("a pin without text"), "{refused}");
    assert!(refused.contains("no RFC 3339 time"), "{refused}");
    // Warm: where it arrived, as a message; the system part is untouched.
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("x"));
    assert_eq!(
        texts(&c)[2],
        format!("[#{pid}] [pinned by orga] The owner is on holiday until Friday.")
    );
    assert!(
        c.body.get("system").and_then(|s| s.get("pinned")).is_none(),
        "the system part is untouched while the cache is warm"
    );
    assert_eq!(listed_messages(&h, &c), c.messages());
    // Rebuilt: in the system part, under its source.
    h.tap(
        &c,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("d")]),
    );
    h.fire(&last_add(&h));
    let c = h.curate("s1", "t3", 0, json!([user("e")]), mode("x"));
    let mut orga = Map::new();
    orga.insert(
        pid.clone(),
        json!({"text": "The owner is on holiday until Friday."}),
    );
    assert_eq!(
        c.body["system"]["pinned"],
        json!({"$replace": true, "orga": orga})
    );
    assert!(!texts(&c).iter().any(|t| t.contains("pinned by")));
    // Pinned again with an end: a new `until`, never a new place.
    h.lane(
        "in_pin",
        json!({}),
        json!({}),
        json!({"pins": [{"text": "The owner is on holiday until Friday.", "source": "orga",
                         "until": "2099-01-01T00:00:00Z"}]}),
    );
    assert_eq!(
        h.rows("SELECT until FROM pins"),
        vec![vec![json!("2099-01-01T00:00:00Z")]]
    );
}

#[test]
fn the_curator_answers_the_menu_ask_with_its_offer() {
    if !shipped() {
        return;
    }
    // A menu question before the first turn (review focus 5): the answer
    // needs no state.
    let mut h = hive_of("talky", &[]);
    h.lane(
        "in_schemas",
        json!({"tool_caller": "talky"}),
        json!({}),
        json!({"messages": [], "tools": ["web_search", "web_fetch"]}),
    );
    let out = h.routed("tool_schemas");
    assert_eq!(out.len(), 1, "{:?}", h.out);
    let a = &out[0];
    assert_eq!(a.hop["operation"], "schemas");
    assert_eq!(a.body["unknown"], json!(["web_search", "web_fetch"]));
    assert_eq!(a.hop["error_code"], "tool_unknown");
    let window = a.body["sidecar"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["section"] == "window")
        .cloned()
        .expect("the window section is offered");
    assert_eq!(window["required"], json!(false));
    assert_eq!(
        window["schema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["pin", "release"]
    );
    assert!(
        window["instruction"]
            .as_str()
            .unwrap()
            .contains("[#0123456789ab]")
    );
    // `*` asks for everything the curator declares and names nothing unknown.
    h.out.clear();
    h.lane(
        "in_schemas",
        json!({}),
        json!({}),
        json!({"messages": [], "tools": ["*"]}),
    );
    let a = &h.routed("tool_schemas")[0];
    assert_eq!(a.body["unknown"], json!([]));
    assert!(a.hop.get("error_code").is_none());
    // One list: an entry with `name` is a tool, one with `section` a section.
    let src = script_of("schemas");
    assert!(src.contains("\nCURATOR_OFFER = ["), "the one list");
}

/// The helpers two cells of the hive both carry are one text: two readings
/// of where a window starts, or of what an id is, would be two windows.
#[test]
fn the_plan_helpers_are_one_text() {
    if !shipped() {
        return;
    }
    let def = |script: &str, name: &str| -> String {
        let at = script
            .find(&format!("\ndef {name}("))
            .unwrap_or_else(|| panic!("no {name}"));
        let rest = &script[at + 1..];
        let end = rest[1..].find("\ndef ").map_or(rest.len(), |e| e + 1);
        rest[..end].trim_end().to_string()
    };
    let (i, p) = (script_of("intake"), script_of("policy"));
    for name in ["plan_cover", "norm_id"] {
        assert_eq!(def(&i, name), def(&p, name), "{name}: intake vs policy");
    }
}

// ================================ 5. the cogny splitter and the wiring

fn table(base: &str, rel: &str) -> EdgeTable {
    let params = read_json(&repo(rel))["params"].clone();
    let hp: HiveParams =
        meclaw_core::serde_json::from_value(params).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let spec: &EdgeSpec = spec;
        let abs = |ep: &str| match ep {
            "." => base.to_string(),
            other => format!("{base}/{}", other.trim_start_matches("./")),
        };
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(&spec.from)),
            to: Path::new(&abs(&spec.to)),
            condition: spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("{rel}: condition {src:?}: {e}"))
            }),
            modifier: spec.modifier.as_ref().map(|m| {
                meclaw_colony::cel_eval::parse_modifier(m)
                    .unwrap_or_else(|(k, e)| panic!("{rel}: modifier {k}: {e}"))
            }),
            is_default: spec.is_default,
            lane: spec.lane.clone(),
        });
    }
    t
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("object")
}

fn deliveries(t: &EdgeTable, from: &str, hop: Value) -> Vec<(String, Map<String, Value>)> {
    let hs = Headers::from_parts(
        map(json!({"session_id": "s1", "turn_id": "t1", "curator_call": "k"})),
        map(hop),
    );
    apply_edges(t, &Path::new(from), &hs)
        .into_iter()
        .map(|d| (d.target.as_str().to_string(), d.headers_out.hop.clone()))
        .collect()
}

fn targets(d: &[(String, Map<String, Value>)]) -> Vec<String> {
    d.iter().map(|(to, _)| to.clone()).collect()
}

#[test]
fn cognys_splitter_is_talkys() {
    if !shipped() {
        return;
    }
    let talky = read_json(&repo("templates/talky/splitter/config.json"));
    let cogny = read_json(&repo("templates/cogny/splitter/config.json"));
    assert_eq!(
        cogny["params"], talky["params"],
        "one script, one nothing-form: the copy is the original"
    );
    assert_eq!(cogny["contract"], talky["contract"]);
    // The core asks for the block its splitter cuts, and its curator renders
    // the same nothing-form.
    let collector = read_json(&repo("templates/cogny/collector/config.json"));
    assert_eq!(collector["override_params"]["assemble"]["sidecar"], "1");
    let curator = read_json(&repo("templates/cogny/curator/config.json"));
    assert_eq!(
        curator["override_params"]["intake"]["nothing_block"],
        cogny["params"]["nothing_block"]
    );
    assert!(
        curator["override_params"]["intake"]
            .get("pass_sections")
            .is_none(),
        "a core keeps no memory: nothing passes"
    );
    // The answer path, as the tree routes it.
    let t = table("/c", "templates/cogny/config.json");
    for finish in ["stop", "tool_calls", "length"] {
        let d: Vec<String> = targets(&deliveries(
            &t,
            "/c/brain",
            json!({"finish_reason": finish}),
        ))
        .into_iter()
        .filter(|to| to != "/c/curator")
        .collect();
        assert_eq!(
            d,
            vec!["/c/splitter"],
            "{finish}: the brain answers through the splitter"
        );
    }
    for finish in ["stop", "tool_calls"] {
        assert_eq!(
            targets(&deliveries(
                &t,
                "/c/splitter",
                json!({"finish_reason": finish})
            )),
            vec!["/c/dispatcher"]
        );
    }
    let d = deliveries(&t, "/c/splitter", json!({"finish_reason": "length"}));
    assert_eq!(targets(&d), vec!["/c/collector"]);
    assert_eq!(d[0].1["route"], "in_answer");
    // The curator's sections to the curator; since GH #916 every other one
    // leaves the core the way it leaves a talky, so an app that offered a
    // section hears it from the core too -- once, never also via the curator.
    for section in ["memory", "window", "gap"] {
        let d = deliveries(
            &t,
            "/c/splitter",
            json!({"route": "sidecar", "section": section}),
        );
        assert_eq!(targets(&d), vec!["/c/curator"], "{section}");
        assert_eq!(d[0].1["route"], "in_section");
    }
    for section in ["probe", "fact"] {
        let d = deliveries(
            &t,
            "/c/splitter",
            json!({"route": "sidecar", "section": section}),
        );
        assert_eq!(targets(&d), vec!["/c"], "{section}");
        assert_eq!(d[0].1["route"], "sidecar");
    }
}

#[test]
fn talkys_splitter_hands_the_curator_its_sections() {
    if !shipped() {
        return;
    }
    let t = table("/t", "templates/talky/config.json");
    for section in ["memory", "window", "gap"] {
        let d = deliveries(
            &t,
            "/t/splitter",
            json!({"route": "sidecar", "section": section}),
        );
        assert_eq!(targets(&d), vec!["/t/curator"], "{section}");
        assert_eq!(d[0].1["route"], "in_section");
    }
    for section in ["fact", "context", "correction"] {
        assert_eq!(
            targets(&deliveries(
                &t,
                "/t/splitter",
                json!({"route": "sidecar", "section": section})
            )),
            vec!["/t"],
            "{section}: the advise sections keep their way out"
        );
    }
    // The memory section leaves the curator on the talky's own sidecar port.
    let d = deliveries(
        &t,
        "/t/curator",
        json!({"route": "sidecar", "section": "memory"}),
    );
    assert_eq!(targets(&d), vec!["/t"]);
    // The menu: the collector's question reaches the curator beside the
    // parent, and the answer comes home under the answerer `curator`.
    let d = deliveries(&t, "/t/collector", json!({"route": "schemas"}));
    assert!(targets(&d).contains(&"/t/curator".to_string()), "{d:?}");
    assert!(targets(&d).contains(&"/t".to_string()), "{d:?}");
    let back = t_answer(&t, "/t");
    assert_eq!(
        back,
        (
            "/t/collector".to_string(),
            json!("in_menu"),
            json!("curator")
        )
    );
    let c = table("/c", "templates/cogny/config.json");
    let back = t_answer(&c, "/c");
    assert_eq!(
        back,
        (
            "/c/collector".to_string(),
            json!("in_menu"),
            json!("curator")
        )
    );
}

/// Where the curator's menu answer goes in `base`: target, lane, answerer.
fn t_answer(t: &EdgeTable, base: &str) -> (String, Value, Value) {
    let hs = Headers::from_parts(
        Map::new(),
        map(json!({"route": "tool_schemas", "operation": "schemas"})),
    );
    let d = apply_edges(t, &Path::new(&format!("{base}/curator")), &hs);
    assert_eq!(d.len(), 1, "one road for the answer");
    (
        d[0].target.as_str().to_string(),
        d[0].headers_out.hop["route"].clone(),
        d[0].headers_out.context["tool_answerer"].clone(),
    )
}
