//! GH #1095 -- "forget that ..." puts a topic aside; it does not delete it.
//!
//! The owner's ruling (R-TR-25): a member who says "forget what I told you
//! about X" wants X not brought up again unasked -- and still answered when
//! they ask for it themselves. Until this lock the close pass CLOSED what a
//! request named (`closure_source forget_request:<turn>`, `closed_at` on the
//! episodes, `expired_at` on the facts), and recall dropped every such row for
//! every asker: the member who asked "what did my neighbour owe me again?" a
//! week later got nothing.
//!
//! The locks, with invented people only:
//! - a request MUTES (`muted_at`, `mute_source`, `mute_words`) and never
//!   closes, and no search is too broad to mute;
//! - recall raises no muted row unasked -- the tier-0 bundle never, an ask
//!   only where its words carry every word of the mute -- and a lifted mute
//!   is an ordinary row again;
//! - the member taking the topic up again in a turn of their own lifts the
//!   mute (`unmuted_at`), in the close pass, deterministically; nobody else
//!   lifts it, and neither does a turn before the request or a second request;
//! - a request already muted is not honoured twice, and one the member takes
//!   up again in its own session is not muted at all;
//! - the correction idiom stays no request;
//! - a belief the night derives from a muted fact is muted with it;
//! - a store of the closing releases is converted: its request closures
//!   become mutes;
//! - every op the mute chain sends is one the store runs (M1b: a two-operator
//!   range was refused, and every close pass of a colony hung without a
//!   report);
//! - a word of the topic is named in another inflection too ("owes" /
//!   "owe"), for a question and for taking the topic up again;
//! - the topic the curator's push puts in front of an ambient ask is not the
//!   asker naming it;
//! - kfm M1c (review of M1b): a turn keeping the topic aside, or saying yes
//!   to the question which topic was meant, is no taking it up; delete and
//!   erase mute only where the closer cites them; a direct question names
//!   the topic in another form or in part; a mute never goes without words
//!   and a mute without words is never unanswerable; a refused upgrade never
//!   stops the pass; the mutes read is the live mutes; a sharpened muted fact
//!   keeps its mute; a request not muted says why (`mute_skipped`).
//!   The sentences of these locks are language data in
//!   `fixtures/mute_<code>.jsonl`, every case in every language.

#[path = "support/memory_hive_subject.rs"]
mod support;

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, shipped_script};
use support::*;

const CLOSE: &str = "../../templates/memory-hive/close-glue/config.json";
const RECALL: &str = "../../templates/memory-hive/recall/config.json";
const DREAM: &str = "../../templates/memory-hive/dream-glue/config.json";
const STORE: &str = "../../templates/memory-hive/store/config.json";
const EXTRACT: &str = "../../templates/memory-hive/extract-glue/config.json";
const MUTE_EN: &str = include_str!("fixtures/mute_en.jsonl");
const MUTE_DE: &str = include_str!("fixtures/mute_de.jsonl");
const AUDIENCE: &str = r#"["agent:a","member:e"]"#;
const MARK: &str = "forget_request:r1";
const ASKED: &str = "2026-02-10T09:00:00Z";
const REQUEST: &str = "Please forget what I told you about the money my neighbour owes me.";

fn config(path: &str) -> Value {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path);
    serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json")
}

fn tool_calls(out: &[Value]) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    for m in out {
        let phase = m["header"]["phase"]
            .as_str()
            .or_else(|| m["header"]["route"].as_str())
            .unwrap_or_default()
            .to_string();
        for x in m["messages"].as_array().into_iter().flatten() {
            if x["type"] == "tool_call"
                && let Some(op) = x["text"]
                    .as_str()
                    .and_then(|t| serde_json::from_str::<Value>(t).ok())
            {
                found.push((phase.clone(), op));
            }
        }
    }
    found
}

/// Run python over blocks of a shipped script plus a program.
fn py(src: &str) -> String {
    let out = meclaw_testing::run_shipped_script(src, "{}");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn turn(id: &str, sender: &str, text: &str, at: &str) -> Value {
    let speaker = if sender == "user" {
        "member:e"
    } else {
        "agent:a"
    };
    json!({"id": id, "session_id": "s-2", "sender": sender, "speaker": speaker,
           "content": text, "happened_at": at, "audience_set": AUDIENCE,
           "channel": "direct:e"})
}

fn parked(kind: &str, rows: Value) -> Value {
    json!({"key": "close:s-2", "kind": kind, "created_at": "2026-01-01T00:00:00Z",
           "payload": json!({"rows": rows, "truncated": false}).to_string()})
}

/// One close pass over `turns`, with the mutes the meeting read parked.
fn close(turns: Vec<Value>, mutes: Value) -> Vec<Value> {
    let rows = json!([
        parked("turns", Value::Array(turns)), parked("facts", json!([])),
        parked("topics", json!([])), parked("exceptions", json!([])),
        parked("mutes", mutes),
        {"key": "close:s-2", "kind": "verdict", "created_at": "2026-01-01T00:00:00Z",
         "payload": json!({"nothing_to_add": true, "forget": [], "close_group": 0}).to_string()}
    ]);
    emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": {"mem_phase": "close-apply", "session_id": "s-2",
                                   "close_group": "0"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result", "text": rows.to_string()}]
        }),
    )
}

fn back(phase: &str, op: &str, rows: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": {"mem_phase": phase, "session_id": "s-2"},
                       "hop": {"operation": op}},
            "messages": [{"origin": "tool", "type": "tool_result", "text": rows.to_string()}]
        }),
    )
}

/// A bundle reply of the store (GH #295): one `tool_result` turn per op, by
/// its tool_call id, and the `results[]` beside them.
fn back_bundle(phase: &str, legs: &[(&str, Value)]) -> Vec<Value> {
    emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": {"mem_phase": phase, "session_id": "s-2"},
                       "hop": {"operation": "bundle", "bundle_errors": 0}},
            "messages": legs.iter().map(|(id, rows)| json!(
                {"origin": "tool", "type": "tool_result", "id": id,
                 "text": rows.to_string()})).collect::<Vec<_>>(),
            "results": legs.iter().map(|(id, _)| json!(
                {"tool_call_id": id, "operation": "select"})).collect::<Vec<_>>()
        }),
    )
}

/// The answer of the exceptions bundle with `mute_up` as the upgrade read.
fn exceptions_answer(mute_up: Value) -> Vec<Value> {
    back_bundle(
        "exceptions",
        &[
            ("c-exceptions", json!([])),
            ("c-mutes", stored_mute()),
            ("c-mute-up", mute_up),
        ],
    )
}

fn report(out: &[Value]) -> Value {
    out.iter()
        .find(|m| m["header"]["route"] == "close_report" && m["header"]["phase"] == "close")
        .unwrap_or_else(|| panic!("no close report: {out:?}"))["header"]
        .clone()
}

