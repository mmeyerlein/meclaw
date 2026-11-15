//! d58-3 suche (GH #1531) -- `history_search` finds every row of the wall.
//!
//! WHY this file exists: the search read the wall page by page, newest
//! first, and stopped at `scan_budget` rows (5000 shipped). A question about
//! the oldest turn of a long wall -- the one a person asks after weeks -- was
//! answered with `truncated_scan` and no hit, and the model told the person
//! it never heard of it. The ledger now carries a full-text index over the
//! text of its blocks, and the search asks the store for the rows that hold
//! the words instead of reading the wall row by row.
//!
//! The pins:
//! - the oldest turn of a wall of more than 5000 rows is found, with the
//!   shipped params and no cut;
//! - a row of another round, or of none, is never a hit and leaves no trace
//!   in the answer (regression guard: the index must not bypass the audience
//!   gate);
//! - a wall written before the index existed is indexed by the store's own
//!   start steps and searched like a new one (backfill, no hand step);
//! - a regular expression takes its candidates out of the index by its
//!   literal parts and checks each of them against the pattern; a pattern
//!   without a literal is refused with `needs_literal`, never cut silently.
//!
//! The history cell runs here the way `gh1225_kh_history_reads_the_whole_wall`
//! runs it: the shipped script, its store messages answered by the store
//! cell's own dispatcher on the hive's ledger. No model is called.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
use curator_hive::*;

use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::collections::VecDeque;

const CALL: &str = "call-d583-suche";

/// Rows of filler the long walls carry above their oldest turn: more than
/// the 5000 rows the shipped `scan_budget` let a search read.
const FILLER: i64 = 5200;

/// One cell's step, the way the hive's code runner hands it a message.
fn step(script: &str, params: &Map<String, Value>, msg: &Msg) -> Vec<Msg> {
    let doc = json!({
        "envelope": {"header": {"context": msg.context, "hop": msg.hop},
                     "target": "/x/curator/history",
                     "reply_to": msg.reply_to},
        "body": msg.body,
        "params": params,
    });
    let out = run_python(script, &doc.to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "history exited non-zero: {err}");
    let emitted: Value = sj::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "history: output is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    let list = match emitted {
        Value::Array(a) => a,
        other => vec![other],
    };
    list.into_iter()
        .map(|m| {
            let mut body = obj(m);
            let hop = body
                .remove("header")
                .and_then(|h| h.as_object().cloned())
                .unwrap_or_default();
            Msg {
                context: msg.context.clone(),
                hop,
                body,
                reply_to: String::new(),
            }
        })
        .collect()
}

