//! GH #709 — the typed turn takes the road every turn takes, and the answer ends at the
//! channel that has no loudspeaker.
//!
//! The cell alone is measured by its sibling file; this one boots a colony around it.
//! `display-hive.md` § 8.2 says where a typed turn goes — "from there it goes the way of
//! every turn: firewall, the member's session, talky" — and a way is a wiring claim, not
//! a claim about a script. Two edges of the member are what make it true, the same two
//! every channel of a person costs (`templates/voice`, and `templates/member/README.md`
//! on the display channel):
//!
//! - UP, `./channels/chat -> ./channels` on `turn || error`, stamping the context the
//!   turn is screened and routed by: which node the answer comes back to, which chat it
//!   is in, which agent it was addressed to, who may hear it, who said it, and the
//!   `turn_id` the channel minted.
//! - DOWN, `./channels -> ./channels/chat` on `answer && context.channel_node == 'chat'`,
//!   re-stamped to `in_answer`.
//!
//! The down-edge looks pointless until you take it away: the answer of this channel then
//! dead-letters with `no_route`, once per answer, because § 8.3 gives it nowhere else to
//! go — the person reads it in the chat app, which hears it on the member's OWN
//! `./assistants -> ./apps` lane. So what this file measures on the way back is a
//! DELIVERY and a silence, not a silence alone.
//!
//! The witness is positive on the way up: the member's firewall is a double that reports
//! the context it was handed on the `reject` lane, which the member ships an exit for. A
//! round routed with the wrong stamps arrives naming the wrong ones; it does not go
//! quiet.
//!
//! Guarded like every other template-reading test (GH #49).

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// `(member, chat-channel)`, or `None` when this tree did not ship both (GH #49).
fn shipped() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let member = repo("templates/member");
    let channel = repo("templates/chat-channel");
    (member.join("config.json").is_file() && channel.join("config.json").is_file())
        .then_some((member, channel))
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent directory")).expect("create the directory");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

/// Copy the template cell by cell: only `config.json` files travel, so the tree under
/// test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create the directory");
    for entry in std::fs::read_dir(src).expect("the template directory is readable") {
        let entry = entry.expect("directory entry");
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).expect("copy the config");
        }
    }
}

// ══════════════════════════════════════════════════════════════════ the doubles

/// A cell that answers nothing: the holders this round never reaches. They exist because
/// a hive door pointing at an absent directory leaves the inside unroutable (GH #286).
const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// The member's screening, doubled at the one question this file asks it: WHAT CONTEXT
/// did the turn arrive with? It reports on `reject`, which the member ships an exit for,
/// so the round ends at the sink with the stamps readable on the hop.
const FIREWALL: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
ctx = hdr.get("context") or {}
hop = hdr.get("hop") or {}
body = doc["body"]
sys.stdout.write(json.dumps({
    "header": {"route": "reject",
               "saw_route": str(hop.get("route") or ""),
               "saw_channel_node": str(ctx.get("channel_node") or ""),
               "saw_channel": str(ctx.get("channel") or ""),
               "saw_assistant": str(ctx.get("assistant") or ""),
               "saw_audience_set": str(ctx.get("audience_set") or ""),
               "saw_user_id": str(ctx.get("user_id") or ""),
               "saw_turn_id": str(ctx.get("turn_id") or ""),
               "saw_text": str(((body.get("messages") or [{}])[0] or {}).get("text") or "")},
    "messages": body.get("messages") or []}))
"#;

/// Puts one message on the lane `hop.lane` names, with the body it was handed.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({
    "header": {"route": str(hop.get("lane") or "in_typed")},
    "messages": doc["body"].get("messages") or []}))
"#;

