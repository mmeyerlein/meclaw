//! GH #459 — a screen is a CHANNEL of the person, and an app is a producer of
//! views that stands beside their agents.
//!
//! `display@1.0.0` and `colony-view@1.0.0` shipped under GH #455 and
//! `member@1.3.0` grew a `channels/` container under GH #454, and nothing joined
//! them: the display's own README named the address
//! (`<member>/channels/display-<screen>`) while the member's graph carried no
//! lane that reached it, `colony-view` was tagged `app` with nowhere to be an
//! app, and the return path the display promised could not be written at all.
//!
//! # Why the owner had to move onto the hop
//!
//! The display stamps the OWNER of a view — the path of the cell that put it up,
//! taken from `envelope.reply_to` and never from the body — onto `event` and
//! `receipt`. It carried it in the **body**, and an edge condition in this
//! substrate is evaluated against `context.*` and `hop.*` and nothing else
//! (`crates/meclaw-colony/src/cel_eval.rs`, `bind_ctx`). So "the member routes on
//! `owner`" was unwritable as an edge. The two keys now ride on the hop as well,
//! always present and empty where the object id would not parse — which is what
//! makes an unattributable event fail every owner guard by construction instead
//! of going somewhere arbitrary.
//!
//! # What the level splits, and what a mutation splits
//!
//! The member splits on the CONTAINER: an owner path under `assistants/` goes to
//! `./assistants`, one under `apps/` goes to `./apps`, and one that is neither
//! leaves the level on `error`. It matches with `contains('/assistants/')` rather
//! than a prefix because the owner is an ABSOLUTE cell path and a template does
//! not know its own absolute prefix.
//!
//! Which agent, and which app, stays the instantiating mutation's edge — the
//! documented per-assistant cost of GH #454, because `Edge.to` is a static path
//! and there is no way to write "send it wherever the header says".
//!
//! # What is booted
//!
//! The SHIPPED `member` and `assistant` templates, cell for cell, with every
//! `ref` marker replaced by an answering `code` double and with the edges an
//! instantiating mutation would draw appended to the member's own graph. The
//! screen's double emits `event` and `receipt` with the hop stamps the shipped
//! `compose.py` really writes — which the first test in this file measures
//! against the shipped bytes, so the double is not a fiction.
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

// ═════════════════════════════ 1. the stamps the double imitates are real

/// Hand the script to the runner on STDIN instead of in argv (GH #349).
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).expect("serialise the script"),
        meclaw_core::serde_json::to_string(stdin_doc).expect("serialise the document"),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// One emission of the SHIPPED compose cell, for a message with this hop, body
