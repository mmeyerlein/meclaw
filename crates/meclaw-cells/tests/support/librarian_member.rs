//! A librarian on a booted colony, for the locks of GH #950 (R-28-15: "a
//! procedure becomes code" -- a model finds a function by name and reads it).
//!
//! THE LAYOUT. The member reduced to the three holders this road touches and a
//! stand-in for its assistant level:
//!
//! - `./file-space` and `./graph-space` exactly as `graph_space_colony.rs`
//!   (`crate::space`, layout **member**) lays them out -- the SHIPPED templates
//!   resolved like the mutation door resolves a `ref`, every `llm` cell at the
//!   local chat stub, `./embed` at the local embeddings stub, timers quiet --
//!   so nothing about how a space boots is retyped here;
//! - `./librarian`, the member's own resident (`templates/member/librarian`)
//!   resolved the same way;
//! - `./assistants`, a stand-in code cell (see [`relay`]): what the test
//!   drives leaves it on `tool` or `schemas`, the lanes the real level emits,
//!   and what comes back on `in_tool` or `in_menu` leaves it as `heard` for
//!   the test's capture.
//!
//! Every member edge between those four is drawn VERBATIM off
//! `templates/member/config.json` ([`member_edges`]): the edges among the
//! three holders (the graph space's index road of GH #945 and the librarian's
//! six of GH #950), and the tool edges of the file space and of the librarian
//! to `./assistants` (GH #908 and the librarian's four of GH #950). Nothing
//! in them is retyped or re-pointed, so the run measures the member as it
//! ships. Whatever
//! no drawn edge takes leaves each holder through one `default` drain to the
//! capture `/park`; a lane nobody drains would be a dead letter, and the runs
//! assert there is none.
//!
//! Members: `""` is the member at the colony's root (the paths are
//! `/librarian`, `/file-space`, ...); any other name `n` puts a whole member
//! under `/n` with the same edges, so two members stand side by side the way
//! two people's members do (the GH #950 isolation lock, B.5).
//!
//! Measured at the receivers: the holders' own `cell.db` files (that an index
//! arrived), the colony's `message_log` (the seams), the captures (the tool
//! results), and the colony's own graph read (`ColonyMsg::ReadGraph`, the
//! message behind `/colony/graph`). No paid provider is reachable by
//! construction. Guarded like every template-reading test (GH #49).
//!
//! The including test file declares `#[path = "mock_openai.rs"] mod
//! mock_openai;` and `#[path = "support/graph_space_colony.rs"] mod space;`
//! at its root.
#![allow(dead_code)]

