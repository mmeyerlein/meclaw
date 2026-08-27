//! GH #286 / GH #283 — the distribution happens INSIDE the tools hive.
//!
//! `templates/tools` is sealed (`params.ports: []`) and states one lane in and
//! one lane out. Everything between those two lanes is its own business, and
//! this file is the runtime proof that it does the business: one `tool_call`
//! reaches exactly one occupant, and a call for a tool nobody wired comes back
//! as a named refusal rather than as a dead letter.
//!
//! # The construct under test (GH #283, ruling Q1)
//!
//! Three ordinary conditioned edges out of the hive path, one per tool, plus
//! **one guarded default edge** to `./unknown`. The whole behaviour follows from
//! two properties of `apply_edges`
//! (`crates/meclaw-colony/src/edge_table.rs`, the two-phase evaluation):
//!
//! * **Suppression is sender-wide.** If ANY regular edge of the sender decided,
//!   the default phase never runs. The three positive edges are mutually
//!   exclusive by construction, so a known tool name fires exactly one of them
//!   and thereby silences the default.
//! * **A guarded default needs BOTH.** Nothing regular fired AND its own
//!   condition holds. An unknown tool name — or a call carrying no
//!   `hop.tool_name` at all — fires no positive edge, so the default is
//!   consulted, its guard (`hop.route == 'tool_call'`) holds, and the message
//!   reaches `./unknown`.
//!
//! Neither property is re-proved here; both have unit pins next to the code.
//! What is proved here is that the shipped template WIRES them that way.
//!
//! # How the tree is built
//!
//! The shipped `templates/tools` directory is copied cell by cell and every
//! directory stays where it is. Only the `config.json` of the three TOOL
//! occupants is overwritten, with a `code` double that answers and stamps the
//! same `hop.operation` the real cell stamps. So the GRAPH under test is the
//! shipped one, byte for byte, and the doubles sit at the end of three of its
//! edges without changing any of them.
//!
//! `./unknown` is never doubled. It is a `code` cell already, it needs no
//! provider and no network, and both tests want the shipped one: in the second
//! because its refusal is the subject, in the first because its silence is.
//!
//! **Observed at the exit, not at the occupants**, and that is a property of the
//! doubles rather than a concession: every occupant of this hive answers, so
//! `hop.operation` on the way out names the one that was served, and a second
//! occupant being served would be a second message rather than a silence to be
//! waited out. Capturing at the occupants instead would leave the hive with
//! three cells that never answer — a different topology from the shipped one on
//! exactly the property under test.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Headers, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Copy the template cell by cell: only `config.json` files travel, so the tree
/// under test IS the template and nothing else.
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

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// The caller around the hive: ONE edge in, stamping the lane, and ONE edge
/// back out on `tool_result`. That pair is the entire wiring recipe the
/// template's README states, and adding a tool changes neither of them.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./surface", "to": "./tools",
         "condition": "has(hop.route) && hop.route == 'call'",
         "modifier": {"set_hop": {"route": "'tool_call'"}}},
        {"from": "./tools", "to": "/sink",
         "condition": "has(hop.route) && hop.route == 'tool_result'"}
    ]}}})
}

/// Stands in for the dispatcher a real caller has: it hands the hive one call
/// and carries the name of the wanted tool on the hop.
const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
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
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "route": {"type": "string", "values": ["call"], "required": true},
                    "tool_name": {"type": "string", "required": false},
                    "tool_call_id": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for the caller that hands the tools hive one call.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// A tool occupant replaced by a `code` double that ANSWERS. `op` is the
/// `hop.operation` the real cell of that directory stamps, so the shipped
/// return edge takes the double exactly as it takes the original.
///
/// **It emits no `tool_name`, and that is faithful rather than convenient.**
/// None of the three real cells does — `bash`, `web_fetch` and `web_search`
/// stamp `operation` and their own outcome keys, and a cell emission mints a
/// fresh hop, so the name the call was dispatched on does not survive the
/// answer. A double that echoed it was how the loop of ruling W7-R4 was found:
/// before the doors read the lane, such an answer went straight back through the
/// door it came in by. Both halves of that guard are pinned below —
/// `the_shipped_dispatch_is_three_narrowing_doors_and_one_guarded_default` for
/// the doors, `no_occupant_answers_with_the_key_the_doors_dispatch_on` for the
/// occupants.
fn double_script(op: &str) -> String {
    format!(
        r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {{}}).get("hop") or {{}})
