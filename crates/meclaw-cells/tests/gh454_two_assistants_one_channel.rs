//! GH #454 — a channel belongs to the MEMBER, and an assistant is addressed
//! through it.
//!
//! Until this task a channel was a pair of nodes inside
//! `<member>/assistants/<agent>/channels`. The container belonged to the
//! ASSISTANT, so the channel did too, and three things followed that nobody
//! wanted:
//!
//! - **a bot was one agent's bot.** A member with two assistants sharing one
//!   chat account could not be reached on both — the second agent needed a
//!   second account and a second address the person had to remember;
//! - **a screen had no owner.** A display is a channel of the person, not of one
//!   of their agents, and two agents may legitimately hold views on it at once;
//! - **a generation swap took the channel with it.** An assistant is replaced
//!   per generation, and a chat account is not a thing a generation owns.
//!
//! So the container moved up to `<member>/channels`, which is where the memory,
//! the record and the screen already were (GH #122, ADR 0012 — *a level owns
//! what its siblings must share*). What is measured here is the consequence, on
//! a booted colony rather than off the files: **one channel delivering to two
//! assistants of one person, and the answer finding its way back.**
//!
//! # The addressing rule, v1
//!
//! The channel stamps `context.assistant` with the name of the agent the message
//! was meant for. Where that name comes from is the channel's own business and
//! never a model's: here the connector parses a `name:` prefix off the text and
//! puts it on `hop.addressed_to`, and the channel's outbound edge falls back to
//! a literal default when it finds none. The member's `./assistants` container
//! then fans out, **one edge per assistant**, guarded on
//! `context.assistant == '<name>'`.
//!
//! One edge per assistant is a rule and not an accident: `Edge.to` is a static
//! `Path` in this substrate (`crates/meclaw-colony/src/edge_table.rs`), so there
//! is no way to write "send it wherever the context says". The cost is one edge
//! per direction per assistant, plus one edge per channel on the way back — a
//! sum, never the cross product.
//!
//! # What is booted
//!
//! The SHIPPED `member` and `assistant` templates, cell for cell, with every
//! `ref` marker replaced by an answering `code` double and with the edges an
//! instantiating mutation would draw appended to the member's own graph.
//! Doubling by REPLACING a `config.json` rather than by deleting a directory is
//! the lesson of GH #286: a hive door pointing at a directory that is not there
//! leaves the inside unroutable, which is a different topology on exactly the
//! property under test.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and a template that did not travel is skipped
//! rather than judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// A path inside this repository, from the crate's manifest directory.
fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The two templates this test boots, or `None` when the tree under test did not
/// travel into the public export.
fn shipped() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let member = repo("templates/member");
    let assistant = repo("templates/assistant");
    (member.join("config.json").is_file() && assistant.join("config.json").is_file())
        .then_some((member, assistant))
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

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

/// Copy the template cell by cell: only `config.json` files travel, so the tree
/// under test IS the template and nothing else.
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

// ══════════════════════════════════════════════════════════════ the doubles

/// A cell that answers nothing: the holders of the member that this round never
/// reaches. They exist because a hive door pointing at an absent directory
/// leaves the inside unroutable.
const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// The member's screen, doubled. It passes every turn and it touches no context:
/// what the channel stamped has to survive the screen, because `context.channel`,
/// `context.user_id`, `context.audience_set` and `context.assistant` are all read
/// further down than this.
const FIREWALL: &str = r#"
import sys, json
doc = json.load(sys.stdin)
sys.stdout.write(json.dumps({
    "header": {"route": "pass", "screened": "1"},
    "messages": doc["body"].get("messages", [])}))
"#;

