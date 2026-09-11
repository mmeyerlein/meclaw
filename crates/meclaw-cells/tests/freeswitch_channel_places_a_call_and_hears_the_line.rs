//! `freeswitch@2.0.0` — a telephone as a CHANNEL of a person: it turns the facts
//! about the line a person could ANSWER into TURNS, books the rest, and offers
//! the assistant five tools of its own.
//!
//! Ruling (2026-09-06): a phone is not an app. An app emits
//! `view`/`error`/`tool_result`; a CHANNEL stamps turns, and a call that came
//! in and a call that was picked up are things that *happen*.
//!
//! GH #614 drew the other half of that line, on the first real inbound call: a
//! fact nobody can answer is not a turn. **A generation reads a turn by
//! ANSWERING it** (ADR-0025), so every fact that became one was a sentence
//! spoken at somebody — two greetings for one inbound call, and a farewell into
//! a line that was already down. Three measurements below are that ruling:
//! `an_inbound_call_answered_at_once_raises_one_turn`,
//! `the_far_end_hanging_up_is_booked_and_said_to_nobody`, and the `uuid_kill`
//! half of `an_incoming_call_becomes_a_turn_and_a_stranger_is_refused`.
//! So the hive holds BOTH halves of one call — the media half (`voice`,
//! unchanged) and the signalling half — bound by one id: FreeSWITCH's channel
//! UUID is the `?session=` of the audio fork and therefore the session an
//! answer is spoken back into.
//!
//! # What is booted
//!
//! The SHIPPED `member` and `assistant`, cell for cell, with `code` doubles in
//! place of every `ref` and of the two holders a round here never reaches —
//! plus the SHIPPED `freeswitch`, cell for cell, with only its media half doubled:
//! a real `voice` cell would bind a socket and want two providers, and nothing
//! measured here is about audio. `dial`, `signal`, `gateway` and `calls` are
//! the template's own files, read off disk.
//!
//! The switch is a mock HTTP server (`meclaw_testing::mock_http`) standing in
//! for `mod_xml_rpc`: it records every request and answers with the canned
//! `+OK` / `-ERR` a switch answers with. `${FREESWITCH_XMLRPC_BASE_URL}` in the
//! template's own params resolves to it out of the colony's `.env`, so the URL
//! the test reads is the URL the template composed.
//!
//! Every assertion is a POSITIVE receipt: a double answers, the answer reaches
//! the sink, and the assertion reads what it says. The one refusal case (a
//! caller in no `callers` entry) carries its positive control in the same file
//! — the very same lane from a KNOWN number has to produce a turn, or the
//! silence would prove nothing.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and a template that did not travel is
//! skipped rather than judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::web_fetch::WebFetchCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::collections::BTreeSet;
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
/// A number in no entry at all.
const STRANGER: &str = "+4930999999";
/// The UUID the dialplan reports for an INBOUND leg — inbound legs are minted
/// by FreeSWITCH, not by this channel.
const INBOUND_UUID: &str = "inbound-0001";

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
    elif mode == "tool":
        # Any tool by name, with the arguments the case wants: the three line
        # tools take a number or a PIN and nothing this file has to model.
        sys.stdout.write(json.dumps({
            "header": {"route": "tool", "tool_name": str(hop.get("tool") or ""),
                       "tool_call_id": "c9"},
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "c9",
                          "text": str(hop.get("args") or "{}")}]}))
    elif mode == "hangup":
        sys.stdout.write(json.dumps({
            "header": {"route": "tool", "tool_name": "hangup", "tool_call_id": "c2"},
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "c2",
                          "text": "{}"}]}))
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
    head = {"route": str(hop.get("ev") or ""),
            "call_uuid": str(hop.get("call_uuid") or ""),
            "number": str(hop.get("number") or ""),
            "cause": str(hop.get("cause") or "")}
    # The stamp a dialplan puts on an inbound call once the switch has looked
    # the number up in its own table. Absent unless the case is about it.
    if hop.get("user_id"):
        head["user_id"] = str(hop["user_id"])
    sys.stdout.write(json.dumps({"header": head, "messages": []}))
