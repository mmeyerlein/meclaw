//! GH #1401 -- the end of a voice call closes the generation of that call, at once.
//!
//! WHY this file exists. The keeper ended a generation only by the night, by an
//! operator's `in_sweep`, by a turn of another round or by the acknowledgement of
//! an owed answer. A phone call that hung up therefore stayed an open generation
//! until the night (measured on the owner's colony, A1 red: the review of a call
//! came the next morning, and the evening round missed the call). The voice cell's
//! final `usage` already reaches talky as `in_call_end` (#1369); since #1401 talky
//! hands it to the keeper as well, whose lane `in_call_end` seals exactly the open
//! generation whose channel node is the one the call came in by -- reason
//! `call_ended` -- and asks for its close right away.
//!
//! Pinned here, on the shipped template and the shipped edges:
//! - with the SHIPPED idle threshold of two hours, the call end closes the phone
//!   generation in the same causal chain (no idle wait, no night), and the close
//!   says `call_ended` -- when the generation owes no answer;
//! - when its last turn still owes its final answer (a brain or a delegation in
//!   flight at the hang-up), the call end only SEALS it, and the answer's
//!   acknowledgement sends the one close, still `call_ended` (the GH #953 order:
//!   the answer is on the wall before the close);
//! - an open text generation of the same member (another channel node) stays open;
//! - a second call end for an already sealed generation sends nothing;
//! - the review downstream (the consumer of the close that hands a `closed` on to
//!   an app's `in_closed`, stubbed here) fires after the call end;
//! - talky draws `in_call_end` to the keeper beside the curator.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_core::{Body, Headers, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const CLOSE: &str = "templates/session-keeper/close/config.json";
const KEEPER: &str = "templates/session-keeper/config.json";
const TALKY: &str = "templates/talky/config.json";

const PHONE: &str = "freeswitch";
const TEXT: &str = "telegram";

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// R2b / GH #49: a tree without the templates skips.
fn shipped() -> bool {
    repo(CLOSE).is_file() && repo(TALKY).is_file()
}

fn read_json(rel: &str) -> Value {
    let p = repo(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

// ------------------------------------------------------------ the close pass alone

fn run_close(doc: Value) -> Vec<Value> {
    let mut doc = doc;
    doc["params"] = json!({});
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(&repo(CLOSE).to_string_lossy()),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "close exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    match sj::from_slice::<Value>(&out.stdout).expect("json") {
        Value::Array(a) => a,
        one => vec![one],
    }
}

fn op_of(msg: &Value) -> Value {
    sj::from_str(msg["messages"][0]["text"].as_str().expect("op text")).expect("op json")
}

fn call_end_doc(node: &str) -> Value {
    json!({
        "header": {"context": {"channel_node": node, "call_id": "call-1",
                               "audience_set": "[\"member:a\"]"},
                   "hop": {"route": "in_call_end", "call_id": "call-1",
                           "session_id": "call-1", "seconds": 42.0, "final": true}},
        "messages": []
    })
}

#[test]
fn gh1401_the_call_end_asks_only_for_the_open_generation_of_its_node() {
    if !shipped() {
        return;
    }
    let out = run_close(call_end_doc(PHONE));
    assert_eq!(out.len(), 1, "one question to the store: {out:?}");
    assert_eq!(out[0]["header"]["route"], "kstore");
    assert_eq!(out[0]["header"]["phase"], "call-end");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "sessions");
    assert_eq!(
        op["where"],
        json!({"closed": 0, "channel_node": PHONE}),
        "only OPEN generations of the call's node, and no idle cutoff: {op}"
    );
}

#[test]
fn gh1401_a_call_end_without_a_node_closes_nothing() {
    if !shipped() {
        return;
    }
    assert!(
        run_close(call_end_doc("")).is_empty(),
        "without a node the lane would match every generation the store holds"
    );
}

fn call_end_rows(owed: &str) -> Value {
    let rows = json!([{"channel": "phone", "session_id": "phone-0001",
                       "opened_at": "2026-11-02T20:00:00.000000Z",
                       "last_seen": "2026-11-02T20:05:00.000000Z", "closed": 0,
                       "closed_at": "", "audience_set": "[\"member:a\"]",
                       "owed_turn": owed, "channel_node": PHONE}]);
    json!({
        "header": {"context": {"channel": "", "ses_phase": "call-end",
                               "store_origin": "keeper-close"},
                   "hop": {"operation": "select", "rows_affected": 1}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                      "text": rows.to_string()}]
    })
}