sys.stdout.write(json.dumps({{
    "header": {{"route": "served", "operation": "{op}",
               "in_route": str(hop.get("route") or "")}},
    "messages": [{{"origin": "tool", "type": "tool_result", "id": "double",
                  "text": "{op}"}}]}}))
"#
    )
}

fn double_cell(op: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {
            "runner": "python3",
            "script_inline": double_script(op),
            "external_timeout_ms": 10000
        },
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "route": {"type": "string", "values": ["served"], "required": true},
                    "operation": {"type": "string", "values": [op], "required": true},
                    "in_route": {"type": "string", "required": false}
                }
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

/// Build the tree. The template is copied whole and the occupant DIRECTORIES
/// stay exactly where they are — only the `config.json` of each name in
/// `doubled` is overwritten with an answering `code` double.
///
/// Removing the directories instead was the first attempt and it does not work:
/// `bootstrap_from_filesystem` builds the edge table off the filesystem tree, so
/// a hive door pointing at a directory that is not there leaves the inside
/// unroutable — the boot itself still succeeds, which is what made it look like
/// a routing bug rather than a tree with holes in it.
fn build_tree(td: &tempfile::TempDir, doubled: &[&str]) {
    let root = td.path();
    write(root, "main/config.json", &main_config());
    write(root, "main/surface/config.json", &surface_cell());
    copy_cells(&templates_root().join("tools"), &root.join("main/tools"));
    for name in doubled {
        let rel = format!("main/tools/{name}/config.json");
        assert!(
            root.join(&rel).exists(),
            "{rel} is not in the shipped template — the double would create a \
             node the hive has no edge to"
        );
        write(root, &rel, &double_cell(name));
    }
}

/// The caller's drain. Everything the hive answers arrives here, and
/// `hop.operation` says which occupant answered.
async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(16);
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

/// One call, with the tool name on the hop.
///
/// The turn is a `tool_call` and therefore carries an `id`: the UBF schema makes
/// it required for exactly the two types that reference a call
/// (`crates/meclaw-core/schemas/ubf-body.json` § `TurnObject.allOf`), and the
/// surface cell re-emits this turn verbatim, so a missing id would be refused on
/// the surface's output and the round would never start.
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

fn hop_of(m: &Message, key: &str) -> String {
    m.headers
        .hop
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn text_of(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][0]["text"].as_str().unwrap_or_default().into(),
        Body::Blob(_) => panic!("inline expected"),
    }
}

/// Failure-marker timeout: generous on purpose (30 s convention), robust against
/// cargo-parallel load.
async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// The settle before a "received nothing" assertion.
///
/// This is a semantic discriminator, so it is argued rather than made generous:
/// every edge out of the hive path is decided in ONE `apply_edges` call on ONE
/// message, so a competing delivery is not a slow second round — it is already
/// in flight by the time the winning occupant has its message. The wait exists
/// only to let an in-flight delivery arrive, and it starts AFTER the positive
/// receipt has been observed.
const SETTLE: Duration = Duration::from_millis(750);