/// and sender.
fn compose_once(body: Value, route: &str, reply_to: &str) -> Value {
    let cfg = read_json(&repo("templates/display/compose/config.json"));
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("display/compose declares script_inline");
    let doc = json!({
        "envelope": {
            "header": {"hop": {"route": route}, "context": {}},
            "target": "/display",
            "trace_id": "00000000-0000-0000-0000-000000000000",
            "ttl": 64,
            "reply_to": reply_to,
        },
        "body": body,
        "params": {},
    });
    let out = run_script_on_stdin(script, &doc.to_string());
    assert!(
        out.status.success(),
        "compose exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("compose stdout is not JSON: {e}"));
    match v {
        Value::Array(mut a) => a.remove(0),
        other => other,
    }
}

/// The routing fact this whole task rests on: `owner` and `view_id` leave the
/// display on the HOP, not only in the body.
///
/// If this ever goes back to being body-only, every owner guard in the member's
/// graph silently evaluates false and every browser event dead-letters — which
/// looks exactly like a screen nobody touched.
#[test]
fn the_display_stamps_the_owner_on_the_hop_of_an_event_and_a_receipt() {
    if !repo("templates/display/compose/config.json").is_file() {
        eprintln!("display did not travel into this tree -- skipped (GH #49)");
        return;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("no python3 -- skipped");
        return;
    }

    // An event on a view of a known owner: the object id carries the owner path
    // with `/` written as `~`, and the hop carries it back as a path.
    let owner = "/os/orgs/example/members/one/assistants/egon/surface";
    let oid = format!("view.{}.note", owner.replace('/', "~"));
    let ev = compose_once(
        json!({"messages": [], "event": {"name": "click", "value": oid}}),
        "event",
        "/anybody",
    );
    assert_eq!(ev["header"]["route"], "event");
    assert_eq!(
        ev["header"]["owner"], owner,
        "the event has to name its owner on the HOP: an edge condition never sees the body"
    );
    assert_eq!(ev["header"]["view_id"], "note");

    // An event whose id will not parse still leaves, and its owner is the empty
    // string rather than an absent key: a key that is sometimes missing is a
    // router branch nobody tests, and an empty owner fails every guard.
    let blind = compose_once(
        json!({"messages": [], "event": {"name": "click", "value": "not-a-view-id"}}),
        "event",
        "/anybody",
    );
    assert_eq!(blind["header"]["route"], "event");
    assert_eq!(
        blind["header"]["owner"], "",
        "an unattributable event leaves anyway, with an EMPTY owner"
    );

    // A refusal names the sender it refused, on the hop, so the member can hand
    // it back to exactly that writer.
    let refused = compose_once(
        json!({"messages": [], "view_id": "note", "owner": "/somebody/else",
               "kind": "prose", "content": {"title": "t", "body": "b"}}),
        "in_view",
        "/os/orgs/example/members/one/apps/colony-view/layout",
    );
    assert_eq!(refused["header"]["route"], "receipt");
    assert_eq!(refused["receipt"]["error_code"], "not_owner");
    assert_eq!(
        refused["header"]["owner"],
        "/os/orgs/example/members/one/apps/colony-view/layout"
    );
    assert_eq!(refused["header"]["view_id"], "note");
}

// ══════════════════════════════════════════════════════════════ the doubles

/// A cell that answers nothing: the holders this round never reaches. They exist
/// because a hive door pointing at an absent directory leaves the inside
/// unroutable (GH #286).
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

/// The SCREEN — `display@1.0.0`, doubled at the two lanes this level cares about.
///
/// - `in_view` is what an agent or an app put up. In production the round ENDS
///   here (a browser sees it and the colony does not); a test needs a witness, so
///   the double reports the delivery on the one exit a channel ships, `error`,
///   which the member carries out of the level. The assertion reads the hop keys.
/// - `in_wire` is the test's own trigger for the two lanes the screen PRODUCES.
///   The stamps it writes are the ones the shipped `compose.py` writes, measured
///   against the shipped bytes by the first test in this file.
const SCREEN: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
route = str(hop.get("route") or "")
body = doc["body"]

if route == "in_wire":
    # The screen speaks: an `event` or a `receipt`, owned by whoever the test
    # names. `owner` and `view_id` ride on the header, always present.
    lane = str(hop.get("lane") or "event")
    owner = str(hop.get("owner_hint") or "")
    out = {"header": {"route": lane, "owner": owner, "view_id": "note"},
           "messages": body.get("messages") or []}
    if lane == "event":
        out["event"] = {"name": "click", "value": "view." + owner.replace("/", "~") + ".note"}
        out["owner"] = owner
    else:
        out["receipt"] = {"error_code": "not_owner", "owner": owner,
                          "view_id": "note", "detail": "the body claimed somebody else"}
    sys.stdout.write(json.dumps(out))
else:
    # A view arrived. Report WHO wrote it and on which channel.
    sys.stdout.write(json.dumps({
        "header": {"error_code": "shown",
                   "shown_owner": str(doc["envelope"].get("reply_to") or ""),
                   "shown_route": route,
                   "shown_channel": str(ctx.get("channel") or ""),
                   "shown_view": str(body.get("view_id") or ""),
                   "shown_kind": str(body.get("kind") or "")},
        "messages": body.get("messages") or []}))
"#;

/// The conversation surface of one generation, doubled.
///
/// Two errands, told apart by the header rather than by the body: a turn that
/// arrived with `hop.kind` set is a SCREEN event or receipt the member re-stamped
/// onto `in_turn`, and the double reports it on `error` so the assertion can read
/// what the surface actually saw. Anything else is an ordinary turn, and the
/// answer it produces is a PROSE VIEW — the smallest view there is, which needs
/// no application at all.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
kind = str(hop.get("kind") or "")

if kind:
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "surface_saw",
                   "saw_kind": kind,
                   "saw_owner": str(hop.get("owner") or ""),
                   "saw_channel": str(ctx.get("channel") or "")},
        "messages": doc["body"].get("messages") or []}))
