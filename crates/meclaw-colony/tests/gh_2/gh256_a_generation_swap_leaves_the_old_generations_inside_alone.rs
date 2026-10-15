//! GH #256 — a `swap_nodes` over a SUBTREE must not carry the subtree's own
//! inward/outward wiring onto the successor.
//!
//! `swap_nodes` re-dedicates the **external** edges of a node onto another one.
//! For a leaf that is unambiguous. For a subtree there are three classes of
//! edge, not two: from outside in, from inside out, and **inside**. The inside
//! class is not only `unit/a → unit/b`; it also holds the two forms in which
//! the subtree ROOT wires its own children — `unit → unit/child` (the mandated
//! hive-boundary form: "inside, the hive distributes on its own, with edges
//! whose `from` is itself") and `unit/child → unit`. Both name the root exactly,
//! so the exact-path swing used to carry them over: the successor ended up
//! addressing the OLD generation's cells and the old generation's cells
//! answering the successor. One turn then ran through BOTH generations.
//!
//! # Why the pin is on the SECOND change
//!
//! A channel ships its generation slot occupied by a `terminal`, and a terminal
//! is a LEAF — it has no inward wiring to drag. So the first generation change
//! is correct no matter what the swing does with the inside class, and a test
//! over a single change is green either way. The defect starts at the second
//! change, when the slot is occupied by a subtree. This test therefore does the
//! whole sequence: `terminal → gen2` (leaf, must stay correct) and then
//! `gen2 → gen3` (subtree, the pin).
//!
//! # Semantics pinned here
//!
//! The inside class stays with the subtree it belongs to. After the second
//! swap:
//!   - `/t1 → /gen3` carries the lane (external edge, swung as always);
//!   - `/gen3` has NO edge into `/gen2`'s subtree and `/gen2`'s subtree has
//!     none into `/gen3` — the two generations are not cross-wired;
//!   - `/gen2 → /gen2/worker` is still there — the disconnected generation is
//!     PRESERVED whole, which is what makes a swing-back restore a working
//!     unit rather than a hollow one;
//!   - positive receipt: one probe reaches the NEW generation's sink, and the
//!     old generation's sink stays silent.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ContractView, DiskBlobStore, MutationOutcome,
    RespawnFn, SpawnedCellKind, bootstrap_from_filesystem, cell_task,
};
use meclaw_core::{
    Body, CellEmission, JsonValue, Message, MessageBuilder, Path, Uuid, serde_json::json,
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
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

/// The persisted edge list as `(from, to)` pairs.
async fn edges(h: &ColonyHandle) -> Vec<(String, String)> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .edges
        .into_iter()
        .map(|e| (e.from, e.to))
        .collect()
}

fn has_edge(all: &[(String, String)], from: &str, to: &str) -> bool {
    all.iter().any(|(f, t)| f == from && t == to)
}

// ─────────────────────────────────────────────────────────────────────────────
// `echo_sub` factory — an echo cell that also carries a boot-inactive respawn,
// so a subtree cell that the swap reconnects comes alive immediately (a plain
// `EchoCellFactory` has no `build_boot_inactive_respawn` and would stay inert).
// Mirrors the `SubtreeEchoFactory` of `paket_2_swap.rs`.
// ─────────────────────────────────────────────────────────────────────────────

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
            cell_task(p, rx, o, cell, None, None, None).await;
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

fn factories_with_subtree_echo() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo_sub".into(),
        Arc::new(SubtreeEchoFactory) as Arc<dyn CellFactory>,
    );
    r
}

const CELL: &str = r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}"#;

/// Write a generation SUBTREE TEMPLATE: a hive that distributes inward to its
/// one worker — the mandated hive-boundary form — plus that worker.
fn write_generation_template(root: &std::path::Path, name: &str, sink: &str) {
    let tpl = root.join("templates").join(name);
    write(&tpl, "template.json", &format!(r#"{{"name":"{name}"}}"#));
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./worker"}
        ]}}}"#,
    );
    write(
        &tpl,
        "worker/config.json",
        &format!(r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"{sink}"}},{CELL}}}"#),
    );
}

