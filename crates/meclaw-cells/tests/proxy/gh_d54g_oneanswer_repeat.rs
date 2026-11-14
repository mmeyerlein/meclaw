//! D54g oneanswer (GH #1505, OR-54g-13), contract point 3 at the channel: an
//! answer whose text already went out in its turn reaches a channel with the
//! hop field `repeat` "1". A channel that writes does not send it and ends
//! its turn as after an answer it sent; a speaking input never gets it -- the
//! composition's edge keeps it out, not the voice cell.
//!
//! The first pin drives the connector's one delivery place with fixture
//! messages; the second boots the shipped `templates/member` with the speaking
//! edge its channel READMEs hand out and two channel doubles.

use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::partial::PartialDelivery;
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{
    Body, CellEmission, Message, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
};
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

const CHAT: i64 = 4254;

fn context() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("chat_id".into(), json!(CHAT));
    m
}

fn assistant(text: &str) -> Body {
    Body::Inline(json!({"messages": [{"origin": "assistant", "type": "text", "text": text}]}))
}

/// One partial of the llm lane contract: cumulative text, `seq` n.
fn partial(text: &str, seq: u64) -> Message {
    let hop: Map<String, Value> = [
        ("route", json!("partial")),
        ("seq", json!(seq)),
        ("final", json!(false)),
        ("partial_of", json!(Uuid::now_v7().to_string())),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/quick"))
        .context(context())
        .hop(hop)
        .body(assistant(text))
        .build()
}

/// An answer as it reaches the connector (`route answer`) with one mark more
/// on its hop: `interim` for a bridge, `repeat` for a text already sent.
fn answer(text: &str, mark: Option<&str>) -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("answer"));
    if let Some(key) = mark {
        hop.insert(key.to_string(), json!("1"));
    }
    MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/collector"))
        .context(context())
        .hop(hop)
        .body(assistant(text))
        .build()
}

struct Rig {
    cell: ProxyCell,
    cap: Arc<Mutex<Vec<CapturedRequest>>>,
    sink: OutputSink,
    _out: mpsc::Receiver<CellEmission>,
    db: DbConn,
    rc: mpsc::Sender<meclaw_cells::proxy::io::ProxyReconfig>,
    _rc_rx: mpsc::Receiver<meclaw_cells::proxy::io::ProxyReconfig>,
}

