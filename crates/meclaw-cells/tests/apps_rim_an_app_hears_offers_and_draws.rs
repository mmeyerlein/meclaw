//! apps-rim — an app of a person HEARS the conversation, OFFERS a tool,
//! ANSWERS with it and DRAWS on the screen, all at the rim of its member.
//!
//! Three ways to connect, and all three are edges (ruling R1, 2026-09-04):
//!
//! - **listening** is a fan-out, never an interception: `./firewall -> ./apps`
//!   carries the SCREENED turn, `./assistants -> ./apps` the answer, and both
//!   are guarded on the channel, so an operator's turn stays the operator's.
//! - **offering** is a v-lane in and a re-stamp back out: the surface calls
//!   `show` straight into the app (ADR-0020), the app answers `tool_result` at
//!   its own rim, and the member's own edge turns that into `in_tool` exactly
//!   as it does for the memory (GH #552).
//! - **writing** is what a member could already do: `view` out of the app, onto
//!   the screen the installing mutation named.
//!
//! # Who draws which edge (ruling M-1, 2026-09-05)
//!
//! The member TEMPLATE declares the observer lanes at its container
//! (`params.contract`, `at: ["./apps"]`) and keeps the two re-stamp edges
//! `./apps -> ./assistants`, which without an app never fire. The observer
//! edges themselves belong to the mutation that INSTALLS an app: whoever
//! listens orders it, the way a voice channel orders `partial`. A member with
//! no listening app therefore carries no such edge and dead-letters nothing —
//! and a SECOND installation draws the same three edges again, which the edge
//! table holds once. The last test in this file measures that, because the
//! failure mode it rules out is silent: two edges of the same shape deliver the
//! same turn twice.
//!
//! # What is booted
//!
//! The SHIPPED `member` and `assistant`, cell for cell, with `code` doubles in
//! place of every `ref`, plus a fixture app `showcase` built out of two `code`
//! cells — `show` answers the tool and the menu, `stage` turns everything it
//! hears into a view. Every edge of the install manifest
//! (`docs/superpowers/specs/2026-09-05-apps-rim-design.md` § 5) is appended to
//! the member's own graph, unchanged.
//!
//! Every assertion is a POSITIVE receipt: a double answers, the answer reaches
//! the sink, and the assertion reads what it says. The one negative case (an
//! operator's turn) carries its positive control in the same round — the answer
//! has to arrive at the member's own exit, or the silence would prove nothing.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and a template that did not travel is skipped
//! rather than judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

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

// ═══════════════════════════════════════════════════════════════ the names

/// The screen the app draws on. One literal, in the app's own outbound edge —
/// which is why the app template says nothing about a display.
const SCREEN_NAME: &str = "display-main";
/// The generation whose surface calls the tool.
const AGENT: &str = "egon";
/// The fixture app. An instance is named after its template.
const APP_NAME: &str = "showcase";
/// The SECOND installation of the same app template (M-1).
const APP2_NAME: &str = "showcase2";
/// Where the member stands, so a test can name an absolute owner path.
const MEMBER: &str = "/person";

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

/// The SCREEN — `display@1.0.0`, doubled at the one lane this file cares about.
///
/// In production the round ENDS here (a browser sees the view and the colony
/// does not), so a test needs a witness: the double reports every delivery on a
/// header carrying `error_code`, which the member carries out of the level.
/// `shown_heard` is the app's own stamp and says WHICH lane produced the view.
const SCREEN: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
body = doc["body"]
sys.stdout.write(json.dumps({
    "header": {"error_code": "shown",
               "shown_owner": str(doc["envelope"].get("reply_to") or ""),
               "shown_route": str(hop.get("route") or ""),
               "shown_heard": str(hop.get("heard") or ""),
               "shown_channel": str(ctx.get("channel") or ""),
               "shown_view": str(body.get("view_id") or "")},
    "messages": []}))
"#;

/// The conversation surface of one generation, doubled — the caller of the tool
/// and the reader of the menu.
///
/// `in_wire` is the test's own trigger: it makes the surface emit the two lanes
/// that leave its rim on a v-lane, `tool` and `schemas`. `in_tool` and
/// `in_menu` are the way BACK, and the double reports each of them on `error`
/// with the two keys the assertions read — the call id the app echoed, and the
/// answerer the member's binding edge stamped.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
route = str(hop.get("route") or "")

