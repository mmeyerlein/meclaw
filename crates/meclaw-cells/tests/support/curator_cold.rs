//! GH #1464 (D58 coldgc) -- the conversations the `gh_d58_cold_*` pins run
//! and the readings they share.
//!
//! WHY this file exists: every pin of the cold collection asks the same
//! thing of a different conversation -- the cache of a round ran out (the
//! clock strikes the order the last answer left, reason `cache`), what does
//! the window look like after it, and which marks stand in the ledger. The
//! conversation with garbage in it, the stroke and the readings are written
//! once here.
//!
//! The garbage of the conversation is of one class, so a pin can count it:
//! two tool rounds, of which `keep_rounds` 1 keeps the result of the younger
//! one raw -- the result of the older one is what a collection takes.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

#![allow(dead_code)]

use crate::curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The mark of a turn or a rebuild that says a cold collection ran.
pub const COLD: &str = "cold";

/// The knobs every cold pin sets beside the value set of its main target
/// (the profile `garbage_talk`/`long_talk` report): the collection on, one
/// tool round kept raw, and the knob under test.
pub fn cold_knobs(gc_on_cold: Option<&str>) -> Vec<(&'static str, &'static str, Value)> {
    let mut over = vec![
        ("policy", "gc", json!("1")),
        ("policy", "gc_scaffold", json!("1")),
        ("policy", "keep_rounds", json!(1)),
    ];
    if let Some(v) = gc_on_cold {
        over.push(("policy", "gc_on_cold", json!(v)));
    }
    over
}

/// The result text of the tool round `n`: long enough to be worth a cut.
pub fn result_text(n: u32) -> String {
    format!("file {n}: {}", "lorem ipsum dolor sit amet ".repeat(40))
}

/// One tool round of the turn `turn`: the words, the call of the tool, its
/// result, the final answer. Returns the result element.
pub fn tool_turn(t: &mut Talk, turn: &str, n: u32, words: &str, reply: &str) -> Value {
    let id = format!("call_tool_cold_{n:04}");
    let c0 = t.call(turn, 0, json!([user(words)]));
    t.answer(&c0, "tool_calls", json!([tool_call(&id, "read_file")]));
    let result = tool_result(&id, &result_text(n));
    let c1 = t.call(
        turn,
        1,
        json!([tool_call(&id, "read_file"), result.clone()]),
    );
    t.answer(&c1, "stop", json!([said(reply)]));
    result
}

/// What the person said and what was answered in [`garbage_talk`], in order.
pub const WORDS: [&str; 4] = [
    "My sister Anna moved to Hamburg.",
    "Read the first file, please.",
    "And now the second file, please.",
    "Thank you, that is all for now.",
];
pub const ANSWERS: [&str; 4] = [
    "Hamburg is lovely.",
    "The first file is read.",
    "The second file is read.",
    "You are welcome.",
];

/// Four turns with two tool rounds in the middle, far under every aim (the
/// usage of [`warm_usage`]), the main target running the example value set
/// `set` (`support/curator_profiles.rs`). Returns the conversation and the
/// two result elements, the older one first -- the older one is the garbage.
pub fn garbage_talk(set: &str, over: &[(&str, &str, Value)]) -> (Talk, Value, Value) {
    clear_of_midnight();
    let mut t = Talk::new(over).reporting(crate::curator_profiles::profile(set));
    t.turn("t1", WORDS[0], ANSWERS[0]);
    let old = tool_turn(&mut t, "t2", 1, WORDS[1], ANSWERS[1]);
    let young = tool_turn(&mut t, "t3", 2, WORDS[2], ANSWERS[2]);
    t.turn("t4", WORDS[3], ANSWERS[3]);
    (t, old, young)
}

/// The cache of the round runs out: the clock strikes the order the last
/// answer left. The order must be the one of an expiring cache.
pub fn cache_expires(t: &mut Talk) {
    let order =
        t.h.clock_order_for(ROUND_E)
            .unwrap_or_else(|| panic!("the last answer left an order: {:?}", t.h.stderr));
    assert_eq!(
        order.body["emit_body"]["reason"], "cache",
        "check of the setup: the order is the one of the cache: {:?}",
        t.h.stderr
    );
    t.h.fire(&order);
}

/// Whether a rebuild of any kind wrote a plan for the round.
pub fn rebuilt(t: &Talk) -> bool {
    t.h.rebuild_in(ROUND_E).get("as_of_ms").is_some()
}

