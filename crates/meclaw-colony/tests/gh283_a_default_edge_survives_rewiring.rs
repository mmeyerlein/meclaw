//! GH #283 — the default phase has to survive the two paths that do not carry
//! an [`meclaw_colony::Edge`] through but REBUILD one from a struct.
//!
//! Tasks 1–4 gave the substrate a default edge: the router consults it only
//! after every ordinary out-edge of the same sender declined (T1), it is
//! declarable in a `params.graph` (T2), it persists (T3) and it is part of edge
//! identity (T4). All of that keeps working as long as an edge is MOVED. Two
//! production paths do not move edges, they re-create them:
//!
//!   (a) `swap_nodes` and `move_nodes` both plan through
//!       `mutation::swap::plan_edge_swing`, which rebuilds every touched edge
//!       from a `SwungEdge` — a struct that listed `from`, `to`, `condition`,
//!       `modifier` and the two source strings, and nothing else. Both consumer
//!       sites in `colony.rs` then inserted `is_default: false` literally. A
//!       swapped-in generation or a relocated cell therefore kept its catch-all
//!       lane as an ORDINARY edge: it fires beside the regular one, which is
//!       double delivery on exactly the tool surface #283 reports.
//!   (b) Subtree instantiation resolves a template's `params.graph` through
//!       `subtree::EdgeSpec` → `subtree::ResolvedEdge`, neither of which read
//!       the flag. A composition template shipping a default edge would be
//!       right in the template and wrong in every instance. This is also the
//!       path a `ref`'d sub-template travels (W3, `expand_ref` →
//!       `edge_spec_from_config`), so the same carry covers refs.
//!
//! # What is asserted, and why it is behaviour
//!
//! Every case asserts the ROUTING, not the field: one probe whose headers match
//! the regular edge must reach the regular sink and leave the default sink
//! silent, and one probe that matches nothing must reach the default sink. That
//! is `EdgeTable::apply_edges` run by the colony over its own post-mutation
//! table — a field assertion would still pass over a table that routes wrongly
//! for some other reason, and the defect this file locks IS a routing defect.
//!
//! When this file was written `/colony/graph` did not expose the flag (a
//! `GraphEdgeDto` gap noted in W4 Task 4), so the post-state table could not be
//! rebuilt in the test process at all. GH #367 closed that gap — the graph read
//! now names the phase — but the assertions here deliberately stay on the
//! colony's own deliveries: a field assertion would still pass over a table
//! that routes wrongly for some other reason, and the defect this file locks IS
//! a routing defect. The graph-read half is pinned in
//! `gh367_the_graph_names_the_default_phase.rs`.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ContractView, DiskBlobStore, MutationOutcome,
    RespawnFn, SpawnedCellKind, bootstrap_from_filesystem, cell_task,
};
use meclaw_core::{
    Body, CellEmission, JsonValue, Message, MessageBuilder, Path, Uuid,
    serde_json::{Map, Value, json},
};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

// ── Harness ──────────────────────────────────────────────────────────────────

const CELL: &str = r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: JsonValue) -> MutationOutcome {
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

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

/// A probe addressed at `to`, carrying one `context` header pair.
///
/// The guard reads `context`, not `hop`, and that is forced rather than
/// stylistic: `hop` is the immediately-preceding cell's own output and is
/// REPLACED on every emission (`Headers::carry_context_with_hop`), so a hop key
/// set on the probe is gone by the time the probed cell's emission is routed.
/// `context` is the compartment that survives the hop, which is what a
/// two-hop routing assertion needs.
fn probe(to: &str, kind: &str) -> Message {
    let mut headers: Map<String, Value> = Map::new();
    headers.insert("kind".to_string(), json!(kind));
    MessageBuilder::new(Path::new(to))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"ping"}]}),
        ))
        .context(headers)
        .ttl(8)
        .build()
}

/// Bounded wait for a receipt (30s failure-marker convention). A miss prints
/// the dead-letter queue, which is where a mis-routed probe ends up.
async fn recv_one(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, ctx: &str) -> Message {
    match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("{ctx}: capture channel closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("{ctx}: no receipt within 30s; DLQ: {dlq:?}")
        }
    }
}