if route == "in_wire":
    if str(hop.get("lane") or "") == "menu":
        sys.stdout.write(json.dumps({"header": {"route": "schemas"},
                                     "messages": [], "tools": ["show"]}))
    else:
        sys.stdout.write(json.dumps({
            "header": {"route": "tool", "tool_name": "show", "tool_call_id": "c1"},
            "messages": [], "arguments": {"kind": "component", "title": "a thing"}}))
elif route == "in_tool":
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "surface_got_in_tool",
                   "got_call_id": str(hop.get("tool_call_id") or ""),
                   "got_answerer": str(ctx.get("tool_answerer") or "")},
        "messages": []}))
elif route == "in_menu":
    names = ",".join(str(s.get("name") or "") for s in (doc["body"].get("schemas") or []))
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "surface_got_in_menu",
                   "got_answerer": str(ctx.get("tool_answerer") or ""),
                   "got_names": names},
        "messages": []}))
elif route == "in_turn":
    sys.stdout.write(json.dumps({
        "header": {"route": "answer"},
        "messages": [{"origin": "assistant", "type": "text", "text": "the agent answered"}],
        "view_id": "note", "kind": "prose",
        "content": {"title": "a note", "body": "written by the agent"}}))
else:
    sys.stdout.write(json.dumps([]))
"#;

/// The TOOL SURFACE of the generation, doubled — the second producer of
/// `tool_result` the app listens in on (ruling R4/R5, O-A4).
const TOOLS: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
if str(hop.get("route") or "") == "in_res":
    sys.stdout.write(json.dumps({
        "header": {"route": "tool_result", "tool_name": "weather", "tool_call_id": "t9"},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "t9",
                      "text": "22 degrees and clear"}]}))
else:
    sys.stdout.write(json.dumps([]))
"#;

/// The app's OFFER — `show`, the connect point of the `tool` and `schemas`
/// v-lanes.
///
/// On `schemas` it answers its WHOLE offer, whatever was asked, with an empty
/// `unknown` (spec O-A3): the question was never addressed to it, the collector
/// merges what every answerer delivered, and a name nobody delivers is the tool
/// hive's business. On a call it echoes the `tool_call_id` — a result the
/// caller cannot pair with its call is a result it drops.
const SHOW: &str = r#"
import sys, json
doc = json.load(sys.stdin); hop = (doc["envelope"].get("header") or {}).get("hop") or {}
route = hop.get("route")
if route == "schemas":
    # An app answers its WHOLE offer, whatever was asked (spec O-A3).
    sys.stdout.write(json.dumps({"header": {"route": "tool_schemas", "operation": "schemas",
        "schema_count": 1, "unknown_count": 0},
        "schemas": [{"name": "show", "description": "show something on the screen",
                     "parameters": {"type": "object", "properties": {"kind": {"type": "string"},
                     "title": {"type": "string"}}, "required": ["kind", "title"]}}],
        "unknown": [], "messages": []}))
else:
    cid = str(hop.get("tool_call_id") or "")
    sys.stdout.write(json.dumps({"header": {"route": "tool_result", "operation": "show",
        "tool_name": "show", "tool_call_id": cid},
        "messages": [{"origin": "tool", "type": "tool_result", "id": cid, "text": "shown"}]}))
"#;

/// The app's EAR and its PEN — `stage`, the connect point of the observed
/// `tool_result` and the door every rim lane of the app leads to.
///
/// Everything it hears becomes one view, stamped with the lane it heard on, so
/// a test can tell a turn from an answer without reading a body. `event` and
/// `receipt` — what a person did to a view it put up — are reported on `error`
/// instead, which is the lane a member carries out of the level.
const STAGE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = (doc["envelope"].get("header") or {}).get("hop") or {}
route = str(hop.get("route") or "")
msgs = doc["body"].get("messages") or []
text = str((msgs[-1].get("text") if msgs else "") or "")
if route in ("turn", "answer", "partial", "tool_result"):
    sys.stdout.write(json.dumps({
        "header": {"route": "view", "heard": route},
        "messages": [], "view_id": "stage", "kind": "component",
        "content": {"heard": route, "text": text}}))
elif route in ("event", "receipt"):
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "app_got_" + route},
        "messages": []}))
else:
    sys.stdout.write(json.dumps([]))
"#;

/// Puts one message on a named lane. `hop.mode` picks the door.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
mode = str(hop.get("mode") or "chan")
route = {"chan": "in_turn", "op": "in_turn", "tool": "in_wire",
         "menu": "in_wire", "toolres": "in_res"}.get(mode, "in_turn")
