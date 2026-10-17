//! GH #1225 (A7, A8, OR-KF-21; history) -- the model reads its whole wall:
//! a block of any size, the topic it is still in, a replayed turn by its time.
//!
//! WHY this file exists: three ways `./history` left the model standing.
//!
//! 1. A8 / EA7: a block larger than a tenth of the reading model's window was
//!    refused with `too_large` -- the window shows such a block shortened
//!    (`[shortened … — history_read("#id")]`), so the one way the window
//!    names led into a refusal with no way on. Now `history_read` by id reads
//!    a block in pages: each page at most the read budget, its text cut with
//!    `...[cut: …]` naming the next `offset`, the pages together the block.
//! 2. A7: `history_outline` took its topics from the newest `scan_budget`
//!    `topic` marks only, and every quiet turn leaves one (a nameless
//!    `continue`): 40 quiet turns under `scan_budget` 40 pushed the named
//!    `start` out, and the outline named no topic at all. Now the outline
//!    seeds the open topic from the newest mark before its window that says
//!    something (the nothing form left out in the store, as `./handover`
//!    reads it -- no JSON path on `marks`, OR-KF.KH.4).
//! 3. OR-KF-21: `since`/`until` filtered on `at`, the ingest clock. A replay
//!    writes yesterday's turns today (`wall.turn_at`, KD): a turn said
//!    yesterday was found by "since this morning" and missed by "yesterday".
//!    Now the bounds hold the time a turn was said (`turn_at`, else `at`).
//!
//! The history cell is not part of the shared hive's cells; its calls run
//! here the way `gh932_the_curator_pages_only_over_visible_rows.rs` runs them:
//! the shipped script, its store messages answered by the store cell's own
//! dispatcher on the hive's ledger.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
use curator_hive::*;

use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::collections::VecDeque;

const CALL: &str = "call-1225-history";

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

/// One history call in the round `ROUND_E`, during a turn of session
/// `s-now`, with the shipped params and `knobs` over them; `input_soft` the
/// reading model's window (tokens) on the call's hop, as the dispatcher
/// hands it. Returns the answer's `hop.error_code` and its payload.
fn history(
    h: &Hive,
    knobs: &[(&str, i64)],
    input_soft: Option<u64>,
    tool: &str,
    args: Value,
) -> (String, Value) {
    let cfg = cell_config("history");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut params = obj(cfg["params"].clone());
    params.remove("script_inline");
    for (k, v) in knobs {
        assert!(params.contains_key(*k), "no such history knob: {k}");
        params.insert((*k).to_string(), json!(v));
    }
    let mut hop = json!({"route": "in_history_call", "tool_name": tool, "tool_call_id": CALL});
    if let Some(soft) = input_soft {
        hop["input_soft"] = json!(soft);
    }
    let first = Msg {
        context: obj(
            json!({"session_id": "s-now", "turn_id": "t-now", "iter": "1",
                            "curator_call": "cc-1225", "audience_set": ROUND_E}),
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
    let payload =
        sj::from_str(turns[0]["text"].as_str().expect("a text")).expect("one JSON object");
    (code, payload)
}

fn stamp(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .expect("an RFC 3339 stamp")
        .with_timezone(&chrono::Utc)
}

/// EA7: a block over a tenth of the window is read whole -- in pages, each
/// within the budget, each cut where the next one starts, none missing.
#[test]
fn a_block_over_a_tenth_of_the_window_is_read_whole_in_pages() {
    let mut h = Hive::new();
    // 2000 characters; the window 1000 tokens: a read budget of a tenth of
    // it at 3 characters a token, 300 characters.
    let text: String = (0..400).map(|i| format!("w{i:03} ")).collect();
    let el = user(&text);
    let (_, hash) = h.row(stamp("2026-10-12T09:00:00Z"), "s1", "t1", "user", &el, 1);
    let id = format!("#{}", &hash[..12]);
    let budget = 300usize;
    assert!(
        canonical(&el).len() > budget,
        "the block is over a tenth of the window"
    );
    let mut args = json!({"id": id});
    let mut pieces = String::new();
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages < 40, "the pages do not come to an end");
        let (code, p) = history(&h, &[], Some(1000), "history_read", args.clone());
        assert_eq!(code, "", "a page, never a refusal: {p}");
        assert_eq!(p["budget"], json!(budget), "{p}");
        let block = &p["blocks"][0]["block"];
        assert!(
            canonical(block).len() <= budget,
            "a page is within the read budget: {} > {budget}: {p}",
            canonical(block).len()
        );
        assert_eq!(block["origin"], "user", "the element as stored: {p}");
        let t = block["text"].as_str().expect("the page's text");
        assert_eq!(
            p["offset"].as_u64(),
            Some(pieces.chars().count() as u64),
            "pages follow one another without a gap: {p}"
        );
        match p["next_offset"].as_u64() {
            Some(next) => {
                let at = t.find("...[cut: ").expect("a cut page says so");
                let mark = &t[at..];
                assert!(
                    mark.contains(&format!("\"offset\": {next}")),
                    "the mark names the next page: {mark}"
                );
                assert!(mark.contains(&id), "and the block: {mark}");
                pieces.push_str(&t[..at]);
                args = json!({"id": id, "offset": next});
            }
            None => {
                assert!(!t.contains("...[cut: "), "the last page is not cut: {p}");
                pieces.push_str(t);
                break;
            }
        }
    }
    assert!(pages > 1, "the block came in more than one page");
    assert_eq!(pieces, text, "the pages together are the block");
    // A page asked for outright: `length` caps it, the end of the text is
    // no page, and a turn range is never paged.
    let (code, p) = history(
        &h,
        &[],
        Some(1000),
        "history_read",
        json!({"id": id, "offset": 10, "length": 50}),
    );
    assert_eq!(code, "", "{p}");
    assert_eq!(
        (
            p["offset"].as_u64(),
            p["length"].as_u64(),
            p["next_offset"].as_u64()
        ),
        (Some(10), Some(50), Some(60)),
        "{p}"
    );
    assert!(
        p["blocks"][0]["block"]["text"]
            .as_str()
            .unwrap()
            .starts_with(&text[10..60]),
        "{p}"
    );
    let (code, p) = history(
        &h,
        &[],
        Some(1000),
        "history_read",
        json!({"id": id, "offset": 2000}),
    );
    assert_eq!(code, "bad_range", "{p}");
    let (code, p) = history(
        &h,
        &[],
        Some(1000),
        "history_read",
        json!({"from_turn": "t1", "offset": 0}),
    );
    assert_eq!(code, "bad_arguments", "{p}");
}