/// Assert nothing arrives — the semantic discriminator of this file, so the
/// window is short on purpose: the two emissions of a double delivery leave the
/// same `route()` call, so by the time the other sink has its receipt the wrong
/// one would already be in flight.
async fn assert_silent(rx: &mut mpsc::Receiver<Message>, ctx: &str) {
    let got = tokio::time::timeout(Duration::from_millis(700), rx.recv()).await;
    assert!(
        got.is_err(),
        "{ctx}: this sink must stay silent, got {got:?}"
    );
}

// ── An echo factory that can be woken after boot ──────────────────────────────
//
// A cell that boots without edges is registered boot-inactive; the swap that
// wires it must be able to wake it, which needs `build_boot_inactive_respawn`.
// Mirrors `SubtreeEchoFactory` of `paket_2_swap.rs` / the GH #256 test.

struct SubtreeEchoFactory;

fn parse_echo_to(params: &JsonValue) -> Result<Path, String> {
    params
        .get("echo_to")
        .and_then(|v| v.as_str())
        .map(Path::new)
        .ok_or_else(|| "params.echo_to missing or not a string".to_string())
}

fn make_echo_build(
    path: Path,
    echo_to: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
) -> impl Fn() -> (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
) {
    move || {
        let (tx, rx) = mpsc::channel::<Message>(1000);
        let (peace_tx, peace_rx) = oneshot::channel();
        let (_backstop_tx, backstop_rx) = oneshot::channel();
        let cell = EchoMockCell::new(path.clone()).emitted_target(echo_to.clone());
        let p = path.clone();
        let o = outputs_tx.clone();
        let join = tokio::spawn(async move {
            let _keep_peace = peace_tx;
            cell_task(p, rx, o, cell, None, None).await;
        });
        (tx, join, peace_rx, backstop_rx)
    }
}

impl CellFactory for SubtreeEchoFactory {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        parse_echo_to(params).map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let echo_to = parse_echo_to(&params)?;
        let build = make_echo_build(path, echo_to, outputs_tx);
        let (sender, join, peace_rx, backstop_rx) = build();
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let respawn: RespawnFn = Box::new(build);
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let echo_to = parse_echo_to(&params).ok()?;
        let build = make_echo_build(path, echo_to, outputs_tx);
        Some(Box::new(build))
    }
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo_sub".into(),
        Arc::new(SubtreeEchoFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo_sub".to_string(),
        Arc::new(SubtreeEchoFactory) as Arc<dyn CellFactory>,
    )]
}

