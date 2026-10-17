//! GH #1225 (R-KF-2) -- a member taking a muted topic up sees it at once, in
//! the session they took it up in; recall reads it by close-glue's one rule.
//!
//! The locks, with invented people only:
//! - F2: recall treats a muted row as visible once a turn of THIS session (or
//!   the asker's words of this very ask) takes the topic up after the mute --
//!   on tier 0 and tier 1, a pure read: another session, an agent's turn, a
//!   turn before the mute, a turn of another round lift nothing, and the close
//!   stays the one writer of `unmuted_at`;
//! - one rule: `takes_up` and everything it calls stand in close-glue and in
//!   recall byte for byte, their language data (`forget_lang`,
//!   `question_words`) is one param with one value;
//! - B3: a question without its mark (speech-to-text) is a question to the
//!   recall gate too;
//! - E7: a request said in a group mutes the member's 1:1 rows as well, and
//!   ten asks of the group round raise none of them.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, shipped_script};
use support::*;

const CLOSE: &str = "../../templates/memory-hive/close-glue/config.json";
const STORE: &str = "../../templates/memory-hive/store/config.json";
const AUDIENCE: &str = r#"["agent:a","member:e"]"#;
const OTHER_ROUND: &str = r#"["agent:b","member:e"]"#;
// OR-KF-48: a mark of this release carries its speaker; the older form
// without one is pinned by its own cold path (gh1225 same session).
const MARK: &str = "forget_request:r1@member:e";
const ASKED: &str = "2026-02-10T09:00:00Z";
const LATER: &str = "2026-03-01T09:00:00Z";
const TAKE_UP: &str = "Did my neighbour ever pay back the money he owes me?";
const PLAIN: &str = "How is he doing these days?";
const SECRET: &str = "two hundred euros";

fn config(path: &str) -> Value {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path);
    serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json")
}

