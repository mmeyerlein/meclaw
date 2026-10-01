//! GH #926 -- a curator counts its own ledger: `in_stats` in, `stats` out.
//!
//! An observer asks a curator how many calls it made and how many marks of
//! each kind its cells wrote inside a window, and gets numbers read out of the
//! hive's own ledger by `./stats` -- deterministic, read only, one ledger round
//! trip, and never a word of what was said: a sample carries `at`, `kind`,
//! `session_id`, `turn_id` and a value that is metadata (a gap's words stay in
//! the ledger, OR-BD-6). Nor whose: the ledger's session id is the session
//! keeper's, `<channel>-<stamp>`, and a channel is a phone number or a chat id,
//! so a sample's `session_id` and `turn_id` are references the answer hands out
//! (`s1`, `s1#1`, ...) and never the ids the ledger stores.
//!
//! Two halves:
//!
//! * The hive in one process (`support/curator_hive.rs`): the shipped script
//!   under python3, the shipped edges under the colony's own CEL, the ledger an
//!   in-memory SQLite driven by the store's own dispatcher. The window's
//!   bounds, the refusals, the budget and the one bundle are measured there.
//! * A colony with one assistant generation (the tree `gh919` builds, without
//!   the member around it): the question enters at the assistant's path with
//!   `hop.stats_role`, the answer is taken at a sink behind the assistant's
//!   `stats` exit -- at the receiver, with `stats_tag` in the echo. The trail is
//!   written straight into the ledger `cell.db` of the test's own colony
//!   (OR-BD-19, precedent `gh462`): the curator hive is sealed and no test cell
//!   reaches `./ledger`; the stores are warmed by a first question so the file
//!   exists before a row is written.
//!
//! Free of a real provider by construction: every `llm` cell points at a local
//! stub and none is called. Guarded like every template-reading test (GH #49).

#[path = "support/curator_hive.rs"]
mod curator_hive;

#[path = "mock_openai.rs"]
mod mock_openai;

use curator_hive::{Hive, Msg};
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::{
    BashCellFactory, EditCellFactory, FileCellFactory, LlmCellFactory, WebFetchCellFactory,
    WebSearchCellFactory,
};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);

const FROM: &str = "2026-09-01T00:00:00Z";
const TO: &str = "2026-09-08T00:00:00Z";
const ALL_KINDS: &str = r#"["calls","gap","reread","ask","correction","tool_error"]"#;
/// The words a gap mark stores in its value -- never in an answer.
const GAP_WORDS: &str = "which platform the probe leaves from";
/// A channel the way the session keeper names one: a phone number (a
/// fictional one). Its digits never leave the ledger.
const CHANNEL: &str = "+15550100042";
const CHANNEL_DIGITS: &str = "15550100042";
/// The session id of the lock's trail, `<channel>-<stamp>` as the keeper
/// stamps it.
const SESSION: &str = "+15550100042-2026-09-01T08:00:00.000000Z";
/// A second session, of a chat channel (a group's chat id, made up).
const CHAT_SESSION: &str = "-1009990001234-2026-09-03T07:00:00.000000Z";
const CHAT_DIGITS: &str = "1009990001234";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/assistant/config.json",
        "templates/talky/config.json",
        "templates/cogny/config.json",
        "templates/curator/config.json",
        "templates/curator/stats/config.json",
        "templates/collector/config.json",
        "templates/dispatcher/config.json",
        "templates/session-keeper/config.json",
        "templates/tools/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

fn as_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().expect("a JSON object")
}

// ─────────────────────────────────────────────────────────────── the trail

/// A stamp of the window's week, in the form every writer of the ledger uses.
fn at(day: u32, hour: u32, micros: u32) -> String {
    format!("2026-09-{day:02}T{hour:02}:00:00.{micros:06}Z")
}

/// One `calls` row, the columns `./policy` writes (the rest are NULL).
fn call(db: &rusqlite::Connection, id: &str, started_at: &str) {
    db.execute(
        "INSERT INTO calls (call_id, session_id, turn_id, iter, trigger, started_at, model) \
         VALUES (?1, ?2, ?3, 0, 'curate', ?4, '')",
        rusqlite::params![id, SESSION, format!("{SESSION}#{id}"), started_at],
    )
    .expect("a calls row");
}

