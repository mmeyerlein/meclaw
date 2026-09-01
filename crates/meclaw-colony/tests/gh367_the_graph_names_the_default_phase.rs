//! GH #367 — `/colony/graph` names an edge's routing phase, and the boot
//! probes read it.
//!
//! W4 gave the substrate a default edge (GH #283): a `"default": true` out-edge
//! is consulted only after every ordinary out-edge of the same sender declined.
//! The phase was declarable, persisted, part of edge identity and survived
//! rewiring — but it was invisible from outside. `GraphEdgeDto` carried
//! endpoints, `condition` and `modifier` and nothing about the phase, so a
//! reader of `/colony/graph` saw a default edge and an ordinary one as the same
//! object.
//!
//! That gap had a second, load-bearing consequence. The boot contract probes
//! (`warn_on_broken_contracts`, `warn_on_missing_drains`) do not read the live
//! edge table — they REBUILD one out of `/colony/graph` and then ask it routing
//! questions. With no phase on the wire the rebuilt table said `false` for every
//! edge, so both probes judged a topology that is not the one the colony runs:
//! a default edge appeared in phase one, where it fires beside the regular arms
//! instead of after them.
//!
//! What this file pins:
//!   (a) the wire shape — an edge object carries a `default` key, always, on
//!       both values;
//!   (b) a colony booted with a declared default edge answers `/colony/graph`
//!       with the flag set on that edge and clear on its regular neighbour;
//!   (c) the fifth term of a `BootEdge` is the phase the graph reported, not a
//!       hardcoded `false`;
//!   (d) and that this changes a boot verdict: a hive whose only door to its
//!       interior is a default edge, shadowed by a regular out-edge that always
//!       fires, has NO door for that lane in the topology that actually runs —
//!       and the rebuilt table now says so.

use meclaw_colony::api_dto::{GraphEdgeDto, GraphNodeDto, ReadGraphReply};
use meclaw_colony::colony_dispatch::build_graph_read_reply;
use meclaw_colony::edge_table::{Edge, EdgeTable};
use meclaw_colony::mutation::hive_contract::{
    BootEdge, HiveContract, Lane, check_lane_doors, edge_table_from_boot_edges,
};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, CellStatus, ColonyMsg, RegistryEntry, boot_edges_from_graph,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{ActorHandle, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use tokio::sync::oneshot;

// ── Fixture (the shape `gh341_graph_filters_on_the_documented_shape` uses) ────

fn entry(path: &Path) -> RegistryEntry {
    let (sender, _receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
    RegistryEntry {
        handle: ActorHandle::new(path.clone(), sender),
        respawn: Box::new(|| unreachable!("fixture cell is never respawned")),
        wake: None,
        restart_count: 0,
        restart_limit: 5,
        cell_id: Uuid::now_v7(),
        cell_type: "echo".into(),
        status: CellStatus::Awake,
        eager_on_reconnect: true,
        active: true,
        failed: false,
        dormant: false,
        stop_tx: None,
        death_ack_rx: None,
    }
}

/// One sender, two out-edges: a guarded regular one and a declared default.
fn one_of_each_fixture() -> (std::collections::HashMap<Path, RegistryEntry>, EdgeTable) {
    let mut registry = std::collections::HashMap::new();
    let mut edges = EdgeTable::new();
    for cell in ["/a", "/regular", "/fallback"] {
        let p = Path::new(cell);
        registry.insert(p.clone(), entry(&p));
    }
    edges.insert(Edge {
        id: Uuid::now_v7(),
        from: Path::new("/a"),
        to: Path::new("/regular"),
        condition: None,
        modifier: None,
        is_default: false,
        lane: None,
    });
    edges.insert(Edge {
        id: Uuid::now_v7(),
        from: Path::new("/a"),
        to: Path::new("/fallback"),
        condition: None,
        modifier: None,
        is_default: true,
        lane: None,
    });
    (registry, edges)
}

fn edge_to<'a>(reply: &'a Value, to: &str) -> &'a Value {
    reply["graph"]["edges"]
        .as_array()
        .expect("the reply carries an edges array")
        .iter()
        .find(|e| e["to"] == to)
        .unwrap_or_else(|| panic!("no edge to {to} in {reply}"))
}

// ── (a) the wire shape ───────────────────────────────────────────────────────

/// The reply the dispatcher puts on the wire for `/colony/graph` — the same
/// `build_graph_read_reply` the `/colony/graph` arm of `dispatch_colony_read`
/// calls — carries the phase on every edge object.
///
/// The key is emitted ALWAYS, on both values, unlike the two optional fields
/// beside it. `condition` and `modifier` skip serialisation when they are
/// `None`, because for those an absent key IS the statement: this edge has no
/// condition. A routing phase is never absent — every edge runs in one of the
/// two — so omitting it on `false` would leave a reader unable to tell "this
/// edge is regular" from "this server does not report phases", which is exactly
/// the ambiguity the boot probes were caught in.
#[test]
fn every_edge_object_names_its_phase() {
    let (registry, edges) = one_of_each_fixture();
    let reply = build_graph_read_reply(&registry, &edges, &json!({}));

    assert_eq!(
        edge_to(&reply, "/fallback")["default"],
        json!(true),
        "a default edge reports its phase"
    );
    assert_eq!(
        edge_to(&reply, "/regular")["default"],
        json!(false),
        "a regular edge reports the phase too — the key is never omitted"
    );
}