/// Assert this capture received nothing at all. `who` names it in the failure.
fn received_nothing(rx: &mut mpsc::Receiver<Message>, who: &str) {
    if let Ok(m) = rx.try_recv() {
        panic!(
            "{who} received a message it must never see: hop {:?}",
            m.headers.hop
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// The three tool occupants, doubled. `unknown` is never doubled: it is a
/// `code` cell already, it answers by itself, and both tests need the SHIPPED
/// one — in the second because its refusal is the subject, in the first because
/// its silence is.
const TOOLS: [&str; 3] = ["bash", "web_fetch", "web_search"];

/// Every occupant of this hive answers, so `hop.operation` on the way out names
/// the one that was served — and a second occupant being served would be a
/// second message, not a silence to be waited out.
///
/// That is the whole reason the doubles emit instead of capturing. A capture
/// would let "was served" be read at the occupant, but it would also leave the
/// hive with three cells that never answer, which is a different topology from
/// the shipped one on exactly the property under test.
fn served_by(m: &Message) -> String {
    hop_of(m, "operation")
}

/// One named call reaches exactly one occupant — and the guarded default stays
/// silent, because a regular edge of the same sender decided.
///
/// The `unknown` zero is the load-bearing one, and here it is a positive
/// statement rather than a wait: the shipped `unknown` cell answers whenever it
/// is reached, with an `error_code` no tool ever carries. Its absence from the
/// exit is therefore evidence that the default phase never ran, not merely that
/// nothing has arrived yet.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_named_call_reaches_that_tool_and_the_default_stays_silent() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &TOOLS);
    let (h, mut sink_rx) = boot(&td).await;

    h.send(call("web_search")).await;
    let answer = recv_bounded(&mut sink_rx)
        .await
        .expect("the call is served and the answer leaves the hive");

    assert_eq!(
        served_by(&answer),
        "web_search",
        "the call reached the occupant its name selected, and no other: {:?}",
        answer.headers.hop
    );
    assert_eq!(
        hop_of(&answer, "in_route"),
        "tool_call",
        "the lane the caller stamped rode all the way to the occupant: {:?}",
        answer.headers.hop
    );
    assert_eq!(
        hop_of(&answer, "route"),
        "tool_result",
        "and the exit put it back on the hive's ONE outward lane: {:?}",
        answer.headers.hop
    );
    assert_eq!(
        hop_of(&answer, "error_code"),
        "",
        "the guarded default fired although a positive edge had already decided \
         — the edge is not declared `default: true`: {:?}",
        answer.headers.hop
    );

    // Nothing else answered: not a second tool, and above all not `./unknown`.
    tokio::time::sleep(SETTLE).await;
    received_nothing(
        &mut sink_rx,
        "the caller — a SECOND occupant answered the same call",
    );

    assert!(
        h.drain_dead_letters().await.is_empty(),
        "a served call must not dead-letter anywhere on the way"
    );
    h.shutdown().await;
}

/// A tool nobody wired is a NAMED refusal that leaves the hive on the contract's
/// one outward lane — not a dead letter, and not a silent nothing.
///
/// Same tree as the test above, so the two differ in exactly one thing: the name
/// on the hop. `unknown` is the shipped cell; the three doubles are what prove
/// the negative, because either of them being served would answer with its own
/// `hop.operation` instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_tool_name_leaves_as_one_named_tool_result() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &TOOLS);
    let (h, mut sink_rx) = boot(&td).await;

    h.send(call("nonesuch")).await;
    let answer = recv_bounded(&mut sink_rx)
        .await
        .expect("the refusal leaves the hive on the tool_result lane");

    assert_eq!(
        served_by(&answer),
        "unknown",
        "no positive edge fired, so the guarded default carried the call to \
         ./unknown — and no tool was served instead: {:?}",
        answer.headers.hop
    );
    assert_eq!(
        hop_of(&answer, "route"),
        "tool_result",
        "the refusal travels the hive's ONE outward lane: {:?}",
        answer.headers.hop
    );
    assert_eq!(
        hop_of(&answer, "error_code"),
        "unknown_tool",
        "and it is typed, so the caller reads the refusal off the message rather \
         than off the route: {:?}",
        answer.headers.hop
    );
    assert_eq!(
        hop_of(&answer, "tool_name"),
        "nonesuch",
        "and it names the tool that was asked for: {:?}",
        answer.headers.hop
    );
    assert!(
        text_of(&answer).contains("nonesuch"),
        "the body says which tool, in words: {}",
        text_of(&answer)
    );

    tokio::time::sleep(SETTLE).await;
    received_nothing(&mut sink_rx, "the caller — a SECOND answer to one call");

    assert!(
        h.drain_dead_letters().await.is_empty(),
        "the whole point of the guarded default: an unknown tool name is a named \
         refusal, never a dead letter"
    );
    h.shutdown().await;
}

// ══════════════════════════════ the gate change of ruling W7-R1, and its limit

/// A reconstruction of the `from == "."` half of
/// `gh173_shipped_hive_contracts::every_lane_the_graph_opens_is_declared`,
/// including the carve-out that driver ruling **W7-R1** added to it.
///
/// It has to be a reconstruction: `crates/meclaw-cells/tests/*.rs` compile into
/// separate binaries, so the gate's own helpers cannot be called from here. The
/// two files therefore carry a cross-reference to each other — the doc comment
/// on `condition_reads_a_hop_key_the_probe_cannot_carry` in `gh173_...` names
/// this module, and this module names that function. **A change to either
/// belongs in both.**
///
/// The reason this reconstruction exists at all: W7-R1 loosens a gate, and a
/// loosening that nobody measures is a claim. What is measured here is the
/// EDGE of it — a door whose condition reads only `hop.route`, a key the bare
/// route probe carries perfectly well, is still judged and still condemned.
mod door_sweep {
    use super::*;

    const HIVE: &str = "/h";

    /// Verbatim from `gh173_shipped_hive_contracts::table_for`, including the
    /// hard `is_default: false` — which is why the guarded default is judged
    /// like a regular edge by this sweep and needs no carve-out of its own.
    fn table_for(hp: &HiveParams) -> EdgeTable {
        let abs = |ep: &str| -> String {
            match ep {
                "." => HIVE.to_string(),
                other => format!("{HIVE}/{}", other.trim_start_matches("./")),
            }
        };
        let mut t = EdgeTable::new();
        for spec in &hp.graph.edges {
            let condition = spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
            });
            t.insert(Edge {
                id: Uuid::now_v7(),
                from: Path::new(&abs(&spec.from)),
                to: Path::new(&abs(&spec.to)),
                condition,
                modifier: None,
                is_default: false,
            });
        }
        t
    }

    fn probe(route: &str) -> Headers {
        let mut hop = meclaw_core::serde_json::Map::new();
        hop.insert("route".into(), Value::String(route.into()));
        Headers::from_parts(meclaw_core::serde_json::Map::new(), hop)
    }

    /// The carve-out of W7-R1, reconstructed. Kept byte-comparable to the
    /// original on purpose.
    fn condition_reads_a_hop_key_the_probe_cannot_carry(spec: &EdgeSpec) -> bool {
        let Some(src) = spec.condition.as_deref() else {
            return false;
        };
        let mut rest = src;
        while let Some(at) = rest.find("hop.") {
            let after = &rest[at + "hop.".len()..];
            let key: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !key.is_empty() && key != "route" {
                return true;
            }
            rest = &after[key.len()..];
        }
        false
    }

    /// The loop body of `every_lane_the_graph_opens_is_declared`, for one door:
    /// `true` means the sweep would fail on it.
    pub fn condemns(hp: &HiveParams, accepts: &[&str], spec: &EdgeSpec) -> bool {
        let table = table_for(hp);
        let hive = Path::new(HIVE);
        let covered = accepts.iter().any(|route| {
            apply_edges(&table, &hive, &probe(route)).iter().any(|d| {
                d.target.as_str() == format!("{HIVE}/{}", spec.to.trim_start_matches("./"))
            })
        }) || condition_reads_a_hop_key_the_probe_cannot_carry(spec);
        !covered
    }

    pub fn hive(edges: Value) -> HiveParams {
        let raw = meclaw_core::serde_json::json!({"ports": [], "graph": {"edges": edges}});
        meclaw_core::serde_json::from_value(raw)
            .expect("the fabricated hive parses through the real EdgeSpec")
    }
}

