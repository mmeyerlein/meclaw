//! GH #620 (b) — **what a second call gets.**
//!
//! `templates/freeswitch/README.md` said the gap out loud until 1.1.0: *A
//! second call at a time. `hangup()` ends the newest live call, and the table
//! holds every call there is; nothing stops two, and nothing arbitrates between
//! them either.* This file is the arbitration.
//!
//! `params.second_call` decides — `busy` refuses with a cause the caller hears
//! as a busy signal, `queue` takes the leg and holds it off the recogniser
//! until the call in front of it ends, `parallel` gives it a session of its own
//! up to `params.capacity`. Every outcome leaves a row in `calls` AND a receipt
//! on its own lane, and `hangup` stops guessing which line the model meant.
//!
//! # What is booted
//!
//! The SHIPPED `member`, `assistant` and `freeswitch`, cell for cell, with
//! `code` doubles in place of every `ref` and of the holders a round never
//! reaches — the arrangement of
//! `freeswitch_channel_places_a_call_and_hears_the_line.rs`, whose harness this
//! file repeats rather than shares: the two files are read separately, and a
//! harness that travelled between them would make each of them a puzzle.
//!
//! Two things this file's copy adds. The install manifest carries the four
//! receipt lanes out — up to `./channels`, then out of the member as `error`
//! with the lane on `hop.kind`, which is ADR-0025's own treatment of a receipt
//! no occupant owns and needs no change to `member`. And the driver can name a
//! call, so the last measurement can send `hangup {call_id}`.
//!
//! The switch is a mock HTTP server standing in for `mod_xml_rpc`. No FS02, no
//! real call, no provider.
//!
//! Guarded like every other template-reading test (GH #49): a template that did
//! not travel into this tree is skipped rather than judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::web_fetch::WebFetchCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};

/// A path inside this repository, from the crate's manifest directory.
fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> Option<(std::path::PathBuf, std::path::PathBuf, std::path::PathBuf)> {
    let member = repo("templates/member");
    let assistant = repo("templates/assistant");
    let fs = repo("templates/freeswitch");
    (member.join("config.json").is_file()
        && assistant.join("config.json").is_file()
        && fs.join("config.json").is_file())
    .then_some((member, assistant, fs))
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

// ═══════════════════════════════════════════════════════════════ the names

/// The generation whose surface calls the tools.
const AGENT: &str = "egon";
/// The channel node. An instance is named after its template — the MACHINE.
const CHANNEL: &str = "freeswitch";
/// The kind of conversation, which is not the machine: it stays `phone`
/// whichever switch is behind it, because the holders count rooms and not
/// vendors (`templates/member/README.md` § *The two channel keys*).
const CHANNEL_KIND: &str = "phone";
/// The number in this member's `callers` table, and the sender it maps to.
const KNOWN: &str = "+4930111111";
const KNOWN_USER: &str = "the-person";
/// The four arbitration receipt lanes, as one CEL guard. One edge and not four:
/// a manifest that forgot one of them would leak that receipt into the
/// dead-letter queue, and one condition cannot half-land.
const RECEIPT_LANES: &str = "has(hop.route) && (hop.route == 'call_accepted' || \
                             hop.route == 'call_queued' || hop.route == 'call_refused' || \
                             hop.route == 'call_abandoned')";

// ══════════════════════════════════════════════════════════════ the doubles

/// A cell that answers nothing: the holders a round never reaches.
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

/// The MEDIA half, doubled. A real `voice` cell binds a socket and wants two
/// providers; nothing here is about audio, so this one only proves the wiring
/// exists — it reports what it was asked to speak, on the channel's own error
/// lane, which the member carries out of the level.
const MEDIA: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
route = str(hop.get("route") or "")
if route == "in_speak":
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "media_got_in_speak",
                   "spoke_session": str(ctx.get("call_id") or "")},
        "messages": []}))
elif route == "end_speak":
    # The `speak_end` lane of a real `voice` cell, on demand: the test decides
    # WHEN a sentence is over, because that is the whole thing under test.
    sys.stdout.write(json.dumps({
        "header": {"route": "speak_end",
                   "session_id": str(hop.get("session_id") or ""),
                   "speak_id": "sp-1",
                   "reason": str(hop.get("reason") or "done"),
                   "platform": "voice"},
        "messages": []}))
else:
    sys.stdout.write(json.dumps([]))
"#;

/// The conversation surface of one generation, doubled — the caller of the two
/// tools and the reader of the menu, and the ear that reports every turn.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
body = doc["body"]
route = str(hop.get("route") or "")