else:
    sys.stdout.write(json.dumps({
        "header": {"route": "in_wire", "mode": mode,
                   "number": str(hop.get("number") or ""),
                   "tool": str(hop.get("tool") or ""),
                   "args": str(hop.get("args") or "")},
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
            "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && (hop.tool_name == 'call' || hop.tool_name == 'hangup' || hop.tool_name == 'add_number' || hop.tool_name == 'set_pin' || hop.tool_name == 'disable_pin')",
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
    signal["params"]["voice_ws_url"] = json!("ws://127.0.0.1:7777/phone/ws");
    // Which member a caller of this line is put through as. Without it the
    // three line tools write nothing and say so, which is its own measurement
    // below.
    signal["params"]["line_user_id"] = json!(KNOWN_USER);
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

/// The same, for a lane whose whole receipt is that NOTHING comes back.
///
/// `round`'s thirty seconds are the failure marker of a test that waits for a
/// message; a test that waits for silence would pay them on every GREEN run.
/// Six seconds is the window — twice the quiet window `round` honours once a
/// first message has arrived, and two orders of magnitude over the store round
/// trip a turn would cost. **No caller of this leans on the silence alone**:
/// each one follows it with a `hangup`, whose answer proves the bundle really
/// ran and really wrote the row.
async fn silent_round(c: &mut Colony, hop: Value) -> Vec<Message> {
    c.h.send(inject(hop)).await;
    gather(&mut c.rx, Duration::from_secs(6), Duration::from_secs(6)).await
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
/// `context.session_id` and the CALL's id on `context.call_id`.
///
/// That pair is what GH #603 § 3 was about and what GH #620 settled: the two
/// keys have two owners, so the call travels under a name of its own and the
/// keeper keeps `session_id`. Nothing restamps anything on the way down any
/// more — the workaround that put the call back on `session_id` is retracted.
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

/// The media half is finished with one sentence. A real `voice` cell emits
/// this on its `speak_end` lane; here the test says when, because the timing
/// IS what is being measured.
///
/// Nothing this round produces reaches the member's rim — what it produces is a
/// command at the SWITCH — so the wait is on the switch and not on the sink.
async fn speak_ends(c: &mut Colony, call: &str, reason: &str, want_paths: usize) -> Vec<String> {
    // GH #612: sent AS THE HIVE. The channel is a sealed hive, so `.../voice`
    // named from OUTSIDE (sender `/`) is refused `hive_boundary` — an outside
    // caller may not know the inside. This message is not an outside caller: the
    // shipped topology raises it on the `speak_end` lane and the test only says
    // WHEN, because the timing is what is being measured. It therefore names the
    // hive as its sender, which is who hands it over in the running colony.
    c.h.send_from(
        Path::new(&format!("/person/channels/{CHANNEL}")),
        MessageBuilder::new(Path::new(&format!("/person/channels/{CHANNEL}/voice")))
            .hop(
                json!({"route": "end_speak", "session_id": call, "reason": reason})
                    .as_object()
                    .expect("an object")
                    .clone(),
            )
            .body(Body::Inline(json!({"messages": []})))
            .ttl(200)
            .build(),
    )
    .await;
    // Two shapes in one wait. `want_paths` is the OPTIMISTIC exit — the round
    // trip through the store takes as long as it takes and a fixed sleep would
    // be either flaky or slow. The 1500 ms quiet window afterwards is the
    // SEMANTIC one: it is what proves that nothing second arrives, and it is
    // what the callers that expect NO new command lean on, so they pass the
    // count they already have and leave the loop at once.
    //
    // Ten seconds and not the thirty-second failure marker: this deadline is
    // never the marker of a failing test — a caller that expects nothing new
    // reaches it on the happy path — so it is sized for the two store round
    // trips a `speak_end` costs and not for the worst runner in the fleet.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while switch_paths(c).await.len() < want_paths && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    switch_paths(c).await
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

/// **The offer, the call, the answer and the hang-up** — one line, from the
/// menu tick to `uuid_kill`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_channel_offers_two_tools_places_a_call_and_hangs_it_up() {
    if skip() {
        return;
    }
    let mut c = start(vec![ok("+OK 8fc7f8ea\n"), ok("+OK\n")]).await;

    // 1. the menu tick reaches the channel and comes back through the member's
    //    NEW restamp edge `./channels -> ./assistants` as `in_menu`.
    let got = round(&mut c, json!({"mode": "menu"})).await;
    let menu = only(&got, "surface_got_in_menu");
    assert_eq!(
        hop_of(&menu, "got_names"),
        "call,hangup,add_number,set_pin,disable_pin",
        "a channel answers its WHOLE offer, whatever was asked"
    );
    assert_eq!(hop_of(&menu, "got_answerer"), CHANNEL);

    // 2. the call. The receipt is immediate and names the session; the switch
    //    has been asked with everything a media edge needs to fork the audio.
    let got = round(&mut c, json!({"mode": "call", "number": KNOWN})).await;
    let receipt = only(&got, "surface_got_in_tool");
    assert_eq!(hop_of(&receipt, "got_call_id"), "c1");
    let session = hop_of(&receipt, "got_session");
    assert!(
        !session.is_empty(),
        "the receipt of a `call` carries the session the call runs under: {receipt:#?}"
    );

    let paths = switch_paths(&c).await;
    assert_eq!(paths.len(), 1, "one originate and nothing else: {paths:?}");
    let originate = &paths[0];
    assert!(
        originate.starts_with("/webapi/originate?"),
        "the switch is asked over /webapi: {originate}"
    );
    let decoded = percent_decode(originate);
    for needle in [
        format!("origination_uuid={session}"),
        "originate_timeout=45".to_string(),
        // GH #603 § 2: starting the stream is an API command, not an
        // application, and `mod_audio_stream` reads `8000` and not `8k`.
        format!("api_on_answer='uuid_audio_stream {session} start "),
        // GH #619: a telephone call is 8 kHz, and the SAME number reaches the
        // media half in the fork URL -- the two places that have to agree are
        // written from one `fork_sample_rate`, so a mismatch is not a thing an
        // operator can produce any more.
        "mono 8000'".to_string(),
        format!("?session={session}&sample_rate=8000"),
        KNOWN.to_string(),
    ] {
        assert!(
            decoded.contains(&needle),
            "the originate has to carry `{needle}`: {decoded}"
        );
    }
    assert!(
        !decoded.contains("execute_on_answer") && !decoded.contains("uuid_audio_fork"),
        "the form that made FreeSWITCH answer `Invalid Application` is gone: {decoded}"
    );

    // 3. the dialplan says it was answered -> a TURN, not a tool result.
    let got = round(
        &mut c,
        json!({"mode": "ev", "ev": "call_answered", "call_uuid": session, "number": KNOWN}),
    )
    .await;
    let turn = only(&got, "surface_got_in_turn");
    assert_eq!(hop_of(&turn, "got_state"), "answered");
    assert_eq!(hop_of(&turn, "got_session"), session);
    assert_eq!(hop_of(&turn, "got_user"), KNOWN_USER);
    assert_eq!(hop_of(&turn, "got_channel"), CHANNEL_KIND);
    assert!(
        hop_of(&turn, "got_text").contains("answered")
            && hop_of(&turn, "got_text").contains("ask how she slept"),
        "the turn says who answered and what the call was for: {turn:#?}"
    );

    // 4. hang up -> `uuid_kill` at the switch, on the call that is running.
    let got = round(&mut c, json!({"mode": "hangup"})).await;
    let bye = only(&got, "surface_got_in_tool");
    assert_eq!(hop_of(&bye, "got_call_id"), "c2");
    let paths = switch_paths(&c).await;
    assert_eq!(paths.len(), 2, "originate, then uuid_kill: {paths:?}");
    assert_eq!(
        paths[1],
        format!("/webapi/uuid_kill?{session}"),
        "the hang-up names the call the table says is running"
    );
}

/// **A call nobody takes is a turn, not a tool result.** The switch's own
/// `-ERR` is the only producer of that outcome — a `+OK` is swallowed, because
/// the ANSWER of a call is the dialplan's event and one outcome with two
/// producers arrives twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_that_is_not_taken_comes_back_as_a_turn() {
    if skip() {
        return;
    }
    let mut c = start(vec![ok("-ERR NO_ANSWER\n")]).await;
    let got = round(&mut c, json!({"mode": "call", "number": KNOWN})).await;

    let receipt = only(&got, "surface_got_in_tool");
    let session = hop_of(&receipt, "got_session");
    assert!(!session.is_empty());

    let turn = only(&got, "surface_got_in_turn");
    assert_eq!(hop_of(&turn, "got_state"), "no_answer");
    assert_eq!(hop_of(&turn, "got_session"), session);
    assert!(
        hop_of(&turn, "got_text").contains("no_answer"),
        "the reason the line did not connect travels with the turn: {turn:#?}"
    );
}

/// **An incoming call is a turn** — and a caller in no `callers` entry is not.
/// Both halves in one file, because the refusal is a SILENCE on the turn lane
/// and a silence proves nothing without its control.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_incoming_call_becomes_a_turn_and_a_stranger_is_refused() {
    if skip() {
        return;
    }
    let mut c = start(vec![ok("+OK\n")]).await;

    let got = round(
        &mut c,
        json!({"mode": "ev", "ev": "call_incoming",
               "call_uuid": INBOUND_UUID, "number": KNOWN}),
    )
    .await;
    let turn = only(&got, "surface_got_in_turn");
    assert_eq!(hop_of(&turn, "got_session"), INBOUND_UUID);
    assert_eq!(hop_of(&turn, "got_user"), KNOWN_USER);
    assert!(
        hop_of(&turn, "got_text").contains(KNOWN),
        "the turn names who is ringing: {turn:#?}"
    );

    let got = round(
        &mut c,
        json!({"mode": "ev", "ev": "call_incoming",
               "call_uuid": "inbound-0002", "number": STRANGER}),
    )
    .await;
    assert!(
        got.iter()
            .all(|m| hop_of(m, "error_code") != "surface_got_in_turn"),
        "a caller this member does not know raises no turn: {:#?}",
        got.iter()
            .map(|m| hop_of(m, "error_code"))
            .collect::<Vec<_>>()
    );
    let refusal = only(&got, "unknown_caller");
    assert!(
        hop_of(&refusal, "detail").contains(STRANGER),
        "the refusal names the number that rang: {refusal:#?}"
    );

    // GH #614: and the refused leg is PUT DOWN. The dialplan answered it in
    // order to `curl` this hive, and its own mailbox fallback fires on a `curl`
    // that FAILS — not on one that comes back carrying a refusal. Without this
    // the caller sat in an answered call nobody would ever speak into.
    let paths = switch_paths(&c).await;
    assert_eq!(
        paths,
        vec!["/webapi/uuid_kill?inbound-0002".to_string()],
        "the stranger's leg is killed on the UUID the dialplan named, and \
         nothing else reaches the switch: {paths:?}"
    );
}

/// **An inbound call is ONE turn, not two.** The dialplan answers an inbound leg
/// at once, so `call_answered` follows `call_incoming` inside the same second —
/// and the assistant, which reads a turn by answering it, greeted the same
/// caller twice (GH #614). `call_incoming` is the announcement; `call_answered`
/// books the state behind it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inbound_call_answered_at_once_raises_one_turn() {
    if skip() {
        return;
    }
    let mut c = start(vec![ok("+OK\n")]).await;

    let got = round(
        &mut c,
        json!({"mode": "ev", "ev": "call_incoming",
               "call_uuid": INBOUND_UUID, "number": KNOWN}),
    )
    .await;
    let turn = only(&got, "surface_got_in_turn");
    assert_eq!(hop_of(&turn, "got_state"), "incoming");
    assert_eq!(hop_of(&turn, "got_session"), INBOUND_UUID);

    // The same call, answered by the dialplan a moment later. Nothing is said.
    let got = silent_round(
        &mut c,
        json!({"mode": "ev", "ev": "call_answered",
               "call_uuid": INBOUND_UUID, "number": KNOWN}),
    )
    .await;
    assert_eq!(
        turns(&got),
        Vec::<String>::new(),
        "the caller is greeted once, not twice: {:#?}",
        turns(&got)
    );

    // The POSITIVE control for that silence: the bundle really ran and really
    // wrote the row, so the `hangup` behind it finds this call running and
    // names it at the switch. A silence with nothing behind it would pass even
    // if the lane had never reached the cell.
    let got = round(&mut c, json!({"mode": "hangup"})).await;
    assert!(
        hop_of(&only(&got, "surface_got_in_tool"), "got_text").contains(KNOWN),
        "the hang-up names the inbound call the table says is running: {got:#?}"
    );
    let paths = switch_paths(&c).await;
    assert_eq!(
        paths,
        vec![format!("/webapi/uuid_kill?{INBOUND_UUID}")],
        "the answered lane booked the state it was given: {paths:?}"
    );
}

/// **The far end hanging up is BOOKED and said to nobody** (GH #614).
///
/// It used to be a turn, and the sentence that argued for it — *an agent that
/// goes on talking into a call that ended is the failure this lane exists to
/// prevent* — is exactly the failure it caused: a generation reads a turn by
/// ANSWERING it, so the assistant said goodbye into a line that was already
/// gone, hung up a call that was no longer running, and said goodbye again.
/// ADR-0025's reasoning one lane over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_far_end_hanging_up_is_booked_and_said_to_nobody() {
    if skip() {
        return;
    }
    let mut c = start(vec![ok("+OK 8fc7f8ea\n")]).await;
    let got = round(&mut c, json!({"mode": "call", "number": KNOWN})).await;
    let session = hop_of(&only(&got, "surface_got_in_tool"), "got_session");

    let got = silent_round(
        &mut c,
        json!({"mode": "ev", "ev": "call_ended", "call_uuid": session,
               "number": KNOWN, "cause": "NORMAL_CLEARING"}),
    )
    .await;
    assert_eq!(
        turns(&got),
        Vec::<String>::new(),
        "nothing is said into a line that is down: {:#?}",
        turns(&got)
    );

    // The POSITIVE control: the row really moved. A `hangup` now finds no call
    // running — which is the very answer the assistant used to get after it had
    // already been told to say goodbye.
    let got = round(&mut c, json!({"mode": "hangup"})).await;
    assert!(
        hop_of(&only(&got, "surface_got_in_tool"), "got_text").contains("no call is running"),
        "the end of the call was booked, cause and all: {got:#?}"
    );
    let paths = switch_paths(&c).await;
    assert_eq!(
        paths.len(),
        1,
        "one originate, and no kill on a call that is already down: {paths:?}"
    );
}