/// One `marks` row of the trail's session, each in a turn of its own.
fn mark(db: &rusqlite::Connection, seq: i64, kind: &str, value: &str, at: &str) {
    mark_in(
        db,
        seq,
        kind,
        value,
        at,
        SESSION,
        &format!("{SESSION}#{seq}"),
    );
}

/// One `marks` row, the columns every mark carries.
fn mark_in(
    db: &rusqlite::Connection,
    seq: i64,
    kind: &str,
    value: &str,
    at: &str,
    session: &str,
    turn: &str,
) {
    db.execute(
        "INSERT INTO marks (seq, session_id, turn_id, kind, value, at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![seq, session, turn, kind, value, at],
    )
    .expect("a marks row");
}

fn gap_value() -> String {
    json!({"engine": "", "id": "call-gap", "searched": 1, "text": GAP_WORDS}).to_string()
}

/// The trail of the lock: in `[FROM, TO)` twelve calls (one exactly on
/// `FROM`), three `gap`, two `reread`, one `tool_error`, one `ask` and one
/// `correction` (one `reread` exactly on `FROM`), and outside it one call and
/// one mark exactly on `TO` and one of each a second before `FROM`. The newest
/// mark inside is a gap with its words.
fn sow(db: &rusqlite::Connection) {
    call(db, "c-from", "2026-09-01T00:00:00.000000Z");
    for i in 0..11u32 {
        call(db, &format!("c{i}"), &at(2 + i % 5, 1 + i, 0));
    }
    call(db, "c-to", "2026-09-08T00:00:00.000000Z");
    call(db, "c-before", "2026-08-31T23:59:59.000000Z");
    mark(
        db,
        1,
        "reread",
        &"a".repeat(64),
        "2026-09-01T00:00:00.000000Z",
    );
    mark(db, 2, "reread", &"b".repeat(64), &at(2, 9, 0));
    mark(db, 3, "tool_error", "file_read:not_found", &at(3, 9, 0));
    mark(db, 4, "gap", &gap_value(), &at(4, 9, 0));
    mark(db, 5, "gap", &gap_value(), &at(5, 9, 0));
    mark(db, 6, "gap", &gap_value(), &at(7, 23, 999_999));
    mark(db, 7, "gap", &gap_value(), "2026-09-08T00:00:00.000000Z");
    mark(db, 8, "tool_error", "x:y", "2026-08-31T23:59:59.000000Z");
    // The two kinds the ledger's writers learnt last (OR-BD-66), each counted
    // once: older than every gap, so the newest samples stay the gaps'.
    mark(db, 10, "ask", "ask_requester", &at(3, 12, 0));
    mark(db, 11, "correction", "", &at(4, 12, 0));
}

fn counts_of(answer: &Value) -> Value {
    answer["stats"]["counts"].clone()
}

fn full_counts() -> Value {
    json!({"calls": 12, "gap": 3, "reread": 2, "ask": 1, "correction": 1, "tool_error": 1})
}

/// The cogny curator's own trail, unlike the talky one in every count: two
/// calls and one `ask`, under a session of a chat channel.
fn sow_cogny(db: &rusqlite::Connection) {
    call(db, "k1", &at(3, 8, 0));
    call(db, "k2", &at(3, 9, 0));
    mark_in(
        db,
        1,
        "ask",
        "ask_requester",
        &at(3, 9, 30),
        CHAT_SESSION,
        &format!("{CHAT_SESSION}#1"),
    );
}

fn cogny_counts() -> Value {
    json!({"calls": 2, "gap": 0, "reread": 0, "ask": 1, "correction": 0, "tool_error": 0})
}

fn no_counts() -> Value {
    json!({"calls": 0, "gap": 0, "reread": 0, "ask": 0, "correction": 0, "tool_error": 0})
}

/// No sample field holds a ledger id or a run of the channel's digits.
fn assert_no_ledger_ids(samples: &[Value], stored: &[&str]) {
    for s in samples {
        for (key, v) in s.as_object().expect("a sample is an object") {
            let v = v.as_str().unwrap_or_default();
            for id in stored {
                assert!(
                    !v.contains(id),
                    "sample `{key}` carries the ledger's {id:?}: {s}"
                );
            }
            for digits in [CHANNEL_DIGITS, CHAT_DIGITS] {
                assert!(
                    !v.contains(&digits[digits.len() - 7..]),
                    "sample `{key}` carries the channel's digits: {s}"
                );
            }
        }
    }
}