fn stored_mute() -> Value {
    json!([{"mute_source": MARK, "mute_words": "money neighbour owes",
            "muted_at": ASKED, "unmuted_at": null}])
}

fn lifts(out: &[Value]) -> Vec<Value> {
    tool_calls(out)
        .into_iter()
        .filter(|(p, _)| p == "unmuted")
        .map(|(_, o)| o)
        .collect()
}

// ─────────────────────────────────────────────────────────── a request mutes

#[test]
fn a_request_mutes_and_never_closes() {
    // the shipped params honour the member's own sentence (the fallback ships
    // on since a request mutes): every search carries who asked, when, and
    // the words of the thing
    let out = close(vec![turn("r1", "user", REQUEST, ASKED)], json!([]));
    let searches: Vec<(String, Value)> = tool_calls(&out)
        .into_iter()
        .filter(|(_, o)| o["operation"] == "search")
        .collect();
    assert_eq!(searches.len(), 4, "{searches:?}");
    for (phase, op) in &searches {
        assert!(
            phase.ends_with(&format!("|r1|{ASKED}|money,neighbour,owes")),
            "{phase}"
        );
        assert!(op.get("limit").is_none(), "a mute may be broad: {op}");
    }
    assert_eq!(report(&out)["mute_searched"], 1);

    // a hundred episodes are a hundred mutes -- nothing is too broad
    let hits = Value::Array((0..100).map(|i| json!({"id": format!("e{i}")})).collect());
    let marked = back(
        &format!("forget-e|r1|{ASKED}|money,neighbour,owes"),
        "search",
        hits,
    );
    let calls = tool_calls(&marked);
    let (_, mute) = calls
        .iter()
        .find(|(_, o)| o["operation"] == "update" && o["table"] == "episodes")
        .unwrap_or_else(|| panic!("no mute: {calls:?}"));
    assert_eq!(
        mute["where"]["id"]["in"].as_array().map(Vec::len),
        Some(100)
    );
    assert_eq!(
        mute["set"],
        json!({"muted_at": ASKED, "mute_source": MARK,
               "mute_words": "money neighbour owes", "unmuted_at": ""}),
        "a mute and nothing else -- no closed_at, no closure_source"
    );
    assert!(
        !calls.iter().any(|(_, o)| o["operation"] == "delete"),
        "{calls:?}"
    );
    assert_the_declaration_admits(CLOSE, &marked);

    // the facts of a muted episode and the beliefs made of them carry the mute
    let facts = back(
        &format!("forget-p|r1|{ASKED}|money,neighbour,owes"),
        "select",
        json!([{"id": "f-owes"}]),
    );
    let calls = tool_calls(&facts);
    for table in ["facts", "beliefs"] {
        let (_, op) = calls
            .iter()
            .find(|(_, o)| o["table"] == table)
            .unwrap_or_else(|| panic!("no {table} mute: {calls:?}"));
        assert_eq!(op["set"]["mute_source"], MARK, "{table}");
        assert!(op["set"].get("closure_source").is_none(), "{table}: {op}");
        assert!(op["set"].get("expired_at").is_none(), "{table}: {op}");
    }
}

#[test]
fn the_store_declares_the_mute_on_every_row_a_request_reaches() {
    let schema = &config(STORE)["params"]["schema"];
    for table in ["episodes", "facts", "beliefs"] {
        for col in ["muted_at", "mute_source", "mute_words", "unmuted_at"] {
            assert_eq!(schema[table][col], "text", "{table}.{col}");
        }
    }
}

// ────────────────────────────────────────────────── recall: asked, not raised

/// The shipped gate of recall over a few rows, for an ask (`query`) on a tier
/// (`legs` is the tier-0 bundle, `t1-fan` an ask of tier 1).
fn gate(phase: &str, query: &str, rows: Value) -> String {
    let script = shipped_script(RECALL);
    let program = format!(
        "import re, json\nphase = {}\nquery = {}\nctx = {{'recall_query': query}}\ndef visible(a, c):\n    return True\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\nprint(json.dumps([visible_row(r) for r in json.loads({})]))",
        serde_json::to_string(phase).expect("phase"),
        serde_json::to_string(query).expect("query"),
        block_of(&script, "FORGET_SOURCE"),
        block_of(&script, "def forgotten"),
        block_of(&script, "def muted"),
        block_of(&script, "def words_of"),
        block_of(&script, "PUSH_PREFIX"),
        block_of(&script, "def asker_words"),
        block_of(&script, "ASKED_WORDS"),
        block_of(&script, "def names_word"),
        block_of(&script, "def asked_for"),
        block_of(&script, "def visible_row"),
        serde_json::to_string(&rows.to_string()).expect("rows"),
    );
    py(&program)
}

fn recall_rows() -> Value {
    json!([
        {"muted_at": ASKED, "mute_source": MARK, "mute_words": "money neighbour owes"},
        {"muted_at": ASKED, "mute_source": MARK, "mute_words": "money neighbour owes",
         "unmuted_at": "2026-03-01T09:00:00Z"},
        {"closure_source": null}
    ])
}

#[test]
fn a_muted_row_answers_a_question_that_names_it() {
    assert_eq!(
        gate(
            "t1-fan",
            "How much money does my neighbour still owe me?",
            recall_rows()
        ),
        "[true, true, true]"
    );
}

#[test]
fn a_muted_row_is_never_raised_unasked() {
    // an ambient ask about something else (the per-turn ask of the curator
    // carries the member's own words), and one naming only part of the topic
    for query in [
        "I went to the garden today.",
        "My neighbour waved at me this morning.",
    ] {
        assert_eq!(
            gate("t1-fan", query, recall_rows()),
            "[false, true, true]",
            "{query}"
        );
    }
    // the tier-0 bundle is built per turn with no question: never, even where
    // the turn's words would name the topic
    assert_eq!(
        gate(
            "legs",
            "How much money does my neighbour still owe me?",
            recall_rows()
        ),
        "[false, true, true]"
    );
}

/// kfm M1 open point (OR-kfm.M1.2): the curator's push asks with
/// `[topic: <open topic>; mentioned: ...] <the person's words>`. The open
/// topic comes from the model's memory section, not from the asker -- a topic
/// whose name carries every word of the mute must not raise the muted row on
/// an ambient turn. The person's own words after it still ask.
#[test]
fn the_topic_the_push_adds_is_not_the_asker_naming_it() {
    for query in [
        "[topic: the money my neighbour owes] I went to the garden today.",
        "[topic: garden; mentioned: Money, Neighbour, Owes] What a lovely day.",
        "[refers to: money neighbour owes] Thanks!",
    ] {
        assert_eq!(
            gate("t1-fan", query, recall_rows()),
            "[false, true, true]",
            "{query}"
        );
    }
    assert_eq!(
        gate(
            "t1-fan",
            "[topic: garden] How much money does my neighbour still owe me?",
            recall_rows()
        ),
        "[true, true, true]"
    );
}