/// **A hang-up in the middle of a sentence waits for the sentence.** Cutting
/// the line here is what a caller hears as the last word turning into a dial
/// tone, so the kill is held until the media half says it is finished — and
/// then it happens exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hangup_waits_for_the_running_sentence() {
    if skip() {
        return;
    }
    let mut c = start(vec![ok("+OK 8fc7f8ea\n"), ok("+OK\n")]).await;

    let got = round(&mut c, json!({"mode": "call", "number": KNOWN})).await;
    let session = hop_of(&only(&got, "surface_got_in_tool"), "got_session");
    let _ = round(
        &mut c,
        json!({"mode": "ev", "ev": "call_answered", "call_uuid": session, "number": KNOWN}),
    )
    .await;

    // The assistant answers. GH #603 § 3 and GH #620: the session keeper owns
    // `context.session_id`, so the call travels beside it under its own name —
    // the media half must be handed the CALL.
    let got = answer_into_the_channel(&mut c, &session, "keeper-generation-7").await;
    let spoke = only(&got, "media_got_in_speak");
    assert_eq!(
        hop_of(&spoke, "spoke_session"),
        session,
        "the media half selects the connection by the CALL, not by the keeper's generation"
    );

    // The hang-up arrives mid-sentence: an answer, and no kill.
    let got = round(&mut c, json!({"mode": "hangup"})).await;
    let bye = only(&got, "surface_got_in_tool");
    assert!(
        hop_of(&bye, "got_text").contains("sentence"),
        "the model is told the line stays up until it has finished: {bye:#?}"
    );
    let paths = switch_paths(&c).await;
    assert_eq!(
        paths.len(),
        1,
        "nothing but the originate has reached the switch yet: {paths:?}"
    );

    // The sentence finishes, and now the line goes down — once.
    let paths = speak_ends(&mut c, &session, "done", 2).await;
    assert_eq!(
        paths,
        vec![paths[0].clone(), format!("/webapi/uuid_kill?{session}")],
        "the `speak_end` of that call is what carries the hang-up out: {paths:?}"
    );
}

