//! GH #598 — a display receipt is FEEDBACK TO A WRITER, never a turn of a
//! person, and a screen is bound on `view` alone.
//!
//! # The loop that paid for this file
//!
//! GH #459 wired a screen as a channel of the member and drew two edges for it.
//! The down-edge took `answer` as well as `view`, on the claim that an agent's
//! ordinary prose answer IS the smallest view there is. It is not: the display's
//! `compose` reads a view out of the BODY (`view_id`, `kind`, `content`) and an
//! answer carries none of them, so every such answer came back refused —
//! `receipt`, `error_code: invalid_view`. The member then routed that receipt
//! back to the writer's container and re-stamped it onto `in_turn` with
//! `hop.kind = 'receipt'`, the generation read it as a turn, the brain answered
//! in prose, and the prose went to the screen again. Measured on a live colony:
//! 44 brain calls in six minutes, and on a second one 68.
//!
//! Both halves are closed here:
//!
//! 1. **The screen is bound on `view` and nothing else.** No shipped recipe
//!    binds it on `answer` any more —
//!    `the_shipped_recipes_bind_a_screen_on_view_alone` reads the two places a
//!    screen is grown from and refuses the `answer` clause by name.
//! 2. **A receipt raises no turn.** The member carries a receipt it cannot hand
//!    to an app out of the level on `error`, with the original lane on
//!    `hop.kind` — the treatment `pack_ack` already documents: a hive that has
//!    no lane for a receipt does not grow one, the receipt is evidence for
//!    whoever operates the colony.
//!
//! # What is booted
//!
//! The SHIPPED `member` and `assistant` templates, cell for cell, with every
//! `ref` marker replaced by an answering `code` double — the arrangement of
//! `gh459_a_screen_is_a_member_channel.rs`, from which the doubles are taken.
//! The screen's down-edge in this tree deliberately keeps the OLD `answer ||
//! view` clause: the point of the measurement is that the loop is closed even
//! when a prose answer does reach a screen, whatever drew that edge.
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

// ══════════════════════════════ 1. no shipped recipe binds a screen on `answer`

/// **The screen takes `view` and nothing else.** A screen is grown in exactly
/// two places — the builder's fast lane (`templates/builder/recipes`) and the
/// declaration it renders (`examples/organism/grow-screen.json`) — and both are
/// read here as text, because what is wrong is one clause and a clause is a
/// string.
#[test]
fn the_shipped_recipes_bind_a_screen_on_view_alone() {
    for rel in [
        "examples/organism/grow-screen.json",
        "templates/builder/recipes/config.json",
    ] {
        let p = repo(rel);
        if !p.is_file() {
            eprintln!("{rel} did not travel into this tree -- skipped (GH #49)");
            continue;
        }
        let raw = std::fs::read_to_string(&p).expect("readable");
        assert!(
            !raw.contains("hop.route == 'answer' || hop.route == 'view'"),
            "{rel} still binds the screen on an agent's prose answer: an answer is not a \
             view, the display refuses it as `invalid_view`, and the refusal was the first \
             half of the loop in GH #598"
        );
        assert!(
            raw.contains("hop.route == 'in_view'") || raw.contains("'in_view'"),
            "{rel} has to keep re-stamping what it does carry onto the display's own lane"
        );
    }
}

// ══════════════════════════════════════════════════════════════ the doubles

/// A cell that answers nothing: the holders this round never reaches.
const INERT: &str = r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps([]))
"#;

/// The member's screen, doubled: it passes every turn and touches no context.
const FIREWALL: &str = r#"
import sys, json
doc = json.load(sys.stdin)
sys.stdout.write(json.dumps({
    "header": {"route": "pass"},
    "messages": doc["body"].get("messages", [])}))
"#;