/// The legs of one store bundle, by id.
fn legs(msg: &Value) -> Vec<(String, Value)> {
    msg["messages"]
        .as_array()
        .expect("legs")
        .iter()
        .map(|m| {
            (
                m["id"].as_str().unwrap_or_default().to_string(),
                sj::from_str(m["text"].as_str().expect("leg text")).expect("leg json"),
            )
        })
        .collect()
}

#[test]
fn gh1401_the_seal_of_a_call_end_says_call_ended() {
    if !shipped() {
        return;
    }
    let out = run_close(call_end_rows(""));
    assert_eq!(out.len(), 1, "one seal: {out:?}");
    let seal = &out[0];
    assert_eq!(seal["header"]["phase"], "call-seal");
    assert_eq!(seal["header"]["session_id"], "phone-0001");
    assert_eq!(seal["header"]["channel"], "phone");
    let l = legs(seal);
    assert_eq!(l.len(), 1, "nothing owed, one leg: {l:?}");
    assert_eq!(l[0].0, "seal-done");
    assert_eq!(
        l[0].1["where"],
        json!({"session_id": "phone-0001", "closed": 0,
               "owed_turn": {"or_null": {"eq": ""}}}),
        "the guard: {:?}",
        l[0].1
    );
    assert_eq!(l[0].1["set"]["closed"], 1);
    let meta: Value =
        sj::from_str(seal["header"]["keeper_meta"].as_str().expect("meta")).expect("meta json");
    assert_eq!(meta["reason"], "call_ended", "{meta}");
    assert_eq!(meta["channel_node"], PHONE, "{meta}");
}

/// C1 of the review: an owed answer is not wiped. The second leg seals the row
/// and keeps the mark, tagged with the cause, so the acknowledgement of that
/// answer knows the close it sends is `call_ended`.
#[test]
fn gh1401_a_call_end_seal_keeps_an_owed_mark() {
    if !shipped() {
        return;
    }
    let out = run_close(call_end_rows("t-1"));
    assert_eq!(out.len(), 1, "one seal: {out:?}");
    let l = legs(&out[0]);
    let ids: Vec<&str> = l.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["seal-done", "seal-owed"], "{l:?}");
    assert_eq!(
        l[1].1["where"],
        json!({"session_id": "phone-0001", "closed": 0, "owed_turn": "t-1"}),
        "{:?}",
        l[1].1
    );
    assert_eq!(l[1].1["set"]["closed"], 1);
    assert_eq!(l[1].1["set"]["owed_turn"], "call_ended:t-1");
}

// ------------------------------------------------------------ the edges

fn table(base: &str, rel: &str) -> EdgeTable {
    let hp: HiveParams =
        sj::from_value(read_json(rel)["params"].clone()).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let spec: &EdgeSpec = spec;
        let at = |ep: &str| match ep {
            "." => base.to_string(),
            other => format!("{base}/{}", other.trim_start_matches("./")),
        };
        let mut e = Edge {
            id: Uuid::now_v7(),
            from: Path::new(&at(&spec.from)),
            to: Path::new(&at(&spec.to)),
            condition: spec.condition.as_deref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
            }),
            modifier: None,
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        };
        e.modifier = spec.modifier.as_ref().map(|m| {
            meclaw_colony::cel_eval::parse_modifier(m)
                .unwrap_or_else(|(k, e)| panic!("{rel}: modifier {k}: {e}"))
        });
        t.insert(e);
    }
    t
}

fn decide(t: &EdgeTable, from: &str, ctx: &Value, hop: &Value) -> Vec<(String, Headers)> {
    let hs = Headers::from_parts(
        ctx.as_object().cloned().expect("context"),
        hop.as_object().cloned().expect("hop"),
    );
    apply_edges(t, &Path::new(from), &hs)
        .into_iter()
        .map(|d| (d.target.as_str().to_string(), d.headers_out))
        .collect()
}