/// `src` runs on python's stdin: a whole shipped script is longer than one
/// argument may be (`-c` ends in E2BIG).
fn py(src: &str) -> String {
    use std::io::Write;
    let mut child = std::process::Command::new("python3")
        .arg("-I")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(src.as_bytes())
        .expect("the program");
    let out = child.wait_with_output().expect("python3");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn mutes() -> Value {
    json!([{"mute_source": MARK, "mute_words": "money neighbour owes",
            "muted_at": ASKED, "unmuted_at": null}])
}

fn turn(id: &str, sender: &str, text: &str, at: &str, audience: &str) -> Value {
    json!({"id": id, "sender": sender, "content": text, "happened_at": at,
           "recorded_at": at, "audience_set": audience, "channel": "direct:e"})
}

fn episode(id: &str, content: &str, audience: &str, muted: Option<&str>) -> Value {
    let mut e = json!({"id": id, "session_id": "s-1", "sender": "user",
                       "speaker": "member:e", "content": content,
                       "happened_at": "2026-02-01T09:00:00Z",
                       "recorded_at": "2026-02-01T09:00:00Z",
                       "channel": "direct:e", "audience_set": audience});
    if let Some(unmuted) = muted {
        e["muted_at"] = json!(ASKED);
        e["mute_source"] = json!(MARK);
        e["mute_words"] = json!("money neighbour owes");
        e["unmuted_at"] = json!(unmuted);
    }
    e
}

/// A tier-0 answer of the store to the legs bundle; the bundle text recall
/// hands the asker.
fn tier0(query: &str, round: &[&str], episodes: Value, mutes: Value, session: Value) -> String {
    tier0_as(query, round, episodes, mutes, session, &json!({}))["messages"][0]["text"]
        .as_str()
        .expect("text")
        .to_string()
}

/// The tier-0 bundle message itself, with `knobs` over the shipped params and
/// the ask's own context keys (`ask`, e.g. its `speaker`). OR-KF-46: the
/// session's turns are the episodes leg's own page (one read of the session),
/// so `session` comes back on `r-leg-episodes`, newest first, after `episodes`.
fn tier0_as(
    query: &str,
    round: &[&str],
    episodes: Value,
    mutes: Value,
    session: Value,
    knobs: &Value,
) -> Value {
    let out = emit_all(
        &shipped_script(RECALL),
        &tier0_input(query, round, episodes, mutes, session, knobs),
    );
    out.iter()
        .find(|m| m["header"]["route"] == "bundle")
        .unwrap_or_else(|| panic!("no bundle: {out:?}"))
        .clone()
}

/// The store's answer to the tier-0 legs bundle, as recall reads it.
fn tier0_input(
    query: &str,
    round: &[&str],
    episodes: Value,
    mutes: Value,
    session: Value,
    knobs: &Value,
) -> Value {
    let mut params = config(RECALL)["params"].clone();
    let mut context = json!({"mem_phase": "legs", "recall_id": "r1", "memory_tier": "0",
                             "recall_query": query,
                             "recall_as_of": "2026-03-02T00:00:00Z",
                             "recall_window_from": "", "recall_window_to": "",
                             "audience_now": round, "channel": "direct:e",
                             "session_id": "s-2"});
    for (k, v) in knobs.as_object().expect("knobs") {
        if let Some(key) = k.strip_prefix("ctx.") {
            context[key] = v.clone();
        } else {
            params[k] = v.clone();
        }
    }
    let mut rows = episodes.as_array().cloned().unwrap_or_default();
    let mut turns = session.as_array().cloned().unwrap_or_default();
    turns.reverse();
    rows.extend(turns);
    let legs = [
        ("r-leg-episodes", Value::Array(rows)),
        ("r-leg-beliefs", json!([])),
        ("r-leg-foresight", json!([])),
        ("r-leg-mutes", mutes),
    ];
    json!({
        "params": params,
        "header": {
            "context": context,
            "hop": {"operation": "bundle", "rows_affected": 2, "bundle_errors": 0}
        },
        "messages": legs.iter().map(|(id, rows)| json!(
            {"origin": "tool", "type": "tool_result", "id": id,
             "text": rows.to_string()})).collect::<Vec<_>>(),
        "results": legs.iter().map(|(id, _)| json!(
            {"tool_call_id": id, "operation": "select"})).collect::<Vec<_>>()
    })
}

const ROUND_A: [&str; 2] = ["agent:a", "member:e"];

fn owed(unmuted: Option<&str>) -> Value {
    json!([
        episode(
            "e1",
            "My neighbour owes me money, two hundred euros.",
            AUDIENCE,
            unmuted
        ),
        episode("e2", "The tomatoes in the garden are ripe.", AUDIENCE, None)
    ])
}

// ──────────────────────────────────────────────── F2: lifted at once

#[test]
fn a_topic_taken_up_is_visible_in_the_same_session() {
    let seen = |query: &str, session: Value| -> bool {
        let text = tier0(query, &ROUND_A, owed(Some("")), mutes(), session);
        assert!(text.contains("tomatoes"), "{text}");
        text.contains(SECRET)
    };
    // the turn that takes it up sees it in its own recall: its episode is not
    // written yet, its words are the ask
    assert!(seen(TAKE_UP, json!([])), "the asking turn takes it up");
    // a follow-up without the words of the mute, after the turn of this session
    assert!(
        seen(PLAIN, json!([turn("t1", "user", TAKE_UP, LATER, AUDIENCE)])),
        "the follow-up of the same session"
    );
    // another session (none of its turns took it up), before the close
    assert!(!seen(PLAIN, json!([])), "another session stays muted");
    // the controls: nobody else, never before the mute, never another round,
    // and a request to forget is no taking it up
    for (why, session) in [
        (
            "an agent's turn",
            json!([turn("t1", "assistant", TAKE_UP, LATER, AUDIENCE)]),
        ),
        (
            "a turn before the mute",
            json!([turn(
                "t1",
                "user",
                TAKE_UP,
                "2026-02-10T08:59:59Z",
                AUDIENCE
            )]),
        ),
        (
            "a turn of another round",
            json!([turn("t1", "user", TAKE_UP, LATER, OTHER_ROUND)]),
        ),
    ] {
        assert!(!seen(PLAIN, session), "{why} lifted the mute");
    }
    assert!(
        !seen("Please forget the money my neighbour owes me.", json!([])),
        "asking to forget it again takes nothing up"
    );
    // after the close the row is open everywhere (`unmuted_at`), no session read
    let text = tier0(PLAIN, &ROUND_A, owed(Some(LATER)), json!([]), json!([]));
    assert!(text.contains(SECRET), "{text}");
}

#[test]
fn the_fan_reads_the_session_and_parks_its_lifts() {
    let ctx = |phase: Option<&str>| {
        let mut c = json!({"recall_id": "r9", "memory_tier": "1", "recall_query": PLAIN,
                           "recall_as_of": "2026-03-02T00:00:00Z",
                           "recall_window_from": "", "recall_window_to": "",
                           "audience_now": ROUND_A, "channel": "direct:e",
                           "session_id": "s-2"});
        if let Some(p) = phase {
            c["mem_phase"] = json!(p);
        }
        c
    };
    // the request: both reads ride the fan bundle
    let out = emit_all(
        &shipped_script(RECALL),
        &json!({
            "params": config(RECALL)["params"].clone(),
            "header": {"context": ctx(None), "hop": {"route": "in_query"}},
            "messages": [{"origin": "user", "type": "text", "text": PLAIN}]
        }),
    );
    let fan = out
        .iter()
        .find(|m| m["header"]["phase"] == "t1-fan")
        .unwrap_or_else(|| panic!("no fan: {out:?}"));
    let ids: Vec<&str> = fan["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|x| x["id"].as_str())
        .collect();
    assert!(
        ids.contains(&"r-fan-mutes") && ids.contains(&"r-fan-session"),
        "{ids:?}"
    );
    // the answer: the lift is parked with the fan, for every later hop
    let legs = [
        ("r-fan-mutes", mutes()),
        (
            "r-fan-session",
            json!([turn("t1", "user", TAKE_UP, LATER, AUDIENCE)]),
        ),
    ];
    let out = emit_all(
        &shipped_script(RECALL),
        &json!({
            "params": config(RECALL)["params"].clone(),
            "header": {"context": ctx(Some("t1-fan")),
                       "hop": {"operation": "bundle", "bundle_errors": 0}},
            "messages": legs.iter().map(|(id, rows)| json!(
                {"origin": "tool", "type": "tool_result", "id": id,
                 "text": rows.to_string()})).collect::<Vec<_>>(),
            "results": legs.iter().map(|(id, _)| json!(
                {"tool_call_id": id, "operation": "select"})).collect::<Vec<_>>()
        }),
    );
    let parked: Vec<Value> = out
        .iter()
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .filter_map(|x| serde_json::from_str::<Value>(x["text"].as_str()?).ok())
        .filter(|op| op["table"] == "recall_scratch" && op["row"]["leg"] == "legs")
        .collect();
    assert_eq!(parked.len(), 1, "{out:?}");
    let payload: Value =
        serde_json::from_str(parked[0]["row"]["payload"].as_str().expect("payload")).expect("json");
    assert_eq!(payload["lifted"], json!([MARK]), "{payload}");
    // a pure read: no write of `unmuted_at` anywhere in recall
    assert!(
        !shipped_script(RECALL).contains("\"unmuted_at\": now"),
        "recall writes no lift"
    );
}

// ──────────────────────────────────────────────── one rule, one data

/// The blocks of the rule, as both scripts must carry them.
const RULE: &[&str] = &[
    "def _data",
    "FORGET_LANG = ",
    "LANGS = ",
    "def _lang_list",
    "FORGET_CUES = ",
    "FORGET_REMINDS = ",
    "FORGET_KEEPS_MUTED = ",
    "FORGET_AFFIRMS = ",
    "QUESTION_WORDS = ",
    "def is_question",
    "def asks_to_forget",
    "def topic_words",
    "def names_word",
    "TIME_FLOOR = ",
    "def mute_time",
    "def in_order",
    "ANY_SPEAKER = ",
    "def identity_of",
    "def request_speaker",
    "def takes_up",
];

#[test]
fn the_rule_is_one_rule_in_close_glue_and_recall() {
    let (close, recall) = (shipped_script(CLOSE), shipped_script(RECALL));
    for head in RULE {
        let (a, b) = (block_of(&close, head), block_of(&recall, head));
        assert!(!a.is_empty(), "close-glue lacks {head}");
        assert_eq!(a, b, "{head} drifted between close-glue and recall");
    }
    for knob in ["forget_lang", "question_words"] {
        let (a, b) = (
            config(CLOSE)["params"][knob].clone(),
            config(RECALL)["params"][knob].clone(),
        );
        assert!(a.is_object(), "close-glue: params.{knob}");
        assert_eq!(a, b, "params.{knob} drifted");
    }
    // the class of B3: a question mark decides nothing outside `is_question`
    for (cell, src) in [("close-glue", &close), ("recall", &recall)] {
        let outside = src.replace(&block_of(src, "def is_question"), "");
        assert!(
            !outside.contains("\"?\" in"),
            "{cell}: a `\"?\" in` without the question words"
        );
        // ... and no question kept by its mark alone (the GH #88 query clamp)
        assert!(
            !outside.contains("endswith(\"?\")"),
            "{cell}: an `endswith(\"?\")` without the question words"
        );
    }
}

// ──────────────────────────────────────────────── B3: no mark, still a question

fn gate(query: &str, rows: Value) -> String {
    let script = shipped_script(RECALL);
    let program = format!(
        "import re, json\nP = json.loads({})\nphase = 't1-fan'\nquery = {}\nctx = {{'recall_query': query}}\ndef visible(a, c):\n    return True\n{}\n",
        serde_json::to_string(
            &json!({"question_words": config(RECALL)["params"]["question_words"].clone()})
                .to_string()
        )
        .expect("params"),
        serde_json::to_string(query).expect("query"),
        [
            "def _knob",
            "def _data",
            "QUESTION_WORDS",
            "def is_question",
            "FORGET_SOURCE",
            "LIFTED",
            "def forgotten",
            "def muted",
            "def words_of",
            "PUSH_PREFIX",
            "def asker_words",
            "ASKED_WORDS",
            "def names_word",
            "def asked_for",
            "def visible_row",
        ]
        .iter()
        .map(|h| block_of(&script, h))
        .collect::<Vec<_>>()
        .join("\n")
    ) + &format!(
        "print(json.dumps([visible_row(r) for r in json.loads({})]))",
        serde_json::to_string(&rows.to_string()).expect("rows")
    );
    py(&program)
}

#[test]
fn a_question_without_its_mark_asks_for_a_muted_row() {
    let row = json!([{"muted_at": ASKED, "mute_source": MARK,
                      "mute_words": "alten schulden bank"}]);
    for (query, visible) in [
        // speech-to-text: the question word, no mark
        ("wie viel schulden hatte ich noch bei der bank", true),
        ("wie viel schulden hatte ich noch bei der bank?", true),
        // a statement naming part of it stays muted
        ("ich war heute bei der bank wegen der schulden", false),
    ] {
        assert_eq!(gate(query, row.clone()), format!("[{visible}]"), "{query}");
    }
}

// ──────────────────────────────────────────────── E7: the village keeps 1:1

const GROUP: &str = r#"["agent:a","member:a","member:b"]"#;
const ONE_TO_ONE: &str = r#"["agent:a","member:a"]"#;

fn run_close(context: Value, hop: Value, text: String) -> Vec<Value> {
    emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": context, "hop": hop},
            "messages": [{"origin": "tool", "type": "tool_result", "text": text}]
        }),
    )
}