/// EA5 (history half): a named topic and 60 quiet turns after it, under a
/// `scan_budget` of 40 marks -- the outline still names the topic open.
#[test]
fn an_outline_names_the_topic_sixty_quiet_turns_later() {
    let mut h = Hive::new();
    let base = stamp("2026-10-12T08:00:00Z");
    for i in 0..=60i64 {
        let at = base + chrono::Duration::minutes(i);
        let turn = format!("t{i}");
        h.row(at, "s1", &turn, "user", &user(&format!("turn {i}")), 0);
        h.row(
            at + chrono::Duration::seconds(1),
            "s1",
            &turn,
            "assistant",
            &said(&format!("answer {i}")),
            1,
        );
        let topic = if i == 0 {
            json!({"movement": "start", "name": "the lighthouse"})
        } else {
            json!({"movement": "continue", "name": ""})
        };
        h.mark(
            at + chrono::Duration::seconds(2),
            "s1",
            &turn,
            "topic",
            &canonical(&topic),
        );
    }
    let (code, p) = history(
        &h,
        &[("scan_budget", 40)],
        None,
        "history_outline",
        json!({}),
    );
    assert_eq!(code, "", "{p}");
    let topics = &p["sessions"][0]["topics"];
    let names: Vec<&str> = topics
        .as_array()
        .expect("topics")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(names, ["the lighthouse"], "the topic still open: {p}");
    assert_eq!(topics[0]["open"], true, "{p}");
    assert_eq!(topics[0]["to_turn"], "t60", "{p}");
}

/// OR-KF-21: a turn replayed today that was said yesterday is yesterday's
/// for `since`/`until` -- in a search and in an outline. A live row (no
/// `turn_at`) keeps its `at`.
#[test]
fn a_replayed_turn_is_found_by_the_time_it_was_said() {
    let mut h = Hive::new();
    let (seq, _) = h.row(
        stamp("2026-10-16T08:00:00Z"),
        "s-replay",
        "t-r1",
        "user",
        &user("the ferry to Faro leaves at noon"),
        1,
    );
    h.db.execute(
        "UPDATE wall SET turn_at = ?1 WHERE seq = ?2",
        rusqlite::params!["2026-10-15T12:00:00.000000Z", seq],
    )
    .expect("the replay's turn time");
    h.row(
        stamp("2026-10-16T09:00:00Z"),
        "s-live",
        "t-l1",
        "user",
        &user("the ferry is late today"),
        1,
    );
    let hits = |p: &Value| -> Vec<String> {
        p["hits"]
            .as_array()
            .unwrap_or_else(|| panic!("hits: {p}"))
            .iter()
            .map(|x| x["session_id"].as_str().unwrap_or("").to_string())
            .collect()
    };
    let (code, p) = history(
        &h,
        &[],
        None,
        "history_search",
        json!({"query": "ferry", "since": "2026-10-16T00:00:00Z"}),
    );
    assert_eq!(code, "", "{p}");
    assert_eq!(
        hits(&p),
        ["s-live"],
        "this morning holds no replayed turn: {p}"
    );
    let (code, p) = history(
        &h,
        &[],
        None,
        "history_search",
        json!({"query": "ferry", "since": "2026-10-15", "until": "2026-10-15"}),
    );
    assert_eq!(code, "", "{p}");
    assert_eq!(hits(&p), ["s-replay"], "yesterday holds it: {p}");
    let (code, p) = history(
        &h,
        &[],
        None,
        "history_search",
        json!({"query": "ferry", "until": "2026-10-15"}),
    );
    assert_eq!(code, "", "{p}");
    assert_eq!(hits(&p), ["s-replay"], "`until` alone, too: {p}");
    let sessions = |p: &Value| -> Vec<String> {
        p["sessions"]
            .as_array()
            .unwrap_or_else(|| panic!("sessions: {p}"))
            .iter()
            .map(|s| s["session_id"].as_str().unwrap_or("").to_string())
            .collect()
    };
    let (code, p) = history(
        &h,
        &[],
        None,
        "history_outline",
        json!({"since": "2026-10-16"}),
    );
    assert_eq!(code, "", "{p}");
    assert_eq!(sessions(&p), ["s-live"], "{p}");
    let (code, p) = history(
        &h,
        &[],
        None,
        "history_outline",
        json!({"since": "2026-10-15"}),
    );
    assert_eq!(code, "", "{p}");
    assert_eq!(sessions(&p), ["s-replay", "s-live"], "{p}");
}