// ── The test ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generation_swap_leaves_the_old_generations_inside_alone() {
    let td = TempDir::new().unwrap();

    // Root hive: the ingress lane `/t1 → /terminal` — the generation slot is
    // occupied by a LEAF at instantiation, exactly as a channel ships it.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/t1","to":"/terminal"}
        ]}}}"#,
    );
    write(
        td.path(),
        "main/t1/config.json",
        &format!(r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/t1"}},{CELL}}}"#),
    );
    // The slot occupant a channel ships: a LEAF. It is never probed — its only
    // job here is to be the thing the FIRST generation change replaces.
    write(
        td.path(),
        "main/terminal/config.json",
        &format!(r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/cap2"}},{CELL}}}"#),
    );
    // Two generation templates, each a SUBTREE with its own sink so a receipt
    // says WHICH generation answered.
    write_generation_template(td.path(), "gen2_hive", "/cap2");
    write_generation_template(td.path(), "gen3_hive", "/cap3");

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo_sub".to_string(),
            Arc::new(SubtreeEchoFactory) as Arc<dyn CellFactory>,
        )],
    );
    rescan_templates(&h, td.path().join("templates")).await;

    // Anti-cascade: every sink is resolved BEFORE bootstrap and probe.
    let (c2_tx, mut c2_rx) = mpsc::channel(16);
    let (c3_tx, mut c3_rx) = mpsc::channel(16);
    h.spawn(Path::new("/cap2"), move || CaptureCell::new(c2_tx.clone()))
        .await;
    h.spawn(Path::new("/cap3"), move || CaptureCell::new(c3_tx.clone()))
        .await;

    bootstrap_from_filesystem(td.path(), &factories_with_subtree_echo(), &h.runtime())
        .await
        .expect("bootstrap succeeds");

    // ── Generation change #1: terminal (LEAF) → gen2 (SUBTREE). ─────────────
    // The shape a channel uses: `add_nodes` puts the generation in the scope,
    // `swap_nodes` swings the slot's external edges onto it, one mutation.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"gen2","template":"gen2_hive"}],
            "swap_nodes":[{"match":{"name":"terminal"},"with":{"name":"gen2"}}],
            "add_edges":[{"from":"gen2/worker","to":"cap2"}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the first generation change must commit; got {outcome:?}"
    );

    let after_first = edges(&h).await;
    assert!(
        has_edge(&after_first, "/t1", "/gen2"),
        "the ingress lane must name the first generation; got {after_first:?}"
    );
    assert!(
        has_edge(&after_first, "/gen2", "/gen2/worker"),
        "gen2's own inward wiring must be intact; got {after_first:?}"
    );

    // Positive receipt: a probe reaches the first generation's sink.
    h.send(
        MessageBuilder::new(Path::new("/t1"))
            .body(Body::Inline(
                json!({"messages":[{"origin":"user","type":"text","text":"first"}]}),
            ))
            .build(),
    )
    .await;
    match tokio::time::timeout(Duration::from_secs(30), c2_rx.recv()).await {
        Ok(Some(m)) => assert_eq!(m.target, Path::new("/cap2")),
        Ok(None) => panic!("/cap2 rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("/cap2 must receive after the FIRST generation change; DLQ: {dlq:?}");
        }
    }

    // ── Generation change #2: gen2 (SUBTREE) → gen3 (SUBTREE). ──────────────
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"gen3","template":"gen3_hive"}],
            "swap_nodes":[{"match":{"name":"gen2"},"with":{"name":"gen3"}}],
            "add_edges":[{"from":"gen3/worker","to":"cap3"}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the second generation change must commit; got {outcome:?}"
    );

    let after = edges(&h).await;

    // The external lane swings, as always.
    assert!(
        has_edge(&after, "/t1", "/gen3"),
        "the ingress lane must name the new generation; got {after:?}"
    );
    assert!(
        !has_edge(&after, "/t1", "/gen2"),
        "the old ingress lane must be gone; got {after:?}"
    );

    // GH #256 — the two generations must not be cross-wired.
    assert!(
        !has_edge(&after, "/gen3", "/gen2/worker"),
        "the new generation must NOT address the old generation's cells; got {after:?}"
    );
    assert!(
        !after
            .iter()
            .any(|(f, t)| f.starts_with("/gen2/") && t == "/gen3"),
        "the old generation's cells must NOT answer the new generation; got {after:?}"
    );

    // The old generation is preserved WHOLE — its inside stayed with it, which
    // is what makes it swappable back.
    assert!(
        has_edge(&after, "/gen2", "/gen2/worker"),
        "the disconnected generation must keep its own inward wiring; got {after:?}"
    );
    // The new generation kept its own — nothing was duplicated onto it either.
    assert_eq!(
        after
            .iter()
            .filter(|(f, t)| f == "/gen3" && t == "/gen3/worker")
            .count(),
        1,
        "the new generation carries its own inward wiring exactly once; got {after:?}"
    );

    // ── Positive receipt: ONE probe, and only the new generation answers. ────
    h.send(
        MessageBuilder::new(Path::new("/t1"))
            .body(Body::Inline(
                json!({"messages":[{"origin":"user","type":"text","text":"second"}]}),
            ))
            .build(),
    )
    .await;
    match tokio::time::timeout(Duration::from_secs(30), c3_rx.recv()).await {
        Ok(Some(m)) => assert_eq!(
            m.target,
            Path::new("/cap3"),
            "the probe must reach the new generation's sink"
        ),
        Ok(None) => panic!("/cap3 rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("/cap3 must receive after the SECOND generation change; DLQ: {dlq:?}");
        }
    }
    assert!(
        c2_rx.try_recv().is_err(),
        "the old generation must not run the turn a second time"
    );
    h.shutdown().await;
}