fn cstore_ops(out: &[Value]) -> Vec<(String, Value)> {
    out.iter()
        .filter(|m| m["header"]["route"] == "cstore")
        .flat_map(|m| {
            let phase = m["header"]["phase"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            m["messages"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(move |x| {
                    serde_json::from_str::<Value>(x["text"].as_str()?)
                        .ok()
                        .map(|op| (phase.clone(), op))
                })
        })
        .collect()
}

#[test]
fn a_request_in_the_group_mutes_the_one_to_one_rows_too() {
    let params = config(STORE)["params"].clone();
    let parsed =
        meclaw_cells::store::params::StoreParams::parse(&params).expect("the store params parse");
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    meclaw_cells::store::query::install_connection_extensions(&conn).expect("extensions");
    meclaw_cells::store::ddl::apply_declared_schema_ddl(&conn, &parsed).expect("ddl");
    // the order of the store factory: schema, indexes, canonical, the search index
    meclaw_cells::store::ddl::apply_index_ddl(&conn, &parsed.indexes).expect("indexes");
    meclaw_cells::store::ddl::apply_canonical_ddl(&conn, &parsed.canonical).expect("canonical");
    meclaw_cells::store::ddl::apply_fts_ddl(&conn, &parsed.fts, &parsed.canonical).expect("fts");
    let store = |op: &Value| -> Value {
        let out = meclaw_cells::store::ops::dispatch_with(&conn, op, &parsed.canonical)
            .unwrap_or_else(|e| panic!("the store refuses {op}: {e}"));
        assert_eq!(out.error_code, None, "{op}: {:?}", out.error_text);
        out.payload
    };
    let request = "Please forget the money my neighbour owes me.";
    for (id, session, content, audience) in [
        (
            "p1",
            "s-0",
            "My neighbour owes me money, two hundred euros.",
            ONE_TO_ONE,
        ),
        (
            "p2",
            "s-0",
            "The neighbour still owes me that money.",
            ONE_TO_ONE,
        ),
        (
            "b1",
            "s-0",
            "My neighbour owes me money as well.",
            r#"["agent:a","member:b"]"#,
        ),
        ("g0", "s-2", "Shall we plan the summer party?", GROUP),
        ("g1", "s-2", request, GROUP),
    ] {
        store(&json!({"operation": "insert", "table": "episodes", "row": {
            "id": id, "session_id": session, "turn_id": id, "sender": "user",
            "speaker": "member:a", "channel": "group:v", "audience_set": audience,
            "content": content, "happened_at": "2026-02-01T09:00:00Z",
            "recorded_at": "2026-02-01T09:00:00Z"}}));
    }
    // the close of the group session: the closer cites the request
    let turns = json!([
        {"id": "g0", "session_id": "s-2", "sender": "user", "speaker": "member:b",
         "content": "Shall we plan the summer party?", "happened_at": "2026-02-01T09:00:00Z",
         "audience_set": GROUP, "channel": "group:v"},
        {"id": "g1", "session_id": "s-2", "sender": "user", "speaker": "member:a",
         "content": request, "happened_at": "2026-02-01T09:00:00Z",
         "audience_set": GROUP, "channel": "group:v"}
    ]);
    let park = |kind: &str, rows: Value| {
        json!({"key": "close:s-2", "kind": kind, "created_at": "2026-01-01T00:00:00Z",
               "payload": json!({"rows": rows, "truncated": false}).to_string()})
    };
    let verdict = json!({"forget": [{"episode_id": "g1", "words": "money neighbour owes"}],
                         "close_group": 0});
    let meeting = json!([
        park("turns", turns),
        park("facts", json!([])),
        park("topics", json!([])),
        park("exceptions", json!([])),
        park("mutes", json!([])),
        {"key": "close:s-2", "kind": "verdict", "created_at": "2026-01-01T00:00:00Z",
         "payload": verdict.to_string()}
    ]);
    let mut pending = cstore_ops(&run_close(
        json!({"mem_phase": "close-apply", "session_id": "s-2", "close_group": "0"}),
        json!({"operation": "select"}),
        meeting.to_string(),
    ));
    let mut searched = 0;
    for _ in 0..64 {
        let Some((phase, op)) = pending.pop() else {
            break;
        };
        if !phase.starts_with("forget-") && phase != "muted" {
            continue;
        }
        let rows = store(&op);
        if phase.starts_with("forget-") {
            searched += 1;
            pending.extend(cstore_ops(&run_close(
                json!({"mem_phase": phase, "session_id": "s-2"}),
                json!({"operation": op["operation"].clone()}),
                rows.to_string(),
            )));
        }
    }
    assert!(searched > 0, "the request was searched");
    let marks = rows_of_conn(&conn, "SELECT id, COALESCE(mute_source, '') FROM episodes");
    for id in ["p1", "p2"] {
        // OR-KF-45: the mark names who asked
        assert_eq!(
            marks.get(id).map(String::as_str),
            Some("forget_request:g1@member:a"),
            "{id}: the 1:1 row is muted by the group request: {marks:?}"
        );
    }
    // the other member's own room stays theirs, the group's other talk too
    for id in ["b1", "g0"] {
        assert_eq!(
            marks.get(id).map(String::as_str),
            Some(""),
            "{id}: {marks:?}"
        );
    }
    // ten asks of the group round: none raises a 1:1 row, a lift in the group
    // session or not
    let private = json!([
        episode(
            "p1",
            "My neighbour owes me money, two hundred euros.",
            ONE_TO_ONE,
            Some("")
        ),
        episode("g0", "Shall we plan the summer party?", GROUP, None)
    ]);
    let round = ["agent:a", "member:a", "member:b"];
    let take_up = json!([turn("t9", "user", TAKE_UP, LATER, GROUP)]);
    for (k, query) in [
        PLAIN,
        TAKE_UP,
        "How much money does my neighbour owe me?",
        "money neighbour owes",
        "What did I say about the neighbour?",
        "Shall we plan the summer party?",
        "wie viel geld schuldet mir der nachbar",
        "Tell me about the two hundred euros.",
        "Who owes money to whom?",
        "neighbour",
    ]
    .into_iter()
    .enumerate()
    {
        let session = if k % 2 == 0 {
            take_up.clone()
        } else {
            json!([])
        };
        let text = tier0(query, &round, private.clone(), mutes(), session);
        assert!(
            !text.contains(SECRET),
            "{query}: a 1:1 row reached the group: {text}"
        );
    }
    // the requester's own 1:1 round sees the row on the audience gate; only the
    // mark the group request left keeps it aside there (an ask that names no
    // word of the mute), and the same row unmarked is raised -- the control
    let mark = marks.get("p1").cloned().expect("p1 marked");
    let marked = |unmarked: bool| {
        let mut row = episode(
            "p1",
            "My neighbour owes me money, two hundred euros.",
            ONE_TO_ONE,
            Some(""),
        );
        if unmarked {
            for k in ["muted_at", "mute_source", "mute_words"] {
                row[k] = json!("");
            }
        } else {
            row["mute_source"] = json!(mark.clone());
        }
        json!([row])
    };
    let live = json!([{"mute_source": mark.clone(), "mute_words": "money neighbour owes",
                       "muted_at": ASKED, "unmuted_at": null}]);
    for query in [
        PLAIN,
        "What did I say about the neighbour?",
        "Tell me about the two hundred euros.",
        "Who owes money to whom?",
        "",
    ] {
        let text = tier0(
            query,
            &["agent:a", "member:a"],
            marked(false),
            live.clone(),
            json!([]),
        );
        assert!(
            !text.contains(SECRET),
            "{query:?}: the mark did not hold: {text}"
        );
        let text = tier0(
            query,
            &["agent:a", "member:a"],
            marked(true),
            json!([]),
            json!([]),
        );
        assert!(text.contains(SECRET), "{query:?}: the control: {text}");
    }
}

// ──────────────────────────────────────────────── OR-KF-45: the asker lifts

const MARK_A: &str = "forget_request:g1@member:a";

fn said_by(id: &str, speaker: &str, text: &str, audience: &str) -> Value {
    let mut t = turn(id, "user", text, LATER, audience);
    t["speaker"] = json!(speaker);
    t
}

fn mutes_a() -> Value {
    json!([{"mute_source": MARK_A, "mute_words": "money neighbour owes",
            "muted_at": ASKED, "unmuted_at": null}])
}

fn group_row() -> Value {
    let mut e = episode(
        "e1",
        "My neighbour owes me money, two hundred euros.",
        GROUP,
        Some(""),
    );
    e["mute_source"] = json!(MARK_A);
    json!([
        e,
        episode("e2", "The tomatoes in the garden are ripe.", GROUP, None)
    ])
}

/// The close's lifts of a pass over `turns` with A's mute (phase `prompt`).
fn close_lifts(turns: Value) -> usize {
    close_lifts_of(mutes_a(), turns)
}

/// The close's lifts of a pass over `turns` with `mutes` (phase `prompt`).
fn close_lifts_of(mutes: Value, turns: Value) -> usize {
    let park = |kind: &str, rows: Value| {
        json!({"key": "close:s-2", "kind": kind, "created_at": "2026-01-01T00:00:00Z",
               "payload": json!({"rows": rows, "truncated": false}).to_string()})
    };
    let meeting = json!([
        park("turns", turns),
        park("facts", json!([])),
        park("topics", json!([])),
        park("exceptions", json!([])),
        park("mutes", mutes),
    ]);
    let out = run_close(
        json!({"mem_phase": "prompt", "session_id": "s-2"}),
        json!({"operation": "select"}),
        meeting.to_string(),
    );
    out.iter()
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .filter_map(|x| serde_json::from_str::<Value>(x["text"].as_str()?).ok())
        .filter(|op| op["operation"] == "update" && op["set"].get("unmuted_at").is_some())
        .count()
}

#[test]
fn only_the_member_who_asked_takes_a_topic_up() {
    let group = ["agent:a", "member:a", "member:b"];
    let knobs = |speaker: &str| json!({"ctx.speaker": speaker});
    // F2, the session's turns: B naming the topic lifts nothing, A does;
    // OR-KF-49: a group turn without its speaker is nobody's and lifts nothing
    for (speaker, lifted) in [("member:b", false), ("member:a", true), ("", false)] {
        let session = json!([said_by("t1", speaker, TAKE_UP, GROUP)]);
        let text = tier0(PLAIN, &group, group_row(), mutes_a(), session);
        assert!(text.contains("tomatoes"), "{text}");
        assert_eq!(
            text.contains(SECRET),
            lifted,
            "a turn of {speaker:?}: {text}"
        );
    }
    // F2, the ask itself: B's ask lifts nothing, A's does
    for (speaker, lifted) in [("member:b", false), ("member:a", true)] {
        let bundle = tier0_as(
            TAKE_UP,
            &group,
            group_row(),
            mutes_a(),
            json!([]),
            &knobs(speaker),
        );
        let text = bundle["messages"][0]["text"].as_str().expect("text");
        assert_eq!(
            text.contains(SECRET),
            lifted,
            "the ask of {speaker}: {text}"
        );
    }
    // the close: for good only where A takes it up
    assert_eq!(
        close_lifts(json!([said_by("t1", "member:b", TAKE_UP, GROUP)])),
        0
    );
    assert_eq!(
        close_lifts(json!([said_by("t1", "member:a", TAKE_UP, GROUP)])),
        3
    );
}

// ──────────────────────────────────────── OR-KF-47: a mark from before the speaker

const OLD_MARK: &str = "forget_request:g1";

fn old_mutes() -> Value {
    json!([{"mute_source": OLD_MARK, "mute_words": "money neighbour owes",
            "muted_at": ASKED, "unmuted_at": null}])
}

fn old_group_row() -> Value {
    let mut rows = group_row();
    rows[0]["mute_source"] = json!(OLD_MARK);
    rows
}

#[test]
fn a_mark_without_its_speaker_is_lifted_by_the_speaker_of_its_request() {
    let group = ["agent:a", "member:a", "member:b"];
    let asked = said_by(
        "g1",
        "member:a",
        "Forget the money my neighbour owes me.",
        GROUP,
    );
    // the request turn is at hand: its speaker is the one who lifts
    for (speaker, lifted) in [("member:b", false), ("member:a", true)] {
        let session = json!([asked.clone(), said_by("t1", speaker, TAKE_UP, GROUP)]);
        let text = tier0(PLAIN, &group, old_group_row(), old_mutes(), session.clone());
        assert!(text.contains("tomatoes"), "{text}");
        assert_eq!(
            text.contains(SECRET),
            lifted,
            "an old mark, a turn of {speaker}: {text}"
        );
        assert_eq!(
            close_lifts_of(old_mutes(), session),
            if lifted { 3 } else { 0 },
            "the close, an old mark, a turn of {speaker}"
        );
    }
    // a turn that names nothing muted reads nothing, out of reach or not
    let (reads, bundle) = tier0_cold(
        PLAIN,
        &group,
        json!([said_by("t1", "member:b", PLAIN, GROUP)]),
        "member:b",
        old_mutes(),
        Err("must_not_read"),
    );
    assert!(reads.is_empty(), "{reads:?}");
    assert!(bundle["header"].get("lift_unread").is_none(), "{bundle}");
}

// ──────────────────────────── OR-KF-48: out of reach, the request turn is read once

const B_ALONE: &str = r#"["agent:a","member:b"]"#;

fn asked_by_a() -> Value {
    json!([{"id": "g1", "speaker": "member:a"}])
}

/// A tier-0 request over `mutes` whose cold path the store answers with
/// `asks` (rows, or the code it refused the read with): every `r-asks` read
/// it made (the counter) and the bundle it answered with.
fn tier0_cold(
    query: &str,
    round: &[&str],
    session: Value,
    speaker: &str,
    mutes: Value,
    asks: Result<Value, &str>,
) -> (Vec<Value>, Value) {
    let mut doc = tier0_input(
        query,
        round,
        old_group_row(),
        mutes,
        session,
        &json!({"ctx.speaker": speaker}),
    );
    let mut reads = Vec::new();
    for _ in 0..3 {
        let out = emit_all(&shipped_script(RECALL), &doc);
        if let Some(bundle) = out.iter().find(|m| m["header"]["route"] == "bundle") {
            return (reads, bundle.clone());
        }
        let detour = out
            .iter()
            .find(|m| m["header"]["route"] == "rstore" && m["header"]["phase"] == "legs")
            .unwrap_or_else(|| panic!("neither a bundle nor a read: {out:?}"));
        let op = |id: &str| -> Value {
            detour["messages"]
                .as_array()
                .expect("messages")
                .iter()
                .find(|m| m["id"] == id)
                .and_then(|m| serde_json::from_str(m["text"].as_str()?).ok())
                .unwrap_or_else(|| panic!("no {id}: {detour}"))
        };
        reads.push(op("r-asks"));
        let parked = op("r-detour-park")["row"]["payload"].clone();
        let mut asked = json!({"tool_call_id": "r-asks", "operation": "select"});
        let text = match &asks {
            Ok(rows) => rows.to_string(),
            Err(code) => {
                asked["error_code"] = json!(code);
                format!("refused: {code}")
            }
        };
        doc["header"]["hop"] = json!({"operation": "bundle", "rows_affected": 3,
                                      "bundle_errors": usize::from(asks.is_err())});
        doc["messages"] = json!([
            {"origin": "tool", "type": "tool_result", "id": "r-detour-park", "text": "[]"},
            {"origin": "tool", "type": "tool_result", "id": "r-asks", "text": text},
            {"origin": "tool", "type": "tool_result", "id": "r-detour-read",
             "text": json!([{"request_id": "r1", "leg": "detour-legs", "payload": parked,
                             "fired": 0}]).to_string()}
        ]);
        doc["results"] = json!([
            {"tool_call_id": "r-detour-park", "operation": "insert"},
            asked,
            {"tool_call_id": "r-detour-read", "operation": "select"}
        ]);
    }
    panic!("the cold path read more than twice")
}

/// The store's answer to one bundle of the close: `legs` by id, `refused`
/// the ids it refused with `code`.
fn run_close_bundle(
    phase: &str,
    legs: &[(String, Value)],
    refused: &[String],
    code: &str,
) -> Vec<Value> {
    emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": config(CLOSE)["params"].clone(),
            "header": {"context": {"mem_phase": phase, "session_id": "s-2"},
                       "hop": {"operation": "bundle", "bundle_errors": refused.len()}},
            "messages": legs.iter().map(|(id, rows)| json!(
                {"origin": "tool", "type": "tool_result", "id": id,
                 "text": if refused.contains(id) { json!(code) } else { json!(rows.to_string()) }}))
                .collect::<Vec<_>>(),
            "results": legs.iter().map(|(id, _)| {
                let mut r = json!({"tool_call_id": id, "operation": "select"});
                if refused.contains(id) {
                    r["error_code"] = json!(code);
                }
                r
            }).collect::<Vec<_>>()
        }),
    )
}