/// The SCREEN — `display@2.0.0`, doubled at the one judgement this file needs:
/// is what arrived on `in_view` a VIEW?
///
/// The shipped `compose` reads `view_id`, `kind` and `content` out of the body
/// and refuses a document that carries none of them with `invalid_view`, naming
/// the sender it refused on `hop.owner` (from `envelope.reply_to`, never from
/// the body). That refusal is what the double writes, byte for byte in the keys
/// the member routes on.
const SCREEN: &str = r#"
import sys, json
doc = json.load(sys.stdin)
body = doc["body"]
owner = str(doc["envelope"].get("reply_to") or "")

if body.get("view_id") and body.get("kind") and body.get("content"):
    # A real view: in production the round ends here, and this file never
    # asserts on it.
    sys.stdout.write(json.dumps([]))
else:
    sys.stdout.write(json.dumps({
        "header": {"route": "receipt", "owner": owner, "view_id": ""},
        "messages": [],
        "receipt": {"error_code": "invalid_view", "owner": owner,
                    "detail": "no view_id, kind or content in the body"}}))
"#;

/// The conversation surface of one generation, doubled — and the BRAIN COUNTER
/// of this file.
///
/// Every wake of this cell is one brain call. A wake carrying `hop.kind` is a
/// screen event or receipt the member re-stamped onto `in_turn`, and the double
/// reports it on `error` so a loop announces itself POSITIVELY at the sink
/// instead of being read out of a silence. Anything else is an ordinary turn,
/// and what it answers is prose: `messages[]` and no view fields at all, which
/// is exactly what a display refuses.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
kind = str(hop.get("kind") or "")

if kind:
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "brain_woken",
                   "saw_kind": kind,
                   "saw_owner": str(hop.get("owner") or "")},
        "messages": []}))
else:
    sys.stdout.write(json.dumps({
        "header": {"route": "answer"},
        "messages": [{"origin": "assistant", "type": "text",
                      "text": "a paragraph, which is not a view"}]}))
"#;

/// Puts one message on a named lane. `hop.mode` picks the door.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
mode = str(hop.get("mode") or "turn")
out = {"header": {"route": {"turn": "in_turn", "screen": "in_wire"}[mode],
                  "lane": str(hop.get("lane") or "receipt"),
                  "owner_hint": str(hop.get("owner_hint") or "")},
       "messages": doc["body"].get("messages", [])}
sys.stdout.write(json.dumps(out))
"#;

/// A screen that speaks on demand: the test's own trigger for the lane a display
/// PRODUCES, so a receipt can be injected without an answer in front of it.
const WIRE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
owner = str(hop.get("owner_hint") or "")
sys.stdout.write(json.dumps({
    "header": {"route": str(hop.get("lane") or "receipt"), "owner": owner,
               "view_id": "note"},
    "messages": [],
    "receipt": {"error_code": "invalid_view", "owner": owner, "detail": "refused"}}))
"#;

/// A `code` double with a fixed script. `emits` is left wide on purpose: what a
/// double may say is decided by the assertions, not by a contract nobody reads.
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

// ══════════════════════════════════════════ the wiring a mutation draws

const SCREEN_NAME: &str = "display-main";
const WIRE_NAME: &str = "wire";
const AGENT: &str = "egon";
const MEMBER: &str = "/person";

/// The screen's edges — and the down-edge is DELIBERATELY the old one.
///
/// `answer || view` is the clause GH #598 removed from every shipped recipe. It
/// stands here because this file measures the SECOND half: what the level does
/// with the refusal, whatever put the answer in front of it. A tree that could
/// not get an answer onto a screen could not measure the loop at all.
fn screen_edges() -> Vec<Value> {
    vec![
        json!({
            "from": format!("./channels/{SCREEN_NAME}"), "to": "./channels",
            "condition": "has(hop.route) && (hop.route == 'event' || hop.route == 'receipt')",
            "modifier": {"set_context": {"channel_node": format!("'{SCREEN_NAME}'"),
                                         "channel": format!("'{SCREEN_NAME}'")}}
        }),
        json!({
            "from": "./channels", "to": format!("./channels/{SCREEN_NAME}"),
            "condition": format!(
                "has(hop.route) && (hop.route == 'answer' || hop.route == 'view') && \
                 has(context.channel_node) && context.channel_node == '{SCREEN_NAME}'"),
            "modifier": {"set_hop": {"route": "'in_view'"}}
        }),
        // The test's own trigger, on a lane no shipped cell reaches.
        json!({
            "from": format!("./channels/{WIRE_NAME}"), "to": "./channels",
            "condition": "has(hop.route) && (hop.route == 'event' || hop.route == 'receipt')",
            "modifier": {"set_context": {"channel_node": format!("'{SCREEN_NAME}'"),
                                         "channel": format!("'{SCREEN_NAME}'")}}
        }),
    ]
}