#[test]
fn gh1401_talky_hands_the_call_end_to_the_keeper_and_the_curator() {
    if !shipped() {
        return;
    }
    let t = table("/t", TALKY);
    let ctx = json!({"channel_node": PHONE, "call_id": "call-1",
                     "audience_set": "[\"member:a\"]"});
    let hop = json!({"route": "in_call_end", "call_id": "call-1", "session_id": "call-1"});
    let mut to: Vec<String> = decide(&t, "/t", &ctx, &hop)
        .into_iter()
        .map(|d| {
            // D58 live (GH #1482): the keeper keeps its lane's name, the
            // curator hears the end of a session on its own `in_live_end`.
            let lane = if d.0 == "/t/curator" {
                "in_live_end"
            } else {
                "in_call_end"
            };
            assert_eq!(d.1.hop["route"], lane, "{}: {:?}", d.0, d.1.hop);
            assert_eq!(d.1.context["channel_node"], PHONE, "the node travels");
            d.0
        })
        .collect();
    to.sort();
    assert_eq!(to, vec!["/t/curator", "/t/session-keeper"]);

    let k = table("/k", KEEPER);
    let out = decide(&k, "/k", &ctx, &hop);
    assert_eq!(out.len(), 1, "one door inside the keeper");
    assert_eq!(out[0].0, "/k/close");
    let accepts: Vec<String> = read_json(KEEPER)["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts")
        .iter()
        .map(|a| a["route"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(accepts.contains(&"in_call_end".to_string()), "{accepts:?}");
}

// ------------------------------------------------------------ the colony

/// Turns a harness message into an inbound turn (the lane is named by the edge).
const PROBE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps({"header": {"route": "turn"}, "messages": d.get("messages", [])}))
"#;

/// Everything downstream of the stamp: answers with the session id it was given.
const REPORT: &str = r#"
import sys, json
doc = json.load(sys.stdin)
ctx = (doc["envelope"].get("header") or {}).get("context") or {}
sys.stdout.write(json.dumps({"header": {"route": "report"},
                             "messages": [{"origin": "assistant", "type": "text",
                                           "text": str(ctx.get("session_id", ""))}]}))
"#;

/// The stand-in for the curator's `in_close` and its conversation event: one
/// `closed` per close, naming the session and the reason the keeper gave.
const CURATOR: &str = r#"
import sys, json
doc = json.load(sys.stdin)
ctx = (doc["envelope"].get("header") or {}).get("context") or {}
sys.stdout.write(json.dumps({"header": {"route": "closed",
                                        "session_id": str(ctx.get("session_id", "")),
                                        "reason": str(ctx.get("close_reason", ""))},
                             "messages": [{"origin": "assistant", "type": "text",
                                           "text": "closed:" + str(ctx.get("session_id", ""))
                                                   + "|" + str(ctx.get("close_reason", ""))}]}))
"#;

fn code_cell(script: &str, routes: &[&str]) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {"route": {"type": "string", "values": routes, "required": false}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the keeper.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The parent around the keeper, drawn like talky draws it: turns in, the call
/// end in on `in_call_end`, the final answer's `turn_write` in on `in_answered`
/// (sent by the test itself, so it decides WHEN the answer lands), the close out
/// to the curator stand-in, whose `closed` reaches the review app's `in_closed`
/// (`/review`).
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./probe", "to": "./session-keeper",
         "condition": "hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"}}},
        {"from": ".", "to": "./session-keeper",
         "condition": "hop.route == 'call_end'",
         "modifier": {"set_hop": {"route": "'in_call_end'"}}},
        {"from": ".", "to": "./session-keeper",
         "condition": "hop.route == 'answered'",
         "modifier": {"set_hop": {"route": "'in_answered'"}}},
        {"from": "./session-keeper", "to": "./report",
         "condition": "hop.route == 'turn'",
         "modifier": {"set_context": {"session_id": "hop.session_id"}}},
        {"from": "./report", "to": "/sink", "condition": "hop.route == 'report'"},
        {"from": "./session-keeper", "to": "./curator",
         "condition": "hop.route == 'close'",
         "modifier": {"set_context": {"session_id": "hop.session_id",
                                      "close_reason": "has(hop.close_reason) ? hop.close_reason : ''"}}},
        {"from": "./curator", "to": "/review",
         "condition": "hop.route == 'closed'",
         "modifier": {"set_hop": {"route": "'in_closed'"}}}
    ]}}})
}

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, sj::to_string_pretty(v).unwrap()).unwrap();
}

/// The shipped keeper with its SHIPPED idle threshold (two hours): a close that
/// arrives here did not wait for silence. Only the night window goes empty, so
/// the run does not depend on its hour (GH #1229).
fn build_tree(td: &tempfile::TempDir) {
    let root = td.path();
    std::fs::write(root.join(".env"), "").unwrap();
    write(root, "main/config.json", &main_config());
    copy_cells(
        &repo("templates/session-keeper"),
        &root.join("main/session-keeper"),
    );
    meclaw_testing::quiet_keeper_night(&root.join("main/session-keeper"));
    write(root, "main/probe/config.json", &code_cell(PROBE, &["turn"]));
    write(
        root,
        "main/report/config.json",
        &code_cell(REPORT, &["report"]),
    );
    write(
        root,
        "main/curator/config.json",
        &code_cell(CURATOR, &["closed"]),
    );
}

