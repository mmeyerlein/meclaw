//! GH #937 — an app OBSERVES the tool of another app: the call on its way in,
//! the result on its way back, without being the app that answers it.
//!
//! # Why this file exists
//!
//! Until GH #937 a consumer that wanted to watch a tool it does not offer could
//! not say so. The `install_app` recipe knew `observes_tool_results` only as a
//! cell name, drawn from the generation's own `./tools` and the memory, and it
//! had no word at all for the CALL. The two edges such an app needs — the call
//! from each surface into the observer, and the result of the offering app into
//! the observer — had to be drawn by hand, and nobody drew them. The recipe now
//! takes `observes_tool_calls: {at, tools, route?}` and the object form of
//! `observes_tool_results`, and this file boots what it renders.
//!
//! # What is measured, and why each half
//!
//! - **Lane, not route.** Every observer edge names the lane it carries
//!   (`tool`, `tool_result`) and re-stamps `hop.route` to the observer's own
//!   word (`in_probe_call`, `in_probe_result`). The substrate's v-lane check
//!   (`v_lane_verdict`) judges the LANE against the target hive's contract, so
//!   the observer declares `{"route": "tool", "at": ["./ear"]}` and
//!   `{"route": "tool_result", "at": ["./ear"]}` — not the re-stamped route.
//!   An app that declares only the re-stamped route is refused by the mutation
//!   door with `v_lane_no_connect_point`; the recipe reads no contract and
//!   cannot catch that, so the last test here pins the refusal.
//! - **Install order does not matter.** The result edge starts at the
//!   CONTAINER `./apps`, where the offering app's own binding edge stamps
//!   `context.tool_answerer`. Neither installation names the other, so the
//!   observer can come first or second; both orders are booted and both commit.
//! - **Observation is a fan-out, never an interception.** The offering app still
//!   receives the call and the surface still gets its `in_tool` back, with the
//!   same call id and the offering app named as the answerer. That positive
//!   control is asserted in every round, because a silent observer and a
//!   swallowed call look alike from the outside.
//! - **Filtered by name.** A call on a tool the observer did not name reaches the
//!   offering app and NOT the observer.
//! - **Counted.** Two surfaces (`talky`, `talky-chat`) each draw one call edge,
//!   but only the calling surface sends — so exactly one `in_probe_call` and one
//!   `in_probe_result` per round.
//! - **A tap, not a taker** (review of #937, Important 2, ruling (b)). A tool of
//!   the generation's own `./tools` travels on the assistant's DEFAULT edge
//!   `./talky -> ./tools`, and the edge table drops a sender's defaults as soon
//!   as one regular edge matches (`edge_table::apply_edges`). Every observer
//!   edge is therefore drawn `"tap": true`, which does not count in that
//!   decision: round four calls `probe_core`, the generation's own tool, and it
//!   both RUNS and is heard. The other direction is pinned too: the same edges
//!   without `tap` take the call away from `./tools` — the documented behaviour
//!   of a regular edge, and the reason the recipe draws taps.
//! - **Its own generation, never itself** (review, Important 1 and Minor 1). The
//!   result producers `./apps` and `./memory-hive` serve every generation of
//!   the member; an observer installed for `gen1` does not hear the result of a
//!   call `gen2` made, and an app that offers and observes the same tool does
//!   not hear its own answer.
//!
//! # What is booted
//!
//! The SHIPPED `member` and `assistant`, with `code` doubles in place of every
//! `ref`, and two app TEMPLATES grown by mutation: `probe-app` offers
//! `probe_timer` and `probe_other` at `./timer`, `probe-observer` hears
//! `probe_timer` at `./ear` and reports what it heard on `error` — the lane a
//! member carries out of the level — through a TEST witness edge that the
//! install mutation carries beside what the recipe rendered.
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
    (member.join("config.json").is_file()
        && assistant.join("config.json").is_file()
        && repo("templates/builder/recipes/config.json").is_file())
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