/// The tier-0 bundle end to end: a muted episode takes no slot.
#[test]
fn the_tier0_bundle_leaves_a_muted_episode_out() {
    let ep = |id: &str, content: &str, muted: bool| {
        let mut e = json!({"id": id, "session_id": "s-1", "sender": "user",
                           "speaker": "member:e", "content": content,
                           "happened_at": "2026-02-01T09:00:00Z",
                           "recorded_at": "2026-02-01T09:00:00Z",
                           "channel": "direct:e", "audience_set": AUDIENCE});
        if muted {
            e["muted_at"] = json!(ASKED);
            e["mute_source"] = json!(MARK);
            e["mute_words"] = json!("money neighbour owes");
        }
        e
    };
    let episodes = json!([
        ep("e1", "My neighbour owes me money, two hundred euros.", true),
        ep("e2", "The tomatoes in the garden are ripe.", false)
    ]);
    let out = emit_all(
        &shipped_script(RECALL),
        &json!({
            "header": {
                "context": {"mem_phase": "legs", "recall_id": "r1", "memory_tier": "0",
                            "recall_query": "money neighbour owes",
                            "recall_as_of": "2026-03-01T00:00:00Z",
                            "recall_window_from": "", "recall_window_to": "",
                            "audience_now": ["agent:a", "member:e"], "channel": "direct:e",
                            "session_id": "s-3"},
                "hop": {"operation": "bundle", "rows_affected": 2, "bundle_errors": 0}
            },
            "messages": [
                {"origin": "tool", "type": "tool_result", "id": "r-leg-episodes",
                 "text": episodes.to_string()},
                {"origin": "tool", "type": "tool_result", "id": "r-leg-beliefs", "text": "[]"},
                {"origin": "tool", "type": "tool_result", "id": "r-leg-foresight", "text": "[]"}
            ],
            "results": [
                {"tool_call_id": "r-leg-episodes", "operation": "select", "rows_affected": 2},
                {"tool_call_id": "r-leg-beliefs", "operation": "select", "rows_affected": 0},
                {"tool_call_id": "r-leg-foresight", "operation": "select", "rows_affected": 0}
            ]
        }),
    );
    let text = out
        .iter()
        .find(|m| m["header"]["route"] == "bundle")
        .unwrap_or_else(|| panic!("no bundle: {out:?}"))["messages"][0]["text"]
        .as_str()
        .expect("text")
        .to_string();
    assert!(text.contains("tomatoes"), "{text}");
    assert!(
        !text.contains("two hundred"),
        "the muted turn was raised: {text}"
    );
}

#[test]
fn every_recall_read_of_a_muted_table_reads_the_mute() {
    let script = shipped_script(RECALL);
    let mut reads = 0;
    for table in ["facts", "episodes", "beliefs"] {
        let key = format!("\"table\": \"{table}\"");
        for (at, _) in script.match_indices(&key) {
            let rest = &script[at..];
            let Some(cols) = rest.find("\"columns\": [") else {
                continue;
            };
            if cols > 900 {
                continue;
            }
            let list = &rest[cols..cols + rest[cols..].find(']').expect("list end")];
            assert!(
                list.contains("\"muted_at\"")
                    && list.contains("\"unmuted_at\"")
                    && list.contains("\"mute_words\""),
                "a {table} read without the mute: {list}"
            );
            reads += 1;
        }
    }
    assert!(reads >= 15, "{reads} reads");
}

// ─────────────────────────────────────────────────────────── taking it up

#[test]
fn the_member_taking_the_topic_up_again_lifts_the_mute() {
    let out = close(
        vec![turn(
            "t1",
            "user",
            "Did my neighbour ever pay back the money he owes me?",
            "2026-03-01T09:00:00Z",
        )],
        stored_mute(),
    );
    let lifted = lifts(&out);
    let tables: Vec<&str> = lifted
        .iter()
        .map(|o| o["table"].as_str().expect("table"))
        .collect();
    assert_eq!(tables, ["episodes", "facts", "beliefs"], "{lifted:?}");
    for op in &lifted {
        assert_eq!(op["operation"], "update");
        assert_eq!(op["where"]["mute_source"], MARK);
        assert_eq!(
            op["set"]
                .as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>()),
            Some(vec!["unmuted_at".to_string()]),
            "only the lift, `muted_at` stays as the record: {op}"
        );
    }
    assert_eq!(report(&out)["unmuted"], 1);
    assert_the_declaration_admits(CLOSE, &out);
}

/// The request said "owes"; the member taking the topic up says "owe". The
/// same word, one rule with recall's question (`names_word`).
#[test]
fn taking_the_topic_up_in_another_inflection_lifts_it() {
    let out = close(
        vec![turn(
            "t1",
            "user",
            "Does my neighbour still owe me that money?",
            "2026-03-01T09:00:00Z",
        )],
        stored_mute(),
    );
    assert_eq!(lifts(&out).len(), 3, "{:?}", tool_calls(&out));
    assert_eq!(report(&out)["unmuted"], 1);
    // a near word alone is not the topic
    let out = close(
        vec![turn(
            "t1",
            "user",
            "My neighbour owes the bakery a visit.",
            "2026-03-01T09:00:00Z",
        )],
        stored_mute(),
    );
    assert!(lifts(&out).is_empty(), "{:?}", tool_calls(&out));
}

#[test]
fn nobody_else_lifts_a_mute() {
    let cases = [
        // the agent naming the topic
        turn(
            "t1",
            "assistant",
            "Your neighbour still owes you the money, I think.",
            "2026-03-01T09:00:00Z",
        ),
        // a turn before the request
        turn(
            "t1",
            "user",
            "My neighbour owes me money.",
            "2026-02-09T09:00:00Z",
        ),
        // part of the topic only
        turn(
            "t1",
            "user",
            "My neighbour has a new car.",
            "2026-03-01T09:00:00Z",
        ),
        // asking to forget it again
        turn(
            "t1",
            "user",
            "Please forget what I told you about the money my neighbour owes me.",
            "2026-03-01T09:00:00Z",
        ),
    ];
    for t in cases {
        let out = close(vec![t.clone()], stored_mute());
        assert!(lifts(&out).is_empty(), "{t}: {:?}", lifts(&out));
        assert_eq!(report(&out)["unmuted"], 0, "{t}");
    }
    // and a lifted mute is not lifted twice
    let mut lifted = stored_mute();
    lifted[0]["unmuted_at"] = json!("2026-02-20T09:00:00Z");
    let out = close(
        vec![turn(
            "t1",
            "user",
            "Did my neighbour pay back the money he owes me?",
            "2026-03-01T09:00:00Z",
        )],
        lifted,
    );
    assert!(lifts(&out).is_empty(), "{:?}", lifts(&out));
}