/// The close pass of `close_lifts_of` with its cold path answered by `asks`:
/// (the `c-mute-up-asks` reads, every store op it sent, its close reports,
/// the lifts it made).
struct ColdClose {
    reads: usize,
    ops: Vec<(String, Value)>,
    reports: Vec<Value>,
    lifts: usize,
}

fn close_cold(mutes: Value, turns: Value, asks: Result<Value, &str>) -> ColdClose {
    let park = |kind: &str, rows: Value| {
        json!({"key": "close:s-2", "kind": kind, "created_at": "2026-01-01T00:00:00Z",
               "payload": json!({"rows": rows, "truncated": false}).to_string()})
    };
    let meeting = |mutes: &Value| {
        json!([
            park("turns", turns.clone()),
            park("facts", json!([])),
            park("topics", json!([])),
            park("exceptions", json!([])),
            park("mutes", mutes.clone()),
        ])
    };
    let mut out = run_close(
        json!({"mem_phase": "prompt", "session_id": "s-2"}),
        json!({"operation": "select"}),
        meeting(&mutes).to_string(),
    );
    let mut cold = ColdClose {
        reads: 0,
        ops: Vec::new(),
        reports: Vec::new(),
        lifts: 0,
    };
    for _ in 0..4 {
        cold.ops.extend(cstore_ops(&out));
        cold.reports.extend(
            out.iter()
                .filter(|m| m["header"]["route"] == "close_report")
                .cloned(),
        );
        let next = out
            .iter()
            .find(|m| {
                m["header"]["route"] == "cstore"
                    && matches!(
                        m["header"]["phase"].as_str(),
                        Some("prompt-asks" | "prompt-asked")
                    )
            })
            .cloned();
        let Some(next) = next else {
            cold.lifts = cstore_ops(&out)
                .iter()
                .filter(|(_, op)| {
                    op["operation"] == "update" && op["set"].get("unmuted_at").is_some()
                })
                .count();
            return cold;
        };
        let calls: Vec<(String, Value)> = next["messages"]
            .as_array()
            .expect("calls")
            .iter()
            .map(|m| {
                (
                    m["id"].as_str().expect("id").to_string(),
                    serde_json::from_str(m["text"].as_str().expect("text")).expect("op"),
                )
            })
            .collect();
        let mut parked = mutes.clone();
        let mut refused = Vec::new();
        let mut legs = Vec::new();
        for (id, op) in &calls {
            if id == "c-park-mutes" {
                let payload: Value =
                    serde_json::from_str(op["row"]["payload"].as_str().expect("payload"))
                        .expect("parcel");
                parked = payload["rows"].clone();
            }
            if id == "c-mute-up-asks" {
                cold.reads += 1;
                if asks.is_err() {
                    refused.push(id.clone());
                }
            }
        }
        for (id, _) in &calls {
            let rows = match id.as_str() {
                "c-meet" => meeting(&parked),
                "c-mute-up-asks" => asks.clone().unwrap_or(json!([])),
                _ => json!([]),
            };
            legs.push((id.clone(), rows));
        }
        let phase = next["header"]["phase"].as_str().expect("phase").to_string();
        out = run_close_bundle(
            &phase,
            &legs,
            &refused,
            asks.as_ref().err().copied().unwrap_or(""),
        );
    }
    panic!("the close read more than its cold path")
}