/// The generation whose surface calls the tool.
const AGENT: &str = "gen1";
/// Where the member stands, so a test can name an absolute owner path.
const MEMBER: &str = "/person";
/// The app that OFFERS the tools. An instance is named after its template.
const OFFERING: &str = "probe-app";
/// The app that OBSERVES one of them.
const OBSERVER: &str = "probe-observer";
/// An observer whose contract names only the re-stamped route (the refusal).
const DEAF: &str = "probe-deaf";
/// The tool the observer names.
const WATCHED: &str = "probe_timer";
/// The tool it does not.
const UNWATCHED: &str = "probe_other";
/// A tool of the generation's OWN `./tools`, reached on the assistant's
/// default edge — the observer watches its calls.
const CORE: &str = "probe_core";
/// The second generation of the member, and the app that offers to it.
const AGENT2: &str = "gen2";
const OFFERING2: &str = "probe-app-two";
/// An app that offers a tool AND observes its results — it must not hear itself.
const SELFISH: &str = "probe-self";
const MIRROR: &str = "probe_mirror";
/// The observer's own words for what it hears.
const CALL_ROUTE: &str = "in_probe_call";
const RESULT_ROUTE: &str = "in_probe_result";
/// `install_app` takes a screen whatever the declaration says; neither probe
/// app declares one, so this literal lands in no edge.
const UNUSED_SCREEN: &str = "display-main";

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

/// The conversation surface of one generation, doubled — the caller.
///
/// `in_wire` is the test's trigger: the surface calls the tool whose NAME the
/// driver put on `hop.lane`, with a call id derived from that name so two
/// rounds can never be confused. `in_tool` is the way back, reported on `error`
/// with the call id the answerer echoed and the answerer the member stamped.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
route = str(hop.get("route") or "")

if route in ("in_wire", "in_wire2"):
    name = str(hop.get("lane") or "")
    sys.stdout.write(json.dumps({
        "header": {"route": "tool", "tool_name": name, "tool_call_id": "c-" + name},
        "messages": [], "arguments": {}}))
elif route == "in_tool":
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "surface_got_in_tool",
                   "got_call_id": str(hop.get("tool_call_id") or ""),
                   "got_answerer": str(ctx.get("tool_answerer") or "")},
        "messages": []}))
else:
    sys.stdout.write(json.dumps([]))
"#;

/// The TOOL SURFACE of the generation, doubled — the second producer of
/// `tool_result`, answering under the WATCHED name so round three can tell
/// whether the observer hears the generation's own tool hive, and answering a
/// real call (round four: `probe_core` on the assistant's default edge, which
/// restamps it `tool_call`).
const TOOLS: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
if str(hop.get("route") or "") == "in_res":
    sys.stdout.write(json.dumps({
        "header": {"route": "tool_result", "tool_name": "probe_timer", "tool_call_id": "t9"},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "t9",
                      "text": "fetched by the tool hive"}]}))
elif str(hop.get("route") or "") == "tool_call":
    name = str(hop.get("tool_name") or "")
    cid = str(hop.get("tool_call_id") or "")
    sys.stdout.write(json.dumps({
        "header": {"route": "tool_result", "tool_name": name, "tool_call_id": cid},
        "messages": [{"origin": "tool", "type": "tool_result", "id": cid,
                      "text": "run by the tool hive"}]}))
else:
    sys.stdout.write(json.dumps([]))
"#;

/// The offering app's one cell: it answers the menu tick with its whole offer
/// and every call with a `tool_result` echoing name and call id.
const TIMER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = (doc["envelope"].get("header") or {}).get("hop") or {}
route = str(hop.get("route") or "")
if route == "schemas":
    sys.stdout.write(json.dumps({"header": {"route": "tool_schemas", "operation": "schemas",
        "schema_count": 2, "unknown_count": 0},
        "schemas": [{"name": n, "description": "a probe tool",
                     "parameters": {"type": "object", "properties": {}}}
                    for n in ("probe_timer", "probe_other")],
        "unknown": [], "messages": []}))