/// W7-R1's limit, and the reason the ruling is a carve-out rather than a hole.
///
/// Three fabricated doors through the same reconstruction:
///
/// 1. A door conditioned on `hop.route == 'undeclared'` — a key the probe DOES
///    carry, so the sweep can decide it, and it does: **condemned.** This is the
///    class W7-R1 must not release, and the whole reason this test exists.
/// 2. A door conditioned on `hop.tool_name` — a key the bare route probe cannot
///    carry, so the sweep cannot decide it: **not condemned.** This is the
///    tools hive's own shape, and the class W7-R1 releases.
/// 3. An unconditional door — fires under the probe, covered the old way:
///    **not condemned.** The base check is untouched.
#[test]
fn the_carve_out_still_condemns_a_door_that_discriminates_on_the_route_itself() {
    let undeclared = door_sweep::hive(json!([
        {"from": ".", "to": "./inside", "condition": "has(hop.route) && hop.route == 'undeclared'"}
    ]));
    assert!(
        door_sweep::condemns(&undeclared, &["tool_call"], &undeclared.graph.edges[0]),
        "W7-R1 released a door that discriminates on `hop.route` itself — the probe \
         carries that key, so the sweep CAN decide it and must go on doing so"
    );

    let foreign = door_sweep::hive(json!([
        {"from": ".", "to": "./bash",
         "condition": "has(hop.tool_name) && hop.tool_name == 'bash'"}
    ]));
    assert!(
        !door_sweep::condemns(&foreign, &["tool_call"], &foreign.graph.edges[0]),
        "the carve-out did not take: a door narrowing WITHIN a declared lane on a \
         key the probe cannot carry is exactly what W7-R1 exempts"
    );

    let plain = door_sweep::hive(json!([{"from": ".", "to": "./inside"}]));
    assert!(
        !door_sweep::condemns(&plain, &["tool_call"], &plain.graph.edges[0]),
        "the base check moved: an unconditional door fires under the probe and was \
         always covered"
    );
}