def last_text():
    for m in reversed(body.get("messages") or []):
        if isinstance(m, dict) and isinstance(m.get("text"), str):
            return m["text"]
    return ""


if route == "in_wire":
    mode = str(hop.get("mode") or "")
    if mode == "menu":
        sys.stdout.write(json.dumps({"header": {"route": "schemas"},
                                     "messages": [], "tools": ["call"]}))
    elif mode == "hangup":
        cid = str(hop.get("call_id") or "")
        args = json.dumps({"call_id": cid}) if cid else "{}"
        sys.stdout.write(json.dumps({
            "header": {"route": "tool", "tool_name": "hangup", "tool_call_id": "c2"},
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "c2",
                          "text": args}]}))
    else:
        args = json.dumps({"number": str(hop.get("number") or ""),
                           "purpose": "ask how she slept"})
        sys.stdout.write(json.dumps({
            "header": {"route": "tool", "tool_name": "call", "tool_call_id": "c1"},
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                          "text": args}]}))
elif route == "in_tool":
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "surface_got_in_tool",
                   "got_call_id": str(hop.get("tool_call_id") or ""),
                   "got_answerer": str(ctx.get("tool_answerer") or ""),
                   "got_session": str(ctx.get("session_id") or ""),
                   "got_text": last_text()},
        "messages": []}))
elif route == "in_menu":
    names = ",".join(str(s.get("name") or "") for s in (body.get("schemas") or []))
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "surface_got_in_menu",
                   "got_answerer": str(ctx.get("tool_answerer") or ""),
                   "got_names": names},
        "messages": []}))
elif route == "in_turn":
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "surface_got_in_turn",
                   "got_state": str(ctx.get("call_state") or ""),
                   "got_session": str(ctx.get("session_id") or ""),
                   "got_call": str(ctx.get("call_id") or ""),
                   "got_user": str(ctx.get("user_id") or ""),
                   "got_channel": str(ctx.get("channel") or ""),
                   "got_text": last_text()},
        "messages": []}))
else:
    sys.stdout.write(json.dumps([]))
"#;

/// Puts one message on a named lane. `hop.mode` picks the door; `hop.ev` is a
/// dialplan event, forwarded with the three keys a dialplan sends.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
mode = str(hop.get("mode") or "")
if mode == "ev":
    sys.stdout.write(json.dumps({
        "header": {"route": str(hop.get("ev") or ""),
                   "call_uuid": str(hop.get("call_uuid") or ""),
                   "number": str(hop.get("number") or ""),
                   "cause": str(hop.get("cause") or "")},
        "messages": []}))
else:
    sys.stdout.write(json.dumps({
        "header": {"route": "in_wire", "mode": mode,
                   "number": str(hop.get("number") or ""),
                   "call_id": str(hop.get("call_id") or "")},
        "messages": doc["body"].get("messages", [])}))
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
            "multi_send_capable": true,
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

// ══════════════════════════════════════ the wiring an installing mutation draws