/// **A barge-in reaches the switch.** The synthesis stopped at the cell, but
/// FreeSWITCH is still holding the audio it was already handed and
/// `mod_audio_stream` takes no stop from the server side — so a `cancelled`
/// `speak_end` becomes an API call on the switch, and nothing hangs up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_sentence_breaks_the_playback_at_the_switch() {
    if skip() {
        return;
    }
    let mut c = start(vec![ok("+OK 8fc7f8ea\n"), ok("+OK\n")]).await;

    let got = round(&mut c, json!({"mode": "call", "number": KNOWN})).await;
    let session = hop_of(&only(&got, "surface_got_in_tool"), "got_session");
    let _ = round(
        &mut c,
        json!({"mode": "ev", "ev": "call_answered", "call_uuid": session, "number": KNOWN}),
    )
    .await;
    let _ = answer_into_the_channel(&mut c, &session, "keeper-generation-7").await;

    let paths = speak_ends(&mut c, &session, "cancelled", 2).await;
    assert_eq!(paths.len(), 2, "one originate, then one break: {paths:?}");
    assert_eq!(
        percent_decode(&paths[1]),
        format!("/webapi/uuid_break?{session} all"),
        "the half sentence the caller is still hearing is stopped at the switch — \
         FreeSWITCH's own command, because `mod_audio_stream` dispatches only \
         start/stop/pause/resume/send_text and offers no clear of its own"
    );
    assert!(
        !paths.iter().any(|p| p.contains("uuid_kill")),
        "a barge-in is not a hang-up: {paths:?}"
    );
}