/// The form the asker writes against (README § 2.2, OR-BD-62), on an answer
/// and a refusal alike: the body is exactly `{messages, stats}` and `stats`
/// exactly `{from, to, counts, samples, truncated}` -- a key more or less is
/// a seam broken on the other side.
fn assert_contract_form(body: &Value) {
    let keys = |v: &Value| -> Vec<String> {
        let mut k: Vec<String> = v
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        k.sort();
        k
    };
    assert_eq!(keys(body), ["messages", "stats"], "{body}");
    assert_eq!(
        keys(&body["stats"]),
        ["counts", "from", "samples", "to", "truncated"],
        "{body}"
    );
}

// ──────────────────────────────────────────────────── the hive, in-process

fn ask_hop(tag: &str, from: &str, to: &str, kinds: &str, samples: Value) -> Value {
    let mut h = json!({"stats_tag": tag, "stats_from": from, "stats_to": to,
                       "stats_kinds": kinds});
    if !samples.is_null() {
        h["stats_samples"] = samples;
    }
    h
}

/// One question on the hive's `in_stats`; the one message it left on `.`.
fn ask(h: &mut Hive, hop: Value) -> Msg {
    h.out.clear();
    h.lane("in_stats", json!({}), hop, json!({"messages": []}));
    assert_eq!(h.out.len(), 1, "one answer per question: {:?}", h.out);
    let m = h.out[0].clone();
    assert_eq!(m.route(), "stats");
    assert_contract_form(&Value::Object(m.body.clone()));
    m
}

fn answer_value(m: &Msg) -> Value {
    json!({"hop": m.hop.clone(), "stats": m.body["stats"].clone(),
           "messages": m.body["messages"].clone()})
}

