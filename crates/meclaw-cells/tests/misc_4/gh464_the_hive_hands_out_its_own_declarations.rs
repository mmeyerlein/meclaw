//! GH #464 — the tools hive hands out its own declarations, and the catalogue's
//! tool cells stand in it.
//!
//! Until `tools@1.2.0` this hive routed a `tool_call` by name and answered
//! `tool_result`, and said nowhere what there was to call. The schemas sat in
//! the callers' prompts, one copy per caller, typed by hand — so adding a tool
//! by mutation meant editing every one of those prompts, and a caller could only
//! ever offer a model a list somebody had written out.
//!
//! The ruling: the template is the contract. A cell that uses tools declares
//! them; at start-up it asks the hive for exactly those names, and the hive
//! answers with the schemas it has under them and nothing else. The hive keeps
//! no table of who asks.
//!
//! What this file holds:
//!
//! 1. **The cell.** The shipped script is run through `python3` exactly as the
//!    substrate runs it: two names give two schemas, `*` gives all of them, an
//!    unknown name comes back NAMED, and a request with no `tools` list at all
//!    is a third state with a code of its own.
//! 2. **The drift lock** (development-rules § 2d). The table lives in the cell's
//!    own script and the doors live in the hive's graph; neither is derived from
//!    the other, so a test walks both and requires them to agree in BOTH
//!    directions. The README sentence that promises it is grepped in the same
//!    test, because a grep alone pins a string and an assertion alone lets the
//!    prose drift away from the mechanism.
//! 3. **No unwired occupant.** Since `tools@1.3.0` every directory in this hive
//!    is reached by a name (GH #547). Until then `mcp` and `vault` stood here
//!    with no edge, and this section asserted the ISLANDS; it now asserts the
//!    reach, which is the same discipline pointed the other way — "no door" and
//!    "somebody forgot the door" look identical in a diff, so one of the two
//!    has to be written down.
//! 4. **Reachability.** Every occupant a name edge points at is reached by that
//!    name on a booted colony, and the guarded default stays silent while it
//!    happens.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::{EditCellFactory, FileCellFactory};
use meclaw_colony::config::HiveParams;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{emit_one, resolve_script_vars, shipped_script};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const SCHEMAS_CELL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/tools/schemas/config.json"
);
const TOOLS_README: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/tools/README.md"
);

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn tools_config() -> Value {
    let raw = std::fs::read_to_string(templates_root().join("tools/config.json"))
        .expect("templates/tools ships");
    meclaw_core::serde_json::from_str(&raw).expect("tools config parses")
}

fn hive_params() -> HiveParams {
    meclaw_core::serde_json::from_value(tools_config()["params"].clone())
        .expect("the shipped params parse through the real HiveParams")
}

/// Ask the SHIPPED script, the way the substrate asks it. The request is a body
/// slot, so it is spelled flat here exactly as `code_stdin` expects.
fn ask(tools: Value) -> Value {
    emit_one(
        &shipped_script(SCHEMAS_CELL),
        &json!({
            "target": "/main/tools/schemas",
            "header": {"hop": {"route": "in_schemas"}, "context": {}},
            "ttl": 64,
            "tools": tools,
            "messages": [],
        }),
    )
}

/// Ask with a body that has no `tools` slot at all.
fn ask_without_the_slot() -> Value {
    emit_one(
        &shipped_script(SCHEMAS_CELL),
        &json!({
            "target": "/main/tools/schemas",
            "header": {"hop": {"route": "in_schemas"}, "context": {}},
            "ttl": 64,
            "messages": [],
        }),
    )
}

fn names_of(answer: &Value) -> Vec<String> {
    answer["schemas"]
        .as_array()
        .expect("schemas is an array")
        .iter()
        .map(|s| {
            s["name"]
                .as_str()
                .expect("every schema is named")
                .to_string()
        })
        .collect()
}

// ══════════════════════════════════════════════════════════════════ 1. the cell