#[test]
fn a_request_is_honoured_once_and_not_against_its_own_session() {
    // a session closed a second time: the request is muted already
    let again = close(vec![turn("r1", "user", REQUEST, ASKED)], stored_mute());
    assert!(
        !tool_calls(&again)
            .iter()
            .any(|(p, _)| p.starts_with("forget-")),
        "{:?}",
        tool_calls(&again)
    );
    // taken up again later in the very session it was asked in: not muted
    let taken_up = close(
        vec![
            turn("r1", "user", REQUEST, ASKED),
            turn(
                "r2",
                "user",
                "Actually, the money my neighbour owes me -- he paid it back today.",
                "2026-02-10T09:05:00Z",
            ),
        ],
        json!([]),
    );
    assert!(
        !tool_calls(&taken_up)
            .iter()
            .any(|(p, _)| p.starts_with("forget-")),
        "{:?}",
        tool_calls(&taken_up)
    );
    assert_eq!(report(&taken_up)["mute_searched"], 0);
}

#[test]
fn the_correction_idiom_mutes_nothing() {
    // review D1 / KF-F: "forget what I said, <new value>" corrects -- with the
    // fallback on by default nothing is muted, the turn of the new value least
    for text in [
        "Forget what I said, the appointment is on Tuesday.",
        "Forget that, Tuesday works.",
    ] {
        let out = close(vec![turn("r1", "user", text, ASKED)], json!([]));
        assert!(
            !tool_calls(&out)
                .iter()
                .any(|(p, _)| p.starts_with("forget-")),
            "{text}: {:?}",
            tool_calls(&out)
        );
    }
}

// ───────────────────────────────────────────────────────────── the night

#[test]
fn a_belief_of_a_muted_fact_is_muted_with_it() {
    let script = shipped_script(DREAM);
    let at = script.find("\"belief-audience-park\")").expect("the park");
    let read = &script[at - 400..at];
    assert!(
        read.contains("\"muted_at\"") && read.contains("\"mute_words\""),
        "the night reads no mute"
    );
    let program = format!(
        "import json\n{}\nprint(json.dumps([belief_mute(['f-b', 'f-a'], {{'f-a': ['{MARK}', 'money neighbour owes', '{ASKED}']}}), belief_mute(['f-b'], {{}})]))",
        block_of(&script, "def belief_mute")
    );
    // compared as JSON, not as text: python's `json.dumps` spaces its
    // separators, serde's `to_string` does not
    let got: Value = serde_json::from_str(&py(&program)).expect("json");
    assert_eq!(
        got,
        json!([{"mute_source": MARK, "mute_words": "money neighbour owes",
                "muted_at": ASKED, "unmuted_at": ""}, {}])
    );
}

// ─────────────────────────────────────────────────────────── the upgrade

#[test]
fn a_closing_request_of_an_older_store_becomes_a_mute() {
    let out = exceptions_answer(json!([
        {"id": "r1", "closure_source": MARK, "content": REQUEST, "happened_at": ASKED,
         "closed_at": "2026-02-10T10:00:00Z"},
        {"id": "e9", "closure_source": MARK, "content": "x",
         "happened_at": "2026-01-02T09:00:00Z", "closed_at": "2026-02-10T10:00:00Z"}
    ]));
    let calls = tool_calls(&out);
    let converted: Vec<&Value> = calls
        .iter()
        .filter(|(_, o)| o["operation"] == "update")
        .map(|(_, o)| o)
        .collect();
    assert_eq!(converted.len(), 3, "{calls:?}");
    for op in &converted {
        assert_eq!(op["where"], json!({"closure_source": MARK}));
        assert_eq!(op["set"]["closure_source"], "");
        assert_eq!(op["set"]["mute_source"], MARK);
        assert_eq!(op["set"]["muted_at"], ASKED);
        assert_eq!(op["set"]["mute_words"], "money neighbour owes");
    }
    assert_eq!(converted[0]["set"]["closed_at"], Value::Null);
    assert_eq!(converted[1]["set"]["expired_at"], Value::Null);
    // in the one bundle that parks the exceptions and the mutes; the meeting
    // follows its answer, as on a converted store
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(calls.iter().all(|(p, _)| p == "meet"), "{calls:?}");
    assert_the_declaration_admits(CLOSE, &out);
    let meet = back_bundle("meet", &[]);
    assert_eq!(tool_calls(&meet)[0].0, "prompt", "{meet:?}");
}

/// kfm M1b: the mute reads and the upgrade read were three stations of their
/// own -- six routing decisions more on the close lane, which then spent 49
/// to 53 of the 48 its budget allows (gh933 (f), the reserve of gh929). They
/// ride the exceptions read as ONE bundle, and its answer parks both sets and
/// converts the closures as ONE more: the lane costs what it cost before.
#[test]
fn the_mute_reads_ride_the_exceptions_read() {
    let out = back("exceptions-read", "insert", json!([]));
    assert_eq!(out.len(), 1, "one message: {out:?}");
    let ids: Vec<&str> = out[0]["messages"]
        .as_array()
        .expect("turns")
        .iter()
        .map(|t| t["id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, ["c-exceptions", "c-mutes", "c-mute-up"]);
    assert_the_declaration_admits(CLOSE, &out);
    // nothing to convert: the answer is one park bundle of the two sets
    let parked = exceptions_answer(json!([]));
    assert_eq!(parked.len(), 1, "{parked:?}");
    let kinds: Vec<String> = tool_calls(&parked)
        .iter()
        .map(|(p, o)| {
            assert_eq!(p, "meet");
            assert_eq!(o["operation"], "insert", "{o}");
            o["row"]["kind"].as_str().expect("kind").to_string()
        })
        .collect();
    assert_eq!(kinds, ["exceptions", "mutes"]);
    // and a refused read of the lane's own sets stops it like any refusal
    // (a refused upgrade does not: a_refused_upgrade_never_stops_the_pass)
    let refused = emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": {"mem_phase": "exceptions", "session_id": "s-2"},
                       "hop": {"operation": "bundle", "bundle_errors": 1}},
            "messages": [],
            "results": [{"tool_call_id": "c-exceptions", "operation": "select",
                         "error_code": "invalid_input"}]
        }),
    );
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0]["header"]["route"], "reject");
    assert_eq!(refused[0]["header"]["store_error"], "invalid_input");
}

// ───────────────────────────────────────────────── the store runs every op