/// **The install manifest, verbatim** — `templates/freeswitch/README.md`
/// § *Wiring it into a member*.
///
/// The first two edges are the channel half and are the `voice` channel's own
/// two, word for word. The last three are the offer half: the app rim's
/// mechanism at a second rim, which `member` declares with
/// `at: ["./apps", "./channels"]`.
fn install_edges(agent: &str, channel: &str) -> Vec<Value> {
    vec![
        json!({
            "from": format!("./channels/{channel}"), "to": "./channels",
            "condition": "has(hop.route) && (hop.route == 'turn' || hop.route == 'partial' || hop.route == 'error')",
            "modifier": {"set_context": {
                "channel_node": format!("'{channel}'"),
                "channel": format!("'{CHANNEL_KIND}'"),
                "assistant": format!("'{agent}'"),
                "audience_set": format!("'[\"agent:{agent}\",\"member:person\"]'"),
                "user_id": format!("has(hop.user_id) && hop.user_id != '' ? hop.user_id : '{KNOWN_USER}'"),
                "call_state": "has(hop.call_state) ? hop.call_state : ''",
                "call_id": "has(hop.call_id) ? hop.call_id : ''",
                "session_id": "has(hop.session_id) ? hop.session_id : ''"}}
        }),
        json!({
            "from": "./channels", "to": format!("./channels/{channel}"),
            "condition": format!(
                "has(hop.route) && hop.route == 'answer' && has(context.channel_node) && \
                 context.channel_node == '{channel}'"),
            "modifier": {"set_hop": {"route": "'in_speak'"}}
        }),
        // The four arbitration receipts (GH #620). They leave the hive on lanes
        // of their own, cross the container, and leave the member on `error`
        // with the lane on `hop.kind` — ADR-0025's own treatment of a receipt no
        // occupant of this level owns.
        json!({
            "from": format!("./channels/{channel}"), "to": "./channels",
            "condition": RECEIPT_LANES,
            "modifier": {"set_context": {"channel_node": format!("'{channel}'"),
                                         "channel": format!("'{CHANNEL_KIND}'")}}
        }),
        json!({
            "from": "./channels", "to": ".",
            "condition": RECEIPT_LANES,
            "modifier": {"set_hop": {"route": "'error'", "kind": "hop.route"}}
        }),
        json!({
            "from": format!("./channels/{channel}"), "to": "./channels",
            "condition": "has(hop.route) && (hop.route == 'tool_result' || hop.route == 'tool_schemas')",
            "modifier": {"set_context": {
                "tool_answerer": format!("'{channel}'"),
                "call_id": "has(hop.call_id) ? hop.call_id : (has(context.call_id) ? context.call_id : '')",
                "session_id": "has(hop.session_id) ? hop.session_id : (has(context.session_id) ? context.session_id : '')"}}
        }),
        json!({
            "from": format!("./assistants/{agent}/talky"),
            "to": format!("./channels/{channel}/dial"), "lane": "tool",
            "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && (hop.tool_name == 'call' || hop.tool_name == 'hangup')",
            "modifier": {"set_context": {"tool_caller": "'talky'", "assistant": format!("'{agent}'")},
                         "delete_context": ["col_phase", "consult_class", "consult_id", "tool_answerer"]}
        }),
        json!({
            "from": format!("./assistants/{agent}/talky"),
            "to": format!("./channels/{channel}/dial"), "lane": "schemas",
            "condition": "has(hop.route) && hop.route == 'schemas'",
            "modifier": {"set_context": {"tool_caller": "'talky'", "assistant": format!("'{agent}'")},
                         "delete_context": ["col_phase", "consult_class", "consult_id", "tool_answerer"]}
        }),
    ]
}

/// The edges one assistant costs — the growth recipe's, cut to what this file
/// drives.
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
            "from": "./assistants", "to": format!("./assistants/{name}"),
            "condition": format!(
                "has(hop.route) && hop.route == 'in_tool' && \
                 (!has(context.assistant) || context.assistant == '{name}')")
        }),
        json!({
            "from": "./assistants", "to": format!("./assistants/{name}"),
            "condition": format!(
                "has(hop.route) && hop.route == 'in_menu' && \
                 (!has(context.assistant) || context.assistant == '{name}')")
        }),
        json!({
            "from": format!("./assistants/{name}"), "to": "./assistants",
            "condition": "has(hop.route) && (hop.route == 'answer' || hop.route == 'error')"
        }),
    ]
}