#[test]
fn two_names_come_back_as_two_schemas_and_nothing_else() {
    let a = ask(json!(["web_search", "bash"]));
    assert_eq!(
        names_of(&a),
        vec!["web_search", "bash"],
        "the hive answered with the schemas it has under the names it was handed, in the \
         order it was handed them — and with nothing else: a caller that declared two tools \
         must not be given a third one to offer its model. {a}"
    );
    assert_eq!(
        a["unknown"],
        json!([]),
        "both names exist, so nothing is unknown: {a}"
    );
    assert_eq!(a["header"]["operation"], json!("schemas"));
    assert_eq!(a["header"]["schema_count"], json!(2));
    assert_eq!(
        a["header"].get("error_code"),
        None,
        "an answer that found everything it was asked for carries no error code: {a}"
    );
    for s in a["schemas"].as_array().unwrap() {
        assert!(
            s["description"]
                .as_str()
                .is_some_and(|d| !d.trim().is_empty()),
            "a declaration with no description is a name a model has to guess at: {s}"
        );
        assert_eq!(
            s["parameters"]["type"],
            json!("object"),
            "the parameters of a tool call are a JSON-Schema object: {s}"
        );
        for key in s.as_object().unwrap().keys() {
            assert!(
                ["name", "description", "parameters"].contains(&key.as_str()),
                "a schema carries {key:?}, which is not part of the shape this lane \
                 promises. Wrapping it into a provider's own envelope is the CALLER's \
                 job — the hive does not know which provider its caller talks to."
            );
        }
    }
}

#[test]
fn a_star_asks_for_everything_the_hive_has() {
    let all = ask(json!(["*"]));
    let names = names_of(&all);
    assert!(
        names.len() >= 5,
        "`*` answered with almost nothing ({names:?}) — the table broke, not the tree"
    );
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        names, sorted,
        "`*` answers in a stable order, so two callers asking the same question get the \
         same menu: {names:?}"
    );
    assert_eq!(all["header"]["schema_count"], json!(names.len()));
    assert_eq!(all["unknown"], json!([]));
}

#[test]
fn a_name_the_hive_does_not_have_comes_back_named() {
    let a = ask(json!(["web_search", "telepathy"]));
    assert_eq!(
        names_of(&a),
        vec!["web_search"],
        "a partial answer is still an answer — the schema that WAS found travels: {a}"
    );
    assert_eq!(
        a["unknown"],
        json!(["telepathy"]),
        "the name the hive does not have is reported, not dropped. A menu that is silently \
         one tool short is a model that never calls it and an author who never learns why: \
         {a}"
    );
    assert_eq!(
        a["header"]["error_code"],
        json!("tool_unknown"),
        "and the caller reads the refusal off the message, exactly as it reads a tool's \
         own: {a}"
    );
}

#[test]
fn a_request_with_no_tools_list_is_a_different_refusal_from_an_empty_one() {
    let empty = ask(json!([]));
    assert_eq!(
        names_of(&empty),
        Vec::<String>::new(),
        "a caller that declares no tools is entitled to an empty menu: {empty}"
    );
    assert_eq!(
        empty["header"].get("error_code"),
        None,
        "an empty declaration is an ANSWER, not a fault: {empty}"
    );

    let missing = ask_without_the_slot();
    assert_eq!(
        missing["header"]["error_code"],
        json!("tools_missing"),
        "a request that carried no `tools` slot at all lost it on the way, and saying so is \
         cheaper than handing back a menu nobody asked for. An empty result and a forgotten \
         call must never look alike: {missing}"
    );
}

// ═══════════════════════════════════════════════════════ 2. the drift lock

/// Every tool name a door dispatches on, read off the shipped graph.
fn dispatched_names() -> BTreeSet<String> {
    let marker = "hop.tool_name == '";
    hive_params()
        .graph
        .edges
        .iter()
        .filter(|e| e.from == "." && !e.is_default)
        .filter_map(|e| {
            let c = e.condition.as_deref()?;
            let at = c.find(marker)? + marker.len();
            let rest = &c[at..];
            Some(rest[..rest.find('\'')?].to_string())
        })
        .collect()
}