use crate::space::{self, Logged};
use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_core::{Body, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

pub const FILE_SPACE: &str = "./file-space";
pub const GRAPH_SPACE: &str = "./graph-space";
pub const LIBRARIAN: &str = "./librarian";
pub const ASSISTANTS: &str = "./assistants";

/// The surface every tool call of these runs comes from: the reasoning core,
/// the one surface that writes (OR-FJ-L2), so the fixture files can be
/// created through the same door the lookups use.
pub const CALLER: &str = "cogny";

/// The four tools the librarian offers (GH #950, spec § 5).
pub const LIB_TOOLS: [&str; 4] = ["lib_find", "lib_related", "lib_symbol", "lib_tree"];

/// Every template this layout boots or reads. One missing = skipped (GH #49).
/// The member's `./librarian` resident is deliberately not part of the
/// guard: the member travels whole, and a member without it is the very
/// thing these locks must see red.
pub fn shipped() -> bool {
    space::shipped() && space::repo("templates/librarian/config.json").is_file()
}

// ══════════════════════════════════════════════════════════════════ the paths

/// The path prefix of a member: `""` for the member at the root, `/<name>`
/// otherwise.
pub fn prefix(member: &str) -> String {
    if member.is_empty() {
        String::new()
    } else {
        format!("/{member}")
    }
}

/// The member's directory under `main/`.
pub fn member_dir(root: &std::path::Path, member: &str) -> std::path::PathBuf {
    let main = root.join("main");
    if member.is_empty() {
        main
    } else {
        main.join(member)
    }
}

/// The capture a member's stand-in hands its results to.
pub fn heard_path(member: &str) -> String {
    if member.is_empty() {
        "/heard".to_string()
    } else {
        format!("/heard-{member}")
    }
}

/// The librarian's own store (spec § 2: table `entries`).
pub fn lib_db(root: &std::path::Path, member: &str) -> std::path::PathBuf {
    member_dir(root, member).join("librarian/store/cell.db")
}

/// The graph space's own store (GH #945: `sources`, `nodes`, `edges`).
pub fn graph_db(root: &std::path::Path, member: &str) -> std::path::PathBuf {
    member_dir(root, member).join("graph-space/store/cell.db")
}

// ═══════════════════════════════════════════════════════════════ the stand-in

/// The assistant level's stand-in. A model's tool round leaves the real level
/// on `tool` (hop `tool_name`, `tool_call_id`, the arguments as the text of
/// one `tool_call` turn) and a menu question on `schemas` (body `tools`); the
/// answers come back on `in_tool` and `in_menu` (GH #908, GH #553). This cell
/// plays exactly that rim and nothing else: the test drives it on `drive`
/// (`hop.emit_as` names the lane to leave on) and reads what came back from
/// the capture behind `heard`, the incoming hop kept and its lane in
/// `hop.lane`. Context is not touched -- a cell never writes it -- so the
/// `tool_caller` the test stamps on the drive travels with the call, as the
/// level's own edges stamp it.
pub const RELAY: &str = r#"import sys, json

doc = json.load(sys.stdin)
body = doc.get("body") or {}
hop = ((doc.get("envelope") or {}).get("header") or {}).get("hop") or {}
lane = str(hop.get("route") or "")
out = {k: v for k, v in body.items() if k != "header"}
if not isinstance(out.get("messages"), list):
    out["messages"] = []
if lane == "drive":
    head = {"route": str(hop.get("emit_as") or "")}
    for k in ("tool_name", "tool_call_id"):
        if hop.get(k):
            head[k] = str(hop[k])
elif lane in ("in_tool", "in_menu"):
    head = {k: v for k, v in hop.items() if k != "route"}
    head["route"] = "heard"
    head["lane"] = lane
else:
    sys.stderr.write("assistants stand-in: lane %r -- dropped\n" % lane)
    sys.stdout.write("[]")
    sys.exit(0)
out["header"] = head
sys.stdout.write(json.dumps([out]))
"#;

/// The stand-in's config.
pub fn relay() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "external_timeout_ms": 10000, "script_inline": RELAY},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {"route": {"type": "string", "values": ["tool", "schemas", "heard"],
                                  "required": true}}
            },
            "consumes": {
                "body": {"messages": {"type": "array", "required": false},
                         "tools": {"type": "array", "required": false}},
                "context": {"tool_caller": {"type": "string", "required": false}}
            },
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for a member's assistant level: it places tool calls and \
                        menu questions and hands back what comes back.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

// ═══════════════════════════════════════════════════════════════════ the edges

fn end(e: &Value, key: &str) -> String {
    e[key].as_str().unwrap_or_default().to_string()
}