/// A colony whose `/old` cell owns two out-edges: a guarded REGULAR one to
/// `/reg_sink` and an unguarded DEFAULT one to `/fb_sink`. `/new` exists,
/// edge-less, as the swap's successor.
async fn colony_with_a_default_lane() -> (
    TempDir,
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/old","to":"/reg_sink","condition":"context.kind == 'work'"},
            {"from":"/old","to":"/fb_sink","default":true}
        ]}}}"#,
    );
    for name in ["old", "new"] {
        write(
            td.path(),
            &format!("main/{name}/config.json"),
            &format!(
                r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/reg_sink"}},{CELL}}}"#
            ),
        );
    }

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());

    // Anti-cascade: both sinks are resolved BEFORE bootstrap and probe.
    let (reg_tx, reg_rx) = mpsc::channel(16);
    let (fb_tx, fb_rx) = mpsc::channel(16);
    h.spawn(Path::new("/reg_sink"), move || {
        CaptureCell::new(reg_tx.clone())
    })
    .await;
    h.spawn(Path::new("/fb_sink"), move || {
        CaptureCell::new(fb_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap succeeds");

    (td, h, reg_rx, fb_rx)
}

/// The two probes that read a sender's default phase off its deliveries.
async fn assert_default_phase_at(
    h: &ColonyHandle,
    sender: &str,
    reg_rx: &mut mpsc::Receiver<Message>,
    fb_rx: &mut mpsc::Receiver<Message>,
    ctx: &str,
) {
    // 1. A message the REGULAR edge takes. The default must stay silent — that
    //    is the whole of the phase, and the half a dropped flag breaks.
    h.send(probe(sender, "work")).await;
    let got = recv_one(h, reg_rx, &format!("{ctx}: the regular lane")).await;
    assert_eq!(got.target, Path::new("/reg_sink"));
    assert_silent(
        fb_rx,
        &format!("{ctx}: the default must not fire beside a regular edge"),
    )
    .await;

    // 2. A message no regular edge takes. Now the default is the consumer of
    //    what would otherwise dead-letter as `no_route`.
    h.send(probe(sender, "play")).await;
    let got = recv_one(h, fb_rx, &format!("{ctx}: the default lane")).await;
    assert_eq!(got.target, Path::new("/fb_sink"));
    assert_silent(
        reg_rx,
        &format!("{ctx}: the guarded regular edge must not fire"),
    )
    .await;
}

// ── (a) a swapped default edge is still a default ────────────────────────────

/// `swap_nodes` re-dedicates `/old`'s external edges onto `/new`. Both of them
/// swing — and the one that was a default has to arrive as one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swapped_default_edge_is_still_a_default() {
    let (_td, h, mut reg_rx, mut fb_rx) = colony_with_a_default_lane().await;

    // Pre-state receipt: the phase works BEFORE the swap, so a red assertion
    // after it can only be the swap's doing.
    assert_default_phase_at(&h, "/old", &mut reg_rx, &mut fb_rx, "before the swap").await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "swap_nodes":[{"match":{"name":"old"},"with":{"name":"new"}}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the swap must commit; got {outcome:?}"
    );

    assert_default_phase_at(&h, "/new", &mut reg_rx, &mut fb_rx, "after the swap").await;

    h.shutdown().await;
}

// ── (a′) a relocated default edge is still a default ─────────────────────────

/// `move_nodes` plans through the SAME `plan_edge_swing`, at the second of its
/// two consumer sites. A relocated cell keeps its identity and its `cell.db` —
/// it must also keep the phase of the edges that name it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relocated_default_edge_is_still_a_default() {
    let (_td, h, mut reg_rx, mut fb_rx) = colony_with_a_default_lane().await;

    assert_default_phase_at(&h, "/old", &mut reg_rx, &mut fb_rx, "before the move").await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "move_nodes":[{"match":{"name":"old"},"to":"moved"}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the move must commit; got {outcome:?}"
    );

    assert_default_phase_at(&h, "/moved", &mut reg_rx, &mut fb_rx, "after the move").await;

    h.shutdown().await;
}

// ── (b) an instantiated default edge is still a default ──────────────────────

/// A SUBTREE template declaring a guarded regular edge and a default in its
/// hive's `params.graph`, instantiated by a mutation. The instance has to route
/// the way the template reads — including through a `ref`, which reaches
/// `edge_spec_from_config` on the same path (W3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_instantiated_default_edge_is_still_a_default() {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );

    // The composition template: one sender, one regular arm, one default arm.
    let tpl = td.path().join("templates").join("unit_tpl");
    write(&tpl, "template.json", r#"{"name":"unit_tpl"}"#);
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./src","to":"./regular","condition":"context.kind == 'work'"},
            {"from":"./src","to":"./fallback","default":true}
        ]}}}"#,
    );
    write(
        &tpl,
        "src/config.json",
        &format!(r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/reg_sink"}},{CELL}}}"#),
    );
    write(
        &tpl,
        "regular/config.json",
        &format!(r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/reg_sink"}},{CELL}}}"#),
    );
    write(
        &tpl,
        "fallback/config.json",
        &format!(r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/fb_sink"}},{CELL}}}"#),
    );

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    rescan_templates(&h, td.path().join("templates")).await;

    let (reg_tx, mut reg_rx) = mpsc::channel(16);
    let (fb_tx, mut fb_rx) = mpsc::channel(16);
    h.spawn(Path::new("/reg_sink"), move || {
        CaptureCell::new(reg_tx.clone())
    })
    .await;
    h.spawn(Path::new("/fb_sink"), move || {
        CaptureCell::new(fb_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap succeeds");

    // Instantiate, and wire both arms out to their sinks in the same diff.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"unit","template":"unit_tpl"}],
            "add_edges":[
                {"from":"unit/regular","to":"reg_sink"},
                {"from":"unit/fallback","to":"fb_sink"}
            ]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the instantiation must commit; got {outcome:?}"
    );

    assert_default_phase_at(
        &h,
        "/unit/src",
        &mut reg_rx,
        &mut fb_rx,
        "the instantiated subtree",
    )
    .await;

    h.shutdown().await;
}
