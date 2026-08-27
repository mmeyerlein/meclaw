//! Paket-2 T11 — Demos (d), (e), (f) + C1 stop-wiring invariant:
//!
//! Demo (d) — completeness: multiple in/out edges, condition + modifier preserved verbatim.
//! Demo (e) — Reject vor-destruktiv: invalid swap rejected atomically, no partial state.
//! Demo (f) — Reboot durability: swung edges + inactive t2 survive colony reboot.
//! C1       — stop_wiring_unavailable: swap of Awake cell with no stop_tx atomically rejected.
//!
//! ─────────────────────────────────────────────────────────────────────────────────────────────
//! Paket-2 T5 — Demos (b') and (c) + Demo (a) (T4):
//!
//! Demo (a) — graph-swap (pure edge-swing) end-to-end (T4, single-cell form).
//!
//! Demo (b') — composite Hive→Hive (A1 pin-test, T5 Part 2):
//!   Bootstrap: t1 (single echo cell), t2 (hive with internal subtree), edge t1→t2.
//!   ONE composite diff: add_nodes(t3 via subtree template) + swap_nodes(t2→t3).
//!   After: edge swung to t1→t3; t2+subtree INACTIVE; t3+subtree ACTIVE.
//!   Pins the "only external edges count for hive connectivity" rule.
//!
//! Demo (c) — rollback / blue-green (T5 Part 3):
//!   After demo (a), apply a second swap diff swinging edges BACK to t2.
//!   After: t2 ACTIVE again (positive receipt); t3 INACTIVE.
//!   Pins No-Delete (t2 was retained and is fully re-activatable).
//!
//! Topology (assert_single_root_dir: `td/main` -> mc-path `/`):
//!   /t1   (echo, emitted_target=/t1 — the edge overrides the emit target anyway)
//!   /t2   (echo, emitted_target=/sink_t2)
//!   Edge (root hive config.json): from=/t1 to=/t2
//!   /sink_t2  (CaptureCell — registry-only via h.spawn, anti-cascade)
//!   /sink_t3  (CaptureCell — registry-only via h.spawn, anti-cascade)
//!
//! Mutation: ONE `swap_nodes` diff
//!   { "match": {"name":"t2"}, "with": {"template":"t3_echo","name":"t3","params":{...}} }
//!
//! AFTER the swap (graph-swap semantics):
//!   - the edge `/t1 -> /t2` is now `/t1 -> /t3` (swung verbatim);
//!   - `/t3` is registered + active;
//!   - `/t2` is inactive (edge-less → recompute flips it inactive).
//!
//! Positive-receipt proof (anti-cascade, sink-before-probe): a probe sent through
//! `/t1` reaches the CaptureCell behind `/t3` (via the swung edge → t3's
//! emitted_target=/sink_t3), while `/sink_t2` receives NOTHING.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ContractView, DbConn, DiskBlobStore,
    MutationOutcome, RespawnFn, SpawnedCellKind, bootstrap_from_filesystem, cell_task,
    cell_task_long_running, set_term_timeout_ms_for_test,
};
use meclaw_core::{
    Body, CellEmission, JsonValue, Message, MessageBuilder, Path, Uuid, serde_json::json,
};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::mocks::{EchoMockCell, MockEvent, ReceiptMockLongRunningCell};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: meclaw_core::JsonValue) -> MutationOutcome {
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

async fn registry_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
}

/// Read the persisted edge list (from, to) from the colony graph DTO.
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

/// Make a single-cell echo template `name` whose cell echoes to `echo`.
fn write_template(root: &std::path::Path, name: &str, echo: &str) {
    let tpl = root.join("templates").join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"{echo}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

fn factories_with_echo() -> meclaw_colony::CellFactoryRegistry {
    let mut r = meclaw_colony::CellFactoryRegistry::new();
    let echo: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);
    r.insert("echo".into(), echo);
    r
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graph_swap_swings_edge_t1_to_t3_and_deactivates_t2() {
    let td = TempDir::new().unwrap();
    // Root hive carries the single edge /t1 -> /t2.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/t1","to":"/t2"}
        ]}}}"#,
    );
    // /t1 echoes nominally to itself; the edge overrides the emit target.
    write(
        td.path(),
        "main/t1/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/t1"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // /t2 echoes to /sink_t2.
    write(
        td.path(),
        "main/t2/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink_t2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // t3 template: a fresh echo cell that echoes to /sink_t3.
    write_template(td.path(), "t3_echo", "/sink_t3");

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
        )],
    );
    rescan_templates(&h, td.path().join("templates")).await;

    // Anti-cascade: both sinks resolved BEFORE the probe.
    let (t2_tx, mut t2_rx) = mpsc::channel(16);
    let (t3_tx, mut t3_rx) = mpsc::channel(16);
    h.spawn(Path::new("/sink_t2"), move || {
        CaptureCell::new(t2_tx.clone())
    })
    .await;
    h.spawn(Path::new("/sink_t3"), move || {
        CaptureCell::new(t3_tx.clone())
    })
    .await;

    meclaw_colony::bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h.runtime())
        .await
        .expect("bootstrap with t1->t2 edge succeeds");

    // ── Pre-swap sanity: the edge is /t1 -> /t2. ─────────────────────────────
    let before = edges(&h).await;
    assert!(
        before.contains(&("/t1".to_string(), "/t2".to_string())),
        "pre-swap edge must be /t1 -> /t2; got {before:?}"
    );

    // ── SWAP: t2 -> fresh t3 (template t3_echo). ─────────────────────────────
    let outcome = send_mutation(
        &h,
        json!({
            "scope":"/",
            "diff":{"swap_nodes":[
                {"match":{"name":"t2"},"with":{"template":"t3_echo","name":"t3","params":{}}}
            ]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "graph-swap must commit; got {outcome:?}"
    );

    // ── AFTER: edge swung to /t1 -> /t3, old /t1 -> /t2 gone. ────────────────
    let after = edges(&h).await;
    assert!(
        after.contains(&("/t1".to_string(), "/t3".to_string())),
        "edge must swing to /t1 -> /t3; got {after:?}"
    );
    assert!(
        !after.contains(&("/t1".to_string(), "/t2".to_string())),
        "old /t1 -> /t2 edge must be gone; got {after:?}"
    );

    // /t3 registered + active.
    let t3 = registry_entry(&h, "/t3").await.expect("/t3 registered");
    assert_eq!(t3.cell_type, "echo");
    assert!(t3.active, "/t3 must be active after swap");

    // /t2 inactive (edge-less → recompute flipped it).
    let t2 = registry_entry(&h, "/t2").await.expect("/t2 still on disk");
    assert!(!t2.active, "/t2 must be inactive after swap; got {t2:?}");

    // ── Positive receipt: probe through /t1 reaches /sink_t3, NOT /sink_t2. ──
    // W2b (Ruling A1): /t3's terminal echo to /sink_t3 needs a wired catch-all
    // out-edge (identity-fallback gone). Both endpoints are live post-swap.
    assert!(
        matches!(
            send_mutation(
                &h,
                json!({"scope":"/","diff":{"add_edges":[{"from":"t3","to":"sink_t3"}]}})
            )
            .await,
            MutationOutcome::Committed { .. }
        ),
        "wiring /t3 -> /sink_t3 must commit"
    );
    let probe = MessageBuilder::new(Path::new("/t1"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"hi"}]}),
        ))
        .build();
    h.send(probe).await;

    let got = match tokio::time::timeout(Duration::from_secs(30), t3_rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("/sink_t3 rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("/sink_t3 must receive via swung edge within 30s; DLQ: {dlq:?}");
        }
    };
    assert_eq!(
        got.target,
        Path::new("/sink_t3"),
        "probe through /t1 must reach /sink_t3 (t3's echo target)"
    );
    // /sink_t2 must NOT have received anything.
    assert!(
        t2_rx.try_recv().is_err(),
        "/sink_t2 must NOT receive — t2 is edge-less + inactive after swap"
    );

    h.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// SubtreeEchoFactory — eager reconnect on add_edges (mirrors phase_13_5_a5_demo).