/// **A call speaks twice, and the hang-up waits for BOTH sentences.**
///
/// `speaking` is a count and not a flag, because the media half queues what it
/// is given: with a flag, the first `speak_end` would take the line down in the
/// middle of the second sentence — the failure the whole mechanism exists to
/// prevent, one sentence later.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hangup_waits_for_every_sentence_that_is_still_queued() {
    if skip() {
        return;
    }
    let mut c = start(vec![ok("+OK 8fc7f8ea\n"), ok("+OK\n")]).await;

    let got = round(&mut c, json!({"mode": "call", "number": KNOWN})).await;
    let session = hop_of(&only(&got, "surface_got_in_tool"), "got_session");
    let _ = round(
        &mut c,
        json!({"mode": "ev", "ev": "call_answered", "call_uuid": session, "number": KNOWN}),
    )
    .await;

    // Two answers in a row: the cell queues them, so the call owes two
    // sentences before it is finished talking.
    let _ = answer_into_the_channel(&mut c, &session, "keeper-generation-7").await;
    let _ = answer_into_the_channel(&mut c, &session, "keeper-generation-7").await;

    let got = round(&mut c, json!({"mode": "hangup"})).await;
    assert!(
        hop_of(&only(&got, "surface_got_in_tool"), "got_text").contains("sentence"),
        "the hang-up is booked, not carried out"
    );

    // The FIRST sentence finishes. One left, so the line stays up — and the
    // quiet window inside `speak_ends` is what makes that a receipt rather
    // than a race won. `1` is the count already on the table, so the wait is
    // the quiet window and nothing else.
    let paths = speak_ends(&mut c, &session, "done", 1).await;
    assert_eq!(
        paths.len(),
        1,
        "one sentence done is not the assistant finished talking: {paths:?}"
    );

    // The second one does it.
    let paths = speak_ends(&mut c, &session, "done", 2).await;
    assert_eq!(
        paths,
        vec![paths[0].clone(), format!("/webapi/uuid_kill?{session}")],
        "the `speak_end` that takes the count to zero carries the hang-up out: {paths:?}"
    );
}