struct Colony {
    h: ColonyHandle,
    sink: mpsc::Receiver<Message>,
    review: mpsc::Receiver<Message>,
}

async fn boot(td: &tempfile::TempDir) -> Colony {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink) = mpsc::channel::<Message>(64);
    let (review_tx, review) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/review"), move || {
        CaptureCell::new(review_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    Colony { h, sink, review }
}

fn turn(channel: &str, node: &str, text: &str, turn_id: &str) -> Message {
    let mut ctx = sj::Map::new();
    ctx.insert("channel".into(), json!(channel));
    ctx.insert("channel_node".into(), json!(node));
    ctx.insert("turn_id".into(), json!(turn_id));
    MessageBuilder::new(Path::new("/probe"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .context(ctx)
        .ttl(64)
        .build()
}

/// The final answer of turn `turn_id` reaching the wall: what talky draws from
/// the writer's `turn_write` onto the keeper's `in_answered`.
fn answered(session: &str, turn_id: &str) -> Message {
    let mut ctx = sj::Map::new();
    ctx.insert("turn_id".into(), json!(turn_id));
    let mut hop = sj::Map::new();
    hop.insert("route".into(), json!("answered"));
    hop.insert("session_id".into(), json!(session));
    MessageBuilder::new(Path::new("/"))
        .hop(hop)
        .context(ctx)
        .body(Body::Inline(json!({"messages": [
            {"origin": "assistant", "type": "text", "text": "the final answer"}]})))
        .ttl(64)
        .build()
}

/// The voice cell's final `usage`, as the set hands it on: `hop.call_id`,
/// `hop.session_id` (the call's own id, never the keeper's), the node in context.
fn call_end(node: &str) -> Message {
    let mut ctx = sj::Map::new();
    ctx.insert("channel_node".into(), json!(node));
    ctx.insert("call_id".into(), json!("call-1"));
    let mut hop = sj::Map::new();
    hop.insert("route".into(), json!("call_end"));
    hop.insert("call_id".into(), json!("call-1"));
    hop.insert("session_id".into(), json!("call-1"));
    hop.insert("final".into(), json!(true));
    MessageBuilder::new(Path::new("/"))
        .hop(hop)
        .context(ctx)
        .body(Body::Inline(json!({"messages": []})))
        .ttl(64)
        .build()
}

fn text_of(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Body::Blob(_) => panic!("inline expected"),
    }
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

async fn say(c: &mut Colony, ch: &str, node: &str, text: &str, turn_id: &str) -> String {
    c.h.send(turn(ch, node, text, turn_id)).await;
    let got = recv_bounded(&mut c.sink)
        .await
        .unwrap_or_else(|| panic!("no report for turn {text:?}"));
    text_of(&got)
}

/// `(session_id, channel_node, closed)` of every generation, oldest first.
fn generations(td: &tempfile::TempDir) -> Vec<(String, String, i64)> {
    let db = td.path().join("main/session-keeper/sessions/cell.db");
    let conn = rusqlite::Connection::open(db).expect("sessions cell.db");
    let mut st = conn
        .prepare(
            "SELECT session_id, COALESCE(channel_node, ''), closed FROM sessions ORDER BY opened_at",
        )
        .expect("prepare");
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .filter_map(Result::ok)
        .collect()
}

/// The seal bundles the close pass asked the store for on a call end, read
/// through the colony's own log (never its database file). A read, not a wait:
/// the callers quiesce first.
async fn call_seals(c: &Colony) -> usize {
    let filter = meclaw_colony::api_dto::MessageLogFilter {
        from_path_prefix: Some("/session-keeper/close".into()),
        to_path_prefix: Some("/session-keeper/sessions".into()),
        ..Default::default()
    };
    c.h.read_log(filter)
        .await
        .into_iter()
        .filter(|row| {
            sj::from_str::<Value>(&row.headers_json)
                .map(|h| h["context"]["ses_phase"] == "call-seal")
                .unwrap_or(false)
        })
        .count()
}

/// The answer lands and the colony settles: the generation owes nothing when
/// the next message arrives.
async fn answer(c: &mut Colony, session: &str, turn_id: &str) {
    c.h.send(answered(session, turn_id)).await;
    c.h.quiesce().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_end_closes_the_phone_generation_at_once() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let mut c = boot(&td).await;

    let phone = say(&mut c, "phone", PHONE, "hello, it is me", "t-1").await;
    // The case without a debt: the answer of the last turn is on the wall.
    answer(&mut c, &phone, "t-1").await;
    // Two hours of silence are the shipped threshold; the call ends now.
    c.h.send(call_end(PHONE)).await;
    let closed = recv_bounded(&mut c.review)
        .await
        .expect("the call end closes the phone generation without waiting for silence");
    assert_eq!(text_of(&closed), format!("closed:{phone}|call_ended"));
    let g = generations(&td);
    assert_eq!(g, vec![(phone, PHONE.to_string(), 1)], "{g:?}");
    c.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_text_generation_of_the_same_member_stays_open() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let mut c = boot(&td).await;

    let text = say(&mut c, "tg:42", TEXT, "I'll call you in a minute", "t-1").await;
    answer(&mut c, &text, "t-1").await;
    let phone = say(&mut c, "phone", PHONE, "hello, it is me", "t-2").await;
    answer(&mut c, &phone, "t-2").await;
    c.h.send(call_end(PHONE)).await;
    let closed = recv_bounded(&mut c.review).await.expect("the phone close");
    assert_eq!(text_of(&closed), format!("closed:{phone}|call_ended"));
    c.h.quiesce().await;
    let g = generations(&td);
    assert!(
        g.contains(&(text.clone(), TEXT.to_string(), 0)),
        "the chat generation is not the call's: {g:?}"
    );
    // And the chat goes on in the same generation.
    let after = say(&mut c, "tg:42", TEXT, "there you are", "t-3").await;
    assert_eq!(after, text);
    c.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_call_end_for_a_closed_generation_is_a_no_op() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let mut c = boot(&td).await;

    let phone = say(&mut c, "phone", PHONE, "hello", "t-1").await;
    answer(&mut c, &phone, "t-1").await;
    c.h.send(call_end(PHONE)).await;
    let closed = recv_bounded(&mut c.review).await.expect("the first close");
    assert_eq!(text_of(&closed), format!("closed:{phone}|call_ended"));
    c.h.send(call_end(PHONE)).await;
    // Settled: every consequence of both call ends is delivered. The second
    // found no open generation and asked for no seal.
    c.h.quiesce().await;
    assert_eq!(call_seals(&c).await, 1, "one seal for one call");
    // The close of the first seal is already in (above); nothing else is queued.
    assert!(
        c.review.try_recv().is_err(),
        "a sealed generation is closed once"
    );
    c.h.shutdown().await;
}

/// The scenario the owner measured (A1): a call ends and its review follows it,
/// not the night. The review app hears `in_closed` once, with the reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_counselor_review_fires_after_the_call_end() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let mut c = boot(&td).await;

    let first = say(&mut c, "phone", PHONE, "good evening", "t-1").await;
    let again = say(&mut c, "phone", PHONE, "how was your day", "t-2").await;
    assert_eq!(first, again, "one call, one generation");
    answer(&mut c, &first, "t-2").await;
    c.h.send(call_end(PHONE)).await;
    let review = recv_bounded(&mut c.review)
        .await
        .expect("the review fires after the call end");
    assert_eq!(review.headers.hop["route"], "in_closed");
    assert_eq!(review.headers.hop["reason"], "call_ended");
    assert_eq!(review.headers.hop["session_id"], json!(first));
    // The next call is a new generation.
    let next = say(&mut c, "phone", PHONE, "me again", "t-3").await;
    assert_ne!(next, first);
    c.h.shutdown().await;
}

/// C1 of the review (GH #953 on the call end): the answer of the last turn is
/// still on its way when the person hangs up. The call end seals the generation
/// but sends no close -- the batch and the review would read a wall without
/// that answer -- and the answer's acknowledgement sends exactly one close,
/// with the reason of the call end, after which the review hears `in_closed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_end_with_an_owed_answer_closes_on_the_answer() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let mut c = boot(&td).await;

    let phone = say(&mut c, "phone", PHONE, "one more thing", "t-1").await;
    c.h.send(call_end(PHONE)).await;
    c.h.quiesce().await;
    assert_eq!(
        call_seals(&c).await,
        1,
        "the call end sealed the generation"
    );
    let g = generations(&td);
    assert_eq!(g, vec![(phone.clone(), PHONE.to_string(), 1)], "{g:?}");
    assert!(
        c.review.try_recv().is_err(),
        "no close while the last answer is on its way"
    );

    c.h.send(answered(&phone, "t-1")).await;
    let closed = recv_bounded(&mut c.review)
        .await
        .expect("the acknowledgement of the answer sends the close");
    assert_eq!(closed.headers.hop["route"], "in_closed");
    assert_eq!(text_of(&closed), format!("closed:{phone}|call_ended"));
    c.h.quiesce().await;
    assert!(c.review.try_recv().is_err(), "exactly one close");
    c.h.shutdown().await;
}