sys.stdout.write(json.dumps({
    "header": {"route": route, "mode": mode, "lane": str(hop.get("lane") or "")},
    "messages": doc["body"].get("messages", [])}))
"#;

/// A `code` double with a fixed script. `emits` is left wide on purpose: what a
/// double may say is decided by the assertions, not by a contract nobody reads.
/// `multi_send_capable` is on so a double may answer with NOTHING — an empty
/// array — on a lane this round does not care about.
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

// ═════════════════════════════════════════════════════════════ the fixture app

/// The contract of the fixture app, the shape of spec § 4.
///
/// `ports: []` — the address IS the hive, and the two connect points are the
/// one exception the app pronounces about ITSELF: `tool` and `schemas` dock on
/// `./show`, the observed `tool_result` on `./stage`. That `tool_result` stands
/// in `accepts` (observed, deep) AND in `emits` (its own answer, at the rim) is
/// deliberate and legal: the two lists are judged separately.
///
/// `partial` is `"required": true`, as spec § 4 writes it: the one lane an app
/// cannot hear unless the installing mutation draws it AND the channel emits
/// it. The birth check (`hive_contract`, third shape) sees this contract only
/// when the app is instantiated by a mutation, which the second-installation
/// test does; the boot-time tree is authorship and is not judged.
fn app_contract() -> Value {
    json!({
        "accepts": [
            {"route": "turn", "because": "the screened turn of a conversation this person is having on one of their channels -- what they said, after the firewall and before the agent answers"},
            {"route": "answer", "because": "what an agent of this person answered on that same channel, so the app sees both halves of the round rather than half of it"},
            {"route": "partial", "required": true, "because": "an interim transcript of a voice channel: the same round, before it is finished. Whoever listens orders it -- the channel emits it only when its own knob says so"},
            {"route": "tool_result", "at": ["./stage"], "because": "the result of a tool call the app did NOT make: the data an agent just fetched, observed at the stage that draws it. It docks on the stage because it is material for a picture, never a call for the app to answer"},
            {"route": "tool", "at": ["./show"], "because": "a call to `show`, arriving straight from the surface that made it (ADR-0020). It docks on `./show`, the one cell of this app that answers a tool"},
            {"route": "schemas", "at": ["./show"], "because": "the menu tick: what do you serve? Same connect point, because the cell that answers a tool is the cell that can declare it"},
            {"route": "event", "because": "a person touched a view this app put up"},
            {"route": "receipt", "because": "a screen refused a write of this app, and the app is the only one who can tell whether that matters"}
        ],
        "emits": [
            {"route": "view", "because": "the picture this app draws. It names no screen: which display it lands on is one literal in the edge that leaves this hive"},
            {"route": "error", "because": "what this app could not do, on the one exit a member carries out of the level"},
            {"route": "tool_result", "because": "the answer to a call on `show`, carrying back the `tool_call_id` it was called with"},
            {"route": "tool_schemas", "because": "the whole offer of this app, whatever was asked (spec O-A3). The member's binding edge stamps who answered"}
        ]
    })
}

/// Write the fixture app under `base` — the same bytes whether it is planted
/// into the member directly or laid down as a template a mutation can grow.
fn write_app(root: &std::path::Path, base: &str) {
    write(
        root,
        &format!("{base}/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {
                "ports": [],
                "contract": app_contract(),
                "graph": {"edges": [
                    {"from": ".", "to": "./stage",
                     "condition": "has(hop.route) && (hop.route == 'turn' || hop.route == 'answer' || hop.route == 'partial' || hop.route == 'event' || hop.route == 'receipt')"},
                    {"from": "./show", "to": ".",
                     "condition": "has(hop.route) && (hop.route == 'tool_result' || hop.route == 'tool_schemas')"},
                    {"from": "./stage", "to": ".",
                     "condition": "has(hop.route) && (hop.route == 'view' || hop.route == 'error')"}
                ]}
            },
            "description": {
                "purpose": "Fixture app: it hears the conversation, offers one tool and draws one view.",
                "use_when": "Test fixture only.",
                "not_in_scope": "Not a shipped template."
            }
        }),
    );
    write(
        root,
        &format!("{base}/show/config.json"),
        &double(
            SHOW,
            "The app's offer: it answers `schemas` and the call on `show`.",
        ),
    );
    write(
        root,
        &format!("{base}/stage/config.json"),
        &double(
            STAGE,
            "The app's ear and pen: everything it hears becomes one view.",
        ),
    );
}