/// The member's own edges this layout draws, read off the shipped member and
/// drawn as they stand (see the file doc). Asserted on the way: the librarian
/// has exactly the ten edges of GH #950 (spec § 6) -- six to the two spaces,
/// four to `./assistants` -- and no edge of the member joins it to anything
/// else, least of all another librarian (B.5). Two of the six are the file
/// space's doors into it, each restoring the TTL: `source_changed` (a head
/// moved) and `source_described` (that head's summary line and tags, which a
/// space writes after its announcement -- OR-BC-68).
pub fn member_edges() -> Vec<Value> {
    let member = space::read_json(&space::repo("templates/member/config.json"));
    let all: Vec<Value> = member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let holders = [FILE_SPACE, GRAPH_SPACE, LIBRARIAN];
    let between: Vec<Value> = all
        .iter()
        .filter(|e| {
            holders.contains(&end(e, "from").as_str()) && holders.contains(&end(e, "to").as_str())
        })
        .cloned()
        .collect();
    // The tool roads of the two holders a model calls. The document intake
    // of GH #907 (`./file-space -> ./assistants` on `turn`) is the turn's
    // road, not the tools'; its lock is gh907's.
    let tools: Vec<Value> = all
        .iter()
        .filter(|e| {
            let (f, t) = (end(e, "from"), end(e, "to"));
            (f == ASSISTANTS && (t == LIBRARIAN || t == FILE_SPACE))
                || (t == ASSISTANTS && (f == LIBRARIAN || f == FILE_SPACE))
        })
        .filter(|e| {
            !e["condition"]
                .as_str()
                .unwrap_or_default()
                .contains("hop.route == 'turn'")
        })
        .cloned()
        .collect();

    let touches = |e: &Value| end(e, "from") == LIBRARIAN || end(e, "to") == LIBRARIAN;
    let librarian: Vec<&Value> = all.iter().filter(|&e| touches(e)).collect();
    assert_eq!(
        librarian.len(),
        10,
        "the member draws ten `./librarian` edges (GH #950, spec § 6): {librarian:#?}"
    );
    for e in &librarian {
        let other = if end(e, "from") == LIBRARIAN {
            end(e, "to")
        } else {
            end(e, "from")
        };
        assert!(
            [FILE_SPACE, GRAPH_SPACE, ASSISTANTS].contains(&other.as_str()),
            "a `./librarian` edge of the member runs to its own spaces and its own \
             assistants only (B.5): {e}"
        );
    }
    assert_eq!(
        between.iter().filter(|&e| touches(e)).count(),
        6,
        "six of them run between the librarian and the two spaces: {between:#?}"
    );
    for lane in ["source_changed", "source_described"] {
        let cond = format!("has(hop.route) && hop.route == '{lane}'");
        let doors: Vec<&Value> = between
            .iter()
            .filter(|e| {
                end(e, "from") == FILE_SPACE
                    && end(e, "to") == LIBRARIAN
                    && e["condition"].as_str() == Some(cond.as_str())
            })
            .collect();
        assert!(
            doors.len() == 1 && doors[0]["modifier"]["restore_ttl"] == json!(true),
            "the file space's `{lane}` is one door into the librarian, restoring the TTL \
             (GH #950, OR-BC-68): {between:#?}"
        );
    }
    assert_eq!(
        tools.iter().filter(|&e| touches(e)).count(),
        4,
        "four of them are the librarian's tool road (`tool` and `schemas` down, \
         `tool_result` and `tool_schemas` up): {tools:#?}"
    );
    assert_eq!(
        tools.iter().filter(|&e| !touches(e)).count(),
        4,
        "the file space keeps its four tool edges (GH #908): {tools:#?}"
    );
    between.into_iter().chain(tools).collect()
}

fn drain(from: &str, cond: &str, to: &str) -> Value {
    json!({"from": from, "to": to, "condition": cond})
}