#[test]
fn a_mark_out_of_reach_lifts_by_the_speaker_its_request_turn_names() {
    let group = ["agent:a", "member:a", "member:b"];
    for (speaker, audience, round, lifted) in [
        // (1) B takes A's topic up in his own 1:1: it stays muted
        ("member:b", B_ALONE, &["agent:a", "member:b"][..], false),
        ("member:b", GROUP, &group[..], false),
        // (2) A takes it up -- in their own 1:1, and in the group
        ("member:a", ONE_TO_ONE, &["agent:a", "member:a"][..], true),
        ("member:a", GROUP, &group[..], true),
    ] {
        let session = json!([said_by("t1", speaker, TAKE_UP, audience)]);
        let (reads, bundle) = tier0_cold(
            PLAIN,
            round,
            session.clone(),
            speaker,
            old_mutes(),
            Ok(asked_by_a()),
        );
        let text = bundle["messages"][0]["text"].as_str().expect("text");
        assert!(text.contains("tomatoes"), "{text}");
        assert_eq!(
            text.contains(SECRET),
            lifted,
            "recall, {speaker} in {audience}: {text}"
        );
        assert_eq!(reads.len(), 1, "one targeted read: {reads:?}");
        assert_eq!(
            reads[0]["where"],
            json!({"id": {"in": ["g1"]}}),
            "{reads:?}"
        );
        let close = close_cold(old_mutes(), session, Ok(asked_by_a()));
        assert_eq!(close.reads, 1, "the close, {speaker} in {audience}");
        assert_eq!(
            close.lifts,
            if lifted { 3 } else { 0 },
            "the close, {speaker} in {audience}: {:?}",
            close.ops
        );
    }
    // (2) ... and A's ask itself, in A's own 1:1
    let (reads, bundle) = tier0_cold(
        TAKE_UP,
        &["agent:a", "member:a"],
        json!([]),
        "member:a",
        old_mutes(),
        Ok(asked_by_a()),
    );
    let text = bundle["messages"][0]["text"].as_str().expect("text");
    assert!(text.contains(SECRET), "the ask in A's own 1:1: {text}");
    assert_eq!(reads.len(), 1, "{reads:?}");
}