/// The shipped store on a fresh connection: its schema, its extensions, its
/// canonical bindings -- the dispatcher the store cell runs.
fn store_conn() -> (
    rusqlite::Connection,
    meclaw_cells::store::params::StoreParams,
) {
    let params = config(STORE)["params"].clone();
    let parsed =
        meclaw_cells::store::params::StoreParams::parse(&params).expect("the store params parse");
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    meclaw_cells::store::query::install_connection_extensions(&conn).expect("extensions");
    meclaw_cells::store::ddl::apply_declared_schema_ddl(&conn, &parsed).expect("ddl");
    (conn, parsed)
}

/// kfm M1b: the upgrade read asked for `closure_source` `gte` AND `lt` in one
/// predicate; the store takes ONE operator per column and refused it, the
/// refused read ended the close lane, and no pass of a colony ever reported
/// again (gh933: 16 of 16 cells timed out). Every op the mute chain sends --
/// both reads, a request's marks, a lift and a conversion -- runs here through
/// the store's own dispatcher.
#[test]
fn every_op_of_the_mute_chain_is_one_the_store_runs() {
    let (conn, parsed) = store_conn();
    let mut ops: Vec<(String, Value)> = Vec::new();
    ops.extend(tool_calls(&back("exceptions-read", "insert", json!([]))));
    ops.extend(tool_calls(&back(
        &format!("forget-e|r1|{ASKED}|money,neighbour,owes"),
        "search",
        json!([{"id": "e1"}]),
    )));
    ops.extend(tool_calls(&back(
        &format!("forget-p|r1|{ASKED}|money,neighbour,owes"),
        "select",
        json!([{"id": "f-owes"}]),
    )));
    ops.extend(tool_calls(&close(
        vec![turn(
            "t1",
            "user",
            "Did my neighbour ever pay back the money he owes me?",
            "2026-03-01T09:00:00Z",
        )],
        stored_mute(),
    )));
    ops.extend(tool_calls(&exceptions_answer(json!([
        {"id": "r1", "closure_source": MARK, "content": REQUEST,
         "happened_at": ASKED, "closed_at": "2026-02-10T10:00:00Z"}
    ]))));
    ops.extend(tool_calls(&back_bundle("meet", &[])));
    let phases: Vec<&str> = ops.iter().map(|(p, _)| p.as_str()).collect();
    for want in ["exceptions", "meet", "unmuted", "muted", "prompt"] {
        assert!(phases.contains(&want), "no `{want}` op: {phases:?}");
    }
    for (phase, op) in &ops {
        if !matches!(
            op["operation"].as_str(),
            Some("select" | "update" | "insert")
        ) {
            continue;
        }
        let out = meclaw_cells::store::ops::dispatch_with(&conn, op, &parsed.canonical)
            .unwrap_or_else(|e| panic!("{phase}: the store refuses {op}: {e}"));
        assert_eq!(out.error_code, None, "{phase}: {op}: {:?}", out.error_text);
    }
}

// ───────────────────────────────────────── kfm M1c: the review of M1b

/// The cases of one lock group, every language (`fixtures/mute_<code>.jsonl`).
fn mute_cases(test: &str) -> Vec<Value> {
    let of = |src: &str| -> Vec<Value> {
        src.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Value>(l).expect("fixture line"))
            .filter(|c| c["test"] == test)
            .collect()
    };
    let (en, de) = (of(MUTE_EN), of(MUTE_DE));
    assert!(
        !en.is_empty() && !de.is_empty(),
        "{test}: a case in every language"
    );
    en.into_iter().chain(de).collect()
}

fn s(c: &Value, key: &str) -> String {
    c[key].as_str().unwrap_or_default().to_string()
}

fn mute_of(words: &str) -> Value {
    json!([{"mute_source": MARK, "mute_words": words, "muted_at": ASKED,
            "unmuted_at": null}])
}

/// The searches a pass sent for the request in turn `request`.
fn searches_of(out: &[Value], request: &str) -> Vec<String> {
    tool_calls(out)
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| p.starts_with("forget-") && p.contains(&format!("|{request}|")))
        .collect()
}

const LATER: &str = "2026-03-01T09:00:00Z";

/// Review 1: "Don't bring up the money my neighbour owes me again" names every
/// word of the topic -- and asks to keep it aside. It lifted the mute (and,
/// later in the request's own session, kept the request from being muted at
/// all). Such a turn is a request of its own where the grammar reads it.
#[test]
fn keeping_a_topic_aside_is_no_taking_it_up() {
    for c in mute_cases("keeps-aside") {
        let text = s(&c, "text");
        let out = close(
            vec![turn("t1", "user", &text, LATER)],
            mute_of(&s(&c, "words")),
        );
        assert!(lifts(&out).is_empty(), "{text}: {:?}", lifts(&out));
        let own = searches_of(&out, "t1");
        if s(&c, "asks").is_empty() {
            assert!(own.is_empty(), "{text}: {own:?}");
        } else {
            let tail = format!("|{}", s(&c, "asks").replace(' ', ","));
            assert!(
                !own.is_empty() && own.iter().all(|p| p.ends_with(&tail)),
                "{text}: its own request: {own:?}"
            );
        }
        assert_the_declaration_admits(CLOSE, &out);
        // later in the session of the request: the request is still muted
        let same = close(
            vec![
                turn("r1", "user", REQUEST, ASKED),
                turn("t1", "user", &text, "2026-02-10T09:05:00Z"),
            ],
            json!([]),
        );
        assert!(
            !searches_of(&same, "r1").is_empty(),
            "{text}: the request was dropped: {:?}",
            tool_calls(&same)
        );
    }
    // a yes to the agent asking which topic was meant
    for c in mute_cases("confirms") {
        let out = close(
            vec![
                turn(
                    "t0",
                    "assistant",
                    &s(&c, "question"),
                    "2026-03-01T08:59:00Z",
                ),
                turn("t1", "user", &s(&c, "text"), LATER),
            ],
            mute_of(&s(&c, "words")),
        );
        assert!(lifts(&out).is_empty(), "{c}: {:?}", lifts(&out));
    }
    // and the control: taking it up, in every language, lifts it
    for c in mute_cases("takes-up") {
        let out = close(
            vec![turn("t1", "user", &s(&c, "text"), LATER)],
            mute_of(&s(&c, "words")),
        );
        assert_eq!(lifts(&out).len(), 3, "{c}: {:?}", tool_calls(&out));
    }
}