/// **A leg handed to a dialplan extension starts no stream of its own.** That
/// extension owns the leg and starts its own; a second start on the same
/// channel is a second socket nobody reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transfer_answer_app_carries_no_stream_command() {
    if skip() {
        return;
    }
    let mut c = start_tuned(vec![ok("+OK 8fc7f8ea\n")], |params| {
        params["answer_app"] = json!("&transfer(7001 XML default)");
    })
    .await;

    let got = round(&mut c, json!({"mode": "call", "number": KNOWN})).await;
    let session = hop_of(&only(&got, "surface_got_in_tool"), "got_session");
    let paths = switch_paths(&c).await;
    assert_eq!(paths.len(), 1, "one originate: {paths:?}");
    let decoded = percent_decode(&paths[0]);
    assert!(
        decoded.contains(&format!("origination_uuid={session}"))
            && decoded.contains("&transfer(7001 XML default)"),
        "the leg is still handed to the extension: {decoded}"
    );
    assert!(
        !decoded.contains("api_on_answer") && !decoded.contains("uuid_audio_stream"),
        "and the channel starts no stream beside it: {decoded}"
    );
}

/// **A `call_incoming` that carries a `user_id` is trusted** (2.0.0).
///
/// The switch is the proxy: it holds the numbers, it asked for the PIN, and it
/// looked the number up in its own table. So the identity it stamped on the
/// event is the identity of that call — and the number is not read as one while
/// the stamp is there. Measured on a number in NO `callers` entry, which is the
/// only way to tell a trusted stamp from the fallback answering underneath it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_incoming_with_a_user_id_is_trusted() {
    if skip() {
        return;
    }
    let mut c = start(vec![]).await;

    let got = round(
        &mut c,
        json!({"mode": "ev", "ev": "call_incoming", "call_uuid": INBOUND_UUID,
               "number": STRANGER, "user_id": "the-verified-one"}),
    )
    .await;
    let turn = only(&got, "surface_got_in_turn");
    assert_eq!(
        hop_of(&turn, "got_user"),
        "the-verified-one",
        "the member the switch put the caller through as is the sender of the \
         turn — a `callers` table that does not carry this number said nothing \
         about it: {turn:#?}"
    );
    assert_eq!(hop_of(&turn, "got_session"), INBOUND_UUID);
    assert!(
        hop_of(&turn, "got_text").contains(STRANGER),
        "the turn still names the number that rang: {turn:#?}"
    );
    // and nothing was put down: the positive control of the refusal above is
    // that the same number without a stamp reaches `uuid_kill`.
    let paths = switch_paths(&c).await;
    assert!(
        paths.is_empty(),
        "a stamped call is a call this channel takes, so no leg is killed: \
         {paths:?}"
    );
}