#[test]
fn a_read_that_fails_keeps_the_mark_muted_and_says_so() {
    let session = json!([said_by("t1", "member:a", TAKE_UP, ONE_TO_ONE)]);
    for (asks, code) in [
        (Err("store_unavailable"), "store_unavailable"),
        (Ok(json!([])), "request_not_found"),
    ] {
        let (reads, bundle) = tier0_cold(
            PLAIN,
            &["agent:a", "member:a"],
            session.clone(),
            "member:a",
            old_mutes(),
            asks.clone(),
        );
        let text = bundle["messages"][0]["text"].as_str().expect("text");
        assert!(text.contains("tomatoes"), "{text}");
        assert!(!text.contains(SECRET), "{code}: still muted: {text}");
        assert_eq!(reads.len(), 1);
        assert_eq!(bundle["header"]["lift_unread"], code, "{bundle}");
        let close = close_cold(old_mutes(), session.clone(), asks);
        assert_eq!(
            (close.reads, close.lifts),
            (1, 0),
            "{code}: {:?}",
            close.ops
        );
        assert!(
            close
                .reports
                .iter()
                .any(|r| r["header"]["lift_unread"] == code
                    && r["header"]["lift_unread_requests"] == "g1"),
            "{code}: said: {:?}",
            close.reports
        );
        assert!(
            !close
                .ops
                .iter()
                .any(|(_, op)| op["set"].get("mute_source").is_some()),
            "{code}: nothing rewritten: {:?}",
            close.ops
        );
    }
    // the store's refusal is said like every other one of the upgrade
    let close = close_cold(old_mutes(), session, Err("store_unavailable"));
    assert!(
        close
            .reports
            .iter()
            .any(|r| r["header"]["store_error"] == "store_unavailable"),
        "{:?}",
        close.reports
    );
}

#[test]
fn a_mark_read_once_is_rewritten_and_never_read_again() {
    let session = json!([said_by("t1", "member:b", TAKE_UP, B_ALONE)]);
    let first = close_cold(old_mutes(), session.clone(), Ok(asked_by_a()));
    let rewrites: Vec<&Value> = first
        .ops
        .iter()
        .map(|(_, op)| op)
        .filter(|op| op["set"].get("mute_source").is_some())
        .collect();
    let tables: Vec<&str> = rewrites
        .iter()
        .map(|op| op["table"].as_str().expect("table"))
        .collect();
    assert_eq!(tables, ["episodes", "facts", "beliefs"], "{rewrites:?}");
    for op in &rewrites {
        assert_eq!(op["set"]["mute_source"], MARK_A, "{op}");
        assert_eq!(op["where"]["mute_source"], OLD_MARK, "{op}");
    }
    // the second pass meets the mark in its new form: no read, A's rule
    let rewritten = json!([{"mute_source": MARK_A, "mute_words": "money neighbour owes",
                            "muted_at": ASKED, "unmuted_at": null}]);
    for (speaker, audience, lifted) in [("member:b", B_ALONE, 0), ("member:a", ONE_TO_ONE, 3)] {
        let turns = json!([said_by("t2", speaker, TAKE_UP, audience)]);
        let second = close_cold(rewritten.clone(), turns.clone(), Err("must_not_read"));
        assert_eq!((second.reads, second.lifts), (0, lifted), "{speaker}");
        let (reads, _) = tier0_cold(
            PLAIN,
            &["agent:a", speaker],
            turns,
            speaker,
            rewritten.clone(),
            Err("must_not_read"),
        );
        assert!(reads.is_empty(), "recall, {speaker}: {reads:?}");
    }
    // the lift of the pass that rewrote reaches the rows in either form
    let lifting = close_cold(
        old_mutes(),
        json!([said_by("t1", "member:a", TAKE_UP, ONE_TO_ONE)]),
        Ok(asked_by_a()),
    );
    let lift = lifting
        .ops
        .iter()
        .find(|(_, op)| op["set"].get("unmuted_at").is_some())
        .expect("a lift");
    assert_eq!(
        lift.1["where"]["mute_source"],
        json!({"in": [MARK_A, OLD_MARK]}),
        "{lift:?}"
    );
}

// ──────────────────────── OR-KF-49: without a speaker, the 1:1 says who it is

const ASK_TO_FORGET: &str = "Forget the money my neighbour owes me.";