/// GH #464, development-rules § 2d: the README promises that the table and the
/// doors cannot drift apart, and this is the test that makes the promise a
/// mechanism. Both halves in one test on purpose — grepping the sentence alone
/// pins a string, asserting the mechanism alone lets the prose walk away from it.
#[test]
fn every_dispatched_tool_has_a_schema_and_every_schema_has_a_door() {
    let readme = std::fs::read_to_string(TOOLS_README).expect("tools README");
    for promise in [
        "The two halves cannot drift apart in silence: a test walks the dispatch graph and requires",
        "that every tool a door names has a row, and that every row names a tool a door dispatches",
    ] {
        assert!(
            readme.contains(promise),
            "the README no longer carries the sentence this test makes true ({promise:?}) — \
             either it moved or the promise was quietly dropped, and both are the § 2d defect"
        );
    }

    let doors = dispatched_names();
    let table: BTreeSet<String> = names_of(&ask(json!(["*"]))).into_iter().collect();

    let unnamed: Vec<&String> = doors.difference(&table).collect();
    assert!(
        unnamed.is_empty(),
        "these tools have a door and no declaration: {unnamed:?}. A model is never told they \
         exist, so the door is one nobody can walk through — which is the exact state GH \
         #464 ended, one tool later."
    );
    let doorless: Vec<&String> = table.difference(&doors).collect();
    assert!(
        doorless.is_empty(),
        "these declarations name a tool no door dispatches to: {doorless:?}. Handing a model \
         a schema for a call that reaches nothing is worse than not offering it: the round \
         ends in a dead letter instead of in a refusal it could read."
    );
    assert!(
        doors.len() >= 5,
        "the sweep found almost no door ({doors:?}) — the condition grammar changed, the \
         tree did not"
    );
}

/// The one `false` in the reentrancy block, held to the cap that implements it.
/// The number is derived from the shipped `config.json` inside the test rather
/// than repeated here, which is what § 2d asks of a number in template prose.
#[test]
fn the_editor_declares_the_serialisation_it_actually_ships() {
    let raw = std::fs::read_to_string(templates_root().join("tools/edit/config.json"))
        .expect("templates/tools/edit ships");
    let edit: Value = meclaw_core::serde_json::from_str(&raw).expect("edit config parses");
    let cap = edit["params"]["max_concurrency"]
        .as_u64()
        .expect("templates/tools/edit declares params.max_concurrency");
    assert_eq!(
        cap, 1,
        "the editor is capped at {cap}. An edit is a read-modify-write with no lock and no \
         tempfile+rename, so two edits of one path race at the filesystem — a hazard the \
         CALLER cannot see. This occupant serialises rather than asking it to."
    );

    let traw = std::fs::read_to_string(templates_root().join("tools/template.json"))
        .expect("tools template.json");
    let t: Value = meclaw_core::serde_json::from_str(&traw).expect("template.json parses");
    assert_eq!(
        t["reentrancy"]["edit"]["reentrant"],
        json!(false),
        "the cap says the calls are serialised and the declaration says they are not — a \
         caller plans its parallel round against the declaration, so the two disagreeing is \
         precisely the surprise GH #286's block exists to prevent"
    );

    let readme = std::fs::read_to_string(TOOLS_README).expect("tools README");
    assert!(
        readme.contains("| `edit` | **no** |"),
        "the reentrancy table in the README no longer carries the `edit` verdict"
    );
}