/// The occupant side of the loop guard, kept independent of the door side.
///
/// **The primary defence is in the doors**, and it is structural: since ruling
/// W7-R4 every one of the three reads `hop.route == 'tool_call'` before it reads
/// `hop.tool_name`, so an answer coming back through the hive path cannot
/// satisfy one however it is stamped. That is what actually closes the hole, and
/// it is asserted in
/// `the_shipped_dispatch_is_three_narrowing_doors_and_one_guarded_default`.
///
/// This test is the **second line**, and it exists because the first one is a
/// condition somebody can shorten. Without the lane guard the doors ask only
/// about `hop.tool_name`, and an occupant's answer travels back through the very
/// hive path the doors leave from — so an answer carrying `hop.tool_name` would
/// satisfy the door it arrived by and be dispatched to its own sender again,
/// round after round until the TTL runs out. **That was observed**, with a test
/// double that echoed the name back, which is how the hole was found at all.
///
/// So the occupant half is pinned here on its own terms, at the level where it
/// is checkable without a colony: no tool occupant DECLARES `tool_name` among
/// the hop keys it emits. Loosen the door condition tomorrow and this still
/// catches the echoing occupant.
///
/// `./unknown` is the one occupant that does declare it, deliberately — echoing
/// the name that was asked for is the whole content of its refusal. It is safe
/// twice over: the lane guard stops its answer at the doors, and it is only ever
/// reached for a name **no** door matched in the first place.
#[test]
fn no_occupant_answers_with_the_key_the_doors_dispatch_on() {
    let raw =
        std::fs::read_to_string(templates_root().join("tools/config.json")).expect("tools ships");
    let v: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
    let hp: HiveParams = meclaw_core::serde_json::from_value(v["params"].clone()).unwrap();

    // The occupants a TOOL door dispatches to, read off the graph rather than
    // listed. Since R6 (GH #425) the hive has doors on a second inbound lane —
    // `in_build_result`, which dispatches on `hop.build_op` and not on a tool
    // name — so the filter names the key this test is about instead of taking
    // every non-default door and hoping they are all tool doors.
    let dispatched: Vec<String> = hp
        .graph
        .edges
        .iter()
        .filter(|e| {
            e.from == "."
                && !e.is_default
                && e.condition
                    .as_deref()
                    .is_some_and(|c| c.contains("hop.tool_name"))
        })
        .map(|e| e.to.trim_start_matches("./").to_string())
        .collect();
    assert_eq!(
        dispatched.len(),
        5,
        "five tool doors — three tools plus the two halves of a build round: {dispatched:?}"
    );

    for name in &dispatched {
        let cfg =
            std::fs::read_to_string(templates_root().join(format!("tools/{name}/config.json")))
                .unwrap_or_else(|e| panic!("tools/{name}: {e}"));
        let occupant: Value = meclaw_core::serde_json::from_str(&cfg).unwrap();
        assert!(
            occupant["contract"]["emits"]["hop"]
                .get("tool_name")
                .is_none(),
            "tools/{name} declares it emits `tool_name`, the key its own door \
             dispatches on. The door's `hop.route == 'tool_call'` guard (W7-R4) \
             still stops that answer today — but this occupant is now one shortened \
             condition away from being handed its own answer back, round after \
             round until the TTL runs out. An answer has no business carrying the \
             dispatch key."
        );
    }

    // And the deliberate exception is still the deliberate exception: if
    // `unknown` ever stopped echoing the name, the refusal would stop naming the
    // tool that was asked for, which is the one thing it is for.
    let unknown: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(templates_root().join("tools/unknown/config.json")).unwrap(),
    )
    .unwrap();
    assert!(
        unknown["contract"]["emits"]["hop"]
            .get("tool_name")
            .is_some(),
        "./unknown stopped naming the tool that was asked for"
    );
}