elif route == "tool":
    name = str(hop.get("tool_name") or "")
    cid = str(hop.get("tool_call_id") or "")
    sys.stdout.write(json.dumps({"header": {"route": "tool_result", "operation": name,
        "tool_name": name, "tool_call_id": cid},
        "messages": [{"origin": "tool", "type": "tool_result", "id": cid, "text": "ticked"}]}))
else:
    sys.stdout.write(json.dumps([]))
"#;

/// The observer's ear: everything it hears is reported on `error`, with the
/// route it arrived on, the tool name, the answerer the context carries and
/// WHICH app heard it (`__WHO__`, replaced per app by [`ear`]).
const EAR: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hdr = doc["envelope"].get("header") or {}
hop = hdr.get("hop") or {}
ctx = hdr.get("context") or {}
route = str(hop.get("route") or "")
if route in ("in_probe_call", "in_probe_result"):
    sys.stdout.write(json.dumps({
        "header": {"route": "error", "error_code": "observer_heard",
                   "heard_route": route,
                   "heard_tool": str(hop.get("tool_name") or ""),
                   "heard_answerer": str(ctx.get("tool_answerer") or ""),
                   "heard_by": "__WHO__"},
        "messages": []}))
else:
    sys.stdout.write(json.dumps([]))
"#;

fn ear(who: &str) -> String {
    EAR.replace("__WHO__", who)
}

/// Puts one message on a named lane. `hop.mode` picks the door, `hop.lane`
/// carries the tool name for the surface.
const DRIVER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
mode = str(hop.get("mode") or "tool")
route = {"tool": "in_wire", "tool2": "in_wire2", "toolres": "in_res"}.get(mode, "in_wire")
sys.stdout.write(json.dumps({
    "header": {"route": route, "mode": mode, "lane": str(hop.get("lane") or "")},
    "messages": doc["body"].get("messages", [])}))
"#;

/// A `code` double with a fixed script. `emits` is left wide on purpose: what a
/// double may say is decided by the assertions, not by a contract nobody reads.
/// `multi_send_capable` is on so a double may answer with NOTHING.
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

// ═════════════════════════════════════════════════════════════ the app templates

/// One app template under `templates/<name>`: a hive with `ports: []`, its own
/// contract and inner graph, and its code cells.
fn write_app_template(
    root: &std::path::Path,
    name: &str,
    contract: Value,
    inner: Vec<Value>,
    cells: &[(&str, &str)],
) {
    write(
        root,
        &format!("templates/{name}/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {
                "ports": [],
                "contract": contract,
                "graph": {"edges": inner}
            },
            "description": {
                "purpose": "Fixture app for GH #937.",
                "use_when": "Test fixture only.",
                "not_in_scope": "Not a shipped template."
            }
        }),
    );
    for (cell, script) in cells {
        write(
            root,
            &format!("templates/{name}/{cell}/config.json"),
            &double(script, "Fixture app cell for GH #937."),
        );
    }
    write(
        root,
        &format!("templates/{name}/template.json"),
        &json!({"name": name, "version": "1.0.0", "tags": ["app"], "author": "meclaw"}),
    );
}

fn offering_contract() -> Value {
    json!({
        "accepts": [
            {"route": "tool", "at": ["./timer"], "because": "a call on one of the two probe tools, straight from the surface that made it (ADR-0020)"},
            {"route": "schemas", "at": ["./timer"], "because": "the menu tick, at the cell that answers the tools"}
        ],
        "emits": [
            {"route": "tool_result", "because": "the answer to a call, carrying back its tool_call_id"},
            {"route": "tool_schemas", "because": "the whole offer of this app"},
            {"route": "error", "because": "what this app could not do"}
        ]
    })
}

fn timer_exit() -> Value {
    json!({
        "from": "./timer", "to": ".",
        "condition": "has(hop.route) && (hop.route == 'tool_result' || hop.route == 'tool_schemas' || hop.route == 'error')"
    })
}

fn ear_exit() -> Value {
    json!({
        "from": "./ear", "to": ".",
        "condition": "has(hop.route) && hop.route == 'error'"
    })
}