//
// EchoCellFactory does NOT implement `build_boot_inactive_respawn`, so a subtree
// cell instantiated inactive via `add_nodes` would never get an eager task on
// reconnect and therefore couldn't carry live traffic. This factory provides a
// real respawn so newly-swung subtree cells come alive immediately when the
// swung edge connects them.
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
        _message_timeout: Option<std::time::Duration>,
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
        _message_timeout: Option<std::time::Duration>,
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

/// Write the `t3_hive` subtree template:
///   root hive, internal edge ./child_a → ./child_b,
///   child_a echoes to /capture (proof cell).
///   child_b echoes to /capture (also; ensures it's wired).
fn write_t3_hive_template(root: &std::path::Path) {
    let tpl = root.join("templates").join("t3_hive");
    write(&tpl, "template.json", r#"{"name":"t3_hive"}"#);
    // Root hive with one internal edge: ./child_a → ./child_b.
    // This is INTERNAL wiring — does NOT self-connect t3 to the graph.
    // After the swap brings t1→t3, t3 gets an external edge → whole subtree active.
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./child_a","to":"./child_b"}
        ]}}}"#,
    );
    // child_a: entry cell; echoes to /capture (positive receipt sink).
    write(
        &tpl,
        "child_a/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/capture"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // child_b: downstream cell; also echoes to /capture.
    write(
        &tpl,
        "child_b/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/capture"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// Helper: read registry entry by path.
async fn registry_entry_full(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 200,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
}

/// Paket-2 T5 Demo (b') — composite Hive→Hive swap (A1 pin-test).
///
/// Bootstrap: t1 single cell; t2 hive with internal subtree (child_a→child_b);
/// edge t1→t2 in root hive config.json.
///
/// ONE composite diff: `add_nodes` t3 (t3_hive subtree template) + `swap_nodes`
/// (existing-form `with:{name:"t3"}`).
///
/// Apply order pinned: add_nodes registers t3 BEFORE swap lowering resolves it.
///
/// Assertions:
///   1. Edge swung: t1→t3 present, t1→t2 gone.
///   2. t2 INACTIVE (external edge gone); t2/child_a and t2/child_b INACTIVE —
///      even though t2 has internal wiring (pins "only external edges count").
///   3. t3/child_a and t3/child_b ACTIVE (t3 now carries the external t1→t3 edge).
///   4. Positive receipt: probe to /t3/child_a reaches /capture CaptureCell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn composite_hive_to_hive_swap_swings_edge_and_pins_connectivity_rule() {
    let td = TempDir::new().unwrap();

    // Root hive carries the single edge /t1 → /t2 (t2 is a hive).
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/t1","to":"/t2"}
        ]}}}"#,
    );
    // /t1 echoes to itself; the edge overrides the emit target.
    write(
        td.path(),
        "main/t1/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/t1"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // /t2 is a hive with an internal subtree (two echo_sub cells + one internal edge).
    write(
        td.path(),
        "main/t2/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./child_a","to":"./child_b"}
        ]}}}"#,
    );
    write(
        td.path(),
        "main/t2/child_a/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/capture"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/t2/child_b/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/capture"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // t3_hive subtree template.
    write_t3_hive_template(td.path());

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo_sub".to_string(),
            Arc::new(SubtreeEchoFactory) as Arc<dyn CellFactory>,
        )],
    );
    rescan_templates(&h, td.path().join("templates")).await;

    // Anti-cascade: /capture resolved BEFORE bootstrap + probe.
    let (capture_tx, mut capture_rx) = mpsc::channel(32);
    h.spawn(Path::new("/capture"), move || {
        CaptureCell::new(capture_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factories_with_subtree_echo(), &h.runtime())
        .await
        .expect("bootstrap with t1→t2 hive edge succeeds");

    // ── Pre-swap sanity: edge /t1 → /t2 present. ─────────────────────────────
    let before = edges(&h).await;
    assert!(
        before.contains(&("/t1".to_string(), "/t2".to_string())),
        "pre-swap edge must be /t1 → /t2; got {before:?}"
    );
    // t2 children must be active before swap (t2 has external edge t1→t2).
    let t2ca_before = registry_entry_full(&h, "/t2/child_a")
        .await
        .expect("/t2/child_a registered at bootstrap");
    assert!(
        t2ca_before.active,
        "/t2/child_a must be active before swap (t2 externally wired via t1→t2)"
    );

    // ── COMPOSITE SWAP: add_nodes(t3_hive) + swap_nodes(t2→t3 existing-form). ─
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "t3", "template": "t3_hive"}],
                "swap_nodes": [{"match": {"name": "t2"}, "with": {"name": "t3"}}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "composite Hive→Hive swap must commit; got {outcome:?}"
    );

    // ── AFTER: edge swung to /t1 → /t3, old /t1 → /t2 gone. ─────────────────
    let after = edges(&h).await;
    assert!(
        after.contains(&("/t1".to_string(), "/t3".to_string())),
        "edge must swing to /t1 → /t3; got {after:?}"
    );
    assert!(
        !after.contains(&("/t1".to_string(), "/t2".to_string())),
        "old /t1 → /t2 edge must be gone; got {after:?}"
    );

    // ── t2 and its ENTIRE subtree INACTIVE (external edge gone). ─────────────
    // Pin: internal wiring /t2/child_a → /t2/child_b does NOT self-activate t2.
    // Only external edges count for hive connectivity (connectivity.rs invariant).
    let t2ca = registry_entry_full(&h, "/t2/child_a")
        .await
        .expect("/t2/child_a still registered (No-Delete)");
    assert!(
        !t2ca.active,
        "/t2/child_a must be INACTIVE after swap — \
         t2 lost external edge; internal wiring cannot self-activate"
    );
    let t2cb = registry_entry_full(&h, "/t2/child_b")
        .await
        .expect("/t2/child_b still registered (No-Delete)");
    assert!(!t2cb.active, "/t2/child_b must be INACTIVE after swap");

    // ── t3 and its subtree ACTIVE (t3 now has external edge t1→t3). ──────────
    let t3ca = registry_entry_full(&h, "/t3/child_a")
        .await
        .expect("/t3/child_a registered after composite diff");
    assert!(
        t3ca.active,
        "/t3/child_a must be ACTIVE (t3 externally wired via swung t1→t3 edge)"
    );
    let t3cb = registry_entry_full(&h, "/t3/child_b")
        .await
        .expect("/t3/child_b registered after composite diff");
    assert!(
        t3cb.active,
        "/t3/child_b must be ACTIVE (connected via internal t3/child_a→t3/child_b edge)"
    );

    // ── Positive receipt: probe to /t3/child_a reaches /capture. ─────────────
    // Semantic justification: /capture is registered BEFORE bootstrap+swap, so
    // there is no async race between the sink registration and the probe send.
    // t3/child_a is active AND its echo_to=/capture is already resolved.
    // W2b (Ruling A1): the terminal child /t3/child_b echoes to /capture — wire
    // that hop's catch-all out-edge (identity-fallback gone). /t3/child_b is a
    // live deep node post-swap (R12 deep resolution), /capture is live.
    assert!(
        matches!(
            send_mutation(
                &h,
                json!({"scope":"/","diff":{"add_edges":[{"from":"t3/child_b","to":"capture"}]}})
            )
            .await,
            MutationOutcome::Committed { .. }
        ),
        "wiring /t3/child_b -> /capture must commit"
    );
    let probe = MessageBuilder::new(Path::new("/t3/child_a"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"b_prime_probe"}]}),
        ))
        .build();
    h.send(probe).await;

    let got = match tokio::time::timeout(Duration::from_secs(30), capture_rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("/capture rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("/capture must receive via /t3/child_a within 30s; DLQ: {dlq:?}");
        }
    };
    assert_eq!(
        got.target,
        Path::new("/capture"),
        "probe to /t3/child_a must reach /capture (t3 subtree is live)"
    );

    h.shutdown().await;
}