async fn rig(mode: Option<PartialDelivery>) -> Rig {
    let (addr, _j, cap) =
        start_mock_server_capturing(vec![MockResponse::ok_json(br#"{"ok":true,"result":{}}"#)])
            .await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    let cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        5000,
        5000,
        "https://api.telegram.org".into(),
    );
    let cell = match mode {
        Some(m) => cell.with_partial_delivery(m),
        None => cell,
    };
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(16);
    let sink = OutputSink::new(
        out_tx,
        Path::new("/p"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );
    let (rc, rc_rx) = mpsc::channel(8);
    let db = DbConn::wrap(rusqlite::Connection::open_in_memory().unwrap(), None);
    Rig {
        cell,
        cap,
        sink,
        _out: out_rx,
        db,
        rc,
        _rc_rx: rc_rx,
    }
}

impl Rig {
    async fn feed(&mut self, msg: Message) {
        self.cell
            .handle(msg, &self.sink, &mut self.db, &self.rc)
            .await;
    }

    /// The text of every `sendMessage` the connector put on the wire, in order.
    async fn sent_texts(&self) -> Vec<String> {
        self.cap
            .lock()
            .await
            .iter()
            .filter(|r| r.path.rsplit('/').next() == Some("sendMessage"))
            .map(|r| {
                let v: Value = meclaw_core::serde_json::from_slice(&r.body).unwrap();
                v["text"].as_str().unwrap_or_default().to_string()
            })
            .collect()
    }
}

/// A repeat is not sent: the person read these words in this turn already.
/// And it ends the channel's turn as a sent answer does -- with an early
/// message on the wire, the turn it opened is closed and no rest is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_oneanswer_a_repeat_is_not_sent_and_ends_the_turn_at_the_channel() {
    let said = "The next train to Hamburg leaves at 14:05.";
    let mut r = rig(None).await;
    r.feed(answer(said, Some("interim"))).await;
    r.feed(answer(said, Some("repeat"))).await;
    assert_eq!(
        r.sent_texts().await,
        vec![said],
        "the words went out once, beside the call; the repeat is not sent"
    );

    let mut p = rig(Some(PartialDelivery::FirstSentence)).await;
    p.feed(partial("One moment.", 1)).await;
    p.feed(answer("One moment. It leaves at 14:05.", Some("repeat")))
        .await;
    assert_eq!(
        p.sent_texts().await,
        vec!["One moment."],
        "a repeat sends no rest either"
    );
    assert_eq!(
        p.cell.open_partial_turns(),
        0,
        "the repeat ends the turn like an answer that was sent"
    );

    // An answer without the mark is sent as before.
    let mut plain = rig(None).await;
    plain.feed(answer(said, None)).await;
    assert_eq!(plain.sent_texts().await, vec![said]);

    // Every connector that writes holds the same rule, each at its own place
    // of delivery. The second one, whose chat address is a string:
    let chat = || json!("C1:100.000100");
    assert_eq!(
        posted(vec![
            answer_in(chat(), said, Some("interim")),
            answer_in(chat(), said, Some("repeat")),
        ])
        .await,
        vec![said],
        "the second connector posts the words once; the repeat is not posted"
    );
    assert_eq!(
        posted(vec![answer_in(chat(), said, None)]).await,
        vec![said],
        "an answer without the mark is posted as before"
    );

    // And the connector to a peer: a repeat is not posted to the far side, and
    // its one receipt is the one of a message that went through.
    let (receipt, connects) = crossing(true).await;
    assert_eq!(connects, 0, "a repeat does not reach the peer: {receipt}");
    assert_eq!(
        receipt["peer_event"],
        json!("crossed"),
        "the repeat ends the turn like a message that went through: {receipt}"
    );
    let (receipt, connects) = crossing(false).await;
    assert_eq!(
        (receipt["peer_event"].clone(), connects),
        (json!("crossed"), 1),
        "a message without the mark is posted as before: {receipt}"
    );
}

/// The edge that turns an answer into something to say is the channel's own:
/// `./channels -> ./channels/<node>`, re-stamped to the speaking lane. The
/// member's edge above it (`./assistants -> ./channels`) serves written and
/// spoken channels alike and stays as it is -- a channel that writes needs the
/// repeat to end its turn. So the condition `!has(hop.repeat)` stands on the
/// speaking edge, as `templates/voice/README.md` hands it to a reader, and
/// this pin boots the shipped `member` with exactly that edge:
/// - a repeat never reaches the speaking input,
/// - the same repeat reaches a channel that writes, behind the member's edge,
/// - an answer without the mark reaches the speaking input.
///
/// Each channel is a double that reports what it heard, so every claim is an
/// arrival; the repeat's absence is read off the order of one road (had it
/// reached the speaking input, its report would be the first).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_oneanswer_a_repeat_never_reaches_a_speaking_input() {
    let Some((member, speaking)) = speaking_edge_shipped() else {
        eprintln!("member/voice did not travel into this tree, or no python3 -- skipped (GH #49)");
        return;
    };
    assert!(
        speaking["condition"]
            .as_str()
            .is_some_and(|c| c.contains("!has(hop.repeat)")),
        "the documented speaking edge lets a repeat through: {speaking}"
    );
    // The telephone's README hands a reader the same edge under its own node name.
    let phone = std::fs::read_to_string(repo_path("templates/freeswitch/README.md"))
        .expect("templates/freeswitch/README.md");
    let same = speaking["condition"]
        .as_str()
        .expect("a condition")
        .replace("'voice'", "'freeswitch'");
    assert!(
        phone.contains(&same),
        "templates/freeswitch/README.md hands out another speaking edge than \
         templates/voice/README.md: {same}"
    );

    let td = tempfile::tempdir().expect("a temporary directory");
    build_member_with_two_channels(&td, &member, &speaking);
    let (h, mut rx) = boot_member(&td).await;

    let twice = "The next train to Hamburg leaves at 14:05.";
    let fresh = "It leaves from platform 7.";
    h.send(say(SPEAKS, Some("repeat"), twice)).await;
    h.send(say(SPEAKS, None, fresh)).await;
    let first = heard(&mut rx).await;
    assert_eq!(
        (first.by.as_str(), first.route.as_str(), first.text.as_str()),
        (SPEAKS, "in_speak", fresh),
        "the first thing the speaking input hears is the answer without the mark; \
         a repeat that reached it would have been first"
    );
    assert_eq!(first.repeat, "", "an answer without the mark carries none");

    h.send(say(WRITES, Some("repeat"), twice)).await;
    let second = heard(&mut rx).await;
    assert_eq!(
        (
            second.by.as_str(),
            second.route.as_str(),
            second.repeat.as_str(),
            second.text.as_str()
        ),
        (WRITES, "answer", "1", twice),
        "the same repeat reaches a channel that writes, so it can end its turn"
    );
    h.shutdown().await;
}

// oneanswer

use meclaw_cells::proxy::meclaw::{cell::MeclawCell, params::MeclawParams};
use meclaw_cells::proxy::slack::cell::SlackCell;
use meclaw_cells::proxy::slack::client::SlackClient;
use meclaw_cells::proxy::slack::db::setup_slack_schema;
use meclaw_cells::proxy::slack::params::SlackParams;
use meclaw_testing::mock_slack::MockSlack;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn sink_of(tx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx,
        Path::new("/p"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    )
}

/// [`answer`] for a connector whose chat address is not a number.
fn answer_in(chat: Value, text: &str, mark: Option<&str>) -> Message {
    let mut ctx = Map::new();
    ctx.insert("chat_id".into(), chat);
    let mut hop = Map::new();
    hop.insert("route".into(), json!("answer"));
    if let Some(key) = mark {
        hop.insert(key.to_string(), json!("1"));
    }
    MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/collector"))
        .context(ctx)
        .hop(hop)
        .body(assistant(text))
        .build()
}

/// Feeds `msgs` to the second writing connector (pattern:
/// `misc_1/gh1015_outbound_proxies_send_once.rs`) and returns the text of
/// every post its platform fake saw, in order.
async fn posted(msgs: Vec<Message>) -> Vec<String> {
    let server = MockSlack::start().await.expect("fake");
    let params = SlackParams::parse(&json!({
        "app_token": "xapp-x", "bot_token": "xoxb-x",
        "emit_to": "/agent", "base_url": server.base_url()
    }))
    .unwrap();
    let mut cell = SlackCell::new(&params, SlackClient::new(&params).unwrap());
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_slack_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (tx, _rx) = mpsc::channel::<CellEmission>(8);
    let sink = sink_of(tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    for msg in msgs {
        cell.handle(msg, &sink, &mut db, &rc_tx).await;
    }
    server
        .posts()
        .await
        .iter()
        .filter_map(|p| p.text().map(str::to_string))
        .collect()
}

/// Reads the head of a request and as much body as `Content-Length` says.
async fn read_request(s: &mut tokio::net::TcpStream) {
    let mut buf = Vec::new();
    let mut b = [0u8; 4096];
    let head = loop {
        match s.read(&mut b).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&b[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let text = String::from_utf8_lossy(&buf[..head]).to_ascii_lowercase();
    let len = text
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < head + len {
        match s.read(&mut b).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&b[..n]),
        }
    }
}

/// Drives the connector to a peer once (pattern:
/// `misc_6/gh840_a_peer_url_outside_egress_is_refused.rs`, lane and body as
/// there) with or without the mark, against a listener that counts every
/// connection and answers each with a `crossed` receipt. Returns the header of
/// the connector's one receipt and the number of connections the peer saw.
async fn crossing(repeat: bool) -> (Value, usize) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let peer = l.local_addr().expect("addr");
    let n = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&n);
    let body = meclaw_cells::proxy::meclaw::wire::crossed_receipt("proposal", "south", &[]);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            seen.fetch_add(1, Ordering::SeqCst);
            let body = body.to_string();
            tokio::spawn(async move {
                read_request(&mut s).await;
                let msg = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                     {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(msg.as_bytes()).await;
                let _ = s.shutdown().await;
            });
        }
    });
    let v = json!({"platform": "meclaw", "mount": "peer", "identity_header": "X-Meclaw-Peer",
        "boundary": "north", "emit_to": "/sink", "external_timeout_ms": 5000,
        "egress": [format!("http://{peer}")], "lanes": {
            "accepts": [],
            "emits": [{"route": "proposal", "fields": ["proposal"], "because": "one proposal"}]}});
    let p = MeclawParams::parse(&v).expect("params");
    let mut cell = MeclawCell::new(&p).expect("cell");
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = sink_of(tx);
    let mut hop = Map::new();
    hop.insert("route".into(), json!("proposal"));
    hop.insert("peer".into(), json!("south"));
    hop.insert("peer_url".into(), json!(format!("http://{peer}/peer/")));
    if repeat {
        hop.insert("repeat".into(), json!("1"));
    }
    let msg = MessageBuilder::new(Path::new("/friend"))
        .ttl(5)
        .hop(hop)
        .body(Body::Inline(json!({"proposal": "a walk"})))
        .build();
    let (rc_tx, _rc_rx) = mpsc::channel(1);
    let mut db = DbConn::wrap(rusqlite::Connection::open_in_memory().expect("db"), None);
    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    let em = tokio::time::timeout(meclaw_testing::deadline::HANG, rx.recv())
        .await
        .expect("a receipt before the hang mark")
        .expect("a receipt");
    (em.content["header"].clone(), n.load(Ordering::SeqCst))
}

