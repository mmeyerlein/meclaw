//! GH #896 -- a session starts with its handover, and a renewed live session
//! gets the conversation so far.
//!
//! Measured AT THE RECEIVER: the request bodies a stub provider recorded, the
//! messages that left the hive, and the rows of the hive's own ledger. The
//! shipped `curator` hive is booted in a test colony in front of a REAL `llm`
//! cell (the brain) pointed at a stub; the hive's own summarizer points at the
//! same stub. A driver plays the parent -- the collector's round
//! (`in_curate`), the session keeper's close (`in_close`), a pinning hive
//! (`in_pin`), an identity pusher (`in_pack`) and the member's renewal
//! (`in_renewed`) -- and every output of the brain comes back on the tap
//! (`in_llm`) through one code cell that stamps a cache expiry an hour ahead,
//! so no rebuild runs -- except on a turn whose id says `soon`, which is
//! stamped two seconds ahead and so orders exactly one rebuild.
//!
//! Pinned:
//!
//! 1. `the_first_request_of_a_new_session_carries_the_handover`: session A has
//!    three turns and closes; the summarizer is asked for a handover note over
//!    A's turns (off the hot path: the close waits on nothing), the note is
//!    kept as a block of kind `summary` with a `marks` row `handover`, and the
//!    FIRST request of session B carries `history.handover` with the note and
//!    the topic still open -- one request later than A's last, no model call
//!    in between. The pin stands in that request once, where the window puts
//!    every pin, and not a second time in the leaf (OR-KY.U.14).
//! 2. `the_first_request_of_a_new_session_is_the_plans_window` (review I-1):
//!    after a rebuild made a plan -- a cover, a row the model pinned under it,
//!    a pin of another hive moved into the system part -- B's first request
//!    is that plan's window: the held row is in it, the covered one is not,
//!    and the pinned leaf is there. The call's reads are `./policy`'s alone.
//! 3. `without_a_close_the_block_needs_no_model`: no close, the first request
//!    of session B still carries a block (the topic still open, OR-KY-71) and
//!    the provider was asked exactly once per turn -- never by the summarizer.
//! 4. `the_block_respects_handover_chars`: a note longer than `handover_chars`
//!    is cut when it is kept, and the leaf cuts it again behind the topic,
//!    which stays whole.
//! 5. `the_pack_handover_family_is_untouched`: an identity pack's `handover`
//!    slot stays the pack's, and the curator's leaf is `history.handover`
//!    (OR-KY-U4); B's first request carries both.
//! 6. `the_leaf_stands_the_session_and_falls_at_the_first_rebuild` (review
//!    I-5a): B's second request still carries the leaf; a rebuild that finds
//!    nothing to condense -- no summary, no model -- takes it down, and the
//!    next request is without it.
//! 7. `a_close_without_turns_hands_nothing_over` (review I-5b): no `marks` row
//!    `handover` and no summarizer request.
//! 8. `a_renewed_call_gets_the_conversation_without_a_model`: `in_renewed`
//!    leaves the hive as ONE `sidecar` section `context` with `renewal_n`,
//!    `context.call_id` and the latest turns under `payload`, no model is
//!    asked (OR-KY-U3), and no block short id is left in the words a live
//!    voice will be told -- neither the bracket `[#<12 hex>]` nor a bare
//!    `#<12..16 hex>` nor a released block's form (OR-KY-68, OR-KY-81).
//!
//! Guarded like every template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;

/// The failure-marker convention of this repo, not a timing discriminator.
const DEADLINE: Duration = Duration::from_secs(30);
/// How long a run is left alone before "no further request" is read: long
/// enough for a summarizer call that should not happen to have been made.
const SETTLE: Duration = Duration::from_secs(3);

const NOTE: &str = "Ada introduced herself and planned a trip to Lisbon in May.";
const PIN: &str = "Ada drinks tea, never coffee.";
const COMMITMENT: &str = "Send Ada the list of hotels in Lisbon.";
const HEAD: &str = "[where the last session left off -- session s-a";
const NOTE_PROMPT: &str = "You write the handover note";
const CONDENSE_PROMPT: &str = "You condense the earlier part of an exchange";
/// How the leaf names the topic still open (OR-KY-71).
const TOPIC_HEAD: &str = "The topic still open: ";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    repo("templates/curator/config.json").is_file()
        && repo("templates/curator/handover/config.json").is_file()
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).expect("write");
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json"
            || src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl")
        {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

/// Emits exactly the `{"header": ..., ...}` it is handed; the context is the
/// injected message's own and rides on.
const DRIVER: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps(json.loads(d["messages"][0]["text"])))
"#;

