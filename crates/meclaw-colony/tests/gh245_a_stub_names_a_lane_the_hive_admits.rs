//! GH #245 — the lane a curator stub names is a lane the hive admits.
//!
//! The collector's curator compresses a tool round by taking the payload out and
//! leaving a stub whose text says, in words, how to get it back:
//!
//! ```text
//! [elided tool_result w0 - 4211 chars - tool=read_file - kind=repeatable
//!  - sha256:2f5d… - recall: thread_recall(call_id="w0")]
//! ```
//!
//! `thread_recall` is served by the collector itself, out of its own round
//! table, on the `in_thread_call` lane — and until `collector@2.0.1` no contract
//! in the library declared that lane. The script dispatched on it, the README
//! documented it, and `params.contract.accepts` did not list it, so the one edge
//! that makes the tool reachable was refused at mutation time with
//! `hive_contract`. Every stub the curator left was a dead end: the model asked
//! for the payload, the call reached nobody, and the round stalled until
//! `round_idle_ms` closed it — once per elided item.
//!
//! # The two claims, and why the second one is the honest half
//!
//! 1. **The lane is declared** — an edge stamping `in_thread_call` into the hive
//!    commits instead of being refused.
//! 2. **The declaration is true of the routing** — a real message on that lane,
//!    sent to the shipped `talky`'s own path, reaches the cell inside the
//!    collector that serves the tool, and its answer leaves through both hive
//!    boundaries again. A contract entry alone would only prove intent; this is
//!    the half that proves the door.
//!
//! And one guard against the drift going the other way: `in_batch` was declared
//! by a collector whose state machine never dispatched on it — a lane into
//! silence, the same defect inverted. It left in the same version, so an edge
//! that names it is refused now rather than parked forever.
//!
//! # What is under test, and how honestly
//!
//! The hive files are the SHIPPED artefacts, read from `templates/` and planted
//! verbatim: `templates/collector/config.json` and `templates/talky/config.json`
//! carry the contracts and the door edges this file is about. What is substituted
//! is only what this crate cannot spawn: `assemble` is a `code` cell and `window`
//! is a `store` cell, both of which live in `meclaw-cells` (downstream of here),
//! so they stand in as echo cells — as do talky's leaf cells. The check under
//! test reads the hive's own `params`, and those are the real ones; the cells
//! behind the doors only have to exist and be interior for the door to be a door.
//!
//! No model and no network.

use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

/// The repository's `templates/` directory — the shipped library, not a fixture.
fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let dir = root.join(rel);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), body).unwrap();
}

/// Plant a shipped hive `config.json` verbatim at `rel`.
fn plant(root: &std::path::Path, rel: &str, template: &str) {
    let src = templates_root().join(template).join("config.json");
    let body = std::fs::read_to_string(&src)
        .unwrap_or_else(|e| panic!("the shipped {template}/config.json must be readable: {e}"));
    write(root, rel, &body);
}