/// **The three line tools write the SWITCH's table** (2.0.0), and nothing else.
///
/// `mod_db` takes `db insert/<realm>/<key>/<value>` and
/// `db delete/<realm>/<key>`, and `/webapi/db?<args>` is that command as a GET.
/// Two shapes of row: one keyed by the NUMBER, carrying two fields —
/// `<line_user_id>|<voice_ws_url>`, the whole stream URL inside the row, so one
/// row is everything the dialplan needs about a line; one keyed `pin.<member>`,
/// carrying the PIN, which `disable_pin` deletes — a DOT because `mod_db` splits
/// its command into four tokens on `/` and only the last of them may carry one.
/// What is measured is the URL the template composed — no row is read back here,
/// because this colony keeps none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_three_line_tools_write_the_switches_table() {
    if skip() {
        return;
    }
    let mut c = start(vec![ok("+OK\n"), ok("+OK\n"), ok("+OK\n")]).await;

    for (tool, args) in [
        ("add_number", json!({"number": STRANGER}).to_string()),
        ("set_pin", json!({"pin": "4711"}).to_string()),
        ("disable_pin", "{}".to_string()),
    ] {
        let got = round(&mut c, json!({"mode": "tool", "tool": tool, "args": args})).await;
        let receipt = only(&got, "surface_got_in_tool");
        assert_eq!(
            hop_of(&receipt, "got_call_id"),
            "c9",
            "the receipt of `{tool}` answers the call the model made: {receipt:#?}"
        );
    }

    let paths = switch_paths(&c).await;
    assert_eq!(
        paths,
        vec![
            format!(
                "/webapi/db?insert%2Fmeclaw_lines%2F%2B4930999999%2F{KNOWN_USER}\
                 %7Cws%3A%2F%2F127.0.0.1%3A7777%2Fphone%2Fws"
            ),
            format!("/webapi/db?insert%2Fmeclaw_lines%2Fpin.{KNOWN_USER}%2F4711"),
            format!("/webapi/db?delete%2Fmeclaw_lines%2Fpin.{KNOWN_USER}"),
        ],
        "three commands, in the order they were asked for, at the realm this \
         channel is configured with: {paths:?}"
    );
}

/// **A PIN the dialplan could not read back is refused, and nothing is written.**
///
/// `play_and_get_digits … ^\d{4,8}$` is what reads it at the switch, so a PIN
/// outside that is a PIN no caller could ever enter — and a row nobody can
/// satisfy is a line that stopped ringing for a reason nobody can find.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pin_the_dialplan_cannot_read_is_refused() {
    if skip() {
        return;
    }
    let mut c = start(vec![]).await;
    let got = round(
        &mut c,
        json!({"mode": "tool", "tool": "set_pin",
               "args": json!({"pin": "letmein"}).to_string()}),
    )
    .await;
    let receipt = only(&got, "surface_got_in_tool");
    assert!(
        hop_of(&receipt, "got_text").contains("4 to 8 digits"),
        "the refusal says what a PIN is: {receipt:#?}"
    );
    let paths = switch_paths(&c).await;
    assert!(paths.is_empty(), "a refused PIN writes no row: {paths:?}");
}