/// Paket-2 T5 Demo (c) — rollback / blue-green (existing-target swing-back).
///
/// After demo (a) swap (t2→t3 instantiate-form), apply a SECOND swap diff using
/// the existing-target form: `{match:{name:"t3"}, with:{name:"t2"}}`.
/// This swings edges BACK to t2 (which was retained by No-Delete).
///
/// Assertions:
///   - t2 ACTIVE again (positive receipt via /sink_t2).
///   - t3 INACTIVE (edge gone → edge-less → recompute inactive).
///
/// Pins No-Delete: t2 was retained through the first swap and is fully re-activatable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rollback_swing_back_to_t2_reactivates_t2_and_deactivates_t3() {
    let td = TempDir::new().unwrap();

    // Same topology as demo (a).
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/t1","to":"/t2"}
        ]}}}"#,
    );
    write(
        td.path(),
        "main/t1/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/t1"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/t2/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink_t2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write_template(td.path(), "t3_echo", "/sink_t3");

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
        )],
    );
    rescan_templates(&h, td.path().join("templates")).await;

    // Anti-cascade: both sinks before bootstrap.
    let (t2_tx, mut t2_rx) = mpsc::channel(16);
    let (t3_tx, mut t3_rx) = mpsc::channel(16);
    h.spawn(Path::new("/sink_t2"), move || {
        CaptureCell::new(t2_tx.clone())
    })
    .await;
    h.spawn(Path::new("/sink_t3"), move || {
        CaptureCell::new(t3_tx.clone())
    })
    .await;

    meclaw_colony::bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h.runtime())
        .await
        .expect("bootstrap succeeds");

    // ── First swap: t2 → t3 (instantiate-form, same as demo a). ─────────────
    let out1 = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [
                {"match": {"name": "t2"}, "with": {"template": "t3_echo", "name": "t3", "params": {}}}
            ]}
        }),
    )
    .await;
    assert!(
        matches!(out1, MutationOutcome::Committed { .. }),
        "first swap (t2→t3) must commit; got {out1:?}"
    );

    // Sanity: t3 active, t2 inactive after first swap.
    let t3_mid = registry_entry(&h, "/t3").await.expect("/t3 registered");
    assert!(t3_mid.active, "/t3 must be active after first swap");
    let t2_mid = registry_entry(&h, "/t2").await.expect("/t2 still on disk");
    assert!(!t2_mid.active, "/t2 must be inactive after first swap");

    // ── Second swap: swing BACK to t2 (existing-target form). ────────────────
    let out2 = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [
                {"match": {"name": "t3"}, "with": {"name": "t2"}}
            ]}
        }),
    )
    .await;
    assert!(
        matches!(out2, MutationOutcome::Committed { .. }),
        "rollback swap (t3→t2) must commit; got {out2:?}"
    );

    // ── AFTER rollback: edge swung back to /t1 → /t2. ────────────────────────
    let after = edges(&h).await;
    assert!(
        after.contains(&("/t1".to_string(), "/t2".to_string())),
        "edge must swing back to /t1 → /t2; got {after:?}"
    );
    assert!(
        !after.contains(&("/t1".to_string(), "/t3".to_string())),
        "old /t1 → /t3 edge must be gone; got {after:?}"
    );

    // t2 ACTIVE again.
    let t2_final = registry_entry(&h, "/t2")
        .await
        .expect("/t2 still registered (No-Delete)");
    assert!(
        t2_final.active,
        "/t2 must be ACTIVE after rollback (No-Delete + re-wiring)"
    );
    // t3 INACTIVE (edge-less).
    let t3_final = registry_entry(&h, "/t3").await.expect("/t3 still on disk");
    assert!(
        !t3_final.active,
        "/t3 must be INACTIVE after rollback (edge-less)"
    );

    // ── Positive receipt: probe through /t1 reaches /sink_t2 (t2 live again). ─
    // Anti-cascade: /sink_t2 was resolved before bootstrap. At this point t2 is
    // active and emitted_target=/sink_t2. The probe triggers the edge t1→t2.
    // W2b (Ruling A1): /t2's terminal echo to /sink_t2 needs a wired catch-all
    // out-edge (identity-fallback gone). /t2 is re-active post-rollback, /sink_t2
    // live.
    assert!(
        matches!(
            send_mutation(
                &h,
                json!({"scope":"/","diff":{"add_edges":[{"from":"t2","to":"sink_t2"}]}})
            )
            .await,
            MutationOutcome::Committed { .. }
        ),
        "wiring /t2 -> /sink_t2 must commit"
    );
    let probe = MessageBuilder::new(Path::new("/t1"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"rollback_probe"}]}),
        ))
        .build();
    h.send(probe).await;

    let got = match tokio::time::timeout(Duration::from_secs(30), t2_rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("/sink_t2 rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("/sink_t2 must receive via swung-back edge within 30s; DLQ: {dlq:?}");
        }
    };
    assert_eq!(
        got.target,
        Path::new("/sink_t2"),
        "probe through /t1 must reach /sink_t2 (t2 re-activated by rollback)"
    );

    // /sink_t3 must NOT have received anything (t3 is inactive).
    assert!(
        t3_rx.try_recv().is_err(),
        "/sink_t3 must NOT receive — t3 is edge-less + inactive after rollback"
    );

    h.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers shared by the T11 demos.