/// Review 2: with the fallback on by default, "Delete the reminder for the
/// dentist" muted everything about reminders and dentists -- a sentence
/// addressed to an appointment, never cited by the closer. Delete and erase
/// mute only where the closer cites the turn.
#[test]
fn delete_and_erase_mute_only_where_the_closer_cites_them() {
    for c in mute_cases("cited-only") {
        let text = s(&c, "text");
        let out = close(vec![turn("r1", "user", &text, ASKED)], json!([]));
        assert!(
            searches_of(&out, "r1").is_empty(),
            "{text}: {:?}",
            tool_calls(&out)
        );
        let cited = emit_all(
            &shipped_script(CLOSE),
            &json!({
                "params": config(CLOSE)["params"].clone(),
                "header": {"context": {"mem_phase": "close-apply", "session_id": "s-2",
                                       "close_group": "0"},
                           "hop": {"operation": "select"}},
                "messages": [{"origin": "tool", "type": "tool_result", "text": json!([
                    parked("turns", json!([turn("r1", "user", &text, ASKED)])),
                    parked("facts", json!([])), parked("topics", json!([])),
                    parked("exceptions", json!([])), parked("mutes", json!([])),
                    {"key": "close:s-2", "kind": "verdict",
                     "created_at": "2026-01-01T00:00:00Z",
                     "payload": json!({"forget": [{"episode_id": "r1",
                                                   "words": s(&c, "words")}],
                                       "close_group": 0}).to_string()}
                ]).to_string()}]
            }),
        );
        assert!(
            searches_of(&cited, "r1").len() >= 2,
            "{text}: cited, it mutes: {:?}",
            tool_calls(&cited)
        );
    }
    // "forget" stays a request of the member's own sentence
    let out = close(vec![turn("r1", "user", REQUEST, ASKED)], json!([]));
    assert_eq!(searches_of(&out, "r1").len(), 4);
}

/// Review 3: a direct question about the topic is answered when it names the
/// topic in another inflection or names it in part -- "How much does my
/// neighbour still owe me?" about "money neighbour owes", "What did the doctor
/// say?" about "ann doctor". A statement naming part of it, an unrelated
/// question and the tier-0 bundle raise nothing.
#[test]
fn a_direct_question_answers_in_another_form_or_in_part() {
    for c in mute_cases("asked") {
        let rows = json!([{"muted_at": ASKED, "mute_source": MARK,
                           "mute_words": s(&c, "words")}]);
        assert_eq!(
            gate("t1-fan", &s(&c, "text"), rows.clone()),
            "[true]",
            "{c}"
        );
        assert_eq!(gate("legs", &s(&c, "text"), rows), "[false]", "tier 0: {c}");
    }
    for c in mute_cases("not-asked") {
        let rows = json!([{"muted_at": ASKED, "mute_source": MARK,
                           "mute_words": s(&c, "words")}]);
        assert_eq!(gate("t1-fan", &s(&c, "text"), rows), "[false]", "{c}");
    }
}

/// Review 4: an upgraded request whose own turn lay beyond the page became a
/// mute with no words -- muted for good and answering nothing. Its closed
/// turns all carried its words (the search was an AND), so the words they
/// share are the mute's; and a mute without words answers an ask.
#[test]
fn an_upgraded_request_beyond_the_page_keeps_its_words() {
    let rows = Value::Array(
        (0..3)
            .map(|i| {
                json!({"id": format!("e{i}"), "closure_source": MARK,
                       "content": format!("My neighbour owes me money, {i}00 euros."),
                       "happened_at": format!("2026-01-0{}T09:00:00Z", i + 1),
                       "closed_at": "2026-02-10T10:00:00Z"})
            })
            .collect(),
    );
    let out = exceptions_answer(rows);
    let words: Vec<String> = tool_calls(&out)
        .iter()
        .filter(|(_, o)| o["operation"] == "update")
        .map(|(_, o)| o["set"]["mute_words"].as_str().expect("words").to_string())
        .collect();
    assert_eq!(words.len(), 3, "{out:?}");
    for w in &words {
        for need in ["money", "neighbour", "owes"] {
            assert!(w.split(' ').any(|x| x == need), "{w}");
        }
    }
    // a mute without words: answered on an ask, never raised on tier 0
    let rows = json!([{"muted_at": ASKED, "mute_source": MARK, "mute_words": ""}]);
    assert_eq!(gate("t1-fan", "Is there news?", rows.clone()), "[true]");
    assert_eq!(gate("legs", "Is there news?", rows), "[false]");
}

/// Review 5: a refused conversion of an older store's closures ended every
/// close pass at the meeting (`reject store_refused`): the colony's memory
/// never closed a session again. The upgrade is reported, never terminal.
#[test]
fn a_refused_upgrade_never_stops_the_pass() {
    let refused = |phase: &str, results: Value| {
        emit_all(
            &shipped_script(CLOSE),
            &json!({
                "params": config(CLOSE)["params"].clone(),
                "header": {"context": {"mem_phase": phase, "session_id": "s-2"},
                           "hop": {"operation": "bundle", "bundle_errors": 1}},
                "messages": [],
                "results": results
            }),
        )
    };
    let meet = refused(
        "meet",
        json!([{"tool_call_id": "c-park-exceptions", "operation": "insert"},
               {"tool_call_id": "c-mute-up-0", "operation": "update",
                "error_code": "write_denied"}]),
    );
    let routes: Vec<(String, String)> = meet
        .iter()
        .map(|m| (s(&m["header"], "route"), s(&m["header"], "phase")))
        .collect();
    assert_eq!(
        routes,
        [
            ("cstore".to_string(), "prompt".to_string()),
            ("close_report".to_string(), "mute-up".to_string())
        ],
        "{meet:?}"
    );
    assert_eq!(meet[1]["header"]["mute_up_refused"], 1);
    assert_eq!(meet[1]["header"]["store_error"], "write_denied");
    assert_the_declaration_admits(CLOSE, &meet);
    // the upgrade READ refused: the sets are parked, the meeting follows
    let read = refused(
        "exceptions",
        json!([{"tool_call_id": "c-mute-up", "operation": "select",
                "error_code": "unknown_column"}]),
    );
    assert_eq!(read[0]["header"]["phase"], "meet", "{read:?}");
    assert_eq!(read[1]["header"]["phase"], "mute-up", "{read:?}");
    // a refused park stays terminal
    let park = refused(
        "meet",
        json!([{"tool_call_id": "c-park-mutes", "operation": "insert",
                "error_code": "write_denied"}]),
    );
    assert_eq!(park.len(), 1, "{park:?}");
    assert_eq!(park[0]["header"]["route"], "reject");
}