/// The shipped params of every occupant #464 added, read by the SUBSTRATE's own
/// factory. A params block a factory refuses is a boot failure, not a
/// documentation defect — and the colony test below cannot catch it, because
/// there the two filesystem cells stand in as `code` doubles.
///
/// `base_path` is the one that has to be spent rather than merely parsed: it must
/// name a directory that EXISTS. Since `tools@1.4.1` it is the literal `/tmp`
/// rather than a `${TOOLS_FILE_ROOT:-/tmp}` token (GH #138), so the resolution
/// below is a no-op for it and the factory is handed the shipped value as it
/// stands -- which is the stronger form of the same claim.
#[test]
fn the_substrate_accepts_the_params_of_every_occupant_that_joined() {
    for (dir, factory) in [
        ("file", Arc::new(FileCellFactory) as Arc<dyn CellFactory>),
        ("edit", Arc::new(EditCellFactory) as Arc<dyn CellFactory>),
    ] {
        let raw =
            std::fs::read_to_string(templates_root().join(format!("tools/{dir}/config.json")))
                .unwrap_or_else(|e| panic!("templates/tools/{dir}: {e}"));
        let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("config parses");
        let params: Value = meclaw_core::serde_json::from_str(&resolve_script_vars(
            &meclaw_core::serde_json::to_string(&cfg["params"]).expect("params re-serialise"),
        ))
        .expect("the resolved params are still JSON");
        factory.validate_params(&params).unwrap_or_else(|e| {
            panic!(
                "the {dir} factory refuses templates/tools/{dir}'s shipped params: {e}. \
                 This is the substrate's own reader, so this is a boot that does not \
                 happen — not a note somebody can fix later."
            )
        });
    }
}

// ══════════════════════════════════════════════ 3. no unwired occupant

/// Since `tools@1.3.0` no occupant of this hive is an island (GH #547).
///
/// Until then `mcp` and `vault` stood here as directories no edge touched, and
/// this test asserted exactly that — it WAS the exemption, written down. The
/// two left with the retraction in the README: a cell type is not a tool, and
/// an occupant nobody can reach is a placeholder with documentation attached.
/// So the assertion turns around and the discipline stays: "there is no door"
/// and "somebody forgot the door" are the same diff, and one of the two has to
/// be a statement somebody can read.
#[test]
fn the_tools_hive_has_no_unwired_occupant() {
    let params = hive_params();
    let touched: BTreeSet<String> = params
        .graph
        .edges
        .iter()
        .flat_map(|e| [e.from.clone(), e.to.clone()])
        .filter(|n| n != ".")
        .collect();

    let mut occupants: Vec<String> = std::fs::read_dir(templates_root().join("tools"))
        .expect("templates/tools is readable")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    occupants.sort();
    assert!(
        occupants.len() >= 5,
        "templates/tools has almost no occupant directories ({occupants:?}) — the walk \
         broke, the tree did not"
    );

    let islands: Vec<&String> = occupants
        .iter()
        .filter(|d| !touched.contains(&format!("./{d}")))
        .collect();
    assert!(
        islands.is_empty(),
        "these occupants of templates/tools are reached by no edge: {islands:?}. Activity \
         in this substrate is derived from the edges alone, so an occupant with no door is \
         a cell the boot registers and never spawns — and since 1.3.0 this hive ships none \
         (GH #547). A cell type is not a tool: the tools hive holds the cells this agent's \
         tool calls REACH. A `vault` answers its broker and stands in the broker's own \
         hive; an `mcp` bridges one named server and is wired the day somebody names it. \
         If a new occupant belongs here, it belongs here WITH its two edges and its schema \
         row, in the same diff."
    );

    for name in ["mcp", "vault"] {
        assert!(
            !templates_root().join(format!("tools/{name}")).exists(),
            "templates/tools/{name} is back. It left with GH #547 for a reason that did not \
             expire: it is a cell type, not a tool of this hive. Wiring it here needs the \
             decision the template cannot make — a server to name, a broker to answer to — \
             and until somebody makes it, the directory teaches the opposite of the \
             arrangement templates/access ships."
        );
    }

    // The retraction rather than a silent deletion (development-rules § 3), and
    // it is grepped WITH the mechanism above rather than in a test of its own.
    let readme = std::fs::read_to_string(TOOLS_README).expect("templates/tools/README.md");
    assert!(
        readme.contains("**A cell type is not a tool.**"),
        "templates/tools/README.md no longer carries the retraction of #547. Retiring a \
         promise takes an explicit retraction in the text, never a silent rewrite."
    );
    assert!(
        readme.contains("RETRACTED in 1.3.0"),
        "the retraction of #547 lost its marker in templates/tools/README.md"
    );

    let table: BTreeSet<String> = names_of(&ask(json!(["*"]))).into_iter().collect();
    for name in ["mcp", "vault"] {
        assert!(
            !table.contains(name),
            "the schemas cell offers `{name}`, which no door reaches. A schema for a call \
             that goes nowhere turns a refusal a model could read into a dead letter."
        );
    }
}