/// The same app once more, as a TEMPLATE a mutation can instantiate.
fn write_app_template(root: &std::path::Path) {
    write_app(root, "templates/showcase");
    write(
        root,
        "templates/showcase/template.json",
        &json!({"name": "showcase", "version": "1.0.0", "tags": ["app"], "author": "meclaw"}),
    );
}

// ══════════════════════════════════════ the wiring an installing mutation draws

/// **The install manifest, verbatim** —
/// `docs/superpowers/specs/2026-09-05-apps-rim-design.md` § 5, `add_edges`.
///
/// The first three are the member-side observer edges. Since ruling M-1 they
/// are NOT in the member template: the template declares the lanes at the
/// container and the installing mutation draws them, because a member with no
/// listening app should carry no edge at all. A second installation draws them
/// again, identically, and the edge table holds them once.
fn install_edges(agent: &str, app: &str, screen: &str) -> Vec<Value> {
    vec![
        // 1. the SCREENED turn, after the firewall, with the firewall's own
        //    hygiene (gh494) — and guarded on the channel, because an app
        //    belongs to the person and hears the person's rooms.
        json!({
            "from": "./firewall", "to": "./apps",
            "condition": "has(hop.route) && hop.route == 'pass' && has(context.channel_node) && context.channel_node != ''",
            "modifier": {"set_hop": {"route": "'turn'"},
                         "delete_context": ["fw_body", "fw_now", "fw_phase", "store_origin"]}
        }),
        // 2. the answer, on the same guard as `./assistants -> ./channels`. The
        //    guarded DEFAULT `./assistants -> .` is untouched and keeps firing
        //    for a channel-less answer (gh302 pins `is_default`).
        json!({
            "from": "./assistants", "to": "./apps",
            "condition": "has(hop.route) && hop.route == 'answer' && has(context.channel_node) && context.channel_node != ''"
        }),
        // 3. the interim transcript of a voice channel (R-V8').
        json!({
            "from": "./channels", "to": "./apps",
            "condition": "has(hop.route) && hop.route == 'partial'"
        }),
        // 4. the binding to THIS app inside the container.
        json!({
            "from": "./apps", "to": format!("./apps/{app}"),
            "condition": "has(hop.route) && (hop.route == 'turn' || hop.route == 'answer' || hop.route == 'partial')"
        }),
        json!({
            "from": "./apps", "to": format!("./apps/{app}"),
            "condition": format!(
                "has(hop.route) && (hop.route == 'event' || hop.route == 'receipt') && \
                 has(hop.owner) && hop.owner.contains('/apps/{app}/')")
        }),
        // 5. what the app DRAWS, and the screen it draws on — one literal.
        json!({
            "from": format!("./apps/{app}"), "to": "./apps",
            "condition": "has(hop.route) && (hop.route == 'view' || hop.route == 'error')",
            "modifier": {"set_context": {"channel_node": format!("'{screen}'"),
                                         "channel": format!("'{screen}'")}}
        }),
        // 6. what the app ANSWERS, stamped with who answered — the mutation is
        //    the only one that knows the instance name (spec § 2.2).
        json!({
            "from": format!("./apps/{app}"), "to": "./apps",
            "condition": "has(hop.route) && (hop.route == 'tool_result' || hop.route == 'tool_schemas')",
            "modifier": {"set_context": {"tool_answerer": format!("'{app}'")}}
        }),
        // 7. the call, as a v-lane straight from the surface's rim. It bypasses
        //    the assistant's own exit, so it carries that exit's stamps itself.
        json!({
            "from": format!("./assistants/{agent}/talky"), "to": format!("./apps/{app}/show"),
            "lane": "tool",
            "condition": "has(hop.route) && hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'show'",
            "modifier": {"set_context": {"tool_caller": "'talky'", "assistant": format!("'{agent}'")},
                         "delete_context": ["col_phase", "consult_class", "consult_id", "tool_answerer"]}
        }),
        // 8. the menu tick, same form, same stamps.
        json!({
            "from": format!("./assistants/{agent}/talky"), "to": format!("./apps/{app}/show"),
            "lane": "schemas",
            "condition": "has(hop.route) && hop.route == 'schemas'",
            "modifier": {"set_context": {"tool_caller": "'talky'", "assistant": format!("'{agent}'")},
                         "delete_context": ["col_phase", "consult_class", "consult_id", "tool_answerer"]}
        }),
        // 9./10. the two producers of a tool result, observed (O-A4). Both are
        //    v-lanes into the stage and NOT container edges — a container edge
        //    would run the observed copy back into the assistant a second time.
        json!({
            "from": format!("./assistants/{agent}/tools"), "to": format!("./apps/{app}/stage"),
            "lane": "tool_result",
            "condition": "has(hop.route) && hop.route == 'tool_result'"
        }),
        json!({
            "from": "./memory-hive", "to": format!("./apps/{app}/stage"),
            "lane": "tool_result",
            "condition": "has(hop.route) && hop.route == 'tool_result'"
        }),
    ]
}