/// Review 6: the mutes read and the upgrade read scanned `episodes` without an
/// index on every close pass, and lifted mutes took rows of the mutes page.
/// Both columns are indexed (the store creates a declared index on boot, an
/// existing store too); the mutes read is the live mutes; a lifted request of
/// this very session is known by its own turn, which the window reads.
#[test]
fn the_mutes_read_is_the_live_mutes_on_an_index() {
    let indexes = &config(STORE)["params"]["indexes"];
    for col in ["mute_source", "closure_source"] {
        assert!(
            indexes
                .as_object()
                .expect("indexes")
                .values()
                .any(|i| i["table"] == "episodes" && i["on"] == json!([col])),
            "no index on episodes({col})"
        );
    }
    let out = back("exceptions-read", "insert", json!([]));
    let mutes = out[0]["messages"]
        .as_array()
        .expect("turns")
        .iter()
        .find(|t| t["id"] == "c-mutes")
        .map(|t| serde_json::from_str::<Value>(t["text"].as_str().expect("text")).expect("op"))
        .expect("the mutes read");
    assert_eq!(
        mutes["where"],
        json!({"mute_source": {"neq": ""}, "unmuted_at": {"or_null": {"eq": ""}}})
    );
    let window = tool_calls(&back("window", "insert", json!([])));
    assert!(
        window[0].1["columns"]
            .as_array()
            .expect("columns")
            .contains(&json!("mute_source")),
        "{window:?}"
    );
    // the request turn carries its own (lifted) mark: not muted again
    let mut own = turn("r1", "user", REQUEST, ASKED);
    own["mute_source"] = json!(MARK);
    let again = close(vec![own], json!([]));
    assert!(
        searches_of(&again, "r1").is_empty(),
        "{:?}",
        tool_calls(&again)
    );
}