/// The tap: every output of the brain, handed back with a cache expiry an hour
/// ahead -- no rebuild -- except the final answer of a turn whose id says
/// `soon`: two seconds ahead, one strike, one rebuild (the pattern of gh888).
const STAMPER: &str = r#"
import sys, json, datetime
doc = json.load(sys.stdin)
header = doc["envelope"]["header"]
hop, ctx, body = header.get("hop") or {}, header.get("context") or {}, doc["body"]
out = dict(hop)
out["route"] = "tap"
soon = "soon" in str(ctx.get("turn_id") or "") and hop.get("finish_reason") == "stop"
at = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(seconds=2 if soon else 3600)
out["cache_expires_at"] = at.strftime("%Y-%m-%dT%H:%M:%SZ")
sys.stdout.write(json.dumps({"header": out, "messages": body.get("messages") or []}))
"#;

fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {"purpose": purpose, "use_when": "Test fixture only.",
                        "not_in_scope": "Not a template."}
    })
}

const SYSTEM_ORDER: [&str; 3] = ["identity", "instructions", "history"];

fn lane_edge(route: &str, lane: &str) -> Value {
    json!({"from": "./driver", "to": "./curator",
           "condition": format!("has(hop.route) && hop.route == '{route}'"),
           "modifier": {"set_hop": {"route": format!("'{lane}'")}}})
}

/// The knobs a test sets on the copied hive.
#[derive(Default)]
struct Knobs {
    /// `./handover` `handover_chars`.
    handover_chars: Option<u64>,
    /// `./policy` `keep_recent`.
    keep_recent: Option<u64>,
}

fn build_tree(td: &tempfile::TempDir, base_url: &str, knobs: Knobs) {
    let root = td.path();
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").expect("env file");
    let mut renewed = lane_edge("renewed", "in_renewed");
    // The member's edge promotes the call onto context (templates/member).
    renewed["modifier"]["set_context"] = json!({"call_id": "hop.call_id"});
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            lane_edge("curate", "in_curate"),
            lane_edge("close", "in_close"),
            lane_edge("pin", "in_pin"),
            lane_edge("pack", "in_pack"),
            lane_edge("section", "in_section"),
            renewed,
            {"from": "./curator", "to": "./brain",
             "condition": "has(hop.route) && hop.route == 'brain'",
             "modifier": {"set_context": {"curator_call": "hop.curator_call",
                                          "turn_id": "hop.turn_id",
                                          "session_id": "hop.session_id",
                                          "iter": "hop.iter"}}},
            {"from": "./brain", "to": "./stamper", "condition": "has(hop.finish_reason)"},
            {"from": "./stamper", "to": "./curator",
             "condition": "has(hop.route) && hop.route == 'tap'",
             "modifier": {"set_hop": {"route": "'in_llm'"}}},
            {"from": "./curator", "to": "/sink",
             "condition": "has(hop.route) && hop.route != 'brain'"}
        ]}}}),
    );
    write(
        root,
        "main/driver/config.json",
        &double(DRIVER, "Test driver: plays the parent."),
    );
    write(
        root,
        "main/stamper/config.json",
        &double(STAMPER, "Test tap: stamps the cache expiry."),
    );
    copy_cells(&repo("templates/curator"), &root.join("main/curator"));
    patch(root, "main/curator/summarizer/config.json", |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!("gpt-4o-mock");
        v["params"]["api_key"] = json!("sk-test");
    });
    if let Some(n) = knobs.handover_chars {
        patch(root, "main/curator/handover/config.json", |v| {
            v["params"]["handover_chars"] = json!(n);
        });
    }
    if let Some(n) = knobs.keep_recent {
        patch(root, "main/curator/policy/config.json", |v| {
            v["params"]["keep_recent"] = json!(n);
        });
    }
    let mut brain = read_json(&repo("templates/curator/summarizer/config.json"));
    brain["params"]["base_url"] = json!(base_url);
    brain["params"]["model"] = json!("gpt-4o-mock");
    brain["params"]["api_key"] = json!("sk-test");
    brain["params"]["system_order"] = json!(SYSTEM_ORDER);
    write(root, "main/brain/config.json", &brain);
}