// ─────────────────────────────────────────────────────────────────────────────

/// Read the full edge DTOs (id, from, to, condition, modifier) from the
/// colony graph.
async fn edges_full(h: &ColonyHandle) -> Vec<meclaw_colony::api_dto::GraphEdgeDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().edges
}

/// Find the first edge DTO matching `(from, to)` endpoints.
fn find_edge<'a>(
    edges: &'a [meclaw_colony::api_dto::GraphEdgeDto],
    from: &str,
    to: &str,
) -> Option<&'a meclaw_colony::api_dto::GraphEdgeDto> {
    edges.iter().find(|e| e.from == from && e.to == to)
}

/// Read + increment a per-cell spawn counter from `{cell_dir}/cell.db`.
/// Returns the NEW value.  Used by the C1 `NoRenotifyLrFactory`.
fn bump_spawn_counter_c1(cell_dir: &std::path::Path) -> i64 {
    let conn = meclaw_colony::persist::cell_db::open_or_create_cell_db(&cell_dir.join("cell.db"))
        .expect("open cell.db for spawn counter");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS spawns (id INTEGER PRIMARY KEY CHECK (id = 1), n INTEGER NOT NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO spawns (id, n) VALUES (1, 1) ON CONFLICT(id) DO UPDATE SET n = n + 1",
        [],
    )
    .unwrap();
    conn.query_row("SELECT n FROM spawns WHERE id = 1", [], |r| r.get(0))
        .unwrap()
}

// ─────────────────────────────────────────────────────────────────────────────
// Demo (d) — completeness
// ─────────────────────────────────────────────────────────────────────────────

/// Paket-2 T11 Demo (d) — completeness: multiple in/out edges + condition/modifier.
///
/// Topology:
///   /x  (echo, emitted_target=/sink_x)
///   /t2 (echo, emitted_target=/sink_d_t2 — should receive nothing after swap)
///   /y  (echo, emitted_target=/sink_d_y — downstream from t2→y edge)
///   Edges in root hive config.json (persisted at bootstrap):
///     x → t2   (INCOMING to t2, carries condition "headers.tier == 'gold'" +
///                modifier set tier=headers.tier)
///     t2 → y   (OUTGOING from t2, no extras)
///
/// Swap: t2 → t3 (template t3d_echo, emitted_target=/sink_d_t3).
///
/// Assertions:
///   1. x→t3 present; t3→y present (all external edges of t2 swung).
///   2. x→t2 and t2→y gone (no incoming or outgoing edge left on t2).
///   3. condition on x→t3 == original condition on x→t2 (byte-identical).
///   4. modifier on x→t3 == original modifier on x→t2 (JSON byte-identical).
///   5. Behavioral proof: message with matching condition header routed through
///      x reaches /sink_d_t3 (via swung x→t3 edge + t3's echo).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swap_completeness_multiple_edges_condition_modifier_preserved() {
    let td = TempDir::new().unwrap();

    // Root hive: two edges — one incoming to t2 with condition+modifier,
    // one outgoing from t2 (bare).
    //
    // W13 hardening: the modifier used to be spelled `{"set": …}`, a field
    // `ModifierSpec` has never had. serde ignored it, so the edge carried an
    // EMPTY modifier — and the byte-identity assertion below compared two empty
    // modifiers to each other. A test called `modifier_preserved` was preserving
    // nothing. With the boot path now as strict as the mutation path, the
    // fixture has to say what it means, and the assertion has teeth.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {
                "from":"/x","to":"/t2",
                "condition":"headers.tier == 'gold'",
                "modifier":{"set_hop":{"tier":"hop.tier"}}
            },
            {"from":"/t2","to":"/y"}
        ]}}}"#,
    );
    // /x echoes to /sink_x (origin cell; not the behavioral proof target).
    write(
        td.path(),
        "main/x/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink_x"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // /t2 echoes to /sink_d_t2 (should receive nothing after swap).
    write(
        td.path(),
        "main/t2/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink_d_t2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // /y echoes to /sink_d_y.
    // Behavioral proof chain: probe→/t3 → t3 emits → t3→y edge → /y → /y echoes → /sink_d_y.
    // /sink_d_y is registered as a CaptureCell (anti-cascade: absorbs /y's echo).
    write(
        td.path(),
        "main/y/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink_d_y"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // t3d_echo template: echo to /sink_d_t3.
    write_template(td.path(), "t3d_echo", "/sink_d_t3");

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
        )],
    );
    rescan_templates(&h, td.path().join("templates")).await;

    // Anti-cascade: register ALL sinks BEFORE bootstrap.
    // /sink_d_y:  behavioral proof sink — /y echoes here via the t3→y chain.
    //             Registered before bootstrap so /y's emitted_target is resolved.
    //             Behavioral proof: probe→/t3 → t3 emits → t3→y edge intercepts
    //             (replaces emission target with /y) → /y processes and echoes to
    //             /sink_d_y → /sink_d_y CaptureCell receives. Proves BOTH swung
    //             edges are live (x→t3 structural; t3→y behavioral via this chain).
    // /sink_d_t2: must receive nothing (t2 is inactive + edge-less after swap).
    // /sink_x:    /x's echo target — registered to avoid DLQ on pre-swap probes.
    let (sink_d_y_tx, mut sink_d_y_rx) = mpsc::channel::<Message>(16);
    let (sink_d_t2_tx, mut sink_d_t2_rx) = mpsc::channel(16);
    let (sink_x_tx, _sink_x_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink_d_y"), move || {
        CaptureCell::new(sink_d_y_tx.clone())
    })
    .await;
    h.spawn(Path::new("/sink_d_t2"), move || {
        CaptureCell::new(sink_d_t2_tx.clone())
    })
    .await;
    h.spawn(Path::new("/sink_x"), move || {
        CaptureCell::new(sink_x_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h.runtime())
        .await
        .expect("bootstrap with x→t2 (cond+mod) and t2→y succeeds");

    // ── Pre-swap: record original condition + modifier from x→t2. ────────────
    let before = edges_full(&h).await;
    let orig_edge =
        find_edge(&before, "/x", "/t2").expect("pre-swap edge /x → /t2 must be present");
    let orig_condition = orig_edge.condition.clone();
    let orig_modifier = orig_edge.modifier.clone();
    assert!(
        orig_condition.is_some(),
        "pre-swap: x→t2 must carry a condition; got {orig_condition:?}"
    );
    assert!(
        orig_modifier.is_some(),
        "pre-swap: x→t2 must carry a modifier; got {orig_modifier:?}"
    );
    assert!(
        find_edge(&before, "/t2", "/y").is_some(),
        "pre-swap edge /t2 → /y must be present"
    );

    // ── SWAP t2 → t3 (template t3d_echo). ────────────────────────────────────
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [
                {"match": {"name": "t2"}, "with": {"template": "t3d_echo", "name": "t3", "params": {}}}
            ]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "swap must commit; got {outcome:?}"
    );

    // ── AFTER: assert all external edges swung. ───────────────────────────────
    let after = edges_full(&h).await;

    // New incoming edge: x → t3 (swung from x → t2).
    let swung_in = find_edge(&after, "/x", "/t3")
        .expect("swung incoming edge /x → /t3 must be present after swap");
    // New outgoing edge: t3 → y (swung from t2 → y).
    assert!(
        find_edge(&after, "/t3", "/y").is_some(),
        "swung outgoing edge /t3 → /y must be present after swap; got {after:?}"
    );
    // Old edges gone (no incoming or outgoing left on t2).
    assert!(
        find_edge(&after, "/x", "/t2").is_none(),
        "old /x → /t2 edge must be gone after swap; got {after:?}"
    );
    assert!(
        find_edge(&after, "/t2", "/y").is_none(),
        "old /t2 → /y edge must be gone after swap; got {after:?}"
    );

    // ── Condition + modifier preserved VERBATIM on swung x→t3. ──────────────
    // Semantic: cond_src and mod_src are byte-identical to the original values
    // on x→t2 — proven at integration level via the DTO read-back.
    assert_eq!(
        swung_in.condition, orig_condition,
        "swung edge x→t3 condition must be byte-identical to original x→t2 condition"
    );
    assert_eq!(
        swung_in.modifier, orig_modifier,
        "swung edge x→t3 modifier must be byte-identical to original x→t2 modifier"
    );

    // ── t3 active, t2 inactive. ───────────────────────────────────────────────
    let t3 = registry_entry(&h, "/t3").await.expect("/t3 registered");
    assert!(t3.active, "/t3 must be active after swap");
    let t2 = registry_entry(&h, "/t2").await.expect("/t2 still on disk");
    assert!(!t2.active, "/t2 must be inactive after swap");

    // ── Behavioral proof: probe to /t3 → t3→y edge → /y → /sink_d_y receives. ─
    // Semantic justification (emission-based edge routing):
    //   Colony routing is EMISSION-based. When /t3 processes the probe, it emits
    //   to its emitted_target (/sink_d_t3). Then `apply_edges(edges, /t3, headers)` fires
    //   and finds the swung t3→y edge — this REPLACES the emission target with /y.
    //   /y processes the message and echoes to /sink_d_y (the CaptureCell).
    //   /sink_d_y receiving proves: (a) t3 received the probe (live), and (b) the
    //   swung t3→y outgoing edge is behaviorally active.
    //   The swung x→t3 incoming edge (with condition+modifier) is proven structurally
    //   via the DTO assertions above. Per the task spec: "at minimum assert the edge
    //   condition/modifier strings survived on t3's edges via the registry/edge query".
    //   Anti-cascade: /sink_d_y and /y (on-disk echo cell) are both resolved before
    //   the probe (sinks registered before bootstrap).
    // W2b (Ruling A1): /y's terminal echo to /sink_d_y needs a wired catch-all
    // out-edge (identity-fallback gone). /y + /sink_d_y are live; the swung t3→y
    // edge (asserted structurally above) carries the probe into /y.
    assert!(
        matches!(
            send_mutation(
                &h,
                json!({"scope":"/","diff":{"add_edges":[{"from":"y","to":"sink_d_y"}]}})
            )
            .await,
            MutationOutcome::Committed { .. }
        ),
        "wiring /y -> /sink_d_y must commit"
    );
    let probe = MessageBuilder::new(Path::new("/t3"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"d_probe"}]}),
        ))
        .build();
    h.send(probe).await;

    let got = match tokio::time::timeout(Duration::from_secs(30), sink_d_y_rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("/sink_d_y rx closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!(
                "/sink_d_y must receive (via t3→y edge + /y echo) within 30s \
                 (proves t3→y swung edge is live); DLQ: {dlq:?}"
            );
        }
    };
    assert_eq!(
        got.target,
        Path::new("/sink_d_y"),
        "/sink_d_y must receive via /y (t3→y swung edge + /y echo proves outgoing edge live)"
    );
    // /sink_d_t2 must have received nothing (t2 is inactive + edge-less).
    assert!(
        sink_d_t2_rx.try_recv().is_err(),
        "/sink_d_t2 must NOT receive anything — t2 is inactive after swap"
    );

    h.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Demo (e) — Reject vor-destruktiv (no partial state, staging cleanup)