/// The screen's two edges, plus the TEST's own witness edge.
///
/// A shipped display ends the round; a test needs the delivery reported, and
/// the third edge is what carries that report out of the level. Nothing a
/// shipped display puts on the wire carries `hop.error_code`, so no production
/// screen matches it.
fn screen_edges() -> Vec<Value> {
    vec![
        json!({
            "from": format!("./channels/{SCREEN_NAME}"), "to": "./channels",
            "condition": "has(hop.route) && (hop.route == 'event' || hop.route == 'receipt')",
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
                "has(hop.route) && hop.route == 'view' && \
                 has(context.channel_node) && context.channel_node == '{SCREEN_NAME}'"),
            "modifier": {"set_hop": {"route": "'in_view'"}}
        }),
    ]
}

/// The edges one assistant costs — the growth recipe's, cut to what this file
/// drives: the addressing edge, the two ways back in from a tool answerer
/// (`examples/organism/grow-assistant.json`), and ONE exit for the lanes that
/// leave the generation here. The recipe draws separate `tool` and `schemas`
/// exits that stamp `context.assistant`; in this tree `schemas` never leaves
/// the generation at its rim (the v-lane starts at the talky rim), so those two
/// are not drawn.
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
/// member emits at its own rim. Draining them all is the point — an undrained
/// lane is a dead letter, and this test would then be reading a silence.
fn main_config() -> Value {
    let mut edges = vec![
        json!({
            "from": "./driver", "to": "./person",
            "condition": "has(hop.route) && hop.route == 'in_turn' && has(hop.mode) && hop.mode == 'chan'",
            "modifier": {"set_context": {
                "channel_node": format!("'{SCREEN_NAME}'"),
                "channel": format!("'{SCREEN_NAME}'"),
                "assistant": format!("'{AGENT}'")
            }}
        }),
        // The OPERATOR's turn: it enters at the member's own `in_turn` door and
        // names no channel, which is exactly what makes it the operator's.
        json!({
            "from": "./driver", "to": "./person",
            "condition": "has(hop.route) && hop.route == 'in_turn' && has(hop.mode) && hop.mode == 'op'",
            "modifier": {"set_context": {"assistant": format!("'{AGENT}'")}}
        }),
        json!({
            "from": "./driver", "to": format!("./person/assistants/{AGENT}/talky"),
            "condition": "has(hop.route) && hop.route == 'in_wire'"
        }),
        json!({
            "from": "./driver", "to": format!("./person/assistants/{AGENT}/tools"),
            "condition": "has(hop.route) && hop.route == 'in_res'"
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
            "Test double for display@1.0.0, one screen of this person.",
        ),
    );
    write_app(root, &format!("main/person/apps/{APP_NAME}"));
    write_app_template(root);

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
    write(
        root,
        &format!("main/person/assistants/{AGENT}/tools/config.json"),
        &double(TOOLS, "Test double for the tool surface of one generation."),
    );
    write(
        root,
        &format!("main/person/assistants/{AGENT}/cogny/config.json"),
        &double(
            INERT,
            "Inert double for the reasoning core, which no round here reaches.",
        ),
    );

    let cfg_path = root.join("main/person/config.json");
    let mut cfg = read_json(&cfg_path);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the member ships a graph");
    edges.extend(screen_edges());
    edges.extend(assistant_edges(AGENT));
    edges.extend(install_edges(AGENT, APP_NAME, SCREEN_NAME));
    std::fs::write(
        &cfg_path,
        meclaw_core::serde_json::to_string_pretty(&cfg).expect("serialise"),
    )
    .expect("write the member config");

    std::fs::write(root.join(".env"), "").expect("write an empty .env");
}

// ═════════════════════════════════════════════════════════════════ the colony

/// One booted colony, kept alive for a whole test: several rounds have to run
/// against the SAME graph, and the second-installation case mutates it.
struct Colony {
    td: tempfile::TempDir,
    h: ColonyHandle,
    rx: mpsc::Receiver<Message>,
}

async fn start() -> Colony {
    let Some((member, assistant)) = shipped() else {
        panic!("guarded by the caller");
    };
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &member, &assistant);

    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )]
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
    rescan_templates(&h, td.path().join("templates")).await;
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped member and assistant must boot");
    Colony { td, h, rx: sink_rx }
}

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    ack_rx
        .await
        .expect("the scan answers")
        .expect("the template scan succeeds");
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    ack_rx.await.expect("the mutation answers")
}