/// One member under `dir`: the librarian resolved, the stand-in, and the hive
/// config with the member's edges, the stand-in's `heard` drain and one
/// `default` drain per holder. The two spaces are already there.
fn lay_member(dir: &std::path::Path, member: &str) {
    let resident = space::repo("templates/member/librarian");
    assert!(
        resident.join("config.json").is_file(),
        "the member houses no `./librarian` (`templates/member/librarian/config.json`) -- \
         GH #950 puts one in every member"
    );
    space::copy_resolved(&resident, &dir.join("librarian"), 0);
    space::write_json(&dir.join("assistants/config.json"), &relay());
    let mut edges = member_edges();
    edges.push(drain(
        ASSISTANTS,
        "has(hop.route) && hop.route == 'heard'",
        &heard_path(member),
    ));
    for holder in [FILE_SPACE, GRAPH_SPACE, LIBRARIAN] {
        let mut rest = drain(holder, "has(hop.route)", "/park");
        rest["default"] = json!(true);
        edges.push(rest);
    }
    space::write_json(
        &dir.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
}

/// Lay out `members` (see the file doc). The two spaces come from
/// `space::build` (layout member) -- its stubs, its timers, its environment
/// file -- and are moved under each member; `space::build`'s own root edges
/// are replaced.
pub fn build(td: &tempfile::TempDir, members: &[&str], stubs: &space::Stubs) {
    assert!(!members.is_empty(), "at least one member");
    space::build(td, space::Layout::Member, stubs);
    let main = td.path().join("main");
    if members.len() == 1 && members[0].is_empty() {
        lay_member(&main, "");
        return;
    }
    assert!(
        members.iter().all(|m| !m.is_empty() && !m.contains('/')),
        "a side-by-side member has a plain name: {members:?}"
    );
    let holders = ["file-space", "graph-space"];
    let (last, rest) = members.split_last().expect("a member");
    for m in rest {
        for holder in holders {
            space::copy_resolved(&main.join(holder), &main.join(m).join(holder), 0);
        }
    }
    std::fs::create_dir_all(main.join(last)).expect("mkdir");
    for holder in holders {
        std::fs::rename(main.join(holder), main.join(last).join(holder))
            .expect("move a space under its member");
    }
    for m in members {
        lay_member(&main.join(m), m);
    }
    space::write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
}

pub struct Ports {
    /// What each member's stand-in heard, in the order of `members`.
    pub heard: Vec<mpsc::Receiver<Message>>,
    /// Held, not dropped: a capture whose receiver is gone turns every
    /// delivery into a send error.
    pub park: mpsc::Receiver<Message>,
}

pub async fn boot(td: &tempfile::TempDir, members: &[&str]) -> (ColonyHandle, Ports) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let mut heard = Vec::new();
    for m in members {
        let (tx, rx) = mpsc::channel::<Message>(1024);
        h.spawn(Path::new(&heard_path(m)), move || {
            CaptureCell::new(tx.clone())
        })
        .await;
        heard.push(rx);
    }
    let (park_tx, park) = mpsc::channel::<Message>(8192);
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the member's spaces, its librarian and the stand-in must boot");
    (h, Ports { heard, park })
}

// ═══════════════════════════════════════════════════════════════════ the rounds

fn drive(member: &str, hop: Value, body: Value) -> Message {
    MessageBuilder::new(Path::new(&format!("{}/assistants", prefix(member))))
        .hop(space::map(hop))
        .context(space::map(json!({"tool_caller": CALLER})))
        .body(Body::Inline(body))
        .ttl(MESSAGE_DEFAULT_TTL)
        .build()
}

/// One tool call placed by the member's stand-in, as the core places it; the
/// JSON the tool's one result carries back.
pub async fn call(
    h: &ColonyHandle,
    heard: &mut mpsc::Receiver<Message>,
    root: &std::path::Path,
    member: &str,
    tool: &str,
    id: &str,
    args: &Value,
) -> Value {
    h.send(drive(
        member,
        json!({"route": "drive", "emit_as": "tool", "tool_name": tool, "tool_call_id": id}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": id,
                             "text": args.to_string()}]}),
    ))
    .await;
    let mut seen = Vec::new();
    let m = space::next_matching(
        heard,
        root,
        &format!("the result of `{tool}` ({id}) in member `{member}`"),
        |m| space::hop_str(m, "lane") == "in_tool" && space::hop_str(m, "tool_call_id") == id,
        &mut seen,
    )
    .await;
    space::tool_answer(&m)
}

/// Write one file through the file space's tool door as the core; returns
/// `(fh-id, version)` off the tool result.
pub async fn create(
    h: &ColonyHandle,
    heard: &mut mpsc::Receiver<Message>,
    root: &std::path::Path,
    member: &str,
    path: &str,
    text: &str,
) -> (String, String) {
    let id = format!("create{}", path.replace(['/', '.'], "-"));
    let a = call(
        h,
        heard,
        root,
        member,
        "file_create",
        &id,
        &json!({"path": path, "text": text}),
    )
    .await;
    assert_eq!(a["ok"], json!(true), "{path} was not created: {a}");
    let file = a["file"].as_str().unwrap_or_default().to_string();
    let version = a["version"].as_str().unwrap_or_default().to_string();
    assert!(file.starts_with("fh-"), "{a}");
    (file, version)
}