// oneanswer: the speaking edge (pin 8)

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;

/// The node the documented speaking edge addresses, and a channel that writes.
const SPEAKS: &str = "voice";
const WRITES: &str = "letters";

fn repo_path(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json_file(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write_json_file(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent directory")).expect("create the directory");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

/// The edge `./channels -> ./channels/voice` of the binding manifest in
/// `templates/voice/README.md`: the first fenced JSON block that grows a node
/// and wires it. What a reader copies out of the README is what is booted.
fn documented_speaking_edge(readme: &str) -> Value {
    for block in readme.split("```json\n").skip(1) {
        let raw = block.split("```").next().unwrap_or_default();
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(raw) else {
            continue;
        };
        if !(v["diff"]["add_edges"].is_array() && v["diff"]["add_nodes"].is_array()) {
            continue;
        }
        return v["diff"]["add_edges"]
            .as_array()
            .expect("edges")
            .iter()
            .find(|e| {
                e["from"] == "./channels" && e["modifier"]["set_hop"]["route"] == "'in_speak'"
            })
            .cloned()
            .expect("the binding manifest draws the edge down to the speaking input");
    }
    panic!("templates/voice/README.md carries no add_nodes+add_edges manifest");
}

/// `(member template, documented speaking edge)`, or `None` where the tree
/// did not ship both or has no python3 for the doubles (GH #49).
fn speaking_edge_shipped() -> Option<(std::path::PathBuf, Value)> {
    let member = repo_path("templates/member");
    let readme = std::fs::read_to_string(repo_path("templates/voice/README.md")).ok()?;
    if !member.join("config.json").is_file() {
        return None;
    }
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .ok()?;
    Some((member, documented_speaking_edge(&readme)))
}

/// Only `config.json` files travel, so the tree under test is the template.
fn copy_configs(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create the directory");
    for entry in std::fs::read_dir(src).expect("the template directory is readable") {
        let entry = entry.expect("directory entry");
        let from = entry.path();
        if from.is_dir() {
            copy_configs(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).expect("copy the config");
        }
    }
}

/// A holder this round never reaches. It exists because a hive door pointing
/// at an absent directory leaves the inside unroutable (GH #286).
const INERT_DOUBLE: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// Outside the member: hands one request to say something on to the member's
/// assistants, naming the channel and the mark.
const DRIVER_DOUBLE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({
    "header": {"route": "say", "to_channel": str(hop.get("to_channel") or ""),
               "mark": str(hop.get("mark") or "")},
    "messages": doc["body"].get("messages") or []}))
"#;

/// The member's assistants, doubled at what leaves them: one `answer`, with
/// the hop field the request asked for and no other.
const ASSISTANTS_DOUBLE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
head = {"route": "answer"}
if hop.get("mark"):
    head[str(hop.get("mark"))] = "1"
sys.stdout.write(json.dumps({"header": head,
                             "messages": doc["body"].get("messages") or []}))
"#;

/// A channel, doubled at the one question this pin asks it: what did it hear?
/// It reports on `error`, the lane the member ships an exit for.
const CHANNEL_DOUBLE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
body = doc["body"]
sys.stdout.write(json.dumps({
    "header": {"route": "error", "heard_by": "@NODE@",
               "heard_route": str(hop.get("route") or ""),
               "heard_repeat": str(hop.get("repeat") or ""),
               "heard_text": str(((body.get("messages") or [{}])[0] or {}).get("text") or "")},
    "messages": body.get("messages") or []}))
"#;

fn code_double(script: &str, purpose: &str) -> Value {
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

/// The shipped member with two channels below `./channels`: the speaking one
/// behind the documented edge, the writing one behind the plain answer edge a
/// channel that writes costs. Both report upwards on `error`.
fn build_member_with_two_channels(
    td: &tempfile::TempDir,
    member: &std::path::Path,
    speaking: &Value,
) {
    let root = td.path();
    let outer = json!([
        {"from": "./driver", "to": "./person/assistants",
         "condition": "has(hop.route) && hop.route == 'say'",
         "modifier": {"set_context": {
             "channel_node": "has(hop.to_channel) ? hop.to_channel : ''"}}},
        {"from": "./person", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'error'"}
    ]);
    write_json_file(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": outer}}}),
    );
    write_json_file(
        root,
        "main/driver/config.json",
        &code_double(
            DRIVER_DOUBLE,
            "Test driver: asks the assistants to say one thing.",
        ),
    );

    copy_configs(member, &root.join("main/person"));
    for holder in [
        "access",
        "affinity",
        "memory-hive",
        "file-space",
        "graph-space",
        "librarian",
        "objects",
        "apps",
        "firewall",
    ] {
        write_json_file(
            root,
            &format!("main/person/{holder}/config.json"),
            &code_double(
                INERT_DOUBLE,
                "Inert double for a holder this round never reaches.",
            ),
        );
    }
    write_json_file(
        root,
        "main/person/assistants/config.json",
        &code_double(
            ASSISTANTS_DOUBLE,
            "Test double for what leaves the assistants.",
        ),
    );
    for node in [SPEAKS, WRITES] {
        write_json_file(
            root,
            &format!("main/person/channels/{node}/config.json"),
            &code_double(
                &CHANNEL_DOUBLE.replace("@NODE@", node),
                "Test double for a channel: reports what it heard.",
            ),
        );
    }

    let cfg_path = root.join("main/person/config.json");
    let mut cfg = read_json_file(&cfg_path);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the member ships a graph");
    edges.push(speaking.clone());
    edges.push(json!({
        "from": "./channels", "to": format!("./channels/{WRITES}"),
        "condition": format!("has(hop.route) && hop.route == 'answer' && \
                              has(context.channel_node) && context.channel_node == '{WRITES}'")
    }));
    for node in [SPEAKS, WRITES] {
        edges.push(json!({
            "from": format!("./channels/{node}"), "to": "./channels",
            "condition": "has(hop.route) && hop.route == 'error'"
        }));
    }
    std::fs::write(
        &cfg_path,
        meclaw_core::serde_json::to_string_pretty(&cfg).expect("serialise"),
    )
    .expect("write the member config");
    std::fs::write(root.join(".env"), "").expect("write an empty .env");
}

async fn boot_member(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
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
        .expect("the shipped member and its two channel doubles must boot");
    (h, sink_rx)
}

/// One request to the driver: say `text` to the channel `node`, with `mark`
/// on the answer's hop or without one.
fn say(node: &str, mark: Option<&str>, text: &str) -> Message {
    let mut hop = Map::new();
    hop.insert("to_channel".into(), json!(node));
    if let Some(key) = mark {
        hop.insert("mark".into(), json!(key));
    }
    MessageBuilder::new(Path::new("/driver"))
        .body(assistant(text))
        .hop(hop)
        .ttl(200)
        .build()
}

/// What a channel double reported it heard.
struct Heard {
    by: String,
    route: String,
    repeat: String,
    text: String,
}

/// The next report at the sink. The wait is a hang mark, not a verdict.
async fn heard(rx: &mut mpsc::Receiver<Message>) -> Heard {
    let m = tokio::time::timeout(meclaw_testing::deadline::HANG, rx.recv())
        .await
        .expect("a channel's report before the hang mark")
        .expect("a channel's report");
    let field = |key: &str| {
        m.headers
            .hop
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Heard {
        by: field("heard_by"),
        route: field("heard_route"),
        repeat: field("heard_repeat"),
        text: field("heard_text"),
    }
}