/// The connector of one channel, doubled — one cell, the way
/// `telegram-connector@2` ships since GH #303.
///
/// Two errands on one wire, told apart the way the shipped connector's callers
/// tell them apart:
///
/// - a WAKE (`in_wire`) is an inbound message from the outside world. It parses
///   the address rule — a leading `name:` prefix — onto `hop.addressed_to` and
///   hands the stripped text up. Whether a name was found is the channel's
///   business; the fallback to the channel's default lives in the outbound edge,
///   as a literal, because CEL cannot read a cell's params.
/// - an `answer` is what an assistant said. In production this is where the
///   round ENDS — the connector writes to a chat account and the colony sees
///   nothing more. A test needs a witness, so the double reports the delivery on
///   the one exit a channel actually ships: `error`, which the member carries out
///   of the level on `./channels -> .`. The assertion reads the hop keys, not the
///   lane name; what matters is that the answer physically reached the cell that
///   owns the account.
const CONNECTOR: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
msgs = doc["body"].get("messages") or []
text = str((msgs[0] if msgs else {}).get("text") or "")

if str(hop.get("route") or "") == "answer":
    sys.stdout.write(json.dumps({
        "header": {"error_code": "delivered",
                   "delivered_by": str(hop.get("served_by") or ""),
                   "delivered_channel_node": str(ctx.get("channel_node") or ""),
                   "delivered_channel": str(ctx.get("channel") or ""),
                   "delivered_assistant": str(ctx.get("assistant") or ""),
                   "delivered_user": str(ctx.get("user_id") or ""),
                   "delivered_audience": str(ctx.get("audience_set") or ""),
                   "delivered_text": text},
        "messages": msgs}))
else:
    addressed, body = "", text
    if ":" in text:
        head, rest = text.split(":", 1)
        head = head.strip()
        if head and head.replace("-", "").isalnum() and head.islower():
            addressed, body = head, rest.strip()
    sys.stdout.write(json.dumps({
        "header": {"wire": "in", "addressed_to": addressed},
        "messages": [{"origin": "user", "type": "text", "text": body}]}))
"#;

/// The conversation surface of one generation, doubled. It answers, and it
/// carries back WHAT IT SAW: the four context keys the ingress stamped, on the
/// hop, so the drift lock can read them at the sink rather than infer them.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
ctx = hdr.get("context") or {}
who = str((doc.get("params") or {}).get("who") or "")
sys.stdout.write(json.dumps({
    "header": {"route": "answer", "served_by": who,
               "saw_channel_node": str(ctx.get("channel_node") or ""),
               "saw_channel": str(ctx.get("channel") or ""),
               "saw_user": str(ctx.get("user_id") or ""),
               "saw_audience": str(ctx.get("audience_set") or ""),
               "saw_assistant": str(ctx.get("assistant") or "")},
    "messages": [{"origin": "assistant", "type": "text", "text": "answered by " + who}]}))
"#;

/// Puts one message on a named lane. `mode` decides which door it takes: the
/// channel's wire, or the member's own `in_turn` rim door.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
mode = str(hop.get("mode") or "wire")
route = "in_wire" if mode == "wire" else "in_turn"
sys.stdout.write(json.dumps({
    "header": {"route": route, "addressed_to": str(hop.get("addressed_to") or "")},
    "messages": doc["body"].get("messages", [])}))
"#;