/// Ask the member's menu question (`tools: ["*"]`) and return the tool names
/// the answer marked `tool_answerer` 'library' carries (GH #950: the
/// librarian's `schemas` and `tool_schemas` edges).
pub async fn menu(
    h: &ColonyHandle,
    heard: &mut mpsc::Receiver<Message>,
    root: &std::path::Path,
    member: &str,
) -> Vec<String> {
    h.send(drive(
        member,
        json!({"route": "drive", "emit_as": "schemas"}),
        json!({"tools": ["*"], "messages": []}),
    ))
    .await;
    let mut seen = Vec::new();
    let m = space::next_matching(
        heard,
        root,
        &format!("the library's menu in member `{member}`"),
        |m| {
            space::hop_str(m, "lane") == "in_menu"
                && m.headers
                    .context
                    .get("tool_answerer")
                    .and_then(Value::as_str)
                    == Some("library")
        },
        &mut seen,
    )
    .await;
    let mut names: Vec<String> = space::body_of(&m)["schemas"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s["name"].as_str().map(str::to_string))
        .collect();
    names.sort();
    names
}

/// The `items` of an answer.
pub fn items(v: &Value) -> Vec<Value> {
    v["items"].as_array().cloned().unwrap_or_default()
}

// ═════════════════════════════════════════════════════════════ the index signals