async fn read_graph(h: &ColonyHandle) -> ReadGraphReply {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .expect("the colony is up");
    ack_rx.await.expect("the graph answers")
}

fn inject(mode: &str, lane: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("mode".into(), json!(mode));
    hop.insert("lane".into(), json!(lane));
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "a person said something"}
        ]})))
        .hop(hop)
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

/// Collect what the round put on the sink. `quiet` is the SEMANTIC window — the
/// round is over once nothing has arrived for that long AFTER the first
/// message — and `budget` is the failure marker, generous by the 30-second
/// convention: until the first message arrives the whole budget is waited,
/// because a round that spawns half a dozen interpreters under a loaded gate
/// may take longer than any quiet window to say its first word.
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

/// One round: inject, and hand back everything the sink saw.
async fn round(c: &mut Colony, mode: &str, lane: &str) -> Vec<Message> {
    c.h.send(inject(mode, lane)).await;
    gather(&mut c.rx, Duration::from_secs(3), Duration::from_secs(30)).await
}

/// The views the app's stage drew, by the lane it heard them on.
fn views_heard(got: &[Message]) -> Vec<(String, String)> {
    got.iter()
        .filter(|m| hop_of(m, "error_code") == "shown" && !hop_of(m, "shown_heard").is_empty())
        .map(|m| (hop_of(m, "shown_heard"), hop_of(m, "shown_owner")))
        .collect()
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

// ═══════════════════════════════════════════════════════════ the measurements

/// **First: the edges are really there.** Every later assertion reads a
/// silence-or-not, and a silence caused by an edge the colony refused at boot
/// looks exactly like a silence caused by a wrong guard. So the install
/// manifest is read back out of `/colony/graph`, with the lane names the three
/// v-lanes declared.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_install_edges_are_in_the_graph() {
    if skip() {
        return;
    }
    let c = start().await;
    let edges = read_graph(&c.h).await.edges;
    let has = |from: &str, to: &str, needle: &str| {
        edges.iter().any(|e| {
            e.from == from
                && e.to == to
                && e.condition.as_deref().unwrap_or_default().contains(needle)
        })
    };
    let m = MEMBER;
    assert!(
        has(&format!("{m}/firewall"), &format!("{m}/apps"), "'pass'"),
        "the screened turn has to reach the container: {edges:#?}"
    );
    assert!(
        has(&format!("{m}/assistants"), &format!("{m}/apps"), "'answer'"),
        "and so does the answer"
    );
    assert!(
        has(&format!("{m}/channels"), &format!("{m}/apps"), "'partial'"),
        "and the interim transcript"
    );
    for (from, to, lane) in [
        (
            format!("{m}/assistants/{AGENT}/talky"),
            format!("{m}/apps/{APP_NAME}/show"),
            "tool",
        ),
        (
            format!("{m}/assistants/{AGENT}/talky"),
            format!("{m}/apps/{APP_NAME}/show"),
            "schemas",
        ),
        (
            format!("{m}/assistants/{AGENT}/tools"),
            format!("{m}/apps/{APP_NAME}/stage"),
            "tool_result",
        ),
        (
            format!("{m}/memory-hive"),
            format!("{m}/apps/{APP_NAME}/stage"),
            "tool_result",
        ),
    ] {
        assert!(
            edges
                .iter()
                .any(|e| e.from == from && e.to == to && e.lane.as_deref() == Some(lane)),
            "the v-lane '{lane}' {from} -> {to} has to stand, named as the lane it carries: \
             {edges:#?}"
        );
    }
    c.h.shutdown().await;
    drop(c.td);
}