// ═══════════════════════════════════════════════════════ 4. reachability

fn write_json(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

/// The caller: one edge in per lane, one edge back per answer. Both pairs are
/// what the README's § *Wiring it* writes out, and neither of them names a cell
/// inside the hive.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./surface", "to": "./tools",
         "condition": "has(hop.route) && hop.route == 'call'",
         "modifier": {"set_hop": {"route": "'tool_call'"}}},
        {"from": "./surface", "to": "./tools",
         "condition": "has(hop.route) && hop.route == 'ask'",
         "modifier": {"set_hop": {"route": "'in_schemas'"}}},
        {"from": "./tools", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'tool_result'"},
        {"from": "./tools", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'tool_schemas'"}
    ]}}})
}

/// Stands in for the dispatcher AND for the start-up question: it puts the turn
/// on one of the two lanes depending on what it was handed.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
tools = doc["body"].get("tools")
if isinstance(tools, list):
    sys.stdout.write(json.dumps({
        "header": {"route": "ask"}, "tools": tools, "messages": []}))
else:
    sys.stdout.write(json.dumps({
        "header": {"route": "call",
                   "tool_name": str(hop.get("tool_name") or ""),
                   "tool_call_id": str(hop.get("tool_call_id") or "")},
        "messages": doc["body"].get("messages", [])}))
"#;