/// An echo cell standing in for a cell this crate cannot spawn. `emitted_header`
/// is what lets a stand-in produce the route its hive's out-door is looking for.
fn echo_cell(root: &std::path::Path, rel: &str, emitted_target: &str, route: Option<&str>) {
    let header = match route {
        Some(r) => format!(r#","emitted_header":{{"key":"route","value":"{r}"}}"#),
        None => String::new(),
    };
    write(
        root,
        rel,
        &format!(
            r#"{{"cell":{{"type":"echo"}},
                "params":{{"emitted_target":"{emitted_target}"{header}}},
                "contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    );
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
        .unwrap();
    ack_rx.await.unwrap()
}

async fn read_graph(h: &ColonyHandle) -> ReadGraphReply {
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

/// The shipped collector as a hive of its own, with a caller beside it. The two
/// interior cells are stand-ins (see the module note); the hive file is real.
fn write_collector_topology(root: &std::path::Path) {
    write(
        root,
        "main",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    echo_cell(root, "main/caller", "/caller", None);
    plant(root, "main/collector", "collector");
    echo_cell(
        root,
        "main/collector/assemble",
        "/collector",
        Some("answer"),
    );
    echo_cell(root, "main/collector/window", "/collector", None);
}

/// The caller addresses the HIVE and names a lane — no cell of the hive appears
/// in the edge, which is the whole point of a lane contract.
fn wire_lane(route: &str) -> Value {
    json!({"diff": {"add_edges": [
        {"from": "./caller", "to": "./collector",
         "modifier": {"set_hop": {"route": format!("'{route}'")}}}
    ]}})
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");
    h
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_collector_admits_the_lane_its_own_stubs_name() {
    let td = tempfile::TempDir::new().unwrap();
    write_collector_topology(td.path());
    let h = boot(&td).await;

    match send_mutation(&h, wire_lane("in_thread_call")).await {
        MutationOutcome::Committed { .. } => {}
        other => panic!(
            "the collector serves thread_recall on in_thread_call and its stubs \
             print that name — the lane has to be wireable, got {other:?}"
        ),
    }
    let edges: Vec<(String, String)> = read_graph(&h)
        .await
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    assert!(
        edges.contains(&("/caller".to_string(), "/collector".to_string())),
        "the caller wired the hive path, not a cell inside it: {edges:?}"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_collector_refuses_a_lane_nothing_behind_its_door_reads() {
    // `in_batch` was declared and never dispatched on: a caller could wire it,
    // the mutation passed, and every message on it was swallowed in silence.
    // A contract that admits a lane it cannot serve is the same lie as one that
    // hides a lane it does serve, so it left with 2.0.1. This assertion is also
    // what keeps the test above from being vacuous — the check is live here.
    let td = tempfile::TempDir::new().unwrap();
    write_collector_topology(td.path());
    let h = boot(&td).await;

    let outcome = send_mutation(&h, wire_lane("in_batch")).await;
    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "hive_contract", "{outcome:?}");
            assert!(
                details.contains("in_batch") && details.contains("in_thread_call"),
                "the refusal names the lane asked for and the lanes on offer: {details}"
            );
        }
        other => panic!("a lane the state machine never reads must be refused, got {other:?}"),
    }
    h.shutdown().await;
}

/// The shipped `talky`, carrying the shipped `collector` as its sub-unit. Only
/// the leaves are stand-ins.
fn write_talky_topology(root: &std::path::Path) {
    write(
        root,
        "main",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./agent","to":"./sink","condition":"has(hop.route) && hop.route == 'answer'"}
        ]}}}"#,
    );
    plant(root, "main/agent", "talky");
    plant(root, "main/agent/collector", "collector");
    // The assembler's stand-in answers on a route that is not an `in_` lane, so
    // the collector's own out-door carries it back over both hive boundaries —
    // the same shape the real cell's `thread_result` emission takes.
    echo_cell(
        root,
        "main/agent/collector/assemble",
        "/agent",
        Some("answer"),
    );
    echo_cell(
        root,
        "main/agent/collector/window",
        "/agent/collector",
        None,
    );
    for leaf in [
        "session-keeper",
        "brain",
        // The sidecar splitter of talky@4.1.0 (GH #379). It carries no lane this
        // test drives -- it sits on the answer path -- but the shipped edge set
        // names it, and an endpoint nothing stands at is a DanglingEndpoint that
        // refuses the whole boot.
        "splitter",
        "dispatcher",
        "summarizer",
        "errors",
    ] {
        echo_cell(root, &format!("main/agent/{leaf}"), "/agent", None);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_recall_call_reaches_the_assembler_inside_the_shipped_talky() {
    // The claim the contract entry alone cannot make: the composite's door edge
    // forwards the lane, the collector's door edge takes it in, and the cell
    // that serves the tool is what the message arrives at. Before 3.0.1 talky's
    // door enumerated the lanes it forwards and this one was not among them, so
    // even a caller who got past the contract would have watched the call stop
    // at the composite's edge.
    let td = tempfile::TempDir::new().unwrap();
    write_talky_topology(td.path());

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    // Anti-cascade: the terminal exists before anything is sent towards it.
    let (sink_tx, mut sink_rx) = mpsc::channel(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the shipped composite must boot");

    let probe = MessageBuilder::new(Path::new("/agent"))
        .body(Body::Inline(json!({
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "tr-1",
                          "text": "{\"call_id\":\"w0\"}"}]
        })))
        .hop({
            let mut m = meclaw_core::serde_json::Map::new();
            m.insert("route".into(), json!("in_thread_call"));
            m
        })
        .ttl(16)
        .build();
    let trace_id = probe.trace_id.to_string();
    h.send(probe).await;

    let got = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect(
            "the sink went quiet — a thread_recall call sent to the composite \
             never reached the cell that serves it",
        )
        .expect("sink channel closed");
    assert_eq!(got.target, Path::new("/sink"));

    // The positive receipt above says the round closed; this says WHERE it went.
    // A lane that reached the composite and stopped at its door would have left
    // the first two rows and nothing after them.
    let db = td.path().join("colony.db");
    meclaw_testing::wait::wait_for_message_log_count(&db, &trace_id, 6, Duration::from_secs(30))
        .await;
    let conn = rusqlite::Connection::open(&db).expect("colony.db");
    let mut stmt = conn
        .prepare("SELECT from_path, to_path FROM message_log WHERE trace_id = ? ORDER BY rowid")
        .unwrap();
    let chain: Vec<(String, String)> = stmt
        .query_map([&trace_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        chain,
        vec![
            ("@external".to_string(), "/agent".to_string()),
            ("/agent".to_string(), "/agent/collector".to_string()),
            (
                "/agent/collector".to_string(),
                "/agent/collector/assemble".to_string()
            ),
            (
                "/agent/collector/assemble".to_string(),
                "/agent/collector".to_string()
            ),
            ("/agent/collector".to_string(), "/agent".to_string()),
            ("/agent".to_string(), "/sink".to_string()),
        ],
        "the lane has to cross BOTH hive boundaries and land on the assembler — \
         that is the difference between a declared lane and a served one"
    );

    h.shutdown().await;
}