#[test]
fn a_question_that_does_not_hold_is_answered_and_reads_nothing() {
    if !shipped() {
        return;
    }
    let long = "x".repeat(65);
    let table: Vec<(Value, &str)> = vec![
        (json!({"stats_tag": ""}), "stats_tag"),
        (json!({"stats_tag": long}), "stats_tag"),
        (json!({"stats_from": "2026-09-01"}), "stats_from"),
        (json!({"stats_to": "next week"}), "stats_to"),
        (json!({"stats_from": TO, "stats_to": FROM}), "stats_from"),
        (json!({"stats_to": "2026-10-02T00:00:01Z"}), "stats_to"),
        (json!({"stats_kinds": "[]"}), "stats_kinds"),
        (
            json!({"stats_kinds": r#"["calls","messages"]"#}),
            "stats_kinds",
        ),
        (json!({"stats_samples": 51}), "stats_samples"),
        (json!({"stats_samples": -1}), "stats_samples"),
    ];
    for (over, key) in table {
        let mut h = Hive::new();
        let mut hop = ask_hop("bad", FROM, TO, ALL_KINDS, Value::Null);
        for (k, v) in as_map(&over) {
            hop[k] = v;
        }
        let m = ask(&mut h, hop);
        assert_eq!(m.hop["error_code"], "invalid_input", "{over}");
        assert_eq!(m.hop["detail"], key, "{over}");
        assert_eq!(m.body["stats"]["counts"], json!({}), "{over}");
        assert_eq!(m.body["stats"]["samples"], json!([]), "{over}");
        assert_eq!(m.body["messages"], json!([]), "{over}");
        assert!(h.ledger_ops.is_empty(), "{over}: nothing read");
    }
    // A tag of 64 characters, exactly 31 days and a JSON number of samples
    // hold, and the window is echoed as it was asked.
    let mut h = Hive::new();
    let tag = "t".repeat(64);
    let m = ask(
        &mut h,
        ask_hop(&tag, FROM, "2026-10-02T00:00:00Z", ALL_KINDS, json!(50)),
    );
    assert_eq!(m.hop["error_code"], "", "{:?}", h.stderr);
    assert_eq!(m.hop["stats_tag"], json!(tag));
    assert_eq!(m.body["stats"]["from"], FROM);
    assert_eq!(m.body["stats"]["to"], "2026-10-02T00:00:00Z");
}

#[test]
fn the_window_counts_its_first_moment_and_not_its_last() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    sow(&h.db);
    let m = ask(&mut h, ask_hop("w", FROM, TO, ALL_KINDS, Value::Null));
    assert_eq!(m.hop["error_code"], "", "{:?}", h.stderr);
    assert_eq!(counts_of(&answer_value(&m)), full_counts());
    assert_eq!(m.body["stats"]["truncated"], false);
    assert_eq!(m.body["stats"]["samples"], json!([]));
    // `counts` holds exactly the kinds asked.
    let m = ask(&mut h, ask_hop("w2", FROM, TO, r#"["gap"]"#, Value::Null));
    assert_eq!(counts_of(&answer_value(&m)), json!({"gap": 3}));
    // One ledger round trip per question, reads only: the first question's
    // two legs (calls and marks) and the second's one.
    let stats_ops: Vec<&Value> = h
        .ledger_ops
        .iter()
        .filter(|(from, _)| from == "stats")
        .map(|(_, a)| a)
        .collect();
    assert_eq!(stats_ops.len(), 3, "{stats_ops:?}");
    assert!(stats_ops.iter().all(|a| a["operation"] == "select"));
    assert!(h.ledger_ops.iter().all(|(from, _)| from == "stats"));
}

#[test]
fn samples_are_the_newest_marks_and_carry_no_text() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    sow(&h.db);
    mark(
        &h.db,
        9,
        "correction",
        "rewrite the standing rule",
        &at(6, 9, 0),
    );
    let m = ask(&mut h, ask_hop("s", FROM, TO, ALL_KINDS, json!("3")));
    let samples = m.body["stats"]["samples"].as_array().cloned().unwrap();
    assert_eq!(samples.len(), 3, "{samples:?}");
    // Newest first: the gap at the window's last microsecond, the correction,
    // the gap of the fifth.
    assert_eq!(samples[0]["kind"], "gap");
    assert_eq!(samples[0]["at"], at(7, 23, 999_999));
    assert_eq!(
        samples[0]["value"], "1",
        "a gap's value is its `searched` flag"
    );
    assert_eq!(samples[1]["kind"], "correction");
    assert_eq!(samples[1]["value"], "", "a correction's value is empty");
    assert_eq!(samples[2]["at"], at(5, 9, 0));
    for s in &samples {
        let keys: Vec<&String> = s.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            ["at", "kind", "session_id", "turn_id", "value"],
            "{s}"
        );
    }
    assert_no_ledger_ids(&samples, &[SESSION]);
    let whole = answer_value(&m).to_string();
    assert!(
        !whole.contains(GAP_WORDS),
        "the gap's words stay in the ledger"
    );
    assert!(!whole.contains("standing rule"));
    assert!(!whole.contains("audience_set"));
    // A stored value is carried as stored for the other kinds.
    let m = ask(
        &mut h,
        ask_hop("s2", FROM, TO, r#"["tool_error","reread"]"#, json!(50)),
    );
    let values: Vec<Value> = m.body["stats"]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["value"].clone())
        .collect();
    assert_eq!(
        values,
        [
            json!("file_read:not_found"),
            json!("b".repeat(64)),
            json!("a".repeat(64))
        ]
    );
}

#[test]
fn a_sample_refers_to_its_session_and_turn_and_never_carries_their_ids() {
    if !shipped() {
        return;
    }
    // The ledger's ids are the session keeper's, `<channel>-<stamp>` and
    // `<session>#<n>`, and a channel is a phone number or a chat id: a sample
    // that carried them would hand an observer -- and the model it asks --
    // who was talking. Two sessions, one of them twice in one turn.
    let phone_turn_1 = format!("{SESSION}#1");
    let phone_turn_2 = format!("{SESSION}#2");
    let chat_turn = format!("{CHAT_SESSION}#4");
    let mut h = Hive::new();
    mark_in(
        &h.db,
        1,
        "gap",
        &gap_value(),
        &at(2, 10, 0),
        SESSION,
        &phone_turn_1,
    );
    mark_in(
        &h.db,
        2,
        "reread",
        &"c".repeat(64),
        &at(3, 10, 0),
        SESSION,
        &phone_turn_1,
    );
    mark_in(
        &h.db,
        3,
        "tool_error",
        "file_read:not_found",
        &at(4, 10, 0),
        CHAT_SESSION,
        &chat_turn,
    );
    mark_in(
        &h.db,
        4,
        "ask",
        "ask_requester",
        &at(5, 10, 0),
        SESSION,
        &phone_turn_2,
    );
    let refs = |m: &Msg| -> Vec<(Value, Value)> {
        m.body["stats"]["samples"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|s| (s["session_id"].clone(), s["turn_id"].clone()))
            .collect()
    };
    let m = ask(&mut h, ask_hop("ids", FROM, TO, ALL_KINDS, json!(50)));
    assert_eq!(m.hop["error_code"], "", "{:?}", h.stderr);
    // Newest first: the ask (phone, its second turn), the tool error (chat),
    // the reread and the gap (phone, one turn). A reference per session in the
    // order the samples first name it, a turn numbered within its session the
    // same way: one session, one reference; one turn, one reference.
    assert_eq!(
        refs(&m),
        [
            (json!("s1"), json!("s1#1")),
            (json!("s2"), json!("s2#1")),
            (json!("s1"), json!("s1#2")),
            (json!("s1"), json!("s1#2")),
        ]
    );
    let samples = m.body["stats"]["samples"].as_array().cloned().unwrap();
    assert_no_ledger_ids(
        &samples,
        &[
            SESSION,
            CHAT_SESSION,
            &phone_turn_1,
            &phone_turn_2,
            &chat_turn,
        ],
    );
    let whole = answer_value(&m).to_string();
    for digits in [CHANNEL_DIGITS, CHAT_DIGITS] {
        assert!(
            !whole.contains(digits),
            "the channel left the ledger: {whole}"
        );
    }
    // The same question, the same references: they are a function of the
    // answer, not a counter that runs on.
    let again = ask(&mut h, ask_hop("ids2", FROM, TO, ALL_KINDS, json!(50)));
    assert_eq!(refs(&again), refs(&m));
}

#[test]
fn a_window_larger_than_the_budget_says_truncated() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("stats", "scan_budget", json!(3))]);
    for i in 0..10u32 {
        call(&h.db, &format!("c{i}"), &at(2, i, 0));
    }
    let m = ask(&mut h, ask_hop("b", FROM, TO, r#"["calls"]"#, Value::Null));
    assert_eq!(m.body["stats"]["truncated"], true);
    assert_eq!(counts_of(&answer_value(&m)), json!({"calls": 3}));
    // Three rows inside and the next one past `TO`: the window ended within
    // the budget, nothing was left uncounted.
    let mut h = Hive::with(&[("stats", "scan_budget", json!(3))]);
    for i in 0..3u32 {
        call(&h.db, &format!("c{i}"), &at(2, i, 0));
    }
    call(&h.db, "c-to", "2026-09-08T00:00:00.000000Z");
    let m = ask(&mut h, ask_hop("b2", FROM, TO, r#"["calls"]"#, Value::Null));
    assert_eq!(m.body["stats"]["truncated"], false);
    assert_eq!(counts_of(&answer_value(&m)), json!({"calls": 3}));
    // The samples of a cut window are the newest of what was counted -- the
    // window's oldest part, since the read ascends from `FROM` -- not the
    // newest of the window (the `because` of `stats` says so).
    let mut h = Hive::with(&[("stats", "scan_budget", json!(3))]);
    for i in 0..10u32 {
        mark(&h.db, i64::from(i) + 1, "gap", "{}", &at(2, i, 0));
    }
    let m = ask(&mut h, ask_hop("b3", FROM, TO, r#"["gap"]"#, json!(2)));
    assert_eq!(m.body["stats"]["truncated"], true);
    assert_eq!(counts_of(&answer_value(&m)), json!({"gap": 3}));
    let ats: Vec<Value> = m.body["stats"]["samples"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| s["at"].clone())
        .collect();
    assert_eq!(ats, [json!(at(2, 2, 0)), json!(at(2, 1, 0))]);
}

#[test]
fn a_ledger_that_refuses_a_read_is_answered_with_store_error() {
    if !shipped() {
        return;
    }
    // Both legs in one bundle, the `marks` table gone: the store refuses
    // that leg, and the answer names the table, not a hop key.
    let mut h = Hive::new();
    h.ledger_may_refuse = true;
    sow(&h.db);
    h.db.execute_batch("DROP TABLE marks").expect("drop marks");
    let m = ask(&mut h, ask_hop("gone", FROM, TO, ALL_KINDS, json!(5)));
    assert_eq!(m.hop["error_code"], "store_error", "{:?}", h.stderr);
    assert_eq!(m.hop["detail"], "marks");
    assert_eq!(m.hop["stats_tag"], "gone");
    assert_eq!(
        m.body["stats"],
        json!({"from": FROM, "to": TO, "counts": {}, "samples": [], "truncated": false})
    );
    assert_eq!(m.body["messages"], json!([]));
    // One read alone (no bundle), the `calls` table gone: the same answer,
    // its own table named.
    let mut h = Hive::new();
    h.ledger_may_refuse = true;
    h.db.execute_batch("DROP TABLE calls").expect("drop calls");
    let m = ask(
        &mut h,
        ask_hop("gone2", FROM, TO, r#"["calls"]"#, Value::Null),
    );
    assert_eq!(m.hop["error_code"], "store_error", "{:?}", h.stderr);
    assert_eq!(m.hop["detail"], "calls");
    assert_eq!(m.body["stats"]["counts"], json!({}));
}

// ──────────────────────────────────────────────── the colony, at the receiver

fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(
        depth < 8,
        "template ref chain does not terminate at {}",
        src.display()
    );
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .expect("a ref names a template");
            let name = reference.split('@').next().unwrap_or_default();
            copy_resolved(&repo("templates").join(name), dst, depth + 1);
            if let Some(over) = cfg["override_params"].as_object() {
                for (cell, params) in over {
                    override_params_on_disk(&dst.join(cell), params);
                }
            }
            return;
        }
    }
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_resolved(&from, &dst.join(name), depth);
        } else if name == "config.json"
            || (src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl"))
        {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).expect("write");
}

fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "seed") {
                configs_under(&p, out);
            }
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// Every `llm` cell at the local stub, every timer out of the run's way
/// (the sweep of `gh889`/`gh919`), every `${VAR}` bound to a dummy.
fn quiet_the_tree(root: &std::path::Path, stub: &str) {
    let main = root.join("main");
    let mut files = Vec::new();
    configs_under(&main, &mut files);
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    let mut n: u64 = 0;
    for f in files {
        let raw = std::fs::read_to_string(&f).unwrap_or_default();
        let mut rest = raw.as_str();
        while let Some(start) = rest.find("${") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            let name = &rest[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                vars.insert(name.to_string(), format!("dummy-{name}"));
            }
            rest = &rest[end + 1..];
        }
        let mut cfg = read_json(&f);
        if cfg["cell"]["type"] == "llm" {
            cfg["params"]["base_url"] = json!(stub);
            cfg["params"]["model"] = json!("stub");
            cfg["params"]["api_key"] = json!("sk-test");
            write_json(&f, &cfg);
        } else if cfg["cell"]["type"] == "timer"
            && let Some(schedules) = cfg["params"]["schedules"].as_array_mut()
        {
            for s in schedules.iter_mut() {
                n += 1;
                if s["schedule_id"]
                    .as_str()
                    .is_some_and(|id| id.contains("${"))
                {
                    s["schedule_id"] =
                        json!(format!("0190a3f2-0000-7000-8000-{:012x}", 0x0926_0000 + n));
                }
                if s.get("cron").is_some() {
                    s["cron"] = json!(NEVER_CRON);
                }
            }
            write_json(&f, &cfg);
        }
    }
    vars.insert("OPENROUTER_API_KEY".into(), "test-key".into());
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("write the env file");
}

/// The generation `scribe` at `main/scribe`; its `stats` goes to the sink,
/// everything else it raises to the park. `talky-chat`'s counter reads three
/// rows at most.
fn build(td: &tempfile::TempDir, stub: &str) {
    let main = td.path().join("main");
    copy_resolved(&repo("templates/assistant"), &main.join("scribe"), 0);
    override_params_on_disk(
        &main.join("scribe/talky-chat/curator/stats"),
        &json!({"scan_budget": 3}),
    );
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./scribe", "to": "/sink",
             "condition": "has(hop.route) && hop.route == 'stats'"},
            {"from": "./scribe", "to": "/park", "condition": "has(hop.route)",
             "default": true}
        ]}}}),
    );
    quiet_the_tree(td.path(), stub);
}