/// The shipped template is the case W7-R1 was forced by, and it is worth
/// asserting rather than assuming: all three positive doors read `hop.tool_name`
/// and the guarded default reads only `hop.route`.
///
/// Which is why the default needs no exemption — under the sweep's own
/// `is_default: false` it fires on `probe("tool_call")` and is covered the
/// ordinary way.
#[test]
fn the_shipped_dispatch_is_narrowing_doors_and_one_guarded_default_per_lane() {
    let raw =
        std::fs::read_to_string(templates_root().join("tools/config.json")).expect("tools ships");
    let v: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
    let hp: HiveParams = meclaw_core::serde_json::from_value(v["params"].clone())
        .expect("the shipped params parse through the real HiveParams");

    let doors: Vec<&EdgeSpec> = hp.graph.edges.iter().filter(|e| e.from == ".").collect();
    assert_eq!(
        doors.len(),
        9,
        "five tool doors, two build-result doors, and one guarded default each: {doors:#?}"
    );

    // ONE guarded default PER LANE. The old wording was "exactly one default
    // edge", and the argument under it is untouched and still enforced below:
    // guarded defaults do not compete, so two that could match the same message
    // would both fire. What R6 (GH #425) added is a default on a SECOND lane,
    // and two defaults whose guards name different lanes can never both match.
    // So the count moved from "one" to "one per lane", and the lane is read off
    // the guard rather than assumed.
    let defaults: Vec<&&EdgeSpec> = doors.iter().filter(|e| e.is_default).collect();
    let mut lanes: Vec<String> = defaults
        .iter()
        .map(|e| {
            let c = e.condition.as_deref().unwrap_or_default();
            let at = c
                .find("hop.route == '")
                .unwrap_or_else(|| panic!("a default with no lane guard: {e:#?}"));
            let rest = &c[at + "hop.route == '".len()..];
            rest[..rest.find('\'').expect("closing quote")].to_string()
        })
        .collect();
    let before = lanes.len();
    lanes.sort();
    lanes.dedup();
    assert_eq!(
        lanes.len(),
        before,
        "two guarded defaults name the SAME lane — they do not compete, so both \
         would fire on the same message: {defaults:#?}"
    );
    assert!(
        defaults.iter().any(|e| e.to == "./unknown"),
        "the tool_call default no longer leads to ./unknown: {defaults:#?}"
    );
    for d in &defaults {
        assert!(
            d.condition
                .as_deref()
                .is_some_and(|c| c.contains("hop.route")),
            "a default is GUARDED — an unguarded one is legal but only earns a boot \
             advisory, and says nothing about which traffic it consumes: {:?}",
            d.condition
        );
    }

    for door in doors.iter().filter(|e| !e.is_default).filter(|e| {
        e.condition
            .as_deref()
            .is_some_and(|c| c.contains("'tool_call'"))
    }) {
        let c = door.condition.as_deref().unwrap_or_default();
        assert!(
            c.contains("hop.tool_name"),
            "a positive door that does not read `hop.tool_name` would overlap its \
             siblings, and overlapping positives stay fan-out: {door:#?}"
        );
        // Ruling W7-R4 — the structural half of the loop guard. An occupant's
        // answer travels back through this very hive path; without the lane term
        // a door asks only about `hop.tool_name` and would hand an answer that
        // carried it straight back to its own sender, round after round until
        // the TTL runs out. The term also makes the three doors symmetric with
        // the guarded default, which has read the lane since it was written.
        assert!(
            c.contains("hop.route == 'tool_call'"),
            "a positive door that does not first ask for the `tool_call` lane \
             dispatches on the return path too (W7-R4): {door:#?}"
        );
    }

    // The return direction: one exit per occupant, each STATING the outward lane.
    // Since R6 the hive has TWO outward lanes, and only one of them is a
    // result: `tool_result` (one exit per occupant) and `build` (the reach of
    // the surface, from the two occupants that have one). The result half is
    // what this test is about, so it is the half that is filtered for.
    let exits: Vec<&EdgeSpec> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.to == ".")
        .filter(|e| {
            e.modifier
                .as_ref()
                .and_then(|m| m.set_hop.get("route"))
                .map(String::as_str)
                == Some("'tool_result'")
        })
        .collect();
    assert_eq!(exits.len(), 6, "one result exit per occupant: {exits:#?}");
    for exit in &exits {
        assert!(
            !exit.is_default,
            "an exit is never a default: suppression is per SENDER, and these \
             senders are the occupants, not the hive path: {exit:#?}"
        );
        assert_eq!(
            exit.modifier
                .as_ref()
                .and_then(|m| m.set_hop.get("route"))
                .map(String::as_str),
            Some("'tool_result'"),
            "every occupant answers on the one outward lane: {exit:#?}"
        );
    }
}