/// The edges one assistant costs, cut to what this file drives.
fn assistant_edges(name: &str) -> Vec<Value> {
    vec![
        json!({
            "from": "./assistants", "to": format!("./assistants/{name}"),
            "condition": format!(
                "has(hop.route) && hop.route == 'in_turn' && \
                 ((has(context.assistant) && context.assistant == '{name}') || \
                  (has(hop.owner) && hop.owner.contains('/assistants/{name}/')))")
        }),
        json!({
            "from": format!("./assistants/{name}"), "to": "./assistants",
            "condition": "has(hop.route) && (hop.route == 'answer' || hop.route == 'error')"
        }),
    ]
}

/// The colony around the member: one driver, and a drain for every lane the
/// member emits. Draining all ten is the point — an undrained lane is a dead
/// letter, and this test would then be reading a silence.
fn main_config() -> Value {
    let mut edges = vec![
        json!({
            "from": "./driver", "to": "./person",
            "condition": "has(hop.route) && hop.route == 'in_turn'",
            "modifier": {"set_context": {
                "channel_node": format!("'{SCREEN_NAME}'"),
                "channel": format!("'{SCREEN_NAME}'"),
                "assistant": format!("'{AGENT}'")
            }}
        }),
        json!({
            "from": "./driver", "to": format!("./person/channels/{WIRE_NAME}"),
            "condition": "has(hop.route) && hop.route == 'in_wire'"
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

fn build_tree(td: &tempfile::TempDir, member: &std::path::Path, assistant: &std::path::Path) {
    let root = td.path();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/driver/config.json",
        &double(DRIVER, "Test driver: puts one message on a named lane."),
    );

    copy_cells(member, &root.join("main/person"));
    for holder in ["access", "affinity", "memory-hive"] {
        write(
            root,
            &format!("main/person/{holder}/config.json"),
            &double(INERT, "Inert double for a holder this round never reaches."),
        );
    }
    write(
        root,
        "main/person/firewall/config.json",
        &double(FIREWALL, "Test double for the member's screen."),
    );
    write(
        root,
        &format!("main/person/channels/{SCREEN_NAME}/config.json"),
        &double(
            SCREEN,
            "Test double for display@2.0.0, one screen of this person.",
        ),
    );
    write(
        root,
        &format!("main/person/channels/{WIRE_NAME}/config.json"),
        &double(WIRE, "Test-only trigger for the lanes a screen produces."),
    );

    let dst = root.join(format!("main/person/assistants/{AGENT}"));
    copy_cells(assistant, &dst);
    write(
        root,
        &format!("main/person/assistants/{AGENT}/talky/config.json"),
        &double(
            SURFACE,
            "Test double for the conversation surface, and the brain counter.",
        ),
    );
    for sibling in ["cogny", "tools"] {
        write(
            root,
            &format!("main/person/assistants/{AGENT}/{sibling}/config.json"),
            &double(
                INERT,
                "Inert double for a sibling this round never reaches.",
            ),
        );
    }

    let cfg_path = root.join("main/person/config.json");
    let mut cfg = read_json(&cfg_path);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the member ships a graph");
    edges.extend(screen_edges());
    edges.extend(assistant_edges(AGENT));
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
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
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

fn inject(mode: &str, lane: &str, owner_hint: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("mode".into(), json!(mode));
    hop.insert("lane".into(), json!(lane));
    hop.insert("owner_hint".into(), json!(owner_hint));
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "hello"}
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

/// The one SEMANTIC timing discriminator in this file: after the receipt has
/// left the level, nothing else may follow. A loop produced its next message in
/// well under a second on the live colonies that paid for this issue (44 brain
/// calls in six minutes is one every eight seconds, and each of them is a full
/// round trip); three seconds of quiet is an order of magnitude over what a
/// looping tree needs to speak again, and no test in this file waits on it in
/// the green case except once.
async fn nothing_follows(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .ok()
        .flatten()
}

fn skip() -> bool {
    if shipped().is_none() {
        eprintln!("member/assistant did not travel into this tree -- skipped (GH #49)");
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

// ═══════════════════════════════════════════════════════ the measurements

/// **A refusal from the screen wakes no brain.** The receipt is injected on the
/// lane a display produces, owned by the surface of this member's generation —
/// the exact message that used to come back re-stamped as `in_turn`.
///
/// It leaves the level on `error` with the original lane on `hop.kind`, ONCE,
/// and the surface — whose every wake reports itself as `brain_woken` — never
/// speaks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_screen_receipt_leaves_the_level_instead_of_waking_the_brain() {
    if skip() {
        return;
    }
    let Some((member, assistant)) = shipped() else {
        return;
    };
    let owner = format!("{MEMBER}/assistants/{AGENT}/talky");
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member, &assistant);
    let (h, mut rx) = boot(&td).await;
    h.send(inject("screen", "receipt", &owner)).await;

    let got = recv_bounded(&mut rx)
        .await
        .expect("the receipt has to leave the level -- an undrained refusal is a dead letter");
    assert_ne!(
        hop_of(&got, "error_code"),
        "brain_woken",
        "a receipt is feedback to a WRITER, not a turn of a person: waking the generation \
         with it is the loop GH #598 measured at 44 brain calls in six minutes: {got:#?}"
    );
    assert_eq!(hop_of(&got, "route"), "error", "{got:#?}");
    assert_eq!(
        hop_of(&got, "kind"),
        "receipt",
        "the lane the message was on has to survive the re-stamp, or the error says nothing \
         about what failed"
    );
    assert_eq!(hop_of(&got, "owner"), owner);

    let after = nothing_follows(&mut rx).await;
    assert!(
        after.is_none(),
        "one refusal, one message out of the level, and then quiet: {after:#?}"
    );
    h.shutdown().await;
}

/// **The whole loop, driven end to end.** A person says something, the
/// generation answers in prose, the answer reaches a screen wired the old way,
/// the screen refuses it — and that is the END of it: exactly ONE message
/// leaves the level after the refusal, and it is the refusal itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prose_answer_on_a_screen_is_refused_exactly_once() {
    if skip() {
        return;
    }
    let Some((member, assistant)) = shipped() else {
        return;
    };
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member, &assistant);
    let (h, mut rx) = boot(&td).await;
    h.send(inject("turn", "", "")).await;

    let got = recv_bounded(&mut rx)
        .await
        .expect("the refusal has to leave the level");
    assert_eq!(
        hop_of(&got, "route"),
        "error",
        "the refused write leaves on the level's own error lane: {got:#?}"
    );
    assert_eq!(hop_of(&got, "kind"), "receipt", "{got:#?}");
    assert_eq!(
        hop_of(&got, "owner"),
        format!("{MEMBER}/assistants/{AGENT}/talky"),
        "the display names the sender it refused, and the level hands the evidence out under \
         that name"
    );

    let after = nothing_follows(&mut rx).await;
    assert!(
        after.is_none(),
        "the second message is the loop: prose -> refusal -> turn -> prose. There must be \
         no second message: {after:#?}"
    );
    h.shutdown().await;
}