// ─────────────────────────────────────────────────────────────────────────────

/// Paket-2 T11 Demo (e) — invalid swap is rejected atomically; no partial state.
///
/// Topology: /t1 (echo) → /t2 (echo).  An invalid swap `with.template` that
/// references an unknown template (`no_such_template`) triggers `template_missing`
/// during the validate-all pre-pass BEFORE any destructive step.
///
/// Assertions:
///   1. MutationOutcome::Rejected with error_code == "template_missing".
///   2. t2 still has its original edge (/t1→/t2 present; /t1→/t3 absent).
///   3. t2 still active; no t3 in registry.
///   4. No `.staging/<mutation_id>/` directory left on disk (staging cleaned or
///      never written — pre-destruktiv reject fires before staging).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swap_invalid_template_rejected_atomically_no_partial_state() {
    let td = TempDir::new().unwrap();

    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/t1","to":"/t2"}
        ]}}}"#,
    );
    write(
        td.path(),
        "main/t1/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/t1"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/t2/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink_e_t2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // Deliberately do NOT write any template "no_such_template".

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
        )],
    );
    // No rescan_templates → template registry stays empty.

    bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h.runtime())
        .await
        .expect("bootstrap t1→t2 succeeds");

    // Pre-swap edges: /t1 → /t2 present.
    let before = edges(&h).await;
    assert!(
        before.contains(&("/t1".to_string(), "/t2".to_string())),
        "pre-reject: /t1 → /t2 must be present; got {before:?}"
    );
    let t2_before = registry_entry(&h, "/t2")
        .await
        .expect("/t2 in registry before swap");
    let t2_cell_id_before = t2_before.cell_id.clone();
    assert!(t2_before.active, "/t2 must be active before swap");

    // ── INVALID SWAP: unknown template → must be rejected BEFORE staging. ────
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [
                {"match": {"name": "t2"}, "with": {"template": "no_such_template", "name": "t3", "params": {}}}
            ]}
        }),
    )
    .await;

    // (a) Rejection with expected error_code.
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "template_missing",
                "unknown template must reject with template_missing; got {outcome:?}"
            );
        }
        other => panic!("expected Rejected(template_missing), got {other:?}"),
    }

    // (b) Edge table unchanged: t1→t2 still there, no t1→t3.
    let after = edges(&h).await;
    assert!(
        after.contains(&("/t1".to_string(), "/t2".to_string())),
        "after rejected swap: /t1 → /t2 must still be present; got {after:?}"
    );
    assert!(
        !after.contains(&("/t1".to_string(), "/t3".to_string())),
        "after rejected swap: /t1 → /t3 must NOT exist; got {after:?}"
    );

    // (c) t2 still active + cell_id unchanged; t3 not created.
    let t2_after = registry_entry(&h, "/t2")
        .await
        .expect("/t2 still in registry after rejected swap");
    assert!(t2_after.active, "/t2 must be active after rejected swap");
    assert_eq!(
        t2_after.cell_id, t2_cell_id_before,
        "/t2 cell_id must be unchanged after rejected swap"
    );
    assert!(
        registry_entry(&h, "/t3").await.is_none(),
        "/t3 must NOT be in registry after rejected swap"
    );

    // (d) No `.staging/<id>/` directory left on disk.
    // The validate-all pre-pass fires BEFORE build_staging_tree_from_templates,
    // so nothing is written to .staging at all. This check proves atomicity.
    let staging_dir = td.path().join(".staging");
    if staging_dir.exists() {
        let leftovers: Vec<_> = std::fs::read_dir(&staging_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            leftovers.is_empty(),
            "no leftover .staging/<id>/ dirs after rejected swap; found: {:?}",
            leftovers.iter().map(|e| e.path()).collect::<Vec<_>>()
        );
    }
    // If .staging doesn't exist at all that's fine too (pre-destruktiv reject).

    h.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Demo (f) — Reboot durability