/// The colony around the member: one driver, and a drain for every lane the
/// member emits at its own rim.
fn main_config() -> Value {
    let mut edges = vec![
        json!({
            "from": "./driver", "to": format!("./person/assistants/{AGENT}/talky"),
            "condition": "has(hop.route) && hop.route == 'in_wire'"
        }),
        json!({
            "from": "./driver", "to": format!("./person/channels/{CHANNEL}"),
            "condition": "has(hop.route) && (hop.route == 'call_incoming' || hop.route == 'call_ringing' || hop.route == 'call_answered' || hop.route == 'call_ended')"
        }),
    ];
    for lane in [
        "answer",
        "bundle",
        "ack",
        "reject",
        "error",
        "write",
        "turn_write",
        "prune",
        "build",
        "close_report",
        "export_done",
        "dump",
        "pack_ack",
    ] {
        edges.push(json!({"from": "./person", "to": "/sink",
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}})
}

fn build_tree(
    td: &tempfile::TempDir,
    member: &std::path::Path,
    assistant: &std::path::Path,
    fs_template: &std::path::Path,
    switch_base: &str,
) {
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

    // The SHIPPED freeswitch, cell for cell — only the media half is doubled.
    let chan = root.join(format!("main/person/channels/{CHANNEL}"));
    copy_cells(fs_template, &chan);
    write(
        root,
        &format!("main/person/channels/{CHANNEL}/voice/config.json"),
        &double(MEDIA, "Test double for the media half of this channel."),
    );
    // The one thing an instance always says about itself: who may ring it.
    let signal_path = chan.join("signal/config.json");
    let mut signal = read_json(&signal_path);
    signal["params"]["callers"] = json!({KNOWN: KNOWN_USER});
    signal["params"]["voice_ws_url"] = json!("ws://127.0.0.1:7900/");
    std::fs::write(
        &signal_path,
        meclaw_core::serde_json::to_string_pretty(&signal).expect("serialise"),
    )
    .expect("write the signal config");

    let dst = root.join(format!("main/person/assistants/{AGENT}"));
    copy_cells(assistant, &dst);
    write(
        root,
        &format!("main/person/assistants/{AGENT}/talky/config.json"),
        &double(
            SURFACE,
            "Test double for the conversation surface of one generation.",
        ),
    );
    for inner in ["tools", "cogny"] {
        write(
            root,
            &format!("main/person/assistants/{AGENT}/{inner}/config.json"),
            &double(INERT, "Inert double for a unit no round here reaches."),
        );
    }

    let cfg_path = root.join("main/person/config.json");
    let mut cfg = read_json(&cfg_path);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the member ships a graph");
    edges.extend(assistant_edges(AGENT));
    edges.extend(install_edges(AGENT, CHANNEL));
    std::fs::write(
        &cfg_path,
        meclaw_core::serde_json::to_string_pretty(&cfg).expect("serialise"),
    )
    .expect("write the member config");

    // The provider lane of the template, resolved to the mock switch.
    std::fs::write(
        root.join(".env"),
        format!("FREESWITCH_XMLRPC_BASE_URL={switch_base}\n"),
    )
    .expect("write the .env");
}

// ═════════════════════════════════════════════════════════════════ the colony

struct Colony {
    #[allow(dead_code)]
    td: tempfile::TempDir,
    h: ColonyHandle,
    rx: mpsc::Receiver<Message>,
    switch: Arc<Mutex<Vec<CapturedRequest>>>,
}

async fn start(answers: Vec<MockResponse>) -> Colony {
    start_tuned(answers, |_| {}).await
}

/// The same colony, with one hand on the signalling half's own `params` before
/// the boot — the `answer_app` case wants a template configured the way an
/// instance configures it, not a second template.
async fn start_tuned(answers: Vec<MockResponse>, tune: impl Fn(&mut Value)) -> Colony {
    let Some((member, assistant, fs_template)) = shipped() else {
        panic!("guarded by the caller");
    };
    let (addr, _join, captured) = start_mock_server_capturing(answers).await;
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(
        &td,
        &member,
        &assistant,
        &fs_template,
        &format!("http://127.0.0.1:{}", addr.port()),
    );
    let signal_path = td
        .path()
        .join(format!("main/person/channels/{CHANNEL}/signal/config.json"));
    let mut signal = read_json(&signal_path);
    tune(&mut signal["params"]);
    std::fs::write(
        &signal_path,
        meclaw_core::serde_json::to_string_pretty(&signal).expect("serialise"),
    )
    .expect("write the tuned signal config");

    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            (
                "store".to_string(),
                Arc::new(StoreCellFactory) as Arc<dyn CellFactory>,
            ),
            (
                "web_fetch".to_string(),
                Arc::new(WebFetchCellFactory) as Arc<dyn CellFactory>,
            ),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(256);
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
        .expect("the shipped member, assistant and freeswitch must boot");
    Colony {
        td,
        h,
        rx: sink_rx,
        switch: captured,
    }
}

fn ok(body: &str) -> MockResponse {
    MockResponse::ok(body.as_bytes())
}

fn inject(hop: Value) -> Message {
    let map = hop.as_object().expect("an object").clone();
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "a person said something"}
        ]})))
        .hop(map)
        .ttl(200)
        .build()
}

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Collect what the round put on the sink. `quiet` is the SEMANTIC window,
/// `budget` the failure marker (30-second convention).
async fn gather(
    rx: &mut mpsc::Receiver<Message>,
    quiet: Duration,
    budget: Duration,
) -> Vec<Message> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut out = Vec::new();
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let wait = if out.is_empty() {
            deadline - now
        } else {
            quiet.min(deadline - now)
        };
        match tokio::time::timeout(wait, rx.recv()).await {
            Ok(Some(m)) => out.push(m),
            Ok(None) | Err(_) => break,
        }
    }
    out
}

async fn round(c: &mut Colony, hop: Value) -> Vec<Message> {
    c.h.send(inject(hop)).await;
    gather(&mut c.rx, Duration::from_secs(3), Duration::from_secs(30)).await
}