fn write_templates(root: &std::path::Path) {
    for name in [OFFERING, OFFERING2] {
        write_app_template(
            root,
            name,
            offering_contract(),
            vec![timer_exit()],
            &[("timer", TIMER)],
        );
    }
    // Offers `probe_mirror` at `./timer` and observes its results at `./ear`.
    let selfish_ear = ear(SELFISH);
    write_app_template(
        root,
        SELFISH,
        json!({
            "accepts": [
                {"route": "tool", "at": ["./timer"], "because": "a call on the mirror tool"},
                {"route": "schemas", "at": ["./timer"], "because": "the menu tick"},
                {"route": "tool_result", "at": ["./ear"], "because": "the results of the mirror tool, observed -- never its own"}
            ],
            "emits": [
                {"route": "tool_result", "because": "the answer to a call"},
                {"route": "tool_schemas", "because": "the whole offer of this app"},
                {"route": "error", "because": "what this app could not do, and what its ear heard"}
            ]
        }),
        vec![timer_exit(), ear_exit()],
        &[("timer", TIMER), ("ear", &selfish_ear)],
    );
    let observer_ear = ear(OBSERVER);
    write_app_template(
        root,
        OBSERVER,
        json!({
            "accepts": [
                {"route": "tool", "at": ["./ear"], "because": "a call on a tool another app offers, observed as a v-lane fan-out; the substrate judges the LANE, so this is declared under `tool` although the edge re-stamps it to in_probe_call"},
                {"route": "tool_result", "at": ["./ear"], "because": "the result of that call, observed as a v-lane fan-out and re-stamped to in_probe_result"}
            ],
            "emits": [
                {"route": "error", "because": "the observer's report of what it heard -- the one lane a member carries out of the level"}
            ]
        }),
        vec![ear_exit()],
        &[("ear", &observer_ear)],
    );
    // The refusal case: the contract names the re-stamped ROUTE at the rim and
    // never the lane `tool` at `./ear`.
    write_app_template(
        root,
        DEAF,
        json!({
            "accepts": [
                {"route": "in_probe_call", "because": "the observed call, named by the route the edge stamps -- which is not what the v-lane check reads"}
            ],
            "emits": [
                {"route": "error", "because": "the observer's report"}
            ]
        }),
        vec![
            json!({
                "from": ".", "to": "./ear",
                "condition": "has(hop.route) && hop.route == 'in_probe_call'"
            }),
            ear_exit(),
        ],
        &[("ear", &ear(DEAF))],
    );
}

// ══════════════════════════════════════ the wiring an installing mutation draws

fn offering_declaration() -> Value {
    json!({"offers": [{"kind": "tool", "at": "./timer", "tools": [WATCHED, UNWATCHED]}]})
}

fn observer_declaration() -> Value {
    json!({
        "observes_tool_calls": {"at": "./ear", "tools": [WATCHED, CORE], "route": CALL_ROUTE},
        "observes_tool_results": {"at": "./ear", "tools": [WATCHED], "route": RESULT_ROUTE}
    })
}

fn offering2_declaration() -> Value {
    json!({"offers": [{"kind": "tool", "at": "./timer", "tools": [WATCHED]}]})
}

fn selfish_declaration() -> Value {
    json!({
        "offers": [{"kind": "tool", "at": "./timer", "tools": [MIRROR]}],
        "observes_tool_results": {"at": "./ear", "tools": [MIRROR], "route": RESULT_ROUTE}
    })
}

fn deaf_declaration() -> Value {
    json!({"observes_tool_calls": {"at": "./ear", "tools": [WATCHED], "route": CALL_ROUTE}})
}

/// **The install diff, as the builder renders it** — `install_app` over the
/// declaration, read out of `manifest[0].diff` unchanged (nodes and edges).
fn rendered_diff(app: &str, generation: &str, declaration: Value) -> Value {
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("a utf-8 path"),
        ),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": {"scope": MEMBER, "app": app,
                                                    "template": format!("{app}@1.0.0"),
                                                    "screen": UNUSED_SCREEN,
                                                    "generation": generation,
                                                    "declaration": declaration}})
                                      .to_string()}],
        }),
    );
    let first = out.first().expect("the recipe emitted nothing");
    assert!(
        first["header"]["error_code"].is_null(),
        "the recipe refused the declaration of {app}: {first}"
    );
    let diff = first["manifest"][0]["diff"].clone();
    assert!(
        diff["add_edges"].is_array() && diff["add_nodes"].is_array(),
        "no nodes or edges in the rendered install of {app}: {first}"
    );
    diff
}