struct Ports {
    sink: mpsc::Receiver<Message>,
    #[allow(dead_code)]
    park: mpsc::Receiver<Message>,
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
            ("bash".to_string(), Arc::new(BashCellFactory)),
            ("edit".to_string(), Arc::new(EditCellFactory)),
            ("file".to_string(), Arc::new(FileCellFactory)),
            ("web_fetch".to_string(), Arc::new(WebFetchCellFactory)),
            ("web_search".to_string(), Arc::new(WebSearchCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(1024);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the generation must boot");
    (
        h,
        Ports {
            sink: sink_rx,
            park: park_rx,
        },
    )
}

/// A question at the generation's path; `role` None sends none.
fn question(tag: &str, role: Option<&str>, from: &str, to: &str, samples: i64) -> Message {
    let mut hop = ask_hop(tag, from, to, ALL_KINDS, json!(samples));
    hop["route"] = json!("in_stats");
    if let Some(r) = role {
        hop["stats_role"] = json!(r);
    }
    MessageBuilder::new(Path::new("/scribe"))
        .hop(as_map(&hop))
        .context(as_map(&json!({"asker": "observer"})))
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

/// The answer the sink received: `{hop, stats, messages}`.
async fn answered(ports: &mut Ports, tag: &str) -> Value {
    let msg = tokio::time::timeout(DEADLINE, ports.sink.recv())
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("no `stats` answer reached the sink for `{tag}`"));
    let body = match &msg.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(id) => panic!("an inline body was expected, got blob {id}"),
    };
    assert_contract_form(&body);
    let answer = json!({"hop": msg.headers.hop.clone(), "stats": body["stats"].clone(),
                        "messages": body["messages"].clone(),
                        "context": msg.headers.context.clone()});
    assert_eq!(answer["hop"]["stats_tag"], tag, "{answer}");
    answer
}

fn ledger_db(td: &tempfile::TempDir, brain: &str) -> rusqlite::Connection {
    let path = td
        .path()
        .join(format!("main/scribe/{brain}/curator/ledger/cell.db"));
    let db =
        rusqlite::Connection::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    db.busy_timeout(Duration::from_secs(5))
        .expect("busy timeout");
    db
}

/// `stats_tag` of every `stats` delivery into the sink the colony logged.
fn sink_deliveries(root: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT to_path, headers FROM message_log ORDER BY rowid")
        .expect("message_log");
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query")
        .filter_map(Result::ok)
        .filter(|(to, _)| to == "/sink")
        .map(|(_, headers)| {
            let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
            h["hop"]["stats_tag"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh926_a_curator_counts_its_own_ledger() {
    if !shipped() {
        return;
    }
    let stub = MockOpenAI::start(vec![canned_chat_completion("unused", "stop")]).await;
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &stub.base_url);
    let (h, mut ports) = boot(&td).await;

    // The first question of each counter warms its ledger, so the file exists
    // before a row is written.
    for (tag, role) in [("warm-talky", "talky"), ("warm-chat", "talky-chat")] {
        h.send(question(tag, Some(role), FROM, TO, 0)).await;
        let a = answered(&mut ports, tag).await;
        assert_eq!(a["stats"]["counts"], no_counts(), "{a}");
    }
    sow(&ledger_db(&td, "talky"));
    sow(&ledger_db(&td, "talky-chat"));

    // 5. The cogny curator, reached by its role. Asked while the talky ledger
    //    already holds its trail, it answers out of its own, still empty one --
    //    a cogny question bent onto `./talky` would answer the talky counts
    //    here.
    h.send(question("cogny-first", Some("cogny"), FROM, TO, 0))
        .await;
    let a = answered(&mut ports, "cogny-first").await;
    assert_eq!(a["hop"]["error_code"], "", "{a}");
    assert_eq!(
        a["stats"]["counts"],
        no_counts(),
        "the cogny role was answered out of a ledger that is not cogny's: {a}"
    );
    //    And with a trail of its own, unlike talky's in every count, that is
    //    the trail it counts.
    sow_cogny(&ledger_db(&td, "cogny"));
    h.send(question("cogny-trail", Some("cogny"), FROM, TO, 1))
        .await;
    let a = answered(&mut ports, "cogny-trail").await;
    assert_eq!(
        a["stats"]["counts"],
        cogny_counts(),
        "the cogny role was answered out of a ledger that is not cogny's: {a}"
    );
    assert_eq!(a["stats"]["samples"][0]["kind"], "ask", "{a}");
    assert_eq!(
        (
            a["stats"]["samples"][0]["session_id"].clone(),
            a["stats"]["samples"][0]["turn_id"].clone()
        ),
        (json!("s1"), json!("s1#1")),
        "{a}"
    );
    assert!(!a.to_string().contains(CHAT_DIGITS), "{a}");

    // 1. The talky curator through the assistant lane: exact counts, the row on
    //    `FROM` counted, the row on `TO` not.
    h.send(question("talky-window", Some("talky"), FROM, TO, 0))
        .await;
    let a = answered(&mut ports, "talky-window").await;
    assert_eq!(a["hop"]["error_code"], "", "{a}");
    assert_eq!(a["stats"]["counts"], full_counts(), "{a}");
    assert_eq!(a["stats"]["truncated"], false);
    assert_eq!(
        (a["stats"]["from"].clone(), a["stats"]["to"].clone()),
        (json!(FROM), json!(TO))
    );
    assert_eq!(a["messages"], json!([]));
    // The asker's context rides back; nothing of the hive's interior does.
    assert_eq!(a["context"]["asker"], "observer");
    for key in ["cur_origin", "cur_phase", "cur_call", "cur_reason"] {
        assert!(a["context"].get(key).is_none(), "{key} left the hive: {a}");
    }

    // 2. Two samples: the newest two marks, metadata only.
    h.send(question("talky-samples", None, FROM, TO, 2)).await;
    let a = answered(&mut ports, "talky-samples").await;
    let samples = a["stats"]["samples"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(samples.len(), 2, "{a}");
    assert_eq!(samples[0]["kind"], "gap");
    assert_eq!(samples[0]["at"], at(7, 23, 999_999));
    for s in &samples {
        assert!(
            s.get("text").is_none() && s.get("audience_set").is_none(),
            "{s}"
        );
    }
    assert!(
        !a.to_string().contains(GAP_WORDS),
        "the gap's words left the ledger: {a}"
    );
    // Two marks of one session in two turns: one session reference, two turn
    // references, and nothing of the channel the ledger's ids are made of.
    assert_eq!(
        samples
            .iter()
            .map(|s| (s["session_id"].clone(), s["turn_id"].clone()))
            .collect::<Vec<_>>(),
        [(json!("s1"), json!("s1#1")), (json!("s1"), json!("s1#2"))],
        "{a}"
    );
    assert!(
        !a.to_string().contains(CHANNEL),
        "the channel left the ledger: {a}"
    );

    // 3. A counter with a budget of three over the same trail.
    h.send(question("chat-budget", Some("talky-chat"), FROM, TO, 0))
        .await;
    let a = answered(&mut ports, "chat-budget").await;
    assert_eq!(a["stats"]["truncated"], true, "{a}");

    // 4. A window upside down is refused, and the refusal names the key.
    h.send(question("upside-down", Some("talky"), TO, FROM, 0))
        .await;
    let a = answered(&mut ports, "upside-down").await;
    assert_eq!(a["hop"]["error_code"], "invalid_input", "{a}");
    assert_eq!(a["hop"]["detail"], "stats_from");
    assert_eq!(a["stats"]["counts"], json!({}));

    h.shutdown().await;
    // One answer per question, and no other message at the sink.
    let mut tags = sink_deliveries(td.path());
    tags.sort();
    assert_eq!(
        tags,
        [
            "chat-budget",
            "cogny-first",
            "cogny-trail",
            "talky-samples",
            "talky-window",
            "upside-down",
            "warm-chat",
            "warm-talky"
        ]
    );
}