fn surface_cell() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": SURFACE, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {
                    "messages": {"type": "array", "required": true},
                    "tools": {"type": "array", "required": false}
                },
                "hop": {
                    "route": {"type": "string", "values": ["call", "ask"], "required": true},
                    "tool_name": {"type": "string", "required": false},
                    "tool_call_id": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for the caller of the tools hive.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// A tool occupant replaced by a `code` double that ANSWERS, stamping the
/// `hop.operation` its real cell stamps — which is not always its directory
/// name: `file` and `edit` label the OP they ran, so the shipped exit edge names
/// several values for each.
///
/// It emits no `tool_name`, faithfully: no real tool cell does, and a double
/// that echoed it would be handed its own answer back through the door it came
/// in by (ruling W7-R4).
fn double_cell(op: &str) -> Value {
    let script = format!(
        r#"
import sys, json
doc = json.load(sys.stdin)
sys.stdout.write(json.dumps({{
    "header": {{"operation": "{op}"}},
    "messages": [{{"origin": "tool", "type": "tool_result", "id": "double",
                  "text": "{op}"}}]}}))
"#
    );
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {"operation": {"type": "string", "values": [op], "required": true}}
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test double for one tool occupant of the tools hive.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// One occupant per name edge, with the label its real cell answers with.
/// `unknown` and `schemas` are never doubled: they are `code` cells already and
/// this file wants the shipped ones.
const DOUBLED: [(&str, &str, &str); 7] = [
    ("bash", "bash", "bash"),
    ("web_fetch", "web_fetch", "web_fetch"),
    ("web_search", "web_search", "web_search"),
    ("file", "file", "read"),
    ("edit", "edit", "find_replace"),
    ("build-draft", "build_topology", "build"),
    ("build-apply", "apply_manifest", "apply"),
];

fn build_tree(td: &tempfile::TempDir) {
    let root = td.path();
    write_json(root, "main/config.json", &main_config());
    write_json(root, "main/surface/config.json", &surface_cell());
    copy_cells(&templates_root().join("tools"), &root.join("main/tools"));
    for (dir, _, op) in DOUBLED {
        let rel = format!("main/tools/{dir}/config.json");
        assert!(
            root.join(&rel).exists(),
            "{rel} is not in the shipped template — the double would create a node the hive \
             has no edge to"
        );
        write_json(root, &rel, &double_cell(op));
    }
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    // `code` serves every doubled occupant AND the two shipped `code` cells this
    // file wants real (`unknown`, `schemas`). Since `tools@1.3.0` there is no
    // third registration to make: the two occupants no edge reached are gone
    // (GH #547), and every cell type this hive still holds is doubled or real.
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
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

fn call(tool_name: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("tool_name".into(), json!(tool_name));
    hop.insert("tool_call_id".into(), json!("call-1"));
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(json!({"messages": [
            {"origin": "assistant", "type": "tool_call", "id": "call-1", "text": "{}"}
        ]})))
        .hop(hop)
        .ttl(200)
        .build()
}

fn start_up_question(tools: Value) -> Message {
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(json!({"messages": [], "tools": tools})))
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

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Every occupant a name edge points at is reached by that name, on a colony
/// booted from the shipped tree — including the two that joined with #464.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_named_occupant_is_reached_by_its_name() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let (h, mut sink_rx) = boot(&td).await;

    for (dir, tool_name, op) in DOUBLED {
        h.send(call(tool_name)).await;
        let answer = recv_bounded(&mut sink_rx)
            .await
            .unwrap_or_else(|| panic!("no answer for `{tool_name}` — ./{dir} was not reached"));
        assert_eq!(
            hop_of(&answer, "operation"),
            op,
            "`{tool_name}` reached the wrong occupant, or none: the answer says {:?} and \
             ./{dir} answers {op:?}",
            answer.headers.hop
        );
        assert_eq!(
            hop_of(&answer, "route"),
            "tool_result",
            "and the exit puts every answer back on the hive's ONE result lane: {:?}",
            answer.headers.hop
        );
        assert_eq!(
            hop_of(&answer, "error_code"),
            "",
            "the guarded default fired although a positive edge had already decided: {:?}",
            answer.headers.hop
        );
    }

    assert!(
        h.drain_dead_letters().await.is_empty(),
        "a served call must not dead-letter anywhere on the way — and neither must the two \
         UNWIRED occupants: nothing routes to an island, so nothing arrives at one"
    );
    h.shutdown().await;
}

/// The start-up question, end to end: the caller names two tools at the hive
/// path, and the declarations come back on the hive's own lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_that_names_its_tools_gets_those_declarations_back() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td);
    let (h, mut sink_rx) = boot(&td).await;

    h.send(start_up_question(json!(["web_search", "telepathy"])))
        .await;
    let answer = recv_bounded(&mut sink_rx)
        .await
        .expect("the declarations leave the hive on tool_schemas");

    assert_eq!(
        hop_of(&answer, "route"),
        "tool_schemas",
        "the answer travels a lane of its own, not the result lane: {:?}",
        answer.headers.hop
    );
    assert_eq!(hop_of(&answer, "operation"), "schemas");
    assert_eq!(
        hop_of(&answer, "error_code"),
        "tool_unknown",
        "one of the two names does not exist and the caller is told so: {:?}",
        answer.headers.hop
    );
    let body = match &answer.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("inline expected"),
    };
    assert_eq!(
        body["schemas"][0]["name"],
        json!("web_search"),
        "the schema of the name that DOES exist came back: {body}"
    );
    assert_eq!(body["unknown"], json!(["telepathy"]));

    assert!(
        h.drain_dead_letters().await.is_empty(),
        "the declaration round must not dead-letter anywhere"
    );
    h.shutdown().await;
}