/// **An app hears both halves of a channel round.** The person said something
/// and the agent answered, and the app saw both of them — through two
/// edges it did not have to be told about, both of them FAN-OUT: the turn still
/// reaches the assistant, and listening is never an interception.
///
/// The answer does NOT reach the screen, and since GH #598 that is the point: a
/// screen takes a `view` and nothing else, an agent emits `answer`, and the one
/// producer in this round that writes view bodies is the APP. What used to
/// happen instead is measured in
/// `crates/meclaw-cells/tests/gh598_a_screen_receipt_is_not_a_turn.rs`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_hears_the_turn_and_the_answer_of_a_channel_conversation() {
    if skip() {
        return;
    }
    let mut c = start().await;
    let got = round(&mut c, "chan", "").await;
    let heard: Vec<String> = views_heard(&got).into_iter().map(|(h, _)| h).collect();
    assert!(
        heard.iter().any(|h| h == "turn"),
        "the app has to hear the SCREENED turn: {got:#?}"
    );
    assert!(
        heard.iter().any(|h| h == "answer"),
        "and the answer of the agent, on the same round: {heard:?}"
    );
    assert!(
        !got.iter()
            .any(|m| hop_of(m, "error_code") == "shown" && hop_of(m, "shown_view") == "note"),
        "an agent's answer must not be re-stamped onto a screen (GH #598): the display \
         refuses it as `invalid_view` and the refusal used to come back as a turn. The two \
         assertions above are this round's positive controls: {got:#?}"
    );
    c.h.shutdown().await;
    drop(c.td);
}

/// **An operator's turn stays the operator's.** A turn injected at the member's
/// own door names no channel; the app is not part of that conversation, and the
/// answer goes back out of the level on the guarded DEFAULT exit.
///
/// The positive control is in the same round: the answer HAS to arrive at the
/// sink. Without it the silence of the app would prove nothing at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operators_turn_stays_the_operators() {
    if skip() {
        return;
    }
    let mut c = start().await;
    let got = round(&mut c, "op", "").await;
    assert!(
        got.iter().any(|m| hop_of(m, "route") == "answer"),
        "positive control: the answer has to leave the member on its default exit: {got:#?}"
    );
    assert_eq!(
        views_heard(&got),
        Vec::new(),
        "an app of this person is not part of an operator's errand -- and a regular fan-out \
         edge here would also kill the guarded default the answer leaves on: {got:#?}"
    );
    c.h.shutdown().await;
    drop(c.td);
}

/// **The surface calls `show`, and the result comes back as `in_tool`.**
///
/// Out on a v-lane straight from the surface's rim (which is why the edge
/// carries the stamps the assistant's own exit would have written), back on the
/// member's own re-stamp edge — the very shape the memory has used since
/// GH #552. The call id is what pairs the two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_surface_calls_show_and_gets_the_result_back_as_in_tool() {
    if skip() {
        return;
    }
    let mut c = start().await;
    let got = round(&mut c, "tool", "call").await;
    let back = got
        .iter()
        .find(|m| hop_of(m, "error_code") == "surface_got_in_tool")
        .unwrap_or_else(|| panic!("the result has to reach the caller: {got:#?}"));
    assert_eq!(
        hop_of(back, "got_call_id"),
        "c1",
        "a result the caller cannot pair with its call is a result it drops"
    );
    assert_eq!(
        hop_of(back, "got_answerer"),
        APP_NAME,
        "and the binding edge names WHO answered -- only the mutation knows the instance name"
    );
    c.h.shutdown().await;
    drop(c.td);
}

/// **The app's offer lands in the menu under its own name.** The surface asks
/// `schemas`, the app answers its WHOLE offer whatever was asked (spec O-A3),
/// and the answer arrives as `in_menu` stamped with the answerer — which is how
/// the collector merges it beside every other answerer's rows without talky,
/// its collector or the growth recipe being touched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_apps_offer_lands_in_the_menu_under_its_own_name() {
    if skip() {
        return;
    }
    let mut c = start().await;
    let got = round(&mut c, "menu", "menu").await;
    let menu = got
        .iter()
        .find(|m| hop_of(m, "error_code") == "surface_got_in_menu")
        .unwrap_or_else(|| panic!("the offer has to reach the menu: {got:#?}"));
    assert_eq!(hop_of(menu, "got_answerer"), APP_NAME);
    assert_eq!(
        hop_of(menu, "got_names"),
        "show",
        "the app answers what it serves, not what it was asked about"
    );
    c.h.shutdown().await;
    drop(c.td);
}

/// **A tool result of the generation's own tool surface is OBSERVED by the
/// app** — on a v-lane into the stage, as a fan-out beside the result's own way
/// back to the brain that asked. That is the half of ruling R4 that makes an
/// app able to draw what an agent just fetched without being the one who
/// fetched it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_result_of_the_tool_hive_is_observed_by_the_app() {
    if skip() {
        return;
    }
    let mut c = start().await;
    let got = round(&mut c, "toolres", "").await;
    let heard: Vec<String> = views_heard(&got).into_iter().map(|(h, _)| h).collect();
    assert!(
        heard.iter().any(|h| h == "tool_result"),
        "the observed result has to reach the stage: {got:#?}"
    );
    c.h.shutdown().await;
    drop(c.td);
}