/// The ledger's answer to one store message, in the store cell's own reply
/// shape. A refused op fails the test where it happens.
fn ledger_answer(db: &rusqlite::Connection, msg: &Msg) -> Msg {
    use meclaw_cells::store::ops::dispatch;
    use meclaw_cells::store::output::{BundleLeg, build_bundle_result, build_tool_result};
    let calls: Vec<Value> = msg
        .messages()
        .into_iter()
        .filter(|m| m["type"] == "tool_call")
        .collect();
    let op =
        |c: &Value| -> Value { sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json") };
    let (body, hop) = if calls.len() == 1 {
        let c = &calls[0];
        let outcome = dispatch(db, &op(c))
            .unwrap_or_else(|e| panic!("a ledger op was refused ({e}): {}", c["text"]));
        build_tool_result(&outcome, c["id"].as_str().unwrap_or("").to_string(), 0)
    } else {
        let legs: Vec<BundleLeg> = calls
            .iter()
            .map(|c| {
                let outcome = dispatch(db, &op(c))
                    .unwrap_or_else(|e| panic!("a ledger op was refused ({e}): {}", c["text"]));
                BundleLeg::from_outcome(&outcome, c["id"].as_str().unwrap_or("").to_string(), 0)
            })
            .collect();
        build_bundle_result(&legs, 0)
    };
    for r in body["results"].as_array().cloned().unwrap_or_default() {
        assert!(
            r["error_code"].as_str().is_none(),
            "a ledger op failed: {r}"
        );
    }
    Msg {
        context: msg.context.clone(),
        hop,
        body: obj(body),
        reply_to: String::new(),
    }
}

/// One `history_search` call in the round `ROUND_E`, during a turn of
/// session `s-now`, with the shipped params. Returns the answer's
/// `hop.error_code`, its payload and the raw text of the answer.
fn search(h: &Hive, args: Value) -> (String, Value, String) {
    let cfg = cell_config("history");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut params = obj(cfg["params"].clone());
    params.remove("script_inline");
    let hop = json!({"route": "in_history_call", "tool_name": "history_search",
                     "tool_call_id": CALL});
    let first = Msg {
        context: obj(
            json!({"session_id": "s-now", "turn_id": "t-now", "iter": "1",
                            "curator_call": "cc-d583-suche", "audience_set": ROUND_E}),
        ),
        hop: obj(hop),
        body: obj(
            json!({"messages": [{"origin": "assistant", "type": "tool_call",
                                       "id": CALL, "text": args.to_string()}]}),
        ),
        reply_to: String::new(),
    };
    let mut queue = VecDeque::from([first]);
    let mut answers = Vec::new();
    let mut steps = 0;
    while let Some(msg) = queue.pop_front() {
        steps += 1;
        assert!(steps < 200, "the history call does not come to rest");
        for out in step(&script, &params, &msg) {
            match out.route() {
                "lstore" => {
                    let mut req = out.clone();
                    for (key, from) in [
                        ("cur_origin", "reply_cell"),
                        ("cur_phase", "phase"),
                        ("cur_call", "cur_call"),
                        ("cur_reason", "cur_reason"),
                    ] {
                        let v = out.hop.get(from).cloned().unwrap_or_else(|| {
                            panic!("a store message without `hop.{from}`: {:?}", out.hop)
                        });
                        req.context.insert(key.to_string(), v);
                    }
                    queue.push_back(ledger_answer(&h.db, &req));
                }
                "tool_result" => answers.push(out),
                other => panic!("history emitted on `{other}`: {:?}", out.hop),
            }
        }
    }
    assert_eq!(answers.len(), 1, "one call, one answer");
    let a = &answers[0];
    let code = a
        .hop
        .get("error_code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let turns = a.messages();
    assert_eq!(turns.len(), 1, "one tool_result turn: {turns:?}");
    let text = turns[0]["text"].as_str().expect("a text").to_string();
    let payload = sj::from_str(&text).expect("one JSON object");
    (code, payload, text)
}

fn stamp(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .expect("an RFC 3339 stamp")
        .with_timezone(&chrono::Utc)
}

/// `n` rows of everyday talk, one second apart from `from` on, alternating
/// user and assistant, each its own block, none holding a word the pins
/// search for.
fn filler(h: &mut Hive, from: &str, n: i64) {
    let t0 = stamp(from);
    for i in 0..n {
        let at = t0 + chrono::Duration::seconds(i);
        let session = format!("s-day{}", i / 400);
        let turn = format!("t-{i}");
        if i % 2 == 0 {
            let el = user(&format!("line {i}: how was the walk along the river today"));
            h.row(at, &session, &turn, "user", &el, 1);
        } else {
            let el = said(&format!(
                "line {i}: the walk was fine, the weather stayed mild"
            ));
            h.row(at, &session, &turn, "assistant", &el, 1);
        }
    }
}

/// The ids of the hits of an answer, in its order.
fn hit_ids(p: &Value) -> Vec<String> {
    p["hits"]
        .as_array()
        .unwrap_or_else(|| panic!("hits: {p}"))
        .iter()
        .map(|x| x["id"].as_str().unwrap_or("").to_string())
        .collect()
}

fn short(hash: &str) -> String {
    format!("#{}", &hash[..12])
}

/// The oldest turn of a wall longer than the shipped `scan_budget` is found:
/// one hit, the oldest row, no cut.
#[test]
fn gh_d583_suche_the_oldest_turn_is_found() {
    let mut h = Hive::new();
    let (_, oldest) = h.row(
        stamp("2026-09-01T08:00:00Z"),
        "s-first",
        "t-0",
        "user",
        &user("my first dog was called Quastenflosser, a grey terrier"),
        1,
    );
    filler(&mut h, "2026-09-02T00:00:00Z", FILLER);
    let (code, p, _) = search(&h, json!({"query": "Quastenflosser"}));
    assert_eq!(code, "", "an answer, never a refusal: {p}");
    assert_eq!(
        hit_ids(&p),
        [short(&oldest)],
        "the oldest turn is the one hit: {p}"
    );
    assert_eq!(p["total_hits"], json!(1), "{p}");
    assert_eq!(p["truncated_scan"], json!(false), "nothing is cut: {p}");
    assert!(p.get("cut_by").is_none(), "no cut names a budget: {p}");
    assert_eq!(p["hits"][0]["session_id"], json!("s-first"), "{p}");
}

/// A row of another round and a row that declares no round are never hits,
/// and neither their ids nor their words reach the answer -- only the row
/// the round may see is found.
#[test]
fn gh_d583_suche_a_hidden_row_is_never_found() {
    let mut h = Hive::new();
    let (_, other_round) = h.row_under(
        Some("[\"member:x\"]"),
        stamp("2026-09-01T08:00:00Z"),
        "s-x",
        "t-x",
        "user",
        &user("the Quastenflosser secret of another member: alpha"),
        1,
    );
    let (_, no_round) = h.row_under(
        None,
        stamp("2026-09-01T08:00:01Z"),
        "s-old",
        "t-old",
        "user",
        &user("a Quastenflosser note of no round: beta"),
        1,
    );
    let (_, visible) = h.row(
        stamp("2026-09-01T08:00:02Z"),
        "s-e",
        "t-e",
        "user",
        &user("the Quastenflosser of this round: gamma"),
        1,
    );
    filler(&mut h, "2026-09-02T00:00:00Z", 50);
    let (code, p, text) = search(&h, json!({"query": "Quastenflosser", "context": 2}));
    assert_eq!(code, "", "an answer, never a refusal: {p}");
    assert_eq!(hit_ids(&p), [short(&visible)], "only the visible row: {p}");
    assert_eq!(p["total_hits"], json!(1), "a hidden row is no count: {p}");
    for hidden in [&other_round, &no_round] {
        assert!(
            !text.contains(&hidden[..12]),
            "a hidden row's id is nowhere in the answer: {text}"
        );
    }
    for word in ["alpha", "beta", "another member", "no round"] {
        assert!(
            !text.contains(word),
            "a hidden row's words are nowhere in the answer ({word}): {text}"
        );
    }
}

/// A wall written before the ledger carried its full-text index -- the
/// ledger as it stood, without the index declaration -- is indexed by the
/// store's own start steps (schema, indexes, canonical, full text: the order
/// the store cell runs at wake) and its oldest turn is found like on a new
/// wall. No hand step, no rewrite of a row.
#[test]
fn gh_d583_suche_backfill_indexes_an_existing_wall() {
    let mut h = Hive::new();
    // The ledger as it stood before this strand: the shipped declaration
    // without a full-text index, built the way the store builds it.
    let mut before = obj(cell_config("ledger")["params"].clone());
    before.remove("fts");
    let before = meclaw_cells::store::StoreParams::parse(&Value::Object(before))
        .expect("the ledger params without an index parse");
    let old = rusqlite::Connection::open_in_memory().expect("sqlite");
    meclaw_cells::store::query::install_connection_extensions(&old).expect("extensions");
    meclaw_cells::store::ddl::apply_declared_schema_ddl(&old, &before).expect("ddl");
    meclaw_cells::store::ddl::apply_index_ddl(&old, &before.indexes).expect("index ddl");
    h.db = old;
    let (_, oldest) = h.row(
        stamp("2026-09-01T08:00:00Z"),
        "s-first",
        "t-0",
        "user",
        &user("we planted the Mirabellenbaum behind the shed"),
        1,
    );
    filler(&mut h, "2026-09-02T00:00:00Z", FILLER);
    // The next wake: the shipped declaration, the store's start steps.
    let shipped = meclaw_cells::store::StoreParams::parse(&cell_config("ledger")["params"])
        .expect("the shipped ledger params parse");
    meclaw_cells::store::ddl::apply_declared_schema_ddl(&h.db, &shipped).expect("ddl");
    meclaw_cells::store::ddl::apply_index_ddl(&h.db, &shipped.indexes).expect("index ddl");
    meclaw_cells::store::ddl::apply_canonical_ddl(&h.db, &shipped.canonical)
        .expect("canonical ddl");
    meclaw_cells::store::ddl::apply_fts_ddl(&h.db, &shipped.fts, &shipped.canonical)
        .expect("fts ddl");
    let (code, p, _) = search(&h, json!({"query": "Mirabellenbaum"}));
    assert_eq!(code, "", "an answer, never a refusal: {p}");
    assert_eq!(
        hit_ids(&p),
        [short(&oldest)],
        "a row written before the index is found: {p}"
    );
    assert_eq!(p["truncated_scan"], json!(false), "nothing is cut: {p}");
}

/// A regular expression takes its candidates out of the index by its literal
/// parts and checks each against the whole pattern: two old rows both hold
/// the literal `dentist appointment on ` and so are both candidates, only the one
/// the pattern matches is a hit, however many rows lie above them -- a search
/// that took every candidate for a hit would answer two. A pattern with no
/// literal to look up is refused with `needs_literal` -- the answer says so
/// instead of a silent cut.
#[test]
fn gh_d583_suche_regex_checks_index_candidates() {
    let mut h = Hive::new();
    let (_, dated) = h.row(
        stamp("2026-09-01T08:00:00Z"),
        "s-first",
        "t-0",
        "user",
        &user("A dentist appointment on 12.03. in the old town"),
        1,
    );
    let (_, undated) = h.row(
        stamp("2026-09-01T08:00:01Z"),
        "s-first",
        "t-1",
        "user",
        &user("dentist appointment on Tuesday, date still open"),
        1,
    );
    filler(&mut h, "2026-09-02T00:00:00Z", FILLER);
    let (code, p, text) = search(
        &h,
        json!({"query": r"dentist appointment on \d{2}\.\d{2}\.", "mode": "regex"}),
    );
    assert_eq!(code, "", "an answer, never a refusal: {p}");
    assert_eq!(
        hit_ids(&p),
        [short(&dated)],
        "the candidate the pattern matches is the one hit: {p}"
    );
    assert!(
        !text.contains(&undated[..12]),
        "a candidate the pattern does not match is no hit: {text}"
    );
    assert_eq!(p["total_hits"], json!(1), "one verified hit: {p}");
    assert_eq!(
        p["scanned"],
        json!(2),
        "both rows holding the literal were checked against the pattern: {p}"
    );
    assert_eq!(p["truncated_scan"], json!(false), "nothing is cut: {p}");

    let (code, p, _) = search(&h, json!({"query": r"\d{4}", "mode": "regex"}));
    assert_eq!(
        code, "needs_literal",
        "a pattern without a literal is refused, not cut: {p}"
    );
    assert_eq!(p["error"], json!("needs_literal"), "{p}");
}

/// A word inside a compound is found: the index takes every three characters
/// of the text as a term (`trigram`), so `arzt` finds the oldest turn's
/// `Zahnarzttermin` on a wall longer than the shipped `scan_budget` -- a
/// part of a word the old scan found and an index over whole words or word
/// starts would lose.
#[test]
fn gh_d583_suche_a_word_inside_a_compound_is_found() {
    let mut h = Hive::new();
    let (_, oldest) = h.row(
        stamp("2026-09-01T08:00:00Z"),
        "s-first",
        "t-0",
        "user",
        &user("next week I have a Zahnarzttermin on Tuesday"),
        1,
    );
    filler(&mut h, "2026-09-02T00:00:00Z", FILLER);
    let (code, p, _) = search(&h, json!({"query": "arzt"}));
    assert_eq!(code, "", "an answer, never a refusal: {p}");
    assert_eq!(
        hit_ids(&p),
        [short(&oldest)],
        "the compound in the oldest turn is the one hit: {p}"
    );
    assert_eq!(p["total_hits"], json!(1), "{p}");
    assert_eq!(p["truncated_scan"], json!(false), "nothing is cut: {p}");
}