/// Review 7: the closer sharpening a muted fact minted a replacement without
/// the mark -- the topic came back unasked. The window the close pass hands
/// the ingress says which statement is muted, and the replacement takes the
/// mute over; an unmuted statement's window is the old one byte for byte.
#[test]
fn a_sharpened_muted_fact_keeps_its_mute() {
    let fact = |id: &str, predicate: &str, claim: &str, muted: bool| {
        let mut f = json!({"id": id, "episode_id": "r0", "subject": "neighbour",
                           "canonical_subject": "neighbour", "predicate": predicate,
                           "canonical_predicate": predicate, "claim": claim,
                           "canonical_claim": claim, "fact_kind": "world",
                           "confidence": 70, "valid_from": "2026-01-01T00:00:00Z",
                           "recorded_at": "2026-01-01T00:00:00Z", "source": "",
                           "audience_set": AUDIENCE});
        if muted {
            f["muted_at"] = json!(ASKED);
            f["mute_source"] = json!(MARK);
            f["mute_words"] = json!("money neighbour owes");
        }
        f
    };
    let read = tool_calls(&back("facts-read", "insert", json!([])));
    let cols = read[0].1["columns"].as_array().expect("columns").clone();
    for col in ["muted_at", "mute_source", "mute_words", "unmuted_at"] {
        assert!(cols.contains(&json!(col)), "the facts read lacks {col}");
    }
    let rows = json!([
        parked("turns", json!([turn("r0", "user", "My neighbour owes me 200 euros.",
                                    "2026-01-01T00:00:00Z")])),
        parked("facts", json!([fact("f1", "owes", "owes me 200 euros", true),
                               fact("f2", "likes", "likes tea", false)])),
        parked("topics", json!([])), parked("exceptions", json!([])),
        parked("mutes", json!([])),
        {"key": "close:s-2", "kind": "verdict", "created_at": "2026-01-01T00:00:00Z",
         "payload": json!({"close_group": 0, "sharpen": [
            {"fact_id": "f1", "subject": "neighbour", "predicate": "owes",
             "claim": "owes me 200 euros since January"},
            {"fact_id": "f2", "subject": "neighbour", "predicate": "likes",
             "claim": "likes green tea"}]}).to_string()}
    ]);
    let out = emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": {"mem_phase": "close-apply", "session_id": "s-2",
                                   "close_group": "0"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result", "text": rows.to_string()}]
        }),
    );
    let block: Value = out
        .iter()
        .find(|m| m["header"]["route"] == "close_write")
        .map(|m| {
            serde_json::from_str(m["messages"][0]["text"].as_str().expect("text")).expect("block")
        })
        .unwrap_or_else(|| panic!("no write block: {out:?}"));
    let shown = block["shown"].as_array().expect("shown").clone();
    let statement = |id: &str| {
        shown
            .iter()
            .flat_map(|a| a["statements"].as_array().expect("statements").clone())
            .find(|st| st["id"] == id)
            .unwrap_or_else(|| panic!("{id} not shown: {shown:?}"))
    };
    assert_eq!(
        statement("f1")["mute"],
        json!({"muted_at": ASKED, "mute_source": MARK, "mute_words": "money neighbour owes"})
    );
    assert!(statement("f2").get("mute").is_none(), "{shown:?}");

    // the ingress: the replacement of f1 carries the mute, that of f2 none
    // the claim hash the ingress parse adds (one per fact, as `inline` does)
    let facts: Vec<Value> = block["facts"]
        .as_array()
        .expect("facts")
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut f = f.clone();
            f["claim_hash"] = json!(format!("h{i}"));
            f
        })
        .collect();
    let staged = json!({"facts": facts, "entities": [], "edges": []});
    let scratch = json!([
        {"key": "b1", "kind": "payload", "payload": staged.to_string()},
        {"key": "b1", "kind": "known", "payload": "[]"},
        {"key": "b1", "kind": "episodes", "payload": json!({"r0": {
            "happened_at": "2026-01-01T00:00:00Z", "session_id": "s-2",
            "channel": "direct:e", "audience_set": AUDIENCE, "source": ""}}).to_string()},
        {"key": "b1", "kind": "window", "payload": Value::Array(shown.clone()).to_string()}
    ]);
    let applied = emit_all(
        &shipped_script(EXTRACT),
        &json!({
            "params": config(EXTRACT)["params"].clone(),
            "header": {"context": {"mem_phase": "apply", "batch_id": "b1", "session_id": "s-2"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result", "text": scratch.to_string()}]
        }),
    );
    let minted: Vec<Value> = tool_calls(&applied)
        .into_iter()
        .filter(|(_, o)| o["operation"] == "insert" && o["table"] == "facts")
        .map(|(_, o)| o["row"].clone())
        .collect();
    assert_eq!(minted.len(), 2, "{applied:?}");
    for row in &minted {
        if row["claim"] == "owes me 200 euros since January" {
            assert_eq!(row["mute_source"], MARK, "{row}");
            assert_eq!(row["muted_at"], ASKED, "{row}");
            assert_eq!(row["mute_words"], "money neighbour owes", "{row}");
            assert_eq!(row["unmuted_at"], "", "{row}");
        } else {
            assert!(row.get("mute_source").is_none(), "{row}");
        }
    }
}

/// Review 8: a request honoured() turned down appeared in no field of the
/// report. `mute_skipped` names each, with why.
#[test]
fn a_request_not_muted_says_why() {
    let again = close(vec![turn("r1", "user", REQUEST, ASKED)], stored_mute());
    assert_eq!(
        report(&again)["mute_skipped"],
        json!([{"request": "r1", "reason": "already_muted"}])
    );
    assert_the_declaration_admits(CLOSE, &again);
    let taken_up = close(
        vec![
            turn("r1", "user", REQUEST, ASKED),
            turn(
                "r2",
                "user",
                "Actually, the money my neighbour owes me -- he paid it back today.",
                "2026-02-10T09:05:00Z",
            ),
        ],
        json!([]),
    );
    assert_eq!(
        report(&taken_up)["mute_skipped"],
        json!([{"request": "r1", "reason": "taken_up"}])
    );
    // a pass with nothing turned down carries no such field
    let plain = close(vec![turn("r1", "user", REQUEST, ASKED)], json!([]));
    assert!(report(&plain).get("mute_skipped").is_none(), "{plain:?}");
}

// ───────────────────────────────── kfm M1e: the lift through the real store

/// kfm M1e: the "after" mini of 08.10. read 0 of 5 rows lifted on a store the
/// backfill had converted -- mute columns grown as TEXT, every marked row
/// `unmuted_at` "", every other row NULL, the request months before the turn
/// that takes the topic up (snapshot clock). The cause was the measurement
/// (an adopted snapshot runs its own instances, the overlay never reached
/// them); this lock is the product's half of the proof: exactly that store
/// form, every read, park and meeting of the pass through the store's own
/// dispatcher, only the closer's verdict scripted -- and the lift lands on
/// every marked row, the NULL variant of `unmuted_at` included.
#[test]
fn a_backfilled_mute_lifts_through_the_real_store() {
    const LIFT_MARK: &str = "forget_request:req-debt";
    const MUTED: &str = "2025-03-18T08:00:00Z";
    const TAKEN_UP: &str = "2025-07-06T09:00:00Z";
    let (conn, parsed) = store_conn();
    let run = |op: &Value| -> Value {
        let out = meclaw_cells::store::ops::dispatch_with(&conn, op, &parsed.canonical)
            .unwrap_or_else(|e| panic!("the store refuses {op}: {e}"));
        assert_eq!(out.error_code, None, "{op}: {:?}", out.error_text);
        out.payload
    };
    let words = "old debt bank";
    for (id, session, content, at, mark, unmuted) in [
        (
            "req-debt",
            "s-1",
            "Please forget my old debt at the bank.",
            MUTED,
            true,
            Some(""),
        ),
        (
            "e-debt1",
            "s-0",
            "My old debt at the bank worries me.",
            "2025-03-01T10:00:00Z",
            true,
            Some(""),
        ),
        (
            "e-debt2",
            "s-0",
            "The bank wrote about the old debt.",
            "2025-03-02T10:00:00Z",
            true,
            None,
        ),
        (
            "e-other",
            "s-0",
            "The garden is in bloom.",
            "2025-03-03T10:00:00Z",
            false,
            None,
        ),
        (
            "lift-1",
            "s-2",
            "I want to talk about my old debt at the bank again. It is on my mind once more.",
            TAKEN_UP,
            false,
            None,
        ),
    ] {
        conn.execute(
            "INSERT INTO episodes (id, session_id, turn_id, sender, speaker, channel, audience_set, \
             content, happened_at, recorded_at, mute_source, mute_words, muted_at, unmuted_at) \
             VALUES (?1, ?2, ?1, 'user', 'member:e', 'direct:e', ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                id, session, AUDIENCE, content, at,
                mark.then_some(LIFT_MARK), mark.then_some(words), mark.then_some(MUTED),
                unmuted
            ],
        )
        .expect("episode");
    }
    for (id, mark, unmuted) in [
        ("f-debt", true, Some("")),
        ("f-debt2", true, None),
        ("f-other", false, None),
    ] {
        conn.execute(
            "INSERT INTO facts (id, episode_id, session_id, audience_set, subject, predicate, claim, \
             recorded_at, mute_source, mute_words, muted_at, unmuted_at) \
             VALUES (?1, 'e-debt1', 's-0', ?2, 'member', 'owes', 'an old debt at the bank', ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                id, AUDIENCE, MUTED, mark.then_some(LIFT_MARK), mark.then_some(words),
                mark.then_some(MUTED), unmuted
            ],
        )
        .expect("fact");
    }

    // The pass from its first read to the meeting, every op through the store.
    let mut out = back("window", "insert", json!([]));
    let mut meeting = None;
    for _ in 0..32 {
        let Some(m) = out
            .iter()
            .find(|m| m["header"]["route"] == "cstore")
            .cloned()
        else {
            panic!("the pass left the store before the meeting: {out:?}")
        };
        let phase = m["header"]["phase"].as_str().expect("phase").to_string();
        let calls: Vec<(String, Value)> = m["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|x| {
                (
                    x["id"].as_str().unwrap_or_default().to_string(),
                    serde_json::from_str::<Value>(x["text"].as_str().expect("text")).expect("op"),
                )
            })
            .collect();
        let answers: Vec<(String, Value)> =
            calls.iter().map(|(id, op)| (id.clone(), run(op))).collect();
        if phase == "prompt" {
            meeting = Some(answers[0].1.clone());
            break;
        }
        out = if calls.len() > 1 || phase == "exceptions" || phase == "meet" {
            let legs: Vec<(&str, Value)> = answers
                .iter()
                .map(|(id, rows)| (id.as_str(), rows.clone()))
                .collect();
            back_bundle(&phase, &legs)
        } else {
            let op = calls[0].1["operation"]
                .as_str()
                .expect("operation")
                .to_string();
            back(&phase, &op, answers[0].1.clone())
        };
    }
    let mut rows = meeting.expect("the pass reached its meeting");
    let parked_mutes = rows
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["kind"] == "mutes")
        .map(|r| r["payload"].clone())
        .expect("the mutes are parked");
    assert!(
        parked_mutes
            .as_str()
            .unwrap_or_default()
            .contains(LIFT_MARK),
        "the mutes read found the backfilled mark: {parked_mutes}"
    );
    rows.as_array_mut().expect("rows").insert(
        0,
        json!({"key": "close:s-2", "kind": "verdict", "created_at": "2099-01-01T00:00:00Z",
               "payload": json!({"nothing_to_add": true, "forget": [], "close_group": 0}).to_string()}),
    );
    let applied = emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": {"mem_phase": "close-apply", "session_id": "s-2",
                                   "close_group": "0"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result", "text": rows.to_string()}]
        }),
    );
    assert_eq!(report(&applied)["unmuted"], 1, "{applied:?}");
    for op in lifts(&applied) {
        run(&op);
    }
    for table in ["episodes", "facts"] {
        let got: Vec<(String, Option<String>)> = conn
            .prepare(&format!("SELECT id, unmuted_at FROM {table} ORDER BY id"))
            .expect("prepare")
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        for (id, unmuted) in got {
            let marked = id.contains("debt");
            let lifted = unmuted.as_deref().is_some_and(|u| u > MUTED);
            assert_eq!(lifted, marked, "{table}.{id}: unmuted_at {unmuted:?}");
        }
    }
}