else:
    sys.stdout.write(json.dumps({
        "header": {"route": "answer"},
        "messages": [],
        "view_id": "note",
        "kind": "prose",
        "content": {"title": "a note", "body": "written by the agent, with no app at all"}}))
"#;

/// `colony-view@1.0.0`, doubled at its two lanes.
///
/// It is display-BLIND: it emits `view` and never names a screen. Which screen
/// it draws on is one literal in the edge that leaves this cell, which is the
/// whole reason the app template says nothing about it.
const APP: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
route = str(hop.get("route") or "")

if route == "event":
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "app_saw",
                   "saw_owner": str(hop.get("owner") or ""),
                   "saw_channel": str(ctx.get("channel") or "")},
        "messages": doc["body"].get("messages") or []}))
else:
    sys.stdout.write(json.dumps({
        "header": {"route": "view"},
        "messages": [],
        "view_id": "colony",
        "kind": "component",
        "content": {"title": "the colony"}}))
"#;

/// Puts one message on a named lane. `hop.mode` picks the door.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
mode = str(hop.get("mode") or "turn")
out = {"header": {"route": {"turn": "in_turn", "screen": "in_wire", "app": "in_tick"}[mode],
                  "lane": str(hop.get("lane") or "event"),
                  "owner_hint": str(hop.get("owner_hint") or "")},
       "messages": doc["body"].get("messages", [])}
sys.stdout.write(json.dumps(out))
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

/// The screen's name, which is also the value `context.channel_node` carries — and
/// `context.channel` too, because a screen is one room (GH #522): a
/// channel's directory name is a fact of the wiring, not a label.
const SCREEN_NAME: &str = "display-main";
const AGENT: &str = "egon";
const APP_NAME: &str = "colony-view";
/// Where the member stands, so a test can name an absolute owner path the way
/// the display would stamp one.
const MEMBER: &str = "/person";

/// The three edges one SCREEN costs, and not one more than a chat channel costs.
///
/// The down-edge is the only display-specific thing in the whole arrangement: a
/// screen takes what an agent said (`answer`) or what an app drew (`view`) and
/// re-stamps it to the display's own `in_view`. A chat channel's down-edge takes
/// `answer` and leaves the lane alone. Same shape, one literal apart.
fn screen_edges() -> Vec<Value> {
    vec![
        json!({
            "from": format!("./channels/{SCREEN_NAME}"), "to": "./channels",
            "condition": "has(hop.route) && (hop.route == 'event' || hop.route == 'receipt')",
            // GH #522 -- both keys, and on a screen they are the same word: a
            // screen IS one room, so its address and its conversation partner
            // coincide. The container routes on the first, the holders read the
            // second.
            "modifier": {"set_context": {"channel_node": format!("'{SCREEN_NAME}'"),
                                         "channel": format!("'{SCREEN_NAME}'")}}
        }),
        json!({
            "from": format!("./channels/{SCREEN_NAME}"), "to": "./channels",
            "condition": "has(hop.error_code)",
            "modifier": {"set_hop": {"route": "'error'"}}
        }),
        json!({
            "from": "./channels", "to": format!("./channels/{SCREEN_NAME}"),
            "condition": format!(
                "has(hop.route) && (hop.route == 'answer' || hop.route == 'view') && \
                 has(context.channel_node) && context.channel_node == '{SCREEN_NAME}'"),
            "modifier": {"set_hop": {"route": "'in_view'"}}
        }),
    ]
}

/// The edges one assistant costs, with the screen wired in.
///
/// The first is GH #454's addressing edge, widened by one clause and no more: a
/// turn names the agent in `context.assistant`, and a SCREEN event names it by
/// the owner path the display stamped. Both are the mutation's business, because
/// `Edge.to` is a static path and a level cannot name an agent it has not met.
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