/// A round-trip through the DTO keeps the key: a client that deserialises the
/// answer back into `GraphEdgeDto` reads the same phase.
#[test]
fn the_phase_survives_the_dto_round_trip() {
    let (registry, edges) = one_of_each_fixture();
    let reply = build_graph_read_reply(&registry, &edges, &json!({}));

    let parsed: Vec<GraphEdgeDto> =
        meclaw_core::serde_json::from_value(reply["graph"]["edges"].clone())
            .expect("the edge array deserialises back into the DTO");
    let fallback = parsed
        .iter()
        .find(|e| e.to == "/fallback")
        .expect("the default edge round-trips");
    assert!(fallback.is_default, "the phase survives the round trip");
}

// ── (b) a real colony answers with the flag ──────────────────────────────────

async fn read_graph_root(h: &ColonyHandle) -> ReadGraphReply {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// A colony booted from a `params.graph` that declares one guarded regular edge
/// and one default edge. Read back over `/colony/graph`, the two are
/// distinguishable — which is the whole report of #367.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_booted_default_edge_is_visible_in_the_graph_read() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./a","to":"./regular","condition":"hop.kind == 'work'"},
            {"from":"./a","to":"./fallback","default":true}
        ]}}}"#,
    );
    for name in ["a", "regular", "fallback"] {
        write(
            td.path(),
            &format!("main/{name}/config.json"),
            r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/dev/null"},
                "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
    }

    let h = ColonyHandle::new_with_echo_at(td.path());
    let mut factories = CellFactoryRegistry::new();
    factories.insert(
        "echo".to_string(),
        std::sync::Arc::new(EchoCellFactory) as std::sync::Arc<dyn CellFactory>,
    );
    bootstrap_from_filesystem(td.path(), &factories, &h.runtime())
        .await
        .expect("the tree boots");

    let graph = read_graph_root(&h).await;

    let find = |to: &str| {
        graph
            .edges
            .iter()
            .find(|e| e.to == to)
            .unwrap_or_else(|| panic!("edge to {to} must be in the graph: {:?}", graph.edges))
            .clone()
    };
    assert!(
        find("/fallback").is_default,
        "the declared default edge reports its phase over /colony/graph"
    );
    assert!(
        !find("/regular").is_default,
        "the regular edge beside it does not"
    );

    h.shutdown().await;
}

// ── (c) the fifth term of a BootEdge ─────────────────────────────────────────

fn dto(from: &str, to: &str, is_default: bool) -> GraphEdgeDto {
    GraphEdgeDto {
        id: Uuid::now_v7().to_string(),
        from: from.to_string(),
        to: to.to_string(),
        condition: None,
        modifier: None,
        is_default,
        // GH #559: no lane — this fixture is about the routing phase.
        lane: None,
    }
}

fn reply_with(edges: Vec<GraphEdgeDto>) -> ReadGraphReply {
    ReadGraphReply {
        scope: "/".to_string(),
        graph_version: 0,
        nodes: Vec::<GraphNodeDto>::new(),
        edges,
    }
}

/// The boot probes rebuild their edge table out of this tuple. Before #367 the
/// fifth term was a literal `false` with a comment saying the DTO could not tell
/// — now it is what the graph reported.
#[test]
fn the_boot_edge_carries_the_phase_the_graph_reported() {
    let boot: Vec<BootEdge> = boot_edges_from_graph(&reply_with(vec![
        dto("/a", "/regular", false),
        dto("/a", "/fallback", true),
    ]));

    assert_eq!(boot.len(), 2);
    let phase = |to: &str| {
        boot.iter()
            .find(|e| e.1 == to)
            .unwrap_or_else(|| panic!("no boot edge to {to}"))
            .4
    };
    assert!(phase("/fallback"), "the default edge boots as a default");
    assert!(!phase("/regular"), "the regular one does not");
}

// ── (d) and it changes a verdict ─────────────────────────────────────────────

/// The reason the fifth term is not cosmetic.
///
/// `/h` is a contracted hive that accepts the lane `work`. Its only edge into
/// the interior is a DEFAULT edge; beside it sits an unconditional regular
/// out-edge to a cell outside the hive. In the topology that actually runs the
/// regular edge fires first, phase two is never reached, and a message on
/// `work` never enters the hive — the lane has no door.
///
/// Read with every phase forced to `false` (the pre-#367 rebuild) the default
/// edge sits in phase one, fires beside the regular arm, and the check happily
/// reports a door that does not exist. The two verdicts differ, so the term is
/// load-bearing.
#[test]
fn the_rebuilt_table_judges_the_topology_that_runs() {
    let contract = HiveContract {
        hive_path: "/h".to_string(),
        accepts: vec![Lane {
            route: "work".to_string(),
            context: Vec::new(),
            at: Vec::new(),
            because: "the lane this hive promises".to_string(),
        }],
        emits: Vec::new(),
    };
    let edges = vec![
        ("/caller".to_string(), "/h".to_string(), None, None, false),
        (
            "/h".to_string(),
            "/elsewhere".to_string(),
            None,
            None,
            false,
        ),
        ("/h".to_string(), "/h/inside".to_string(), None, None, true),
    ];

    let honest = edge_table_from_boot_edges(&edges);
    assert!(
        check_lane_doors(std::slice::from_ref(&contract), &honest).is_err(),
        "the door is a shadowed default edge — the running topology has none"
    );

    let flattened: Vec<BootEdge> = edges
        .iter()
        .map(|(f, t, c, m, _)| (f.clone(), t.clone(), c.clone(), m.clone(), false))
        .collect();
    let pre_367 = edge_table_from_boot_edges(&flattened);
    assert!(
        check_lane_doors(std::slice::from_ref(&contract), &pre_367).is_ok(),
        "with the phase flattened the check sees a door that does not run — \
         the exact blind spot #367 closes"
    );
}