// ─────────────────────────────────────────────────────────────────────────────

/// Paket-2 T11 Demo (f) — swung edges + inactive t2 survive a colony REBOOT.
///
/// Topology: /t1 (echo) → /t2 (echo).  Swap t2 → t3 (template t3f_echo).
/// Then REBOOT: shut down colony1, bootstrap colony2 from the SAME filesystem
/// root + colony.db.
///
/// Assertions post-reboot:
///   1. Swung edge /t1 → /t3 is present (durable in colony.db, re-hydrated).
///   2. t2 is INACTIVE (edge-less → compute_active → inactive).
///   3. t3 is registered + ACTIVE.
///   4. Positive receipt: probe to /t1 reaches /sink_f_t3 via swung edge.
///
/// Reboot harness: same pattern as `reboot_after_swap_persists_new_config_and_status`
/// in the deleted `phase_13_5_slice_4_swap_reboot.rs` — `h.shutdown().await` +
/// fresh `ColonyHandle::new_with_factories_at` + second `bootstrap_from_filesystem`.
/// Using `EchoCellFactory` (stateless) keeps the test minimal: no cell.db
/// continuity required, purely graph/edge durability.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reboot_durability_swung_edge_and_inactive_t2_survive() {
    let td = TempDir::new().unwrap();

    // Root hive: single edge /t1 → /t2.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/t1","to":"/t2"}
        ]}}}"#,
    );
    write(
        td.path(),
        "main/t1/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/t1"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/t2/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/sink_f_t2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // t3f_echo template: echo to /sink_f_t3.
    write_template(td.path(), "t3f_echo", "/sink_f_t3");

    // ── Boot 1 ────────────────────────────────────────────────────────────────
    let h1 = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
        )],
    );
    rescan_templates(&h1, td.path().join("templates")).await;

    // Anti-cascade: sinks before bootstrap.
    let (t2_tx, mut _t2_rx) = mpsc::channel::<Message>(16);
    let (t3_tx, mut _t3_rx) = mpsc::channel::<Message>(16);
    h1.spawn(Path::new("/sink_f_t2"), {
        let t = t2_tx.clone();
        move || CaptureCell::new(t.clone())
    })
    .await;
    h1.spawn(Path::new("/sink_f_t3"), {
        let t = t3_tx.clone();
        move || CaptureCell::new(t.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h1.runtime())
        .await
        .expect("boot1 bootstrap t1→t2 succeeds");

    // Perform the swap: t2 → t3 (template-form).
    let outcome = send_mutation(
        &h1,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [
                {"match": {"name": "t2"}, "with": {"template": "t3f_echo", "name": "t3", "params": {}}}
            ]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "swap must commit; got {outcome:?}"
    );

    // Pre-reboot checks.
    let after_boot1 = edges(&h1).await;
    assert!(
        after_boot1.contains(&("/t1".to_string(), "/t3".to_string())),
        "pre-reboot: /t1 → /t3 must be present; got {after_boot1:?}"
    );
    let t2_before_reboot = registry_entry(&h1, "/t2").await.expect("/t2 pre-reboot");
    assert!(!t2_before_reboot.active, "/t2 inactive pre-reboot");

    // ── REBOOT: shutdown colony1, start colony2 from same root. ──────────────
    h1.shutdown().await;
    drop(t2_tx);
    drop(t3_tx);

    // Boot 2: fresh handle + bootstrap from the SAME {root}/colony.db.
    let h2 = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
        )],
    );
    rescan_templates(&h2, td.path().join("templates")).await;

    // Anti-cascade for the reboot: register the NEW sink before bootstrap.
    let (sink_f_t3_tx2, mut sink_f_t3_rx2) = mpsc::channel::<Message>(16);
    h2.spawn(Path::new("/sink_f_t3"), {
        let t = sink_f_t3_tx2.clone();
        move || CaptureCell::new(t.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factories_with_echo(), &h2.runtime())
        .await
        .expect("boot2 (reboot) bootstrap must succeed");

    // ── POST-REBOOT ASSERTIONS ────────────────────────────────────────────────

    // (1) Swung edge /t1 → /t3 present after reboot (durable in colony.db).
    let after_boot2 = edges(&h2).await;
    assert!(
        after_boot2.contains(&("/t1".to_string(), "/t3".to_string())),
        "post-reboot: /t1 → /t3 must be present (persisted + rehydrated); got {after_boot2:?}"
    );
    assert!(
        !after_boot2.contains(&("/t1".to_string(), "/t2".to_string())),
        "post-reboot: old /t1 → /t2 edge must still be gone; got {after_boot2:?}"
    );

    // (2) t2 is INACTIVE after reboot (edge-less → compute_active → inactive).
    let t2_post = registry_entry(&h2, "/t2")
        .await
        .expect("/t2 still in registry after reboot (No-Delete)");
    assert!(
        !t2_post.active,
        "/t2 must be INACTIVE after reboot (edge-less → re-derived inactive)"
    );

    // (3) t3 is ACTIVE after reboot (carries the swung t1→t3 edge).
    let t3_post = registry_entry(&h2, "/t3")
        .await
        .expect("/t3 in registry after reboot");
    assert!(
        t3_post.active,
        "/t3 must be ACTIVE after reboot (carries swung edge)"
    );

    // (4) Positive receipt post-reboot: probe through /t1 reaches /sink_f_t3.
    // Semantic justification: /sink_f_t3 is registered BEFORE bootstrap (boot2),
    // so it is fully resolved when the probe arrives. t3's emitted_target=/sink_f_t3 was
    // written by the swap (config.json overwrite, durable on disk).
    // W2b (Ruling A1): /t3's terminal echo to /sink_f_t3 needs a wired catch-all
    // out-edge (identity-fallback gone). Added post-reboot on h2 where both /t3
    // and /sink_f_t3 are live (the swung t1→t3 edge survived the reboot).
    assert!(
        matches!(
            send_mutation(
                &h2,
                json!({"scope":"/","diff":{"add_edges":[{"from":"t3","to":"sink_f_t3"}]}})
            )
            .await,
            MutationOutcome::Committed { .. }
        ),
        "wiring /t3 -> /sink_f_t3 must commit"
    );
    let probe = MessageBuilder::new(Path::new("/t1"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"f_reboot_probe"}]}),
        ))
        .build();
    h2.send(probe).await;

    let got = match tokio::time::timeout(Duration::from_secs(30), sink_f_t3_rx2.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("/sink_f_t3 rx closed after reboot"),
        Err(_) => {
            let dlq = h2.drain_dead_letters().await;
            panic!("/sink_f_t3 must receive via swung edge after reboot within 30s; DLQ: {dlq:?}");
        }
    };
    assert_eq!(
        got.target,
        Path::new("/sink_f_t3"),
        "post-reboot probe through /t1 must reach /sink_f_t3 (t3 live with persisted config)"
    );

    h2.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// C1 — stop_wiring_unavailable invariant rewrite as graph-swap test