async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    tokio::sync::mpsc::Receiver<meclaw_core::Message>,
) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped curator must boot");
    (h, rx)
}

/// One message at the curator's door, the way the parent's edge hands it on:
/// `spec` is the whole emission (`header` + body slots), `ctx` its context.
fn at_door(spec: Value, ctx: Value) -> meclaw_core::Message {
    let ctx: Map<String, Value> = ctx.as_object().cloned().unwrap_or_default();
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": spec.to_string()}]})))
        .context(ctx)
        .ttl(400)
        .build()
}

/// One round of `session`, as the collector hands it.
fn round(session: &str, turn_id: &str, said: &str) -> meclaw_core::Message {
    at_door(
        json!({"header": {"route": "curate", "session_id": session, "turn_id": turn_id,
                          "iter": "0", "phase": ""},
               "messages": [{"origin": "user", "type": "text", "text": said}],
               "system": {"instructions": {"mode": {"text": "Answer briefly."}}}}),
        json!({"session_id": session, "turn_id": turn_id, "iter": "0"}),
    )
}

fn close(session: &str) -> meclaw_core::Message {
    at_door(
        json!({"header": {"route": "close"}, "messages": []}),
        json!({"session_id": session}),
    )
}

/// A pinning hive's door (`in_pin`, GH #892): `pins[]` of `{text, source}`.
fn pin(text: &str, source: &str) -> meclaw_core::Message {
    at_door(
        json!({"header": {"route": "pin"}, "messages": [],
               "pins": [{"text": text, "source": source}]}),
        json!({}),
    )
}

/// One section of the model's sidecar block, as the parent's splitter hands it
/// to the curator (`in_section`, GH #892): the section's own body under
/// `payload`, the answer's context.
fn section(session: &str, turn_id: &str, name: &str, payload: Value) -> meclaw_core::Message {
    at_door(
        json!({"header": {"route": "section", "section": name},
               "messages": [], "section": name, "payload": payload}),
        json!({"session_id": session, "turn_id": turn_id}),
    )
}

/// The `memory` section that opens a topic with a name (OR-KY-71).
fn topic(session: &str, turn_id: &str, name: &str) -> meclaw_core::Message {
    section(
        session,
        turn_id,
        "memory",
        json!({"topic": {"movement": "start", "name": name}}),
    )
}