/// The values of every mark of `kind` the ledger holds, oldest first.
pub fn marks_of(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null))
    .collect()
}

/// The `curate` marks that name a cold collection (`curate_mark` "cold").
pub fn cold_marks(h: &Hive) -> Vec<Value> {
    marks_of(h, "curate")
        .into_iter()
        .filter(|v| v["curate_mark"] == COLD)
        .collect()
}

/// The ids the plan of the round shows in their one-line form.
pub fn shrunk(t: &Talk) -> Vec<String> {
    t.h.plan_in(ROUND_E)["shrunk"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// The texts of one more call of the conversation: the window after
/// whatever the stroke did.
pub fn next_window(t: &mut Talk, turn: &str) -> Vec<String> {
    let c = t.call(turn, 0, json!([user("One more thing.")]));
    texts(&c)
}

/// The figures `rebuild` and `gc` of the hive's own `stats` door over the
/// last day and the next.
pub fn rebuild_and_gc_figures(h: &mut Hive) -> (Value, Value) {
    let now = chrono::Utc::now();
    let day = chrono::Duration::days(1);
    h.out.clear();
    h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": "cold:1",
               "stats_from": (now - day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
               "stats_to": (now + day).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
               "stats_kinds": "[\"rebuild\",\"gc\"]"}),
        json!({"messages": []}),
    );
    assert_eq!(h.out.len(), 1, "one answer: {:?}", h.out);
    let m = &h.out[0];
    assert_eq!(m.hop["error_code"], "", "{:?}", h.stderr);
    (
        m.body["stats"]["counts"]["rebuild"].clone(),
        m.body["stats"]["counts"]["gc"].clone(),
    )
}

/// Words long enough that three turns of them outweigh a small aim: about
/// 2900 characters, the story `n`.
pub fn long_words(n: u32) -> String {
    format!(
        "Story {n}: {}",
        "the garden behind the old house was full of apple trees ".repeat(50)
    )
}

/// The usage of the last answer of [`long_talk`] whose package names
/// `input_soft` -- the aim a rebuild of the round inherits is
/// `input_soft` x `rebuild_to` / `compress_at`. The measured prompt stays
/// under it, so the turn orders nothing but the cache.
pub fn package_usage(input_soft: i64) -> Value {
    json!({"tokens_prompt": 2000, "tokens_cached": 1000,
           "input_soft": input_soft, "input_hard": 10_000_000,
           "cost_in": 10, "cost_cached_in": 0,
           "cache_expires_at": "2099-01-01T00:00:00Z"})
}

/// Three turns of [`long_words`], then the two tool rounds and the last
/// turn of [`garbage_talk`]; the last answer comes with `last_usage`.
/// The main target runs the example value set `set`. Returns the
/// conversation and the older tool result (the garbage).
pub fn long_talk(set: &str, over: &[(&str, &str, Value)], last_usage: Value) -> (Talk, Value) {
    clear_of_midnight();
    let mut t = Talk::new(over).reporting(crate::curator_profiles::profile(set));
    for n in 1..=3 {
        t.turn(
            &format!("l{n}"),
            &long_words(n),
            &format!("I see, story {n}."),
        );
    }
    let old = tool_turn(&mut t, "t2", 1, WORDS[1], ANSWERS[1]);
    tool_turn(&mut t, "t3", 2, WORDS[2], ANSWERS[2]);
    let c = t.call("t4", 0, json!([user(WORDS[3])]));
    t.h.tap(&c, "stop", last_usage, json!([said(ANSWERS[3])]));
    (t, old)
}

/// The aim the policy names for a rebuild after an expired cache, in
/// tokens, given the aim the order of the cache inherited: the pure
/// function `cold_aim_of` of the shipped policy under `over`.
pub fn cold_aim(over: Value, inherited: i64) -> Value {
    let (got, err) = policy_scope(over, "cold_aim_of(ARGS)", json!(inherited));
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// Whether `line` stands in `window` in its own words: the text of a
/// message, as it is or behind the short id the window names a block by
/// (`[#<id>] `, `short_ids`).
pub fn stands(window: &[String], line: &str) -> bool {
    let tail = format!("] {line}");
    window
        .iter()
        .any(|w| w == line || (w.starts_with("[#") && w.ends_with(&tail)))
}