/// A `code` double with a fixed script and, optionally, one param the script
/// reads. `emits` is left wide on purpose: what a double may say is decided by
/// the assertions, not by a contract nobody else reads.
fn double(script: &str, params: Value, purpose: &str) -> Value {
    let mut p = json!({
        "runner": "python3",
        "script_inline": script,
        "external_timeout_ms": 10000
    });
    if let Value::Object(extra) = params {
        for (k, v) in extra {
            p[k] = v;
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": p,
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

// ══════════════════════════════════════════════════════ the wiring a mutation draws

/// The name the channel is instantiated under, which is also the value
/// `context.channel_node` carries: a channel's directory name is a fact of the
/// wiring, not a label. Since GH #522 it is the ADDRESS half of the old
/// `context.channel`; the other half is the chat this test's double parks on
/// `CHAT`.
const CHANNEL: &str = "chat";
/// What the surface calls the same conversation partner — the value the session
/// keeper opens a generation for and the firewall rate-limits (GH #522). A
/// double raises no real chat id, so the wiring names one.
const CHAT: &str = "chat-4711";
/// The assistant a message that named nobody goes to.
const DEFAULT_ASSISTANT: &str = "alpha";
/// The assistant a message has to NAME to reach.
const OTHER_ASSISTANT: &str = "beta";

/// The three edges one channel costs, exactly as `examples/organism` draws them.
///
/// The address rule sits in the first of them and nowhere else. Its three cases
/// are, in order: a context an operator or a test set explicitly; the name the
/// connector parsed off the wire; and the channel's own default, written as a
/// literal because CEL has no access to a cell's params.
fn channel_edges() -> Vec<Value> {
    let address_rule = format!(
        "has(context.assistant) && context.assistant != '' ? context.assistant : \
         (has(hop.addressed_to) && hop.addressed_to != '' ? hop.addressed_to : '{DEFAULT_ASSISTANT}')"
    );
    vec![
        json!({
            "from": format!("./channels/{CHANNEL}"), "to": "./channels",
            "condition": "!has(hop.error_code)",
            "modifier": {
                "set_hop": {"route": "'turn'"},
                "set_context": {
                    "channel_node": format!("'{CHANNEL}'"),
                    "channel": format!("has(hop.chat_id) ? hop.chat_id : '{CHAT}'"),
                    "user_id": "has(hop.user_id) ? hop.user_id : 'u-1'",
                    "audience_set": "'[\"member:person\"]'",
                    "assistant": address_rule
                }
            }
        }),
        json!({
            "from": format!("./channels/{CHANNEL}"), "to": "./channels",
            "condition": "has(hop.error_code)",
            "modifier": {"set_hop": {"route": "'error'"}}
        }),
        json!({
            "from": "./channels", "to": format!("./channels/{CHANNEL}"),
            "condition": format!(
                "has(hop.route) && hop.route == 'answer' && has(context.channel_node) \
                 && context.channel_node == '{CHANNEL}'")
        }),
    ]
}

/// The two edges one assistant costs. There is no third: everything else the
/// generation raises is already carried by the member's own graph.
fn assistant_edges(name: &str) -> Vec<Value> {
    vec![
        json!({
            "from": "./assistants", "to": format!("./assistants/{name}"),
            "condition": format!(
                "has(hop.route) && hop.route == 'in_turn' && has(context.assistant) && context.assistant == '{name}'")
        }),
        json!({
            "from": format!("./assistants/{name}"), "to": "./assistants",
            "condition": "has(hop.route) && hop.route == 'answer'"
        }),
    ]
}

/// The colony around the member: one driver, and a drain for every lane the
/// member emits. Draining all ten is the point — an undrained lane is a dead
/// letter, and this test would then be reading a silence.
fn main_config() -> Value {
    let mut edges = vec![
        json!({
            "from": "./driver", "to": format!("./person/channels/{CHANNEL}"),
            "condition": "has(hop.route) && hop.route == 'in_wire'"
        }),
        json!({
            "from": "./driver", "to": "./person",
            "condition": "has(hop.route) && hop.route == 'in_turn'",
            "modifier": {"set_context": {
                "assistant": "has(hop.addressed_to) ? hop.addressed_to : ''"
            }}
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
        "build",
        "close_report",
        "export_done",
    ] {
        edges.push(json!({"from": "./person", "to": "/sink",
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

/// Stage the tree: the shipped member, the shipped assistant twice, the doubles,
/// and the edges an instantiating mutation would draw appended to the member's
/// own graph.
fn build_tree(td: &tempfile::TempDir, member: &std::path::Path, assistant: &std::path::Path) {
    let root = td.path();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/driver/config.json",
        &double(
            DRIVER,
            json!({}),
            "Test driver: puts one message on a named lane.",
        ),
    );

    copy_cells(member, &root.join("main/person"));
    for holder in ["affinity", "memory-hive", "export-sink"] {
        write(
            root,
            &format!("main/person/{holder}/config.json"),
            &double(
                INERT,
                json!({}),
                "Inert double for a holder this round never reaches.",
            ),
        );
    }
    write(
        root,
        "main/person/firewall/config.json",
        &double(FIREWALL, json!({}), "Test double for the member's screen."),
    );
    write(
        root,
        &format!("main/person/channels/{CHANNEL}/config.json"),
        &double(
            CONNECTOR,
            json!({}),
            "Test double for the connector of one channel.",
        ),
    );

    for who in [DEFAULT_ASSISTANT, OTHER_ASSISTANT] {
        let dst = root.join(format!("main/person/assistants/{who}"));
        copy_cells(assistant, &dst);
        write(
            root,
            &format!("main/person/assistants/{who}/talky/config.json"),
            &double(
                SURFACE,
                json!({"who": who}),
                "Test double for the conversation surface of one generation.",
            ),
        );
        for sibling in ["cogny", "tools"] {
            write(
                root,
                &format!("main/person/assistants/{who}/{sibling}/config.json"),
                &double(
                    INERT,
                    json!({}),
                    "Inert double for a sibling this round never reaches.",
                ),
            );
        }
    }

    // The instantiation edges: the member's own graph plus what the mutations
    // that stage a channel and two assistants would add to it.
    let cfg_path = root.join("main/person/config.json");
    let mut cfg = read_json(&cfg_path);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the member ships a graph");
    edges.extend(channel_edges());
    edges.extend(assistant_edges(DEFAULT_ASSISTANT));
    edges.extend(assistant_edges(OTHER_ASSISTANT));
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
        .expect("the shipped member and assistant must boot");
    (h, sink_rx)
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// One message into the driver. `mode` picks the door, `text` is what the person
/// typed, `addressed_to` is what an operator asserts when it bypasses a channel.
fn inject(mode: &str, text: &str, addressed_to: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("mode".into(), json!(mode));
    hop.insert("addressed_to".into(), json!(addressed_to));
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": text}
        ]})))
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

/// Drive one round and hand back what reached the sink. Every double answers, so
/// the receipt is POSITIVE: a round that went to the wrong assistant arrives
/// naming the wrong one, it does not go quiet.
async fn round(mode: &str, text: &str, addressed_to: &str) -> Message {
    let Some((member, assistant)) = shipped() else {
        panic!("guarded by the caller");
    };
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member, &assistant);
    let (h, mut rx) = boot(&td).await;
    h.send(inject(mode, text, addressed_to)).await;
    let got = recv_bounded(&mut rx)
        .await
        .expect("the round has to reach the sink -- every double answers");
    h.shutdown().await;
    got
}

// ══════════════════════════════════════════════════════════════ the measurements

/// A turn that NAMES an assistant reaches that assistant, and the answer comes
/// back at the channel it arrived on.
///
/// This is the whole acceptance of #454 in one round: one member, two
/// assistants, ONE channel. Before the move the second agent needed a second
/// account, because the container the channel stood in belonged to the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_names_an_assistant_reaches_that_assistant() {
    if shipped().is_none() {
        eprintln!("member/assistant did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let got = round("wire", &format!("{OTHER_ASSISTANT}: hello"), "").await;

    assert_eq!(
        hop_of(&got, "error_code"),
        "delivered",
        "the receipt has to be the connector's delivery, not something else: {:?}",
        got.headers.hop
    );
    assert_eq!(
        hop_of(&got, "delivered_by"),
        OTHER_ASSISTANT,
        "the named assistant is the one that answered: {:?}",
        got.headers.hop
    );
    assert_eq!(
        hop_of(&got, "delivered_assistant"),
        OTHER_ASSISTANT,
        "and the name it was addressed under travelled the whole way with it"
    );
    assert_eq!(
        hop_of(&got, "delivered_channel_node"),
        CHANNEL,
        "the answer came back at the channel the turn arrived on -- the member's \
         `./assistants -> ./channels` edge routes it by `context.channel_node`, and the \
         per-channel edge inside the container turns that name into an address"
    );
    assert_eq!(
        hop_of(&got, "delivered_channel"),
        CHAT,
        "and the CHAT rode along beside the address (GH #522): it is what the \
         session keeper opens a generation for and what the reply goes to"
    );
    assert_eq!(
        hop_of(&got, "delivered_text"),
        format!("answered by {OTHER_ASSISTANT}"),
        "the answer that came back is the one that assistant produced"
    );
}

/// A turn that names NOBODY reaches the channel's default assistant.
///
/// The default is the CHANNEL's, written as a literal in the channel's own
/// outbound edge — not a rule of the container, and not a decision a model
/// makes. A second channel of the same person may default somewhere else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_names_nobody_reaches_the_channels_default() {
    if shipped().is_none() {
        eprintln!("member/assistant did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let got = round("wire", "hello", "").await;

    assert_eq!(
        hop_of(&got, "delivered_by"),
        DEFAULT_ASSISTANT,
        "an unaddressed turn takes the channel's default: {:?}",
        got.headers.hop
    );
    assert_eq!(
        hop_of(&got, "delivered_assistant"),
        DEFAULT_ASSISTANT,
        "the default is stamped as a name like any other -- the container's fan-out \
         never sees an empty `context.assistant`, which is why it needs no default edge"
    );
    assert_eq!(hop_of(&got, "delivered_channel_node"), CHANNEL);
    assert_eq!(hop_of(&got, "delivered_channel"), CHAT);
}

/// The ingress stamps the channel writes are the ones the surface reads, three
/// cells further down — unchanged, and unchanged by the screen in between.
///
/// This is a DRIFT LOCK. `context.channel_node`, `context.channel`,
/// `context.user_id` and
/// `context.audience_set` are stamped by the channel's outbound edge, promoted
/// again by the member's `./channels -> ./firewall` door, and read at the bottom
/// of the generation. A stamp lost on the way does not make a turn vanish — it
/// gives it a different rate bucket, an empty audience or a session that belongs
/// to nobody, all of which look like working software.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ingress_stamps_reach_the_surface_unchanged() {
    if shipped().is_none() {
        eprintln!("member/assistant did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let got = round("wire", &format!("{OTHER_ASSISTANT}: hello"), "").await;

    assert_eq!(
        hop_of(&got, "delivered_channel_node"),
        CHANNEL,
        "the channel name has to survive the screen"
    );
    assert_eq!(
        hop_of(&got, "delivered_channel"),
        CHAT,
        "and so does the chat it is in -- a chat lost on the way does not make \
         a turn vanish, it opens the session of somebody else (GH #522)"
    );
    assert_eq!(
        hop_of(&got, "delivered_user"),
        "u-1",
        "the user the rate window is kept per has to survive it too"
    );
    assert_eq!(
        hop_of(&got, "delivered_audience"),
        r#"["member:person"]"#,
        "and the audience the round is asked in, byte for byte: an audience that \
         arrives empty is not a recall without a filter, it is one that answers \
         the wrong person"
    );
}

/// A turn that entered at the member's OWN door, naming no channel, leaves the
/// member on `answer` — the guarded default, not a dead letter.
///
/// The lane was declared at this level and at both levels above it long before
/// anything could travel it: `org` and `meclaw-os` both say "a turn answered, a
/// brief read". Until #454 only the brief half could ever fire, because the
/// connector that consumed the answer stood inside the generation. Now the
/// member routes an answer that names a channel INTO that channel and carries
/// the rest out, and exactly one of the two edges fires per message (GH #283).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_that_names_no_channel_leaves_the_member() {
    if shipped().is_none() {
        eprintln!("member/assistant did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let got = round("rim", "hello", DEFAULT_ASSISTANT).await;

    assert_eq!(
        hop_of(&got, "route"),
        "answer",
        "an operator's turn is answered on the member's own answer lane: {:?}",
        got.headers.hop
    );
    assert_eq!(
        hop_of(&got, "served_by"),
        DEFAULT_ASSISTANT,
        "and by the assistant the operator named in context"
    );
    assert_eq!(
        hop_of(&got, "error_code"),
        "",
        "it did NOT go through a channel -- a delivery receipt here would mean both \
         edges fired, and the guarded default exists precisely so that only one does"
    );
    assert_eq!(
        hop_of(&got, "saw_channel_node"),
        "",
        "the turn named no channel, which is what made the default the one that fires"
    );
}