// ─────────────────────────────────────────────────────────────────────────────

/// Serialises swap tests that trigger the swap-arm's disconnect pre-pass guard
/// (peace-stop or stop_wiring_unavailable) so the process-global
/// `notify_swap_stop_fired` rendezvous stays unambiguous within this binary.
static C1_SWAP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Long-running factory that does NOT call `renotify_stop_wiring` in its
/// `RespawnFn`.  A reconnect-eager re-spawn therefore lands the cell Awake
/// with `stop_tx == None` — the exact un-restored state that the C1 guard
/// must reject a swap on.
///
/// Mirrors `NoRenotifyLrFactory` from the deleted
/// `phase_13_5_slice_4_swap_demo.rs` (commit 5c7de9e^).
struct NoRenotifyLrFactory {
    spawns: Arc<AtomicI64>,
    inject_slot: Arc<std::sync::Mutex<Option<mpsc::Sender<MockEvent>>>>,
}

impl CellFactory for NoRenotifyLrFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        _params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        _contract: ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<Arc<DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (stop_tx, initial_stop_rx) = oneshot::channel::<()>();
        let (initial_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();

        let spawns = self.spawns.clone();
        let inject_slot = self.inject_slot.clone();
        let cdir = cell_dir.clone();

        let build = move |stop_rx: Option<oneshot::Receiver<()>>,
                          death_ack: Option<oneshot::Sender<()>>|
              -> (
            mpsc::Sender<Message>,
            JoinHandle<()>,
            oneshot::Receiver<()>,
            oneshot::Receiver<()>,
        ) {
            let n = bump_spawn_counter_c1(&cdir);
            spawns.store(n, Ordering::SeqCst);

            let (cell, inject_tx) = ReceiptMockLongRunningCell::new();
            *inject_slot.lock().unwrap() = Some(inject_tx.clone());

            let conn =
                meclaw_colony::persist::cell_db::open_or_create_cell_db(&cdir.join("cell.db"))
                    .expect("open cell.db for lr task");
            let db = DbConn::wrap(conn, None);
            let (tx, rx) = mpsc::channel::<Message>(1000);
            let (peace_tx, peace_rx) = oneshot::channel();
            let (_backstop_tx, backstop_rx) = oneshot::channel();
            let p = path.clone();
            let o = outputs_tx.clone();
            let cit = colony_inbox_tx.clone();
            let join = tokio::spawn(async move {
                let _keep_inject = inject_tx;
                cell_task_long_running(
                    p,
                    rx,
                    o,
                    64,
                    cell,
                    db,
                    Some(peace_tx),
                    Some(cit),
                    stop_rx,
                    death_ack,
                    None,
                    None,
                    Default::default(),
                )
                .await;
            });
            (tx, join, peace_rx, backstop_rx)
        };

        let (sender, join, peace_rx, backstop_rx) =
            build(Some(initial_stop_rx), Some(initial_death_ack_tx));
        // RespawnFn intentionally does NOT call renotify_stop_wiring:
        // reconnect-eager re-spawn → Awake with stop_tx == None.
        let respawn: RespawnFn = Box::new(move || build(None, None));
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
}