/// Every arbitration receipt in a batch, as `(lane, call_id, cause)`.
///
/// A receipt reaches the sink as an `error` carrying the lane it was raised on
/// in `hop.kind`, which is what the install manifest's last edge does and what
/// ADR-0025 already does for a receipt no app owns.
fn receipts(got: &[Message]) -> Vec<(String, String, String, String)> {
    got.iter()
        .filter(|m| hop_of(m, "kind").starts_with("call_"))
        .map(|m| {
            (
                hop_of(m, "kind"),
                hop_of(m, "call_id"),
                hop_of(m, "cause"),
                // The EFFECTIVE policy, which is what a receipt is for: a knob
                // with a typo in it falls back, and the receipt is where that
                // shows up rather than in a log nobody reads.
                hop_of(m, "policy"),
            )
        })
        .collect()
}

/// One receipt, as the tuple `receipts` yields.
fn receipt(kind: &str, call: &str, cause: &str, policy: &str) -> (String, String, String, String) {
    (
        kind.to_string(),
        call.to_string(),
        cause.to_string(),
        policy.to_string(),
    )
}

/// What the BOOK says is on the line, read through the one cell that may read
/// it: `hangup` with no argument names the running call, or refuses.
///
/// The store itself is not readable from here. A `select` addressed at
/// `<hive>/calls` reaches the cell, but its answer is addressed at a path
/// OUTSIDE the hive and never comes back — the boundary is what a hive IS, and
/// a test that got around it would be measuring a topology no colony has. So
/// the row is pinned the way a colony reads it: through the channel.
async fn the_line_says(c: &mut Colony) -> String {
    let got = round(c, json!({"mode": "hangup"})).await;
    hop_of(&only(&got, "surface_got_in_tool"), "got_text")
}

/// The index of the first switch command containing `needle`, or a panic naming
/// what did arrive. Used where the ORDER is the claim: a `contains` sweep would
/// pass on a resume that reached the switch before the pause.
fn at(calls: &[String], needle: &str) -> usize {
    calls
        .iter()
        .position(|p| p.contains(needle))
        .unwrap_or_else(|| panic!("no switch command carries `{needle}`: {calls:?}"))
}

/// The switch commands so far, percent-decoded — the form the assertions read.
async fn switch_calls(c: &Colony) -> Vec<String> {
    switch_paths(c)
        .await
        .iter()
        .map(|p| percent_decode(p))
        .collect()
}

/// One inbound call from the allowlisted number.
async fn rings(c: &mut Colony, uuid: &str) -> Vec<Message> {
    round(
        c,
        json!({"mode": "ev", "ev": "call_incoming", "call_uuid": uuid, "number": KNOWN}),
    )
    .await
}

/// Every `surface_got_in_turn` in a batch, as the text the surface reported.
fn turns(got: &[Message]) -> Vec<String> {
    got.iter()
        .filter(|m| hop_of(m, "error_code") == "surface_got_in_turn")
        .map(|m| hop_of(m, "got_text"))
        .collect()
}

/// One finished assistant turn, entering the channels container the way the
/// member's own answer edge delivers it — with the session keeper's id on
/// `context.session_id` and the CALL's id on `context.call_id` — the pair GH
/// #603 § 3 found and GH #620 settled: two owners, two names.
async fn answer_into_the_channel(c: &mut Colony, call: &str, keeper: &str) -> Vec<Message> {
    let mut context = meclaw_core::serde_json::Map::new();
    context.insert("channel_node".into(), json!(CHANNEL));
    context.insert("channel".into(), json!(CHANNEL_KIND));
    context.insert("session_id".into(), json!(keeper));
    context.insert("call_id".into(), json!(call));
    c.h.send(
        MessageBuilder::new(Path::new("/person/channels"))
            .context(context)
            .hop(
                json!({"route": "answer"})
                    .as_object()
                    .expect("an object")
                    .clone(),
            )
            .body(Body::Inline(json!({"messages": [
                {"origin": "assistant", "type": "text", "text": "I am still speaking."}
            ]})))
            .ttl(200)
            .build(),
    )
    .await;
    gather(&mut c.rx, Duration::from_secs(3), Duration::from_secs(30)).await
}

/// The one message the surface reported on `code`, or a panic naming what did
/// arrive — a silence is never read as a pass.
fn only(got: &[Message], code: &str) -> Message {
    let hits: Vec<&Message> = got
        .iter()
        .filter(|m| hop_of(m, "error_code") == code)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one `{code}`, got {:#?}",
        got.iter()
            .map(|m| (hop_of(m, "error_code"), hop_of(m, "got_text")))
            .collect::<Vec<_>>()
    );
    hits[0].clone()
}