/// The TEST's witness edge: the observer's report leaves the app onto the
/// container, where the member's own `./apps -> .` carries `error` out of the
/// level. No production app needs it — an observer acts on what it hears.
fn witness_edge(app: &str) -> Value {
    json!({
        "from": format!("./apps/{app}"), "to": "./apps",
        "condition": "has(hop.route) && hop.route == 'error'"
    })
}

/// The edges one assistant costs, as in `apps_rim_an_app_hears_offers_and_draws.rs`:
/// the addressing edge, the two ways back in, and one exit.
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
            "from": "./driver", "to": format!("./person/assistants/{AGENT}/tools"),
            "condition": "has(hop.route) && hop.route == 'in_res'"
        }),
        json!({
            "from": "./driver", "to": format!("./person/assistants/{AGENT2}/talky"),
            "condition": "has(hop.route) && hop.route == 'in_wire2'"
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
    for holder in [
        "access",
        "affinity",
        "memory-hive",
        "file-space",
        "graph-space",
        "librarian",
        "objects",
    ] {
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
    write_templates(root);
    for name in [AGENT, AGENT2] {
        plant_generation(root, assistant, name);
    }

    let cfg_path = root.join("main/person/config.json");
    let mut cfg = read_json(&cfg_path);
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the member ships a graph");
    edges.extend(assistant_edges(AGENT));
    edges.extend(assistant_edges(AGENT2));
    std::fs::write(
        &cfg_path,
        meclaw_core::serde_json::to_string_pretty(&cfg).expect("serialise"),
    )
    .expect("write the member config");

    std::fs::write(root.join(".env"), "").expect("write an empty .env");
}

/// One generation of the shipped assistant, its four cells doubled.
fn plant_generation(root: &std::path::Path, assistant: &std::path::Path, name: &str) {
    let dst = root.join(format!("main/person/assistants/{name}"));
    copy_cells(assistant, &dst);
    write(
        root,
        &format!("main/person/assistants/{name}/talky/config.json"),
        &double(
            SURFACE,
            "Test double for the conversation surface of one generation.",
        ),
    );
    write(
        root,
        // A second surface: the recipe draws a call edge from it too, and it
        // never calls -- which is what makes the per-round count exactly one.
        &format!("main/person/assistants/{name}/talky-chat/config.json"),
        &double(
            INERT,
            "Inert double for the typed surface, which never calls here.",
        ),
    );
    write(
        root,
        &format!("main/person/assistants/{name}/tools/config.json"),
        &double(TOOLS, "Test double for the tool surface of one generation."),
    );
    write(
        root,
        &format!("main/person/assistants/{name}/cogny/config.json"),
        &double(
            INERT,
            "Inert double for the reasoning core, which no round here reaches.",
        ),
    );
}

// ═════════════════════════════════════════════════════════════════ the colony

struct Colony {
    td: tempfile::TempDir,
    h: ColonyHandle,
    rx: mpsc::Receiver<Message>,
}

/// Boot the member with NO app installed; the apps come by mutation.
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

/// Install one app for `gen1`: the rendered diff, plus the witness edge where
/// asked.
async fn install(
    h: &ColonyHandle,
    app: &str,
    declaration: Value,
    witness: bool,
) -> MutationOutcome {
    install_as(h, app, AGENT, declaration, witness, false).await
}

/// Install one app for `generation`; `untap` strips `"tap"` off every rendered
/// edge — the regular observer edge the recipe no longer draws, kept to pin
/// what it would do.
async fn install_as(
    h: &ColonyHandle,
    app: &str,
    generation: &str,
    declaration: Value,
    witness: bool,
    untap: bool,
) -> MutationOutcome {
    let mut diff = rendered_diff(app, generation, declaration);
    if untap {
        for e in diff["add_edges"]
            .as_array_mut()
            .expect("checked in rendered_diff")
        {
            if let Some(o) = e.as_object_mut() {
                o.remove("tap");
            }
        }
    }
    if witness {
        diff["add_edges"]
            .as_array_mut()
            .expect("checked in rendered_diff")
            .push(witness_edge(app));
    }
    send_mutation(h, json!({"scope": MEMBER, "diff": diff})).await
}

/// Which app goes in first.
#[derive(Clone, Copy, Debug)]
enum Order {
    OfferingFirst,
    ObserverFirst,
}

/// Boot, then install both apps in `order`; each installation must COMMIT —
/// no `edge_schema`, no `contract_locality`, no v-lane refusal.
async fn start_with(order: Order) -> Colony {
    start_with_taps(order, true).await
}

async fn start_with_taps(order: Order, taps: bool) -> Colony {
    let c = start().await;
    let steps: [(&str, Value, bool); 2] = match order {
        Order::OfferingFirst => [
            (OFFERING, offering_declaration(), false),
            (OBSERVER, observer_declaration(), true),
        ],
        Order::ObserverFirst => [
            (OBSERVER, observer_declaration(), true),
            (OFFERING, offering_declaration(), false),
        ],
    };
    for (app, declaration, witness) in steps {
        let outcome = install_as(&c.h, app, AGENT, declaration, witness, !taps).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "installing {app} ({order:?}) is one ordinary mutation: {outcome:?}"
        );
    }
    c
}

fn inject(mode: &str, lane: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("mode".into(), json!(mode));
    hop.insert("lane".into(), json!(lane));
    MessageBuilder::new(Path::new("/driver"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "a probe round"}
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

/// Collect what the round put on the sink: `quiet` is the semantic window after
/// the first message, `budget` the 30-second failure marker.
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

async fn round(c: &mut Colony, mode: &str, lane: &str) -> Vec<Message> {
    c.h.send(inject(mode, lane)).await;
    gather(&mut c.rx, Duration::from_secs(3), Duration::from_secs(30)).await
}

/// What the OBSERVER reported: `(heard_route, heard_tool, heard_answerer)`.
fn heard(got: &[Message]) -> Vec<(String, String, String)> {
    heard_by(got, OBSERVER)
}

/// What the app `who` reported hearing.
fn heard_by(got: &[Message], who: &str) -> Vec<(String, String, String)> {
    got.iter()
        .filter(|m| hop_of(m, "error_code") == "observer_heard" && hop_of(m, "heard_by") == who)
        .map(|m| {
            (
                hop_of(m, "heard_route"),
                hop_of(m, "heard_tool"),
                hop_of(m, "heard_answerer"),
            )
        })
        .collect()
}

/// The positive control of every call round: the offering app answered, and
/// the surface got the result back paired and attributed.
fn assert_answered(got: &[Message], tool: &str) {
    assert_answered_by(got, tool, OFFERING);
}

/// The surface got its call on `tool` back, answered by `answerer` (`""` for
/// the generation's own `./tools`, which stamps no answerer).
fn assert_answered_by(got: &[Message], tool: &str, answerer: &str) {
    let back = got
        .iter()
        .find(|m| {
            hop_of(m, "error_code") == "surface_got_in_tool"
                && hop_of(m, "got_call_id") == format!("c-{tool}")
        })
        .unwrap_or_else(|| {
            panic!("the call on {tool} has to reach the offering app and come back: {got:#?}")
        });
    assert_eq!(
        hop_of(back, "got_answerer"),
        answerer,
        "observation is a fan-out: the one that offers {tool} still answers it"
    );
}

/// Round one, shared by both orders: the watched call and its result are heard
/// exactly once each, and the call is still answered.
async fn the_watched_round(c: &mut Colony) {
    let got = round(c, "tool", WATCHED).await;
    assert_answered(&got, WATCHED);
    let heard = heard(&got);
    let calls: Vec<_> = heard.iter().filter(|(r, _, _)| r == CALL_ROUTE).collect();
    assert_eq!(
        calls.len(),
        1,
        "exactly one observed call -- two surfaces draw an edge, one calls: {got:#?}"
    );
    assert_eq!(calls[0].1, WATCHED, "the observed call names its tool");
    let results: Vec<_> = heard.iter().filter(|(r, _, _)| r == RESULT_ROUTE).collect();
    assert_eq!(
        results.len(),
        1,
        "exactly one observed result, out of the container `./apps`: {got:#?}"
    );
    assert_eq!(results[0].1, WATCHED, "the observed result names its tool");
    assert_eq!(
        results[0].2, OFFERING,
        "and carries the answerer the offering app's binding edge stamped"
    );
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

/// **Observer installed after the offering app.** The edges stand, named as
/// the lanes they carry; the watched call and its result are heard once each;
/// an unwatched call is answered and NOT heard; and a result of the
/// generation's own `./tools` under the watched name is heard too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_observes_another_apps_tool_installed_after_it() {
    if skip() {
        return;
    }
    let mut c = start_with(Order::OfferingFirst).await;

    // The edges are really there: a silence caused by a refused edge looks
    // exactly like a silence caused by a wrong guard.
    let edges = read_graph(&c.h).await.edges;
    let ear = format!("{MEMBER}/apps/{OBSERVER}/ear");
    for (from, lane) in [
        (format!("{MEMBER}/assistants/{AGENT}/talky"), "tool"),
        (format!("{MEMBER}/assistants/{AGENT}/talky-chat"), "tool"),
        (format!("{MEMBER}/assistants/{AGENT}/tools"), "tool_result"),
        (format!("{MEMBER}/memory-hive"), "tool_result"),
        (format!("{MEMBER}/apps"), "tool_result"),
    ] {
        assert!(
            edges
                .iter()
                .any(|e| e.from == from && e.to == ear && e.lane.as_deref() == Some(lane) && e.tap),
            "the observer TAP {from} -> {ear} on lane '{lane}' has to stand: {edges:#?}"
        );
    }

    // 1. The watched tool.
    the_watched_round(&mut c).await;

    // 2. The unwatched tool: answered, and the observer hears nothing.
    let got = round(&mut c, "tool", UNWATCHED).await;
    assert_answered(&got, UNWATCHED);
    assert!(
        heard(&got).is_empty(),
        "a tool the observer did not name must not reach it, neither call nor result: {got:#?}"
    );

    // 3. A result of the generation's own tool hive under the watched name.
    let got = round(&mut c, "toolres", "").await;
    let heard_now = heard(&got);
    assert_eq!(
        heard_now
            .iter()
            .filter(|(r, t, _)| r == RESULT_ROUTE && t == WATCHED)
            .count(),
        1,
        "the tool hive's result reaches the observer once, on the edge out of \
         ./assistants/{AGENT}/tools: {got:#?}"
    );
    assert!(
        !heard_now.iter().any(|(r, _, _)| r == CALL_ROUTE),
        "and no call was made in this round: {heard_now:?}"
    );

    // 4. A call on the generation's OWN tool: the assistant's default edge
    //    `./talky -> ./tools` still carries it -- the tap does not count when
    //    the edge table drops a sender's defaults -- and the observer hears it.
    let got = round(&mut c, "tool", CORE).await;
    assert_answered_by(&got, CORE, "");
    let calls: Vec<_> = heard(&got)
        .into_iter()
        .filter(|(r, t, _)| r == CALL_ROUTE && t == CORE)
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "the observer hears the call on the generation's own tool once: {got:#?}"
    );

    c.h.shutdown().await;
    drop(c.td);
}

/// **Without `tap` the observer TAKES the generation's own tool** — the pinned
/// other direction of ruling (b). The same rendered edges with `"tap"` removed
/// are regular; one of them matches the call on `probe_core`, the edge table
/// drops the sender's defaults, and `./tools` never sees the call. That is the
/// documented behaviour of a regular edge (`edge_table::apply_edges`), and the
/// reason the recipe draws every observer edge as a tap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_tap_the_observer_takes_the_generations_own_tool() {
    if skip() {
        return;
    }
    let mut c = start_with_taps(Order::OfferingFirst, false).await;
    let got = round(&mut c, "tool", CORE).await;
    assert_eq!(
        heard(&got)
            .iter()
            .filter(|(r, t, _)| r == CALL_ROUTE && t == CORE)
            .count(),
        1,
        "the regular observer edge carries the call -- the round ran: {got:#?}"
    );
    assert!(
        !got.iter()
            .any(|m| hop_of(m, "error_code") == "surface_got_in_tool"),
        "and the default `./talky -> ./tools` was dropped: the tool never answered: {got:#?}"
    );
    // An app tool is unaffected either way: its offer edge is regular too.
    the_watched_round(&mut c).await;
    c.h.shutdown().await;
    drop(c.td);
}

/// **Its own generation, and never itself.** `probe-app-two` offers the
/// watched tool to `gen2`; `gen2` calls it, the result enters `./apps` like
/// any other — and the observer, installed for `gen1`, does not hear it. And
/// `probe-self` offers `probe_mirror` and observes its results: its answer
/// reaches the surface and not its own ear.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observer_hears_its_own_generation_and_never_itself() {
    if skip() {
        return;
    }
    let mut c = start_with(Order::OfferingFirst).await;
    for (app, generation, declaration, witness) in [
        (OFFERING2, AGENT2, offering2_declaration(), false),
        (SELFISH, AGENT, selfish_declaration(), true),
    ] {
        let outcome = install_as(&c.h, app, generation, declaration, witness, false).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "installing {app} for {generation} is one ordinary mutation: {outcome:?}"
        );
    }

    // gen2's call on the watched tool: answered by the app that offers to gen2,
    // never heard by an observer of gen1.
    let got = round(&mut c, "tool2", WATCHED).await;
    assert_answered_by(&got, WATCHED, OFFERING2);
    assert!(
        heard(&got).is_empty(),
        "a result of another generation's round must not reach the observer: {got:#?}"
    );

    // An app that offers and observes the same tool.
    let got = round(&mut c, "tool", MIRROR).await;
    assert_answered_by(&got, MIRROR, SELFISH);
    assert!(
        heard_by(&got, SELFISH).is_empty(),
        "an app does not hear its own answer: {got:#?}"
    );

    // The positive control in the same colony: gen1's watched round is heard.
    the_watched_round(&mut c).await;
    c.h.shutdown().await;
    drop(c.td);
}