/// **What the app draws lands on the screen, owned by the app.** The owner of a
/// view is the path of the cell that emitted it and nothing else — there is no
/// privileged writer, and an agent's answer and an app's picture arrive on the
/// same screen through the same door.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_apps_view_lands_on_the_screen() {
    if skip() {
        return;
    }
    let mut c = start().await;
    let got = round(&mut c, "chan", "").await;
    let drawn = views_heard(&got);
    assert!(!drawn.is_empty(), "the app has to draw something: {got:#?}");
    let owner = format!("{MEMBER}/apps/{APP_NAME}/stage");
    assert!(
        drawn.iter().any(|(_, o)| *o == owner),
        "the view has to arrive owned by the app that drew it, at {owner}: {drawn:?}"
    );
    assert!(
        got.iter().any(|m| hop_of(m, "error_code") == "shown"
            && hop_of(m, "shown_route") == "in_view"
            && hop_of(m, "shown_channel") == SCREEN_NAME
            && hop_of(m, "shown_owner") == owner),
        "and on the channel the installing mutation named, as the screen's OWN lane, \
         owned by the app's stage (the agent's prose answer lands there too and must \
         not stand in for it): {got:#?}"
    );
    c.h.shutdown().await;
    drop(c.td);
}

/// **A second installation leaves the member-side edge single (ruling M-1).**
///
/// The second app's manifest draws the same three observer edges again — it has
/// to, because it cannot know whether anybody listened before it. The edge
/// table holds an identical edge once (from/to/condition/modifier/default), so
/// the commit is idempotent and the turn arrives at the container ONCE. Two
/// edges of the same shape would deliver it twice, and nothing about that looks
/// wrong from the outside; this is the assertion that would notice.
///
/// The same mutation is also the first time the FULL manifest of § 5 is judged
/// as a mutation rather than read at boot: the three v-lanes have to survive the
/// port boundary of a hive this very diff gives birth to (GH #562/#567).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_installation_leaves_the_member_edge_single() {
    if skip() {
        return;
    }
    let mut c = start().await;

    let outcome = send_mutation(
        &c.h,
        json!({"scope": MEMBER, "diff": {
            "add_nodes": [{"name": format!("apps/{APP2_NAME}"), "template": "showcase@1.0.0"}],
            "add_edges": install_edges(AGENT, APP2_NAME, SCREEN_NAME)
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "installing a second app is one ordinary mutation: {outcome:?}"
    );

    let edges = read_graph(&c.h).await.edges;
    let count = |from: String, to: String, needle: &str| {
        edges
            .iter()
            .filter(|e| {
                e.from == from
                    && e.to == to
                    && e.condition.as_deref().unwrap_or_default().contains(needle)
            })
            .count()
    };
    assert_eq!(
        count(
            format!("{MEMBER}/firewall"),
            format!("{MEMBER}/apps"),
            "'pass'"
        ),
        1,
        "the member-side observer edge is ONE edge however many apps listen: {edges:#?}"
    );
    assert_eq!(
        count(
            format!("{MEMBER}/assistants"),
            format!("{MEMBER}/apps"),
            "'answer'"
        ),
        1,
        "and so is the answer edge"
    );
    assert_eq!(
        count(
            format!("{MEMBER}/channels"),
            format!("{MEMBER}/apps"),
            "'partial'"
        ),
        1,
        "and the partial edge"
    );

    // And the positive half: ONE turn, BOTH apps. The container edge is single;
    // the binding edges are two.
    let got = round(&mut c, "chan", "").await;
    let turns: Vec<String> = views_heard(&got)
        .into_iter()
        .filter(|(h, _)| h == "turn")
        .map(|(_, o)| o)
        .collect();
    assert!(
        turns.contains(&format!("{MEMBER}/apps/{APP_NAME}/stage")),
        "the first app still hears the turn: {got:#?}"
    );
    assert!(
        turns.contains(&format!("{MEMBER}/apps/{APP2_NAME}/stage")),
        "and the newly installed one hears it too: {got:#?}"
    );
    assert_eq!(
        turns.len(),
        2,
        "exactly twice -- once per app, never twice per app: {turns:?}"
    );

    c.h.shutdown().await;
    drop(c.td);
}