/// The two edges one APP costs. The first names the screen — one literal, in the
/// app's own outbound edge, which is why the app template never mentions a
/// display. The second reads the owner the display stamped.
fn app_edges(name: &str) -> Vec<Value> {
    vec![
        json!({
            "from": format!("./apps/{name}"), "to": "./apps",
            "condition": "has(hop.route) && (hop.route == 'view' || hop.route == 'error')",
            "modifier": {"set_context": {"channel_node": format!("'{SCREEN_NAME}'"),
                                         "channel": format!("'{SCREEN_NAME}'")}}
        }),
        json!({
            "from": "./apps", "to": format!("./apps/{name}"),
            "condition": format!(
                "has(hop.route) && (hop.route == 'event' || hop.route == 'receipt') && \
                 has(hop.owner) && hop.owner.contains('/apps/{name}/')")
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
            "from": "./driver", "to": format!("./person/channels/{SCREEN_NAME}"),
            "condition": "has(hop.route) && hop.route == 'in_wire'"
        }),
        json!({
            "from": "./driver", "to": format!("./person/apps/{APP_NAME}"),
            "condition": "has(hop.route) && hop.route == 'in_tick'"
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
    for holder in ["affinity", "memory-hive", "export-sink"] {
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
            "Test double for display@1.0.0, one screen of this person.",
        ),
    );
    write(
        root,
        &format!("main/person/apps/{APP_NAME}/config.json"),
        &double(
            APP,
            "Test double for colony-view@1.0.0, an app of this person.",
        ),
    );

    let dst = root.join(format!("main/person/assistants/{AGENT}"));
    copy_cells(assistant, &dst);
    write(
        root,
        &format!("main/person/assistants/{AGENT}/surface/config.json"),
        &double(
            SURFACE,
            "Test double for the conversation surface of one generation.",
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
    edges.extend(app_edges(APP_NAME));
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

fn inject(mode: &str, lane: &str, owner_hint: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("mode".into(), json!(mode));
    hop.insert("lane".into(), json!(lane));
    hop.insert("owner_hint".into(), json!(owner_hint));
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "a person did something"}
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
/// the receipt is POSITIVE: a round routed to the wrong container arrives naming
/// the wrong one, it does not go quiet.
async fn round(mode: &str, lane: &str, owner_hint: &str) -> Message {
    let Some((member, assistant)) = shipped() else {
        panic!("guarded by the caller");
    };
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member, &assistant);
    let (h, mut rx) = boot(&td).await;
    h.send(inject(mode, lane, owner_hint)).await;
    let got = recv_bounded(&mut rx)
        .await
        .expect("the round has to reach the sink -- every double answers");
    h.shutdown().await;
    got
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

/// **The smallest view needs no app.** An agent answers a turn that arrived on
/// the screen, and the answer lands on the screen as a view — through the very
/// same `./assistants -> ./channels` edge GH #454 drew for a chat answer.
///
/// This is the half of GH #455 that had no home: "a prose view is the smallest
/// view there is" was true of the display and unreachable from a member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agents_prose_answer_lands_on_the_screen_as_a_view() {
    if skip() {
        return;
    }
    let got = round("turn", "", "").await;
    assert_eq!(hop_of(&got, "error_code"), "shown", "{got:#?}");
    assert_eq!(
        hop_of(&got, "shown_route"),
        "in_view",
        "the screen has to see the agent's answer as its OWN lane: the channel's down-edge \
         re-stamps it, which is the one display-specific thing in the whole arrangement"
    );
    assert_eq!(hop_of(&got, "shown_kind"), "prose");
    assert_eq!(
        hop_of(&got, "shown_owner"),
        format!("{MEMBER}/assistants/{AGENT}/surface"),
        "the owner of a view is the path of the cell that emitted it, and nothing else"
    );
    assert_eq!(hop_of(&got, "shown_channel"), SCREEN_NAME);
}

/// **An app writes to the same screen, and is display-blind while doing it.**
/// `colony-view` emits `view` and names no display; the screen is one literal in
/// the edge that leaves the app.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_apps_view_lands_on_the_same_screen_under_its_own_owner() {
    if skip() {
        return;
    }
    let got = round("app", "", "").await;
    assert_eq!(hop_of(&got, "error_code"), "shown", "{got:#?}");
    assert_eq!(hop_of(&got, "shown_route"), "in_view");
    assert_eq!(hop_of(&got, "shown_view"), "colony");
    assert_eq!(
        hop_of(&got, "shown_owner"),
        format!("{MEMBER}/apps/{APP_NAME}"),
        "an app owns its views by the same rule an agent does — there is no privileged writer"
    );
}

/// **A browser event goes back to the AGENT whose view it was**, as an ordinary
/// turn carrying `hop.kind == "event"`, so the brain reads it the way it reads a
/// message. `assistant@2.0.0` has no event lane and did not grow one: `in_turn`
/// is the lane it accepts, and the header says what kind of turn this is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_on_an_agents_view_reaches_that_agent_as_a_turn() {
    if skip() {
        return;
    }
    let owner = format!("{MEMBER}/assistants/{AGENT}/surface");
    let got = round("screen", "event", &owner).await;
    assert_eq!(hop_of(&got, "error_code"), "surface_saw", "{got:#?}");
    assert_eq!(
        hop_of(&got, "saw_kind"),
        "event",
        "the agent has to be able to tell a browser event from something a person typed"
    );
    assert_eq!(hop_of(&got, "saw_owner"), owner);
    assert_eq!(
        hop_of(&got, "saw_channel"),
        SCREEN_NAME,
        "the screen the event happened on travels with it, which is how the answer finds \
         its way back to the same surface"
    );
}

/// **A refused write goes back to the agent that asked for it**, on the same
/// path and told apart by the same key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_receipt_reaches_the_agent_whose_write_was_refused() {
    if skip() {
        return;
    }
    let owner = format!("{MEMBER}/assistants/{AGENT}/surface");
    let got = round("screen", "receipt", &owner).await;
    assert_eq!(hop_of(&got, "error_code"), "surface_saw", "{got:#?}");
    assert_eq!(hop_of(&got, "saw_kind"), "receipt");
    assert_eq!(hop_of(&got, "saw_owner"), owner);
}

/// **The same event, owned by an app, reaches the app** — and it is the OWNER
/// path that decides, nothing else. The member splits on the container; which
/// app inside it is the mutation's edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_on_an_apps_view_reaches_that_app() {
    if skip() {
        return;
    }
    let owner = format!("{MEMBER}/apps/{APP_NAME}/layout");
    let got = round("screen", "event", &owner).await;
    assert_eq!(hop_of(&got, "error_code"), "app_saw", "{got:#?}");
    assert_eq!(hop_of(&got, "saw_owner"), owner);
    assert_eq!(hop_of(&got, "saw_channel"), SCREEN_NAME);
}