/// A `code` double with a fixed script. `emits` is left wide on purpose: what a double
/// may say is decided by the assertions, not by a contract nobody reads.
fn double(script: &str, purpose: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {
            "runner": "python3",
            "script_inline": script,
            "external_timeout_ms": 10000
        },
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {"body": {"messages": {"type": "array", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": purpose,
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

// ═════════════════════════════════════════════ the wiring a mutation draws

const AGENT: &str = "egon";
const MEMBER: &str = "alex";
const USER: &str = "4711";

/// **The two edges the channel `chat` costs** — plan § 4, contract 4; the pattern
/// `templates/voice` set and `templates/chat-channel/README.md` states.
///
/// `turn_id` is carried from the hop into the context on the way up, because the hop is
/// per-message and the context is what survives to the talky and back (§ 8.2: the answer
/// carries its turn's id). `user_id` likewise: § 8.7 puts the identity on the channel,
/// and an edge that wrote a literal here would put it back on the installer.
fn chat_edges() -> Vec<Value> {
    vec![
        json!({
            "from": "./channels/chat", "to": "./channels",
            "condition": "has(hop.route) && (hop.route == 'turn' || hop.route == 'error')",
            "modifier": {"set_context": {
                "channel_node": "'chat'",
                "channel": "'chat'",
                "assistant": format!("'{AGENT}'"),
                "audience_set": format!("'[\"agent:{AGENT}\",\"member:{MEMBER}\"]'"),
                "user_id": "has(hop.user_id) ? hop.user_id : ''",
                "turn_id": "has(hop.turn_id) ? hop.turn_id : ''"
            }}
        }),
        json!({
            "from": "./channels", "to": "./channels/chat",
            "condition": "has(hop.route) && hop.route == 'answer' && \
                          has(context.channel_node) && context.channel_node == 'chat'",
            "modifier": {"set_hop": {"route": "'in_answer'"}}
        }),
    ]
}

/// The colony around the member: one driver and a drain for every lane the member emits
/// that this round can reach. An undrained lane is a dead letter, and the assertions
/// would then be reading a silence.
fn main_config() -> Value {
    let mut edges = vec![
        json!({
            "from": "./driver", "to": "./person/channels/chat",
            "condition": "has(hop.route) && hop.route == 'in_typed'"
        }),
        // The answer, arriving at the member's channels container the way the
        // generation's own `./assistants -> ./channels` edge would deliver it.
        json!({
            "from": "./driver", "to": "./person/channels",
            "condition": "has(hop.route) && hop.route == 'answer'",
            "modifier": {"set_context": {"channel_node": "'chat'", "channel": "'chat'"}}
        }),
    ];
    for lane in [
        "answer",
        "ack",
        "reject",
        "error",
        "write",
        "turn_write",
        "prune",
    ] {
        edges.push(json!({"from": "./person", "to": "/sink",
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

fn build_tree(td: &tempfile::TempDir, member: &std::path::Path, channel: &std::path::Path) {
    let root = td.path();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/driver/config.json",
        &double(DRIVER, "Test driver: puts one message on a named lane."),
    );

    copy_cells(member, &root.join("main/person"));
    for holder in ["access", "affinity", "memory-hive", "assistants", "apps"] {
        write(
            root,
            &format!("main/person/{holder}/config.json"),
            &double(INERT, "Inert double for a holder this round never reaches."),
        );
    }
    write(
        root,
        "main/person/firewall/config.json",
        &double(FIREWALL, "Test double for the member's screening."),
    );

    // The SHIPPED channel, at the address the member wires it under.
    let mut chat = read_json(&channel.join("config.json"));
    chat["params"]["user_id"] = json!(USER);
    write(root, "main/person/channels/chat/config.json", &chat);

    let cfg_path = root.join("main/person/config.json");
    let mut cfg = read_json(&cfg_path);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the member ships a graph");
    edges.extend(chat_edges());
    std::fs::write(
        &cfg_path,
        meclaw_core::serde_json::to_string_pretty(&cfg).expect("serialise"),
    )
    .expect("write the member config");

    std::fs::write(root.join(".env"), "").expect("write an empty .env");
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped member and chat-channel must boot");
    (h, sink_rx)
}

fn inject(lane: &str, text: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("lane".into(), json!(lane));
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .hop(hop)
        .ttl(200)
        .build()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// `(to_path, headers)` of every logged message.
fn log_rows(root: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT to_path, headers FROM message_log")
        .expect("message_log");
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query")
        .filter_map(Result::ok)
        .collect()
}

/// `(error_code, resolved_target)` of every dead letter.
fn dead_letters(root: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT error_code, resolved_target FROM dead_letters")
        .expect("dead_letters");
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query")
        .filter_map(Result::ok)
        .collect()
}

/// Poll the message log until `pred` holds, or give up. Thirty seconds is the
/// failure-marker convention of this repo, not a timing discriminator.
async fn wait_for_log(
    root: &std::path::Path,
    pred: impl Fn(&(String, String)) -> bool,
) -> Vec<(String, String)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let rows = log_rows(root);
        if rows.iter().any(&pred) || std::time::Instant::now() > deadline {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn skip() -> bool {
    if shipped().is_none() {
        eprintln!("member/chat-channel did not travel into this tree -- skipped (GH #49)");
        return true;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("no python3 -- skipped");
        return true;
    }
    false
}

// ═══════════════════════════════════════════════════════════════ the measurements

/// **The typed turn arrives at the screening with the whole context of a turn.**
///
/// Not one of these six keys is the app's: the channel mints the id, the channel knows
/// the person (§ 8.7), and the member's own edge names the agent and the audience. What
/// the app sends is a sentence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_typed_sentence_reaches_the_screening_stamped_as_a_turn() {
    if skip() {
        return;
    }
    let (member, channel) = shipped().expect("guarded above");
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member, &channel);
    let (h, mut rx) = boot(&td).await;

    h.send(inject("in_typed", "how will the weather be")).await;
    let got = recv_bounded(&mut rx)
        .await
        .expect("the round has to reach the sink -- the screening double answers");

    assert_eq!(
        hop_of(&got, "saw_route"),
        "in_turn",
        "the member's own edge re-stamps a channel's `turn` to `in_turn` on the way into \
         the screening -- the same edge every channel is screened through"
    );
    assert_eq!(hop_of(&got, "saw_channel_node"), "chat");
    assert_eq!(hop_of(&got, "saw_channel"), "chat");
    assert_eq!(hop_of(&got, "saw_assistant"), AGENT);
    assert_eq!(
        hop_of(&got, "saw_audience_set"),
        format!("[\"agent:{AGENT}\",\"member:{MEMBER}\"]")
    );
    assert_eq!(
        hop_of(&got, "saw_user_id"),
        USER,
        "§ 8.7: the identity is the channel's. It travels from the cell's hop into the \
         context on the member's edge, because the hop is per-message and the context is \
         what reaches the talky"
    );
    assert!(
        hop_of(&got, "saw_turn_id").starts_with("chat#"),
        "§ 8.2: the turn_id is minted by the channel on acceptance and travels in the \
         context, so the answer and every window built from it can carry it: {:?}",
        hop_of(&got, "saw_turn_id")
    );
    assert_eq!(hop_of(&got, "saw_text"), "how will the weather be");
    h.shutdown().await;
}

/// **The answer of this channel is delivered and absorbed.**
///
/// Delivered: a row in the log addressed at the channel, on `in_answer`. Absorbed: no
/// dead letter, and nothing left the member because of it. Both halves are needed —
/// without the down-edge the first is missing and the DLQ carries a `no_route` per
/// answer; without the cell's silence the second is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_answer_reaches_the_channel_and_stops_there() {
    if skip() {
        return;
    }
    let (member, channel) = shipped().expect("guarded above");
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member, &channel);
    let (h, mut rx) = boot(&td).await;

    h.send(inject("answer", "it is going to be sunny")).await;
    let rows = wait_for_log(td.path(), |(to, headers)| {
        to.ends_with("/person/channels/chat") && headers.contains("\"in_answer\"")
    })
    .await;
    assert!(
        rows.iter()
            .any(|(to, headers)| to.ends_with("/person/channels/chat")
                && headers.contains("\"in_answer\"")),
        "the member's down-edge has to deliver the answer to the channel on `in_answer`. \
         Without it every answer of this channel dead-letters with no_route, once per \
         answer, because § 8.3 gives it nowhere else to go. Log:\n{rows:#?}"
    );

    // Give an emission that should not exist the time to arrive.
    let stray = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(
        stray.is_err(),
        "§ 8.3: the channel `chat` has no loudspeaker, so the answer ENDS here. What \
         reached the sink instead: {stray:?}"
    );
    let dlq = dead_letters(td.path());
    assert!(
        dlq.is_empty(),
        "an absorbed answer is not a dead letter -- the channel took it: {dlq:?}"
    );
    h.shutdown().await;
}