/// Write a simple long-running template (type "lr", timeout=-1).
fn write_lr_template_c1(root: &std::path::Path, name: &str) {
    let tpl = root.join("templates").join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        format!(
            r#"{{"cell":{{"type":"lr","timeout":-1}},"params":{{"marker":"{name}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

/// Paket-2 T11 C1 — swap of an Awake cell with stop_tx == None is atomically
/// rejected with `stop_wiring_unavailable`.
///
/// This rewrites the invariant previously tested by the deleted
/// `swap_awake_cell_without_stop_wiring_rejects_no_double_spawn` test in
/// `phase_13_5_slice_4_swap_demo.rs` (commit 5c7de9e^), now as a graph-swap test
/// in the Paket-2 swap harness.
///
/// Mechanism to reproduce "Awake with stop_tx == None":
///   1. Register a long-running cell `/lr` via `NoRenotifyLrFactory`, which
///      provides the initial stop_tx normally.
///   2. Connect `/lr → /peer` via `add_edges` → cell becomes Awake + active.
///   3. Disconnect → peace-stop → cell enters NotYetSpawned, stop_tx consumed.
///   4. Reconnect via `add_edges` → reconnect-eager RespawnFn fires with
///      stop_rx=None (NO renotify) → cell is Awake with stop_tx == None.
///   5. Attempt `swap_nodes` on `/lr` → the swap-arm's disconnect pre-pass guard
///      detects `stop_tx == None` → atomically Rejected `stop_wiring_unavailable`.
///
/// Assertions:
///   a. Rejected with error_code == "stop_wiring_unavailable".
///   b. No double-spawn (spawn counter unchanged after rejected swap).
///   c. Original live task still running (inject proves channel still open).
///   d. config.json + cell_id unchanged (no FS rename, no registry edit).
///   e. Cell still Awake + active (atomic reject did not touch state).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c1_swap_awake_cell_without_stop_wiring_rejected_stop_wiring_unavailable() {
    // Generous term-timeout: the first disconnect peace-stop must not fire
    // spuriously. The rejected swap itself never reaches death-ack; this value
    // is only a backstop for the setup disconnect.
    set_term_timeout_ms_for_test(30_000);
    // Serialize swap tests that touch the global swap-stop rendezvous.
    let _swap_lock = C1_SWAP_LOCK.lock().await;

    let td = TempDir::new().unwrap();
    // Root hive with no initial edges (we add them via mutations below).
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/lr/config.json",
        r#"{"cell":{"type":"lr","timeout":-1},"params":{"marker":"v1"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/peer/config.json",
        r#"{"cell":{"type":"lr","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    // lr_v2 is pre-written as an existing cell (not a template); the C1 swap
    // uses the existing-node form `with: {"name": "lr_v2"}` to avoid any staging
    // (staging would create a live t3 before the guard fires, causing a leak).
    write(
        td.path(),
        "main/lr_v2/config.json",
        r#"{"cell":{"type":"lr","timeout":-1},"params":{"marker":"v2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write_lr_template_c1(td.path(), "lr_tpl");

    let lr_dir = td.path().join("main/lr");
    std::fs::create_dir_all(&lr_dir).unwrap();
    let peer_dir = td.path().join("main/peer");
    std::fs::create_dir_all(&peer_dir).unwrap();
    let lr_v2_dir = td.path().join("main/lr_v2");
    std::fs::create_dir_all(&lr_v2_dir).unwrap();

    let spawns = Arc::new(AtomicI64::new(0));
    let inject_slot = Arc::new(std::sync::Mutex::new(None::<mpsc::Sender<MockEvent>>));

    let lr_factory: Arc<dyn CellFactory> = Arc::new(NoRenotifyLrFactory {
        spawns: spawns.clone(),
        inject_slot: inject_slot.clone(),
    });
    // Peer and lr_v2 use separate NoRenotifyLrFactory instances with their own counters.
    let peer_factory: Arc<dyn CellFactory> = Arc::new(NoRenotifyLrFactory {
        spawns: Arc::new(AtomicI64::new(0)),
        inject_slot: Arc::new(std::sync::Mutex::new(None)),
    });
    let lr_v2_factory: Arc<dyn CellFactory> = Arc::new(NoRenotifyLrFactory {
        spawns: Arc::new(AtomicI64::new(0)),
        inject_slot: Arc::new(std::sync::Mutex::new(None)),
    });

    let h = ColonyHandle::new_with_factories_at(&td, vec![("lr".to_string(), lr_factory.clone())]);
    rescan_templates(&h, td.path().join("templates")).await;

    // Manually spawn and register /peer, /lr_v2, and /lr (mirrors the old C1 demo
    // pattern which pre-registers cells for precise factory control).
    // /peer and /lr_v2 are Awake but disconnected (no edges yet) → inactive.
    let peer_spawned = peer_factory
        .spawn_cell(
            Path::new("/peer"),
            json!({}),
            h.runtime().outputs_tx.clone(),
            peer_dir.clone(),
            ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned_typed(Path::new("/peer"), peer_spawned, "lr")
        .await;

    // /lr_v2 is the existing-node swap target. Registered as Awake but inactive
    // (no edges connecting it). The existing-node swap form `with: {"name":"lr_v2"}`
    // just needs it in the registry — no staging occurs.
    let lr_v2_spawned = lr_v2_factory
        .spawn_cell(
            Path::new("/lr_v2"),
            json!({"marker":"v2"}),
            h.runtime().outputs_tx.clone(),
            lr_v2_dir.clone(),
            ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned_typed(Path::new("/lr_v2"), lr_v2_spawned, "lr")
        .await;

    let lr_spawned = lr_factory
        .spawn_cell(
            Path::new("/lr"),
            json!({"marker":"v1"}),
            h.runtime().outputs_tx.clone(),
            lr_dir.clone(),
            ContractView::default(),
            h.inbox_tx.clone(),
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned_typed(Path::new("/lr"), lr_spawned, "lr")
        .await;

    let lr_cell_id = registry_entry(&h, "/lr").await.expect("/lr").cell_id;
    let cfg_before = std::fs::read_to_string(lr_dir.join("config.json")).unwrap();

    // ── Step 1: Connect /lr → /peer → cell Awake + active with live stop_tx. ─
    let out = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"lr","to":"peer"}]}}),
    )
    .await;
    assert!(
        matches!(out, MutationOutcome::Committed { .. }),
        "connect /lr→/peer must commit; got {out:?}"
    );

    // ── Step 2: Disconnect → peace-stop → NotYetSpawned, stop_tx consumed. ───
    let out = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"lr","to":"peer"}}]}}),
    )
    .await;
    assert!(
        matches!(out, MutationOutcome::Committed { .. }),
        "disconnect /lr→/peer must commit; got {out:?}"
    );

    // ── Step 3: Reconnect → reconnect-eager, NO renotify → Awake, stop_tx=None ─
    let out = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"lr","to":"peer"}]}}),
    )
    .await;
    assert!(
        matches!(out, MutationOutcome::Committed { .. }),
        "reconnect /lr→/peer must commit; got {out:?}"
    );
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        2,
        "reconnect must have produced exactly one re-spawn (counter=2)"
    );
    let e = registry_entry(&h, "/lr").await.expect("/lr");
    assert!(
        e.active && e.lifecycle_status == "Awake",
        "/lr must be Awake + active after reconnect; got {e:?}"
    );

    // Prove the re-spawned task is live BEFORE the swap attempt.
    let inject = inject_slot
        .lock()
        .unwrap()
        .clone()
        .expect("inject sender slot");
    inject
        .send(MockEvent("pre_swap".into()))
        .await
        .expect("inject pre_swap event");
    // 200 ms is generous for a cooperative task under cargo load.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── Step 4: SWAP /lr → lr_v2 (Awake, stop_tx == None). ──────────────────
    // Use the existing-node form `with: {"name": "lr_v2"}` to avoid staging
    // (staging would spawn a new cell before the guard fires, causing a leak).
    // /lr_v2 is pre-registered as an existing node (see setup above).
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [
                {"match": {"name": "lr"}, "with": {"name": "lr_v2"}}
            ]}
        }),
    )
    .await;

    // (a) Atomic reject with stop_wiring_unavailable.
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "stop_wiring_unavailable",
                "swap of Awake cell with stop_tx==None must reject \
                 stop_wiring_unavailable; got {outcome:?}"
            );
        }
        other => panic!("expected Rejected(stop_wiring_unavailable), got {other:?}"),
    }

    // (b) No double-spawn: counter still == 2.
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        2,
        "rejected swap must NOT spawn a second task (no double-spawn)"
    );

    // (c) Original task still live: inject a second event, it must be received
    // (channel still open → task is alive).
    inject
        .send(MockEvent("post_swap".into()))
        .await
        .expect("inject post_swap event — channel must still be open (task alive)");

    // (d) config.json + cell_id unchanged (no FS rename happened).
    let cfg_after = std::fs::read_to_string(lr_dir.join("config.json")).unwrap();
    assert_eq!(
        cfg_after, cfg_before,
        "config.json must be untouched by rejected swap"
    );
    assert!(
        !cfg_after.contains("lr_v2"),
        "config.json must NOT reference lr_v2 after rejected swap"
    );
    let e = registry_entry(&h, "/lr")
        .await
        .expect("/lr present after rejected swap");
    assert_eq!(e.cell_id, lr_cell_id, "cell_id must be unchanged");

    // (e) Cell still Awake + active (atomic reject did not touch state).
    assert_eq!(
        e.lifecycle_status, "Awake",
        "/lr must still be Awake after rejected swap"
    );
    assert!(e.active, "/lr must still be active after rejected swap");

    h.shutdown().await;
}