/// **Observer installed BEFORE the offering app.** The result edge starts at
/// the container, so it needs nothing of an app that is not there yet; both
/// installations commit, and the watched round is heard exactly as in the
/// other order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_observes_another_apps_tool_installed_before_it() {
    if skip() {
        return;
    }
    let mut c = start_with(Order::ObserverFirst).await;
    the_watched_round(&mut c).await;
    c.h.shutdown().await;
    drop(c.td);
}

/// **An observer whose contract names the route instead of the lane is
/// refused.** The v-lane check reads the LANE (`tool`) against the target's
/// connect points; a contract that only accepts the re-stamped `in_probe_call`
/// names none, and the mutation door says `v_lane_no_connect_point`. The
/// recipe renders the edges regardless — it reads no contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observer_without_the_lane_at_its_ear_is_refused() {
    if skip() {
        return;
    }
    let c = start().await;
    let outcome = install(&c.h, DEAF, deaf_declaration(), true).await;
    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            violations,
            ..
        } => assert!(
            error_code == "v_lane_no_connect_point"
                || violations
                    .iter()
                    .any(|v| v.code == "v_lane_no_connect_point"),
            "the refusal has to name the missing connect point: {outcome:?}"
        ),
        MutationOutcome::Committed { .. } => {
            panic!(
                "an observer that declares no connect point for `tool` must be refused: {outcome:?}"
            )
        }
    }
    c.h.shutdown().await;
    drop(c.td);
}