async fn switch_paths(c: &Colony) -> Vec<String> {
    c.switch
        .lock()
        .await
        .iter()
        .map(|r| r.path.clone())
        .collect()
}

fn skip() -> bool {
    if shipped().is_none() {
        eprintln!("member/assistant/freeswitch did not travel into this tree -- skipped (GH #49)");
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

// ═══════════════════════════════════════════════════════════ the measurements

/// **`busy` refuses the second call, and says so on both sides.**
///
/// One turn for the call that got the line, one `USER_BUSY` at the switch for
/// the one that did not, and one receipt each. The refusal carries a CAUSE
/// rather than a bare kill: a caller must hear a busy signal, not a line that
/// died for no reason they can name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_refuses_the_second_call_with_a_receipt() {
    if skip() {
        return;
    }
    let mut c = start(vec![ok("+OK\n"); 4]).await;

    let got = rings(&mut c, "call-a").await;
    assert_eq!(turns(&got).len(), 1, "the first call is a turn: {got:#?}");
    assert_eq!(
        receipts(&got),
        vec![receipt("call_accepted", "call-a", "", "busy")],
        "a call that was taken leaves a receipt saying so, under the policy that \
         took it: {got:#?}"
    );

    let got = rings(&mut c, "call-b").await;
    assert_eq!(
        turns(&got),
        Vec::<String>::new(),
        "a refused call is nobody's turn: {:#?}",
        turns(&got)
    );
    assert_eq!(
        receipts(&got),
        vec![receipt("call_refused", "call-b", "busy", "busy")],
        "and it says why: {got:#?}"
    );
    let calls = switch_calls(&c).await;
    assert_eq!(
        calls,
        vec!["/webapi/uuid_kill?call-b USER_BUSY".to_string()],
        "the refused leg is ended with a cause, and nothing else reaches the switch: {calls:?}"
    );

    // The BOOK, not the receipt. A refusal that was emitted but never written
    // down would pass every assertion above and leave a colony believing two
    // calls were running: exactly one is, and `hangup` is what says so.
    let line = the_line_says(&mut c).await;
    assert!(
        line.contains("hung up") && line.contains(KNOWN),
        "one call is on the line, and it is the one that was accepted: {line}"
    );
    let line = the_line_says(&mut c).await;
    assert!(
        line.contains("no call is running"),
        "and the refused call was never booked as live: {line}"
    );
}

/// **`queue` holds the second call and runs it when the first ends.**
///
/// A waiting caller is taken OFF the recogniser first (R-G6): the dialplan
/// answered the leg and started its audio fork, and words spoken into a queue
/// must not become turns of a conversation that has not started.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_holds_the_second_call_and_runs_it_when_the_first_ends() {
    if skip() {
        return;
    }
    let mut c = start_tuned(vec![ok("+OK\n"); 12], |p| {
        p["second_call"] = json!("queue");
    })
    .await;

    let _ = rings(&mut c, "call-a").await;
    let got = rings(&mut c, "call-b").await;
    assert_eq!(
        turns(&got),
        Vec::<String>::new(),
        "a waiting caller is in no conversation yet: {:#?}",
        turns(&got)
    );
    assert_eq!(
        receipts(&got),
        vec![receipt("call_queued", "call-b", "", "queue")]
    );
    let mut calls = switch_calls(&c).await;
    calls.sort();
    assert_eq!(
        calls,
        vec![
            "/webapi/uuid_audio_stream?call-b pause".to_string(),
            "/webapi/uuid_broadcast?call-b local_stream://moh aleg".to_string(),
        ],
        "PAUSE, not stop — `stop` closes the websocket and would end the media \
         half's session, so the promotion would have to compose a fresh `start` \
         out of this hive's own knobs. Exactly these two, and nothing else, \
         reaches the switch: {calls:?}"
    );

    // The first call ends, and the queue empties.
    let got = round(
        &mut c,
        json!({"mode": "ev", "ev": "call_ended", "call_uuid": "call-a",
               "number": KNOWN, "cause": "NORMAL_CLEARING"}),
    )
    .await;
    assert_eq!(
        turns(&got).len(),
        1,
        "the promoted caller is announced NOW, which is when somebody is \
         actually on the line: {got:#?}"
    );
    assert_eq!(
        receipts(&got),
        vec![receipt("call_accepted", "call-b", "dequeued", "queue")],
        "and the receipt says the call waited first: {got:#?}"
    );

    // **What IS ordered, and what is not.** The hold and the break are in two
    // different bundles, separated by a store round trip and a `call_ended`, so
    // the hold really does reach the switch before the break that stops it —
    // that is a claim about this channel and it is asserted.
    //
    // Within ONE bundle nothing is ordered, and asserting it was the defect
    // this test carried: `web_fetch` is a STATELESS cell, so its dispatcher
    // spawns one worker per message up to `params.max_concurrency` (4 in this
    // hive), and the pause and the hold media — or the break and the resume —
    // are two messages that run at once. Whichever the mock switch records
    // first is a race, and a green run was the race going one way.
    let calls = switch_calls(&c).await;
    let hold = at(&calls, "uuid_broadcast?call-b");
    let brk = at(&calls, "uuid_break?call-b all");
    assert!(
        hold < brk,
        "the hold media plays before the command that stops it: {calls:?}"
    );
    for needle in [
        "uuid_audio_stream?call-b pause",
        "uuid_audio_stream?call-b resume",
    ] {
        let _ = at(&calls, needle);
    }
    assert!(
        !calls.iter().any(|p| p.contains("call-b start")),
        "and nothing is reconstructed: the stream the dialplan started was \
         paused, never closed: {calls:?}"
    );

    // And the promoted call is ADDRESSABLE. A promotion that moved a row and
    // started a socket but left the answer unable to find the line would look
    // exactly like this one from the switch's side.
    let got = answer_into_the_channel(&mut c, "call-b", "keeper-generation-7").await;
    assert_eq!(
        hop_of(&only(&got, "media_got_in_speak"), "spoke_session"),
        "call-b",
        "the assistant speaks into the call that was just promoted, not into \
         the keeper's generation: {got:#?}"
    );
}