/// **An event the level cannot attribute leaves it on `error`.**
///
/// The display emits an unparseable event ANYWAY, with an empty owner, because a
/// view it holds and cannot attribute is a defect somebody has to see. That is
/// only true if somebody does see it: the member re-stamps it onto the `error`
/// lane it already emits — no new exit is owed to the parent — and the original
/// lane name survives on `hop.kind`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_event_with_no_owner_leaves_the_level_instead_of_dead_lettering() {
    if skip() {
        return;
    }
    let got = round("screen", "event", "").await;
    assert_eq!(
        hop_of(&got, "kind"),
        "event",
        "the lane the message was on has to survive the re-stamp, or the error says \
         nothing about what failed: {got:#?}"
    );
    assert_eq!(
        hop_of(&got, "owner"),
        "",
        "this is the branch for an owner nobody can place"
    );
}

/// A receipt with an owner outside BOTH containers is the same defect: a path
/// that is neither an agent of this person nor an app of this person is not
/// something to guess about.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_receipt_owned_by_a_stranger_leaves_the_level_rather_than_being_guessed_at() {
    if skip() {
        return;
    }
    let got = round("screen", "receipt", "/person-next-door/somebody/else").await;
    assert_eq!(hop_of(&got, "kind"), "receipt", "{got:#?}");
    assert_eq!(hop_of(&got, "owner"), "/person-next-door/somebody/else");
}