/// A's request `g1` without a speaker, as the store gives it back (`audience`
/// is the round it was said in).
fn asked_in(audience: &str) -> Value {
    json!([{"id": "g1", "speaker": "", "audience_set": audience}])
}

/// The marks the close's cold path rewrote `OLD_MARK` into.
fn rewritten_to(close: &ColdClose) -> Vec<Value> {
    close
        .ops
        .iter()
        .filter(|(_, op)| op["set"].get("mute_source").is_some())
        .map(|(_, op)| op["set"]["mute_source"].clone())
        .collect()
}

const GROUP_ROUND: [&str; 3] = ["agent:a", "member:a", "member:b"];

#[test]
fn a_member_with_a_speaker_lifts_no_request_of_another_one_to_one() {
    let group = GROUP_ROUND;
    // (1) B, with his speaker, names A's topic; A asked in A's 1:1 without one:
    // the request is A's (A != B), so it stays muted -- at hand and out of
    // reach (the group round sees no row of A's 1:1: recall reads it)
    let b_says = said_by("t1", "member:b", TAKE_UP, GROUP);
    let at_hand = json!([said_by("g1", "", ASK_TO_FORGET, ONE_TO_ONE), b_says.clone()]);
    assert_eq!(
        close_lifts_of(old_mutes(), at_hand),
        0,
        "(1) the close, at hand"
    );
    let (reads, bundle) = tier0_cold(
        PLAIN,
        &group,
        json!([b_says.clone()]),
        "member:b",
        old_mutes(),
        Ok(asked_in(ONE_TO_ONE)),
    );
    let text = bundle["messages"][0]["text"].as_str().expect("text");
    assert_eq!(reads.len(), 1, "{reads:?}");
    assert!(!text.contains(SECRET), "(1) recall, out of reach: {text}");
    let close = close_cold(old_mutes(), json!([b_says]), Ok(asked_in(ONE_TO_ONE)));
    assert_eq!((close.reads, close.lifts), (1, 0), "(1) {:?}", close.ops);
    assert_eq!(
        rewritten_to(&close),
        vec![json!(MARK_A); 3],
        "(1) the mark is A's"
    );
}

#[test]
fn a_member_without_a_speaker_in_his_one_to_one_lifts_no_mark_of_another() {
    let b_alone = ["agent:a", "member:b"];
    // (2) B without a speaker in his own 1:1 is B: A's group mark stays muted
    let b_alone_says = json!([said_by("t1", "", TAKE_UP, B_ALONE)]);
    let text = tier0(
        PLAIN,
        &b_alone,
        group_row(),
        mutes_a(),
        b_alone_says.clone(),
    );
    assert!(text.contains("tomatoes"), "{text}");
    assert!(!text.contains(SECRET), "(2) recall: {text}");
    assert_eq!(close_lifts(b_alone_says), 0, "(2) the close");
    let bundle = tier0_as(
        TAKE_UP,
        &b_alone,
        group_row(),
        mutes_a(),
        json!([]),
        &json!({}),
    );
    let text = bundle["messages"][0]["text"].as_str().expect("text");
    assert!(
        !text.contains(SECRET),
        "(2) B's ask without a speaker: {text}"
    );
}

#[test]
fn a_member_without_a_speaker_in_their_one_to_one_lifts_their_own_request() {
    let a_alone = ["agent:a", "member:a"];
    // (3) A without a speaker in A's 1:1 lifts A's request without one
    let a_alone_says = said_by("t1", "", TAKE_UP, ONE_TO_ONE);
    let at_hand = json!([
        said_by("g1", "", ASK_TO_FORGET, ONE_TO_ONE),
        a_alone_says.clone()
    ]);
    let text = tier0(
        PLAIN,
        &a_alone,
        old_group_row(),
        old_mutes(),
        at_hand.clone(),
    );
    assert!(text.contains(SECRET), "(3) recall, at hand: {text}");
    assert_eq!(
        close_lifts_of(old_mutes(), at_hand),
        3,
        "(3) the close, at hand"
    );
    let close = close_cold(
        old_mutes(),
        json!([a_alone_says.clone()]),
        Ok(asked_in(ONE_TO_ONE)),
    );
    assert_eq!((close.reads, close.lifts), (1, 3), "(3) {:?}", close.ops);
    assert_eq!(
        rewritten_to(&close),
        vec![json!(MARK_A); 3],
        "(3) the mark is A's"
    );
    let (_, bundle) = tier0_cold(
        TAKE_UP,
        &a_alone,
        json!([]),
        "",
        old_mutes(),
        Ok(asked_in(ONE_TO_ONE)),
    );
    let text = bundle["messages"][0]["text"].as_str().expect("text");
    assert!(
        text.contains(SECRET),
        "(3) A's ask without a speaker: {text}"
    );
}

#[test]
fn a_group_without_a_speaker_is_nobody_and_lifts_nothing() {
    let group = GROUP_ROUND;
    // (4) a group says nobody: no turn of it lifts, no request of it is lifted
    let nobody = json!([said_by("t1", "", TAKE_UP, GROUP)]);
    let text = tier0(PLAIN, &group, group_row(), mutes_a(), nobody.clone());
    assert!(!text.contains(SECRET), "(4) recall, a turn: {text}");
    assert_eq!(close_lifts(nobody), 0, "(4) the close, a turn");
    let bundle = tier0_as(
        TAKE_UP,
        &group,
        group_row(),
        mutes_a(),
        json!([]),
        &json!({}),
    );
    let text = bundle["messages"][0]["text"].as_str().expect("text");
    assert!(!text.contains(SECRET), "(4) an ask: {text}");
    let a_says = said_by("t1", "member:a", TAKE_UP, GROUP);
    let at_hand = json!([said_by("g1", "", ASK_TO_FORGET, GROUP), a_says.clone()]);
    let text = tier0(PLAIN, &group, old_group_row(), old_mutes(), at_hand.clone());
    assert!(
        !text.contains(SECRET),
        "(4) recall, a request at hand: {text}"
    );
    assert_eq!(
        close_lifts_of(old_mutes(), at_hand),
        0,
        "(4) the close, a request at hand"
    );
    let (_, bundle) = tier0_cold(
        PLAIN,
        &group,
        json!([a_says.clone()]),
        "member:a",
        old_mutes(),
        Ok(asked_in(GROUP)),
    );
    let text = bundle["messages"][0]["text"].as_str().expect("text");
    assert!(
        !text.contains(SECRET),
        "(4) recall, a request out of reach: {text}"
    );
    let close = close_cold(old_mutes(), json!([a_says.clone()]), Ok(asked_in(GROUP)));
    assert_eq!((close.reads, close.lifts), (1, 0), "(4) {:?}", close.ops);
    let nobodys = json!(format!("{OLD_MARK}@"));
    assert_eq!(rewritten_to(&close), vec![nobodys.clone(); 3]);
    // ... and the mark it was rewritten to is read never again
    let marked = json!([{"mute_source": nobodys, "mute_words": "money neighbour owes",
                         "muted_at": ASKED, "unmuted_at": null}]);
    let again = close_cold(marked, json!([a_says]), Err("must_not_read"));
    assert_eq!((again.reads, again.lifts), (0, 0), "(4) {:?}", again.ops);
}