/// **A caller who hangs up while waiting leaves a receipt, and promotes
/// nobody.** No capacity came free — the call in front is still running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_who_hangs_up_while_waiting_leaves_a_receipt() {
    if skip() {
        return;
    }
    let mut c = start_tuned(vec![ok("+OK\n"); 12], |p| {
        p["second_call"] = json!("queue");
    })
    .await;
    let _ = rings(&mut c, "call-a").await;
    let _ = rings(&mut c, "call-b").await;
    let before = switch_paths(&c).await.len();

    let got = round(
        &mut c,
        json!({"mode": "ev", "ev": "call_ended", "call_uuid": "call-b",
               "number": KNOWN, "cause": "ORIGINATOR_CANCEL"}),
    )
    .await;
    assert_eq!(
        receipts(&got),
        vec![receipt(
            "call_abandoned",
            "call-b",
            "ORIGINATOR_CANCEL",
            "queue"
        )],
        "an abandoned wait is neither an acceptance nor a refusal: {got:#?}"
    );
    assert_eq!(turns(&got), Vec::<String>::new());
    assert_eq!(
        switch_paths(&c).await.len(),
        before,
        "nothing is started for a caller who is gone"
    );

    // The POSITIVE control for both silences: the call in front really is
    // still running, so a `hangup` finds it and names it at the switch.
    let got = round(&mut c, json!({"mode": "hangup"})).await;
    assert!(
        hop_of(&only(&got, "surface_got_in_tool"), "got_text").contains("hung up"),
        "the call that was never in the queue is still on the line: {got:#?}"
    );
}

/// **`parallel` runs two calls and refuses the third.**
///
/// Each accepted call is a turn of its own, and each carries its own
/// `call_id` — which is what lets an answer name the line it is meant for,
/// whatever spoke last.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_runs_two_calls_and_refuses_the_third() {
    if skip() {
        return;
    }
    let mut c = start_tuned(vec![ok("+OK\n"); 12], |p| {
        p["second_call"] = json!("parallel");
        p["capacity"] = json!(2);
    })
    .await;

    for uuid in ["call-a", "call-b"] {
        let got = rings(&mut c, uuid).await;
        assert_eq!(turns(&got).len(), 1, "{uuid} is announced once: {got:#?}");
        assert_eq!(
            receipts(&got),
            vec![receipt("call_accepted", uuid, "", "parallel")]
        );
        let turn = only(&got, "surface_got_in_turn");
        assert_eq!(
            hop_of(&turn, "got_call"),
            uuid,
            "the turn names the call it came out of: {turn:#?}"
        );
    }

    // **Two live lines, addressed independently** — the one thing `call_id`
    // buys that nothing else can. The SAME keeper generation answers into both
    // calls, crossed, and each answer reaches the line it names; with the old
    // key both would have gone wherever the keeper last stamped.
    //
    // What this does NOT show is two CONVERSATIONS. The fixture wires
    // `context.channel = 'phone'` the way the shipped manifest does, so both
    // calls share one room and one session generation. Separating those is the
    // one manifest line `templates/freeswitch/README.md` § *What a second call
    // gets* documents, and it is the operator's call rather than this
    // template's.
    for uuid in ["call-b", "call-a"] {
        let got = answer_into_the_channel(&mut c, uuid, "keeper-generation-7").await;
        assert_eq!(
            hop_of(&only(&got, "media_got_in_speak"), "spoke_session"),
            uuid,
            "the answer reaches the line it names, not the one that spoke last: {got:#?}"
        );
    }

    let got = rings(&mut c, "call-c").await;
    assert_eq!(
        receipts(&got),
        vec![receipt("call_refused", "call-c", "busy", "parallel")],
        "the third call is over capacity, and the receipt names the policy that \
         allowed the first two: {got:#?}"
    );
}