async fn requests(mock: &MockOpenAI, n: usize) -> Vec<Value> {
    for _ in 0..300 {
        let reqs = mock.recorded_requests().await;
        if reqs.len() >= n {
            return reqs.into_iter().map(|r| r.body).collect();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the provider was called fewer than {n} time(s) within {DEADLINE:?}");
}

fn try_ledger(td: &tempfile::TempDir) -> rusqlite::Result<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(
        td.path().join("main/curator/ledger/cell.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
}

fn ledger(td: &tempfile::TempDir) -> rusqlite::Connection {
    try_ledger(td).expect("the ledger")
}

/// Poll the ledger until `sql` counts at least `n`, or give up at the marker.
async fn until_count(td: &tempfile::TempDir, sql: &str, n: i64) {
    for _ in 0..300 {
        let got: i64 = try_ledger(td)
            .and_then(|c| c.query_row(sql, [], |r| r.get(0)))
            .unwrap_or(0);
        if got >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("`{sql}` did not reach {n} within {DEADLINE:?}");
}

/// Poll the ledger until `sql` counts nothing, or give up at the marker.
async fn until_none(td: &tempfile::TempDir, sql: &str) {
    for _ in 0..300 {
        let got: i64 = try_ledger(td)
            .and_then(|c| c.query_row(sql, [], |r| r.get(0)))
            .unwrap_or(1);
        if got == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("`{sql}` still counted rows after {DEADLINE:?}");
}

const ANSWERED: &str = "SELECT COUNT(*) FROM calls WHERE model != ''";
const PREPARED: &str =
    "SELECT COUNT(*) FROM marks WHERE kind = 'handover' AND value LIKE '%\"prepared\"%'";
const TOPICS: &str = "SELECT COUNT(*) FROM marks WHERE kind = 'topic'";
const LEAF: &str = "SELECT COUNT(*) FROM slots WHERE path = 'history.handover'";

/// The text of the `history.handover` leaf the ledger holds now.
fn leaf_text(td: &tempfile::TempDir) -> String {
    let body: String = ledger(td)
        .query_row(
            "SELECT b.body FROM slots s JOIN blocks b ON b.hash = s.hash \
             WHERE s.path = 'history.handover'",
            [],
            |r| r.get(0),
        )
        .expect("the leaf");
    let el: Value = meclaw_core::serde_json::from_str(&body).expect("a block");
    el["text"].as_str().unwrap_or_default().to_string()
}

/// The block the close prepared: `(kind, body)` of the block the `marks` row
/// `handover` in state `prepared` names.
fn prepared_block(td: &tempfile::TempDir) -> (String, String) {
    let conn = ledger(td);
    let mut st = conn
        .prepare("SELECT value FROM marks WHERE kind = 'handover' ORDER BY seq")
        .unwrap();
    let values: Vec<String> = st
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let hash = values
        .iter()
        .filter_map(|v| meclaw_core::serde_json::from_str::<Value>(v).ok())
        .find(|v| v["state"] == "prepared")
        .and_then(|v| v["hash"].as_str().map(str::to_string))
        .expect("a prepared handover mark");
    conn.query_row(
        "SELECT kind, body FROM blocks WHERE hash = ?1",
        [hash],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .expect("the prepared block")
}

/// How many calls of the brain the tap has filled in so far.
fn answered(td: &tempfile::TempDir) -> i64 {
    try_ledger(td)
        .and_then(|c| c.query_row(ANSWERED, [], |r| r.get(0)))
        .unwrap_or(0)
}

/// One turn of `session`: the round goes in, the brain is asked -- the `n`-th
/// request the provider sees, the summarizer's counted -- and the tap fills
/// the call record, one more than before (the summarizer's requests are no
/// calls of the brain).
async fn turn(
    h: &ColonyHandle,
    td: &tempfile::TempDir,
    mock: &MockOpenAI,
    n: usize,
    session: &str,
    turn_id: &str,
    said: &str,
) -> Value {
    let before = answered(td);
    h.send(round(session, turn_id, said)).await;
    let reqs = requests(mock, n).await;
    until_count(td, ANSWERED, before + 1).await;
    reqs[n - 1].clone()
}

fn system_of(req: &Value) -> String {
    let msgs = req["messages"].as_array().expect("messages");
    msgs.iter()
        .find(|m| m["role"] == "system")
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Every message of a request that is not its system prompt, as one text.
fn messages_of(req: &Value) -> String {
    let msgs = req["messages"].as_array().expect("messages");
    msgs.iter()
        .filter(|m| m["role"] != "system")
        .map(|m| m["content"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_note(req: &Value) -> bool {
    system_of(req).contains(NOTE_PROMPT)
}

fn is_condense(req: &Value) -> bool {
    req["messages"][0]["content"]
        .as_str()
        .is_some_and(|s| s.contains(CONDENSE_PROMPT))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_request_of_a_new_session_carries_the_handover() {
    if !shipped() {
        eprintln!("the curator template did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Hello Ada.", "stop"),
        canned_chat_completion("Gladly, when?", "stop"),
        canned_chat_completion("May it is.", "stop"),
        canned_chat_completion(NOTE, "stop"),
        canned_chat_completion("Welcome back.", "stop"),
    ])
    .await;
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, &mock.base_url, Knobs::default());
    let (h, _sink) = boot(&td).await;

    turn(&h, &td, &mock, 1, "s-a", "a1", "My name is Ada.").await;
    turn(
        &h,
        &td,
        &mock,
        2,
        "s-a",
        "a2",
        "Let us plan a trip to Lisbon.",
    )
    .await;
    h.send(topic("s-a", "a2", "Trip to Lisbon")).await;
    until_count(&td, TOPICS, 1).await;
    turn(&h, &td, &mock, 3, "s-a", "a3", "In May.").await;
    h.send(pin(PIN, "orga")).await;
    until_count(&td, "SELECT COUNT(*) FROM pins", 1).await;

    // The close: the note is asked for over A's turns, and kept.
    h.send(close("s-a")).await;
    let reqs = requests(&mock, 4).await;
    assert!(
        is_note(&reqs[3]),
        "the fourth request is the note: {}",
        reqs[3]
    );
    let transcript = reqs[3]["messages"][1]["content"].as_str().unwrap_or("");
    assert!(
        transcript.contains("user: My name is Ada.")
            && transcript.contains("assistant: May it is."),
        "the note is asked over the session's turns: {transcript}"
    );
    until_count(&td, PREPARED, 1).await;
    let (kind, body) = prepared_block(&td);
    assert_eq!(
        kind, "summary",
        "the note is kept as a block of kind summary"
    );
    assert!(
        body.contains(NOTE) && !body.contains(PIN),
        "the note alone is kept; pins are read fresh where the note is used (M-6): {body}"
    );

    // Session B: its FIRST request carries the handover.
    let b1 = turn(&h, &td, &mock, 5, "s-b", "b1", "Hi again.").await;
    let system = system_of(&b1);
    assert!(
        system.contains(HEAD)
            && system.contains(NOTE)
            && system.contains(&format!("{TOPIC_HEAD}Trip to Lisbon")),
        "the first request of B carries history.handover with the note and the open topic: \
         {system}"
    );
    // The pin stands in B's first request where the window puts every pin --
    // here, without a plan yet, where it arrived -- and only there: the leaf
    // does not repeat it (OR-KY.U.14), and a call the handover handed back is
    // read like any other call (review I-1).
    let messages = messages_of(&b1);
    assert!(
        messages.contains(&format!("[pinned by orga] {PIN}")),
        "the pin of another hive is in B's first request: {messages}"
    );
    assert!(
        !system.contains(PIN),
        "the handover leaf does not repeat a pin the window shows: {system}"
    );
    let conn = ledger(&td);
    let owner: String = conn
        .query_row(
            "SELECT owner FROM slots WHERE path = 'history.handover'",
            [],
            |r| r.get(0),
        )
        .expect("the leaf");
    assert_eq!(owner, "curator");
    let served: String = conn
        .query_row(
            "SELECT value FROM state WHERE key = 'handover_for'",
            [],
            |r| r.get(0),
        )
        .expect("handover_for");
    assert_eq!(served, "s-b");
    tokio::time::sleep(SETTLE).await;
    assert_eq!(
        mock.recorded_requests().await.len(),
        5,
        "no model call on the way into B"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_close_the_block_needs_no_model() {
    if !shipped() {
        eprintln!("the curator template did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Noted.", "stop"),
        canned_chat_completion("Good morning.", "stop"),
    ])
    .await;
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, &mock.base_url, Knobs::default());
    let (h, _sink) = boot(&td).await;

    turn(&h, &td, &mock, 1, "s-a", "a1", "Remind me of the hotels.").await;
    h.send(topic("s-a", "a1", "Hotels in Lisbon")).await;
    until_count(&td, TOPICS, 1).await;
    h.send(pin(COMMITMENT, "commitment")).await;
    until_count(&td, "SELECT COUNT(*) FROM pins", 1).await;
    let b1 = turn(&h, &td, &mock, 2, "s-b", "b1", "Morning.").await;
    let system = system_of(&b1);
    assert!(
        system.contains(HEAD)
            && system.contains("not closed")
            && system.contains(&format!("{TOPIC_HEAD}Hotels in Lisbon")),
        "without a close B still starts with a block -- the topic still open: {system}"
    );
    assert!(
        messages_of(&b1).contains(COMMITMENT) && !system.contains(COMMITMENT),
        "an open commitment is a pin, and stands where the window puts pins"
    );
    tokio::time::sleep(SETTLE).await;
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "one request per turn and none else");
    assert!(
        !reqs.iter().any(|r| is_note(&r.body)),
        "the summarizer was never asked"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_block_respects_handover_chars() {
    if !shipped() {
        eprintln!("the curator template did not travel into this tree -- skipped (GH #49)");
        return;
    }
    const LIMIT: usize = 200;
    let long = "x".repeat(2_000);
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Once upon a time.", "stop"),
        canned_chat_completion(&long, "stop"),
        canned_chat_completion("Again.", "stop"),
    ])
    .await;
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(
        &td,
        &mock.base_url,
        Knobs {
            handover_chars: Some(LIMIT as u64),
            ..Knobs::default()
        },
    );
    let (h, _sink) = boot(&td).await;

    turn(&h, &td, &mock, 1, "s-a", "a1", "Tell me a story.").await;
    h.send(topic("s-a", "a1", "A story about a fox")).await;
    until_count(&td, TOPICS, 1).await;
    h.send(close("s-a")).await;
    requests(&mock, 2).await;
    until_count(&td, PREPARED, 1).await;
    let (_, body) = prepared_block(&td);
    let el: Value = meclaw_core::serde_json::from_str(&body).expect("a block");
    let text = el["text"].as_str().expect("text");
    assert!(
        text.chars().count() <= LIMIT && text.ends_with('…'),
        "the kept note is cut at handover_chars {LIMIT}: {} characters",
        text.chars().count()
    );

    turn(&h, &td, &mock, 3, "s-b", "b1", "Again.").await;
    let leaf = leaf_text(&td);
    let (_, block) = leaf.split_once('\n').expect("a head line and the block");
    assert!(
        block.chars().count() <= LIMIT,
        "{} characters over handover_chars {LIMIT}: {block}",
        block.chars().count()
    );
    assert!(
        block.contains(&format!("{TOPIC_HEAD}A story about a fox")),
        "the topic outranks the note and stays whole: {block}"
    );
    assert!(block.ends_with('…'), "the note is cut at its end: {block}");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pack_handover_family_is_untouched() {
    if !shipped() {
        eprintln!("the curator template did not travel into this tree -- skipped (GH #49)");
        return;
    }
    const PACKED: &str = "PACK HANDOVER: the pusher's own words.";
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Hi.", "stop"),
        canned_chat_completion("Again, hi.", "stop"),
    ])
    .await;
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, &mock.base_url, Knobs::default());
    let (h, mut sink) = boot(&td).await;

    h.send(at_door(
        json!({"header": {"route": "pack"}, "messages": [],
               "system": {"handover": {"text": PACKED}}}),
        json!({}),
    ))
    .await;
    let acked = tokio::time::timeout(DEADLINE, async {
        loop {
            let m = sink.recv().await.expect("the sink is open");
            if m.headers.hop.get("route").and_then(Value::as_str) == Some("pack_ack") {
                return m;
            }
        }
    })
    .await
    .expect("the pack is acknowledged");
    assert_eq!(
        acked.headers.hop.get("error_code").and_then(Value::as_str),
        Some(""),
        "the pack family `handover` is accepted as before"
    );
    turn(&h, &td, &mock, 1, "s-a", "a1", "Hello.").await;
    h.send(topic("s-a", "a1", "Greetings")).await;
    until_count(&td, TOPICS, 1).await;
    let b1 = turn(&h, &td, &mock, 2, "s-b", "b1", "Again.").await;

    let conn = ledger(&td);
    let mut st = conn
        .prepare("SELECT path, owner FROM slots WHERE path IN ('handover', 'history.handover') ORDER BY path")
        .unwrap();
    let slots: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        slots,
        vec![
            ("handover".to_string(), "pack".to_string()),
            ("history.handover".to_string(), "curator".to_string())
        ],
        "one family, one owner (OR-KY-U4)"
    );
    let system = system_of(&b1);
    assert!(
        system.contains(PACKED) && system.contains(HEAD),
        "B's first request carries the pack's handover and the curator's: {system}"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_request_of_a_new_session_is_the_plans_window() {
    if !shipped() {
        eprintln!("the curator template did not travel into this tree -- skipped (GH #49)");
        return;
    }
    const HELD: &str = "My locker code is 4711.";
    const COVERED: &str = "What is the weather today?";
    const SUMMARY: &str = "Ada gave a locker code and asked about the weather.";
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Noted.", "stop"),
        canned_chat_completion("Sunny.", "stop"),
        canned_chat_completion("At noon.", "stop"),
        canned_chat_completion(SUMMARY, "stop"),
        canned_chat_completion(NOTE, "stop"),
        canned_chat_completion("Hello again, Ada.", "stop"),
    ])
    .await;
    let td = tempfile::tempdir().expect("tempdir");
    // One round raw: a rebuild condenses everything before the newest round,
    // so the plan has a cover -- and the row the model pins stays under it.
    build_tree(
        &td,
        &mock.base_url,
        Knobs {
            keep_recent: Some(1),
            ..Knobs::default()
        },
    );
    let (h, _sink) = boot(&td).await;

    turn(&h, &td, &mock, 1, "s-a", "a1", HELD).await;
    let held: String = ledger(&td)
        .query_row(
            "SELECT hash FROM wall WHERE turn_id = 'a1' AND kind = 'user'",
            [],
            |r| r.get(0),
        )
        .expect("the held row");
    h.send(section(
        "s-a",
        "a1",
        "window",
        json!({"pin": [format!("#{}", &held[..12])]}),
    ))
    .await;
    until_count(&td, "SELECT COUNT(*) FROM marks WHERE kind = 'pin'", 1).await;
    h.send(pin(PIN, "orga")).await;
    until_count(&td, "SELECT COUNT(*) FROM pins", 1).await;
    turn(&h, &td, &mock, 2, "s-a", "a2", COVERED).await;
    // The final answer of this turn is stamped two seconds ahead: one strike,
    // one rebuild, one summary.
    turn(&h, &td, &mock, 3, "s-a", "a3-soon", "And lunch?").await;
    until_count(&td, "SELECT COUNT(*) FROM summaries", 1).await;
    until_count(
        &td,
        "SELECT COUNT(*) FROM state WHERE key = 'window_plan' AND value LIKE '%\"keep\":[1%'",
        1,
    )
    .await;

    h.send(close("s-a")).await;
    until_count(&td, PREPARED, 1).await;
    let b1 = turn(&h, &td, &mock, 6, "s-b", "b1", "Hello again.").await;
    let reqs = mock.recorded_requests().await;
    assert!(
        is_condense(&reqs[3].body) && is_note(&reqs[4].body),
        "the fourth request condensed A, the fifth wrote its note"
    );

    let system = system_of(&b1);
    let messages = messages_of(&b1);
    assert!(
        system.contains(HEAD) && system.contains(NOTE),
        "B's first request carries history.handover: {system}"
    );
    assert!(
        messages.contains(HELD),
        "the row the plan holds under its cover is in B's first request (review I-1): \
         {messages}"
    );
    assert!(
        !messages.contains(COVERED),
        "the plan's cover holds: a covered row is not in B's first request: {messages}"
    );
    assert!(
        system.contains(PIN) && !messages.contains(PIN),
        "the pin of another hive is a leaf of the system part since the rebuild, in B's \
         first request too (review I-1): {system}"
    );
    assert!(
        system.contains(SUMMARY),
        "and the summary of the plan stands: {system}"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_leaf_stands_the_session_and_falls_at_the_first_rebuild() {
    if !shipped() {
        eprintln!("the curator template did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("Hello Ada.", "stop"),
        canned_chat_completion(NOTE, "stop"),
        canned_chat_completion("Welcome back.", "stop"),
        canned_chat_completion("Good.", "stop"),
        canned_chat_completion("Fine.", "stop"),
    ])
    .await;
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, &mock.base_url, Knobs::default());
    let (h, _sink) = boot(&td).await;

    turn(&h, &td, &mock, 1, "s-a", "a1", "My name is Ada.").await;
    h.send(close("s-a")).await;
    until_count(&td, PREPARED, 1).await;
    let b1 = turn(&h, &td, &mock, 3, "s-b", "b1", "Hi again.").await;
    assert!(system_of(&b1).contains(HEAD), "B starts with the leaf");
    // The second request of the session still carries it: the leaf stands
    // the session over. This turn's answer orders the rebuild.
    let b2 = turn(&h, &td, &mock, 4, "s-b", "b2-soon", "Still here.").await;
    assert!(
        system_of(&b2).contains(HEAD),
        "the leaf stands the session over: {}",
        system_of(&b2)
    );
    // Three rounds under `keep_recent` 12: the rebuild has nothing to
    // condense, asks no model and still takes the leaf down.
    until_count(
        &td,
        "SELECT COUNT(*) FROM state WHERE key = 'window_plan' AND value != ''",
        1,
    )
    .await;
    until_none(&td, LEAF).await;
    let b3 = turn(&h, &td, &mock, 5, "s-b", "b3", "And now?").await;
    assert!(
        !system_of(&b3).contains(HEAD),
        "after the first rebuild the leaf is gone: {}",
        system_of(&b3)
    );
    tokio::time::sleep(SETTLE).await;
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 5, "three turns of B, A's turn and its note");
    assert!(
        !reqs.iter().any(|r| is_condense(&r.body)),
        "the rebuild asked no model"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_without_turns_hands_nothing_over() {
    if !shipped() {
        eprintln!("the curator template did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mock = MockOpenAI::start(vec![canned_chat_completion("never", "stop")]).await;
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, &mock.base_url, Knobs::default());
    let (h, mut sink) = boot(&td).await;

    h.send(close("s-z")).await;
    // The writer's answer to the close is the sign the close went through the
    // hive; the handover's own reads ran beside it.
    tokio::time::timeout(DEADLINE, async {
        loop {
            let m = sink.recv().await.expect("the sink is open");
            if m.headers.hop.get("route").and_then(Value::as_str) == Some("write") {
                return;
            }
        }
    })
    .await
    .expect("the close is written");
    tokio::time::sleep(SETTLE).await;
    assert_eq!(
        mock.recorded_requests().await.len(),
        0,
        "a session that left no turn asks the summarizer for nothing"
    );
    let marks: i64 = ledger(&td)
        .query_row(
            "SELECT COUNT(*) FROM marks WHERE kind = 'handover'",
            [],
            |r| r.get(0),
        )
        .expect("marks");
    assert_eq!(marks, 0, "and leaves no handover mark");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renewed_call_gets_the_conversation_without_a_model() {
    if !shipped() {
        eprintln!("the curator template did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("The dentist at nine.", "stop"),
        // The model may quote a block's short id in every form the colony
        // makes one -- the window's bracket, the bare id a history tool
        // answers with (up to sixteen digits), the window's form of a
        // released block; a live voice must never hear any (OR-KY-68,
        // OR-KY-81).
        canned_chat_completion(
            "Lunch with Bob [#0123456789ab] #0123456789abcdef \
             [#fedcba987654 released \u{2014} history_read(\"#fedcba987654\")].",
            "stop",
        ),
    ])
    .await;
    let td = tempfile::tempdir().expect("tempdir");
    build_tree(&td, &mock.base_url, Knobs::default());
    let (h, mut sink) = boot(&td).await;

    turn(&h, &td, &mock, 1, "call-1", "call-1#0", "What is on today?").await;
    turn(&h, &td, &mock, 2, "call-1", "call-1#1", "And after that?").await;
    h.send(pin(&format!("{PIN} [#abcdefabcdef]"), "orga")).await;
    until_count(&td, "SELECT COUNT(*) FROM pins", 1).await;
    h.send(at_door(
        json!({"header": {"route": "renewed", "call_id": "call-1", "session_id": "call-1",
                          "renewal_n": 1, "renewed_at": 1_790_000_000_000_u64},
               "messages": []}),
        json!({"channel_node": "voice"}),
    ))
    .await;
    let side = tokio::time::timeout(DEADLINE, async {
        loop {
            let m = sink.recv().await.expect("the sink is open");
            if m.headers.hop.get("route").and_then(Value::as_str) == Some("sidecar") {
                return m;
            }
        }
    })
    .await
    .expect("one sidecar leaves the hive");
    let hop = &side.headers.hop;
    assert_eq!(hop.get("section").and_then(Value::as_str), Some("context"));
    assert_eq!(hop.get("renewal_n").and_then(Value::as_u64), Some(1));
    let ctx = &side.headers.context;
    assert_eq!(
        ctx.get("call_id").and_then(Value::as_str),
        Some("call-1"),
        "the channel's in_advise finds the call by context.call_id"
    );
    assert_eq!(
        ctx.get("channel_node").and_then(Value::as_str),
        Some("voice"),
        "the context of the renewal rides back"
    );
    assert!(
        !ctx.contains_key("cur_origin"),
        "the hive's own keys stay inside"
    );
    let Body::Inline(body) = &side.body else {
        panic!("an inline body")
    };
    let payload = body["payload"].as_str().expect("the words under payload");
    assert!(
        payload.contains("user: And after that?")
            && payload.contains("assistant: Lunch with Bob.")
            && payload.contains(PIN),
        "the latest turns and the pin: {payload}"
    );
    assert!(
        !payload.contains('#'),
        "no block id in any form reaches the voice, which would read it out \
         (OR-KY-68, OR-KY-81): {payload}"
    );
    tokio::time::sleep(SETTLE).await;
    assert_eq!(
        mock.recorded_requests().await.len(),
        2,
        "no model for a renewal (OR-KY-U3)"
    );
    h.shutdown().await;
}