/// OR-KF.KM.15 (delta review M2): the cold path's probe "anyone" is no
/// speaker. A request turn said by `*` is that speaker's like any other:
/// another member naming the topic lifts nothing, a turn of `*` does.
#[test]
fn a_request_of_the_speaker_star_is_lifted_by_that_speaker_only() {
    let group = GROUP_ROUND;
    let by_star = || json!([{"id": "g1", "speaker": "*"}]);
    let b_says = said_by("t1", "member:b", TAKE_UP, GROUP);
    // out of reach: the request turn is read once and names `*`
    let (reads, bundle) = tier0_cold(
        PLAIN,
        &group,
        json!([b_says.clone()]),
        "member:b",
        old_mutes(),
        Ok(by_star()),
    );
    let text = bundle["messages"][0]["text"].as_str().expect("text");
    assert_eq!(reads.len(), 1, "{reads:?}");
    assert!(!text.contains(SECRET), "recall, out of reach: {text}");
    let close = close_cold(old_mutes(), json!([b_says.clone()]), Ok(by_star()));
    assert_eq!((close.reads, close.lifts), (1, 0), "{:?}", close.ops);
    let star_mark = json!(format!("{OLD_MARK}@*"));
    assert_eq!(rewritten_to(&close), vec![star_mark.clone(); 3]);
    // at hand: the mark names `*`; B lifts nothing, `*` lifts it
    let marked = json!([{"mute_source": star_mark, "mute_words": "money neighbour owes",
                         "muted_at": ASKED, "unmuted_at": null}]);
    assert_eq!(close_lifts_of(marked.clone(), json!([b_says])), 0, "B");
    assert_eq!(
        close_lifts_of(marked, json!([said_by("t2", "*", TAKE_UP, GROUP)])),
        3,
        "the speaker `*`"
    );
}

// ──────────────────────────────────────────────── OR-KF-46: a bounded, cheap lift

/// The tier-0 request's reads: (id, limit) of every op of the legs bundle.
fn tier0_reads(knobs: &Value) -> Vec<(String, Value)> {
    let mut params = config(RECALL)["params"].clone();
    for (k, v) in knobs.as_object().expect("knobs") {
        params[k] = v.clone();
    }
    let out = emit_all(
        &shipped_script(RECALL),
        &json!({
            "params": params,
            "header": {"context": {"recall_id": "", "mem_phase": "", "memory_tier": "0",
                                   "recall_query": PLAIN, "audience_now": ROUND_A,
                                   "channel": "direct:e", "session_id": "s-2"},
                       "hop": {"route": "in_query", "phase": "recall"}},
            "messages": []
        }),
    );
    out.iter()
        .filter(|m| m["header"]["phase"] == "legs")
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .filter_map(|x| {
            let op = serde_json::from_str::<Value>(x["text"].as_str()?).ok()?;
            Some((x["id"].as_str()?.to_string(), op["limit"].clone()))
        })
        .collect()
}

#[test]
fn the_lift_reads_as_far_as_its_bound() {
    // one read of the session: the episodes leg, as deep as the lift needs
    let reads = tier0_reads(&json!({}));
    let ids: Vec<&str> = reads.iter().map(|(i, _)| i.as_str()).collect();
    assert_eq!(
        ids,
        [
            "r-leg-episodes",
            "r-leg-beliefs",
            "r-leg-foresight",
            "r-leg-mutes"
        ]
    );
    assert_eq!(reads[0].1, json!(512));
    assert_eq!(reads[3].1, json!(512));
    let reads = tier0_reads(&json!({"lift_turns": 3, "lift_marks": 2}));
    assert_eq!(reads[0].1, json!(12), "the leg's own depth wins: {reads:?}");
    assert_eq!(reads[3].1, json!(2), "{reads:?}");
    // the scan reads the newest `lift_turns` turns of the page and no more, and
    // a full page is said: the page is [e1, e2, t2, t1], the take-up is t1
    let session = json!([
        turn("t1", "user", TAKE_UP, LATER, AUDIENCE),
        turn("t2", "user", PLAIN, "2026-03-01T09:01:00Z", AUDIENCE),
    ]);
    for (bound, lifted, cut) in [
        (3, false, json!("turns")),
        (4, true, json!("turns")),
        (5, true, Value::Null),
    ] {
        let bundle = tier0_as(
            PLAIN,
            &ROUND_A,
            owed(Some("")),
            mutes(),
            session.clone(),
            &json!({"lift_turns": bound}),
        );
        let text = bundle["messages"][0]["text"].as_str().expect("text");
        assert!(text.contains("tomatoes"), "{bound}: {text}");
        assert_eq!(text.contains(SECRET), lifted, "lift_turns {bound}: {text}");
        assert_eq!(bundle["header"]["lift_cut"], cut, "{bound}: {bundle}");
    }
    // the marks the same way
    let bundle = tier0_as(
        PLAIN,
        &ROUND_A,
        owed(Some("")),
        mutes(),
        json!([]),
        &json!({"lift_marks": 1}),
    );
    assert_eq!(bundle["header"]["lift_cut"], json!("marks"), "{bundle}");
}

/// The rule's blocks out of recall, run in python with `asks_to_forget`
/// counted: how often the grammar ran for `turns` user turns that do not name
/// the topic, and for one that does.
#[test]
fn the_word_match_runs_before_the_grammar() {
    let script = shipped_script(RECALL);
    let mut heads = vec!["def _knob"];
    heads.extend_from_slice(RULE);
    let blocks = heads
        .iter()
        .map(|h| block_of(&script, h))
        .collect::<Vec<_>>()
        .join("\n");
    let program = format!(
        "import re, json, datetime\nP = json.loads({})\n{}\n\
N = [0]\n_ask = asks_to_forget\n\
def asks_to_forget(text):\n    N[0] += 1\n    return _ask(text)\n\
turns = [{{'id': 't%d' % i, 'sender': 'user', 'speaker': 'member:e', 'content': 'We had coffee and talked about the summer.',\n\
          'happened_at': '2026-03-01T09:00:%02dZ' % (i % 60)}} for i in range(200)]\n\
print(takes_up(['money', 'neighbour', 'owes'], '2026-02-10T09:00:00Z', 'r1', turns, 'member:e'))\n\
print(N[0])\n\
turns.append({{'id': 'tx', 'sender': 'user', 'speaker': 'member:e', 'content': {}, 'happened_at': '2026-03-01T10:00:00Z'}})\n\
print(takes_up(['money', 'neighbour', 'owes'], '2026-02-10T09:00:00Z', 'r1', turns, 'member:e'))\n\
print(N[0])\n",
        serde_json::to_string(&config(RECALL)["params"].to_string()).expect("params"),
        blocks,
        serde_json::to_string(TAKE_UP).expect("take up"),
    );
    let out = py(&program);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines, ["None", "0", "tx", "1"], "{out}");
}

fn rows_of_conn(
    conn: &rusqlite::Connection,
    sql: &str,
) -> std::collections::BTreeMap<String, String> {
    let mut st = conn.prepare(sql).expect("prepare");
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}