/// The first column of the first row of `sql` as a number; 0 for a store
/// that has not woken yet.
pub fn count(db: &std::path::Path, sql: &str) -> i64 {
    space::rows(db, sql)
        .first()
        .and_then(|r| r.first())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Whether the member's librarian catalogued `path` at the version last
/// announced for it (an `info` or `outline` answer landed under the CAS of
/// spec § 3), with `name` among its top-level names when one is given.
pub fn catalogued(root: &std::path::Path, member: &str, path: &str, name: Option<&str>) -> bool {
    let sql = format!(
        "SELECT names FROM entries WHERE path = '{path}' AND tomb = '' \
         AND version != '' AND version = announced"
    );
    let found = space::rows(&lib_db(root, member), &sql);
    let Some(row) = found.first() else {
        return false;
    };
    match name {
        None => true,
        Some(n) => {
            let names = row.first().map_or("[]", |s| s.as_str());
            sj::from_str::<Vec<String>>(names).is_ok_and(|v| v.iter().any(|x| x == n))
        }
    }
}

/// Whether the member's graph space holds a node named `name`.
pub fn graph_knows(root: &std::path::Path, member: &str, name: &str) -> bool {
    count(
        &graph_db(root, member),
        &format!("SELECT count(*) FROM nodes WHERE name = '{name}'"),
    ) > 0
}

/// Wait until `cond` holds; on the deadline, panic with what every member's
/// catalog and graph hold and the dead letters of the run.
pub async fn wait_for(
    root: &std::path::Path,
    members: &[&str],
    what: &str,
    cond: impl Fn() -> bool,
) {
    let deadline = Instant::now() + space::DEADLINE;
    while !cond() {
        if Instant::now() >= deadline {
            let mut held = String::new();
            for m in members {
                held.push_str(&format!(
                    "\n[member `{m}`] catalog: {:#?}\n[member `{m}`] graph sources: {:#?}",
                    space::rows(
                        &lib_db(root, m),
                        "SELECT source, path, kind, names, version, announced, tomb FROM entries"
                    ),
                    space::rows(
                        &graph_db(root, m),
                        "SELECT source, path, module, version, announced, tomb FROM sources"
                    )
                ));
            }
            panic!(
                "{what}: not within {:?}.{held}\nDead letters: {:#?}",
                space::DEADLINE,
                space::dead_letters(root)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ══════════════════════════════════════════════════════════════════ the seams

pub fn op_id(r: &Logged) -> String {
    r.hop["op_id"].as_str().unwrap_or_default().to_string()
}

fn inside(path: &str, holder: &str) -> bool {
    path.starts_with(&format!("{holder}/"))
}

/// GH #950's seam claim, measured in the message log: every question the
/// librarian put to a space -- a `lib:` `op_id` delivered into a cell of the
/// graph space on `in_graph`, or into a cell of the file space on `in_read` --
/// was delivered exactly once, and exactly one answer came back into the
/// librarian on `in_pulled` under the same `op_id` (into `./index` for an
/// index pull `lib:f:i:`, into `./query` for every other). Returns the
/// `(graph, file)` `op_id`s, so a run can say which delegations it saw.
pub fn one_question_one_answer(log: &[Logged], member: &str) -> (Vec<String>, Vec<String>) {
    let p = prefix(member);
    let graph = format!("{p}/graph-space");
    let files = format!("{p}/file-space");
    let lib = format!("{p}/librarian");
    let mut asked_graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut asked_files: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in log {
        let id = op_id(r);
        if !id.starts_with("lib:") {
            continue;
        }
        if inside(&r.to, &graph) && r.route() == "in_graph" {
            asked_graph.entry(id).or_default().push(r.say());
        } else if inside(&r.to, &files) && r.route() == "in_read" {
            asked_files.entry(id).or_default().push(r.say());
        }
    }
    let answered = |id: &str, cell: &str| -> Vec<String> {
        log.iter()
            .filter(|r| r.to == format!("{lib}/{cell}") && r.route() == "in_pulled")
            .filter(|r| op_id(r) == id)
            .map(Logged::say)
            .collect()
    };
    for (id, asked) in asked_graph.iter().chain(asked_files.iter()) {
        assert_eq!(
            asked.len(),
            1,
            "the question `{id}` reached a space {} times: {asked:#?}",
            asked.len()
        );
        let cell = if id.starts_with("lib:f:i:") {
            "index"
        } else {
            "query"
        };
        let back = answered(id, cell);
        assert_eq!(
            back.len(),
            1,
            "the question `{id}` came back into `{lib}/{cell}` {} times: {back:#?}",
            back.len()
        );
    }
    (
        asked_graph.into_keys().collect(),
        asked_files.into_keys().collect(),
    )
}

/// The questions to the graph space that descend from the tool call `call_id`:
/// walked back along their parents to the delivery of that call into the
/// librarian's `./tools` (a causal chain, not a time window).
pub fn graph_questions_of<'a>(log: &'a [Logged], member: &str, call_id: &str) -> Vec<&'a Logged> {
    let p = prefix(member);
    let graph = format!("{p}/graph-space");
    let tools = format!("{p}/librarian/tools");
    let seam = |r: &Logged| -> bool {
        r.to == tools && r.route() == "in_tool" && r.hop["tool_call_id"].as_str() == Some(call_id)
    };
    log.iter()
        .filter(|r| {
            inside(&r.to, &graph) && r.route() == "in_graph" && op_id(r).starts_with("lib:")
        })
        .filter(|&r| space::chain_to_seam(log, r, seam).is_some())
        .collect()
}

/// The routes a code cell of the librarian declares it emits; None when its
/// contract names none (`./schemas` answers on `operation`, its route is the
/// hive edge's).
pub fn librarian_routes(cell: &str) -> Option<Vec<String>> {
    let cfg = space::read_json(&space::repo(&format!(
        "templates/librarian/{cell}/config.json"
    )));
    cfg["contract"]["emits"]["hop"]["route"]["values"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
}

/// Every delivery out of a code cell of the librarian carries a route its
/// contract declares (`contract.emits` in the run, not on paper), and every
/// cell of the hive -- the four code cells and the store -- ran.
pub fn librarian_within_contract(log: &[Logged], member: &str) {
    let p = prefix(member);
    for cell in ["index", "query", "tools", "schemas", "store"] {
        let from = format!("{p}/librarian/{cell}");
        assert!(
            log.iter().any(|r| r.from == from),
            "{from} never emitted in this run"
        );
        if cell == "store" {
            continue;
        }
        let Some(declared) = librarian_routes(cell) else {
            continue;
        };
        for r in log.iter().filter(|r| r.from == from) {
            assert!(
                declared.contains(&r.route()),
                "{from} emitted `{}`, which its contract does not declare ({declared:?}): {}",
                r.route(),
                r.say()
            );
        }
    }
}

/// `(from, to)` of every edge of the running colony, read the way
/// `/colony/graph` reads them (the colony's own `ReadGraph`, scope `/`).
pub async fn colony_edges(h: &ColonyHandle) -> Vec<(String, String)> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox closed");
    ack_rx
        .await
        .expect("the colony answers its graph read")
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect()
}