/// **A line this colony puts down itself empties the queue too.**
///
/// The other half of *the queue is emptied by the end of a call*: `call_ended`
/// is the far end hanging up, and a `hangup` the model called is this end. A
/// promotion that ran only on the first would leave a caller waiting for ever
/// whenever the assistant ended the call itself, and nothing would say so —
/// which is the failure mode a queue has.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hangup_this_channel_fires_itself_promotes_the_waiting_caller() {
    if skip() {
        return;
    }
    let mut c = start_tuned(vec![ok("+OK\n"); 12], |p| {
        p["second_call"] = json!("queue");
    })
    .await;

    let _ = rings(&mut c, "call-a").await;
    let got = rings(&mut c, "call-b").await;
    assert_eq!(
        receipts(&got),
        vec![receipt("call_queued", "call-b", "", "queue")],
        "somebody is waiting before the hang-up: {got:#?}"
    );

    // The model ends the call that is on the line. Nothing arrives from the far
    // end at all — the switch is a mock and sends no `call_ended`.
    let got = round(&mut c, json!({"mode": "hangup", "call_id": "call-a"})).await;
    assert!(
        hop_of(&only(&got, "surface_got_in_tool"), "got_text").contains("hung up"),
        "the running call goes down: {got:#?}"
    );
    assert_eq!(
        receipts(&got),
        vec![receipt("call_accepted", "call-b", "dequeued", "queue")],
        "and the caller who was waiting is taken on in the same breath: {got:#?}"
    );
    assert_eq!(turns(&got).len(), 1, "with the turn that announces them");

    let calls = switch_calls(&c).await;
    assert!(
        at(&calls, "uuid_kill?call-a") < at(&calls, "uuid_audio_stream?call-b resume"),
        "the line goes down before the next one is fed: {calls:?}"
    );
}

/// **`hangup` refuses to guess which line, and takes a `call_id`** (R-G5).
///
/// The defect this pins: the tool ended "the newest live call", which was a
/// coincidence dressed as a rule. With two calls running, ending the wrong one
/// cannot be undone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hangup_refuses_to_guess_which_line_and_takes_a_call_id() {
    if skip() {
        return;
    }
    let mut c = start_tuned(vec![ok("+OK\n"); 12], |p| {
        p["second_call"] = json!("parallel");
        p["capacity"] = json!(2);
    })
    .await;
    for uuid in ["call-a", "call-b"] {
        let _ = rings(&mut c, uuid).await;
    }

    let got = round(&mut c, json!({"mode": "hangup"})).await;
    let refusal = only(&got, "surface_got_in_tool");
    let text = hop_of(&refusal, "got_text");
    assert!(
        text.contains("call-a") && text.contains("call-b"),
        "the refusal names every line it could have meant: {refusal:#?}"
    );
    assert!(
        !switch_calls(&c)
            .await
            .iter()
            .any(|p| p.contains("uuid_kill")),
        "and it ends none of them"
    );

    let got = round(&mut c, json!({"mode": "hangup", "call_id": "call-a"})).await;
    assert!(
        hop_of(&only(&got, "surface_got_in_tool"), "got_text").contains("hung up"),
        "naming one ends that one: {got:#?}"
    );
    let calls = switch_calls(&c).await;
    assert_eq!(
        calls,
        vec!["/webapi/uuid_kill?call-a".to_string()],
        "the line the model named is the only line that goes down: {calls:?}"
    );
}

/// Percent-decoding, for reading back a query the template composed. Only what
/// the assertion needs: `%XX` and nothing else.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escape = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| u8::from_str_radix(&s[i + 1..i + 3], 16).ok())
            .flatten();
        match escape {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}