/// **A row the switch refused is not a silence** (2.0.0).
///
/// The three tools answer the moment the command leaves, because that is all the
/// cell knows. A write the switch refuses — no `mod_db` loaded, a realm nobody
/// configured, an XML-RPC that answers `401` — would otherwise leave an operator
/// with a line that asks for no PIN and a model that says it does. So the answer
/// of a `db` command is read for a refusal, and the refusal comes back on the
/// tool call that asked for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_the_switch_refused_answers_the_tool_call() {
    if skip() {
        return;
    }
    let mut c = start(vec![ok("-ERR no reply\n")]).await;
    let got = round(
        &mut c,
        json!({"mode": "tool", "tool": "set_pin",
               "args": json!({"pin": "4711"}).to_string()}),
    )
    .await;
    let said: Vec<String> = got
        .iter()
        .filter(|m| hop_of(m, "error_code") == "surface_got_in_tool")
        .map(|m| hop_of(m, "got_text"))
        .collect();
    assert_eq!(
        said.len(),
        2,
        "the receipt of the command that left, and the refusal that came back: \
         {said:?}"
    );
    assert!(
        said.iter().any(|t| t.contains("on its way to the switch")),
        "the first answer says the command LEFT: {said:?}"
    );
    assert!(
        said.iter()
            .any(|t| t.contains("refused `set_pin`") && t.contains("no reply")),
        "the second names the operation and what the switch said: {said:?}"
    );

    // The positive control: a `+OK` says nothing and answers nothing, so the
    // model is told once and not twice.
    let mut c = start(vec![ok("+OK\n")]).await;
    let got = round(
        &mut c,
        json!({"mode": "tool", "tool": "set_pin",
               "args": json!({"pin": "4711"}).to_string()}),
    )
    .await;
    let said: Vec<String> = got
        .iter()
        .filter(|m| hop_of(m, "error_code") == "surface_got_in_tool")
        .map(|m| hop_of(m, "got_text"))
        .collect();
    assert_eq!(
        said.len(),
        1,
        "a written row is one receipt and no second sentence: {said:?}"
    );
}

/// **A number that is not a number writes no row** (2.0.0).
///
/// `mod_db` splits `insert/<realm>/<key>/<value>` on `/`, so a number carrying
/// one would write a row nobody asked for — and the tool schema asks for
/// international form, which this is what makes true.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_number_that_is_not_one_writes_no_row() {
    if skip() {
        return;
    }
    let mut c = start(vec![]).await;
    for bad in ["+49/30/111", "not-a-number", ""] {
        let got = round(
            &mut c,
            json!({"mode": "tool", "tool": "add_number",
                   "args": json!({"number": bad}).to_string()}),
        )
        .await;
        let receipt = only(&got, "surface_got_in_tool");
        assert!(
            hop_of(&receipt, "got_text").contains("international form"),
            "the refusal of {bad:?} says what a number is: {receipt:#?}"
        );
    }
    let paths = switch_paths(&c).await;
    assert!(paths.is_empty(), "no row was written: {paths:?}");
}

/// **The README names the tools the offer carries, and only those** — the drift
/// lock of 2.0.0.
///
/// A charter and a menu that disagree tell a model it has a capability it does
/// not have (`docs/development-rules.md` § 8), and a README is the charter a
/// human wires against. Both directions are compared against the README's own
/// tool block, so a sixth tool documented and never offered fails here too.
#[test]
fn the_readme_names_the_tools_the_offer_carries() {
    let Some((_, _, fs_template)) = shipped() else {
        return;
    };
    let dial = read_json(&fs_template.join("dial/config.json"));
    let script = dial["params"]["script_inline"]
        .as_str()
        .expect("the offer is a script");
    let offered: BTreeSet<String> = script
        .split("{\"name\": \"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(|n| n.to_string())
        .collect();

    // The README's own block: the fenced list under § *The five tools*, where
    // every line that offers something reads `name(args)`.
    let readme = std::fs::read_to_string(fs_template.join("README.md")).expect("the README");
    let block = readme
        .split("## The five tools")
        .nth(1)
        .and_then(|rest| rest.split("```").nth(1))
        .expect("the tool block stands under its own heading, fenced");
    let documented: BTreeSet<String> = block
        .lines()
        .filter_map(|l| l.split_once('('))
        .map(|(head, _)| head.trim().to_string())
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .collect();

    assert_eq!(
        offered, documented,
        "the offer and the README's own tool block name different sets — a \
         reader wires against the README and a model is given the offer"
    );
    assert_eq!(
        offered.len(),
        5,
        "the offer of this channel is five tools: {offered:?}"
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
