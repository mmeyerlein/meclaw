//! Phase-13.5 Lifecycle-3b disconnect-remainder regression.
//!
//! The transplant-era `swap_nodes` demos (in-place stop→replace→respawn) were
//! retired with the graph-swap re-dedication (paket-2 T4); their replacements
//! live in `paket_2_swap.rs` + later T5/T11 tasks. The ONE non-transplant test
//! kept here proves that a normal DISCONNECT (remove_edges while the cell is
//! blocked) still DLQs its mailbox remainder as `cell_inactive` — i.e. the
//! biased peace-stop path is intact (no A1 regression).

use meclaw_colony::{
    CellFactory, ColonyMsg, DbConn, MutationOutcome, RespawnFn, SpawnedCellKind, WakeFn,
    build_stateful_task_with_peace, set_term_timeout_ms_for_test,
};
use meclaw_core::{Body, CellEmission, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::{mpsc, oneshot};

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

/// A stateful cell that blocks in its FIRST `handle()` on a shared gate, then
/// echoes a marker to its sink. The gate lets the test hold the cell busy so a
/// follow-up message is buffered in the mailbox when the disconnect fires.
struct GatedEchoCell {
    echo_to: meclaw_core::Path,
    /// One-shot latch: the VERY FIRST handle blocks until released; every later
    /// handle runs free.
    gate: Arc<tokio::sync::Notify>,
    latch: Arc<std::sync::atomic::AtomicBool>,
}

impl meclaw_colony::stateful_cell::StatefulCell for GatedEchoCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a meclaw_core::OutputSink,
        _db: &'a mut DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            if self.latch.swap(false, std::sync::atomic::Ordering::SeqCst) {
                self.gate.notified().await;
            }
            let marker = match &msg.body {
                Body::Inline(v) => v
                    .get("marker")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string(),
                Body::Blob(_) => String::new(),
            };
            let _ = sink
                .push(meclaw_core::CellOutput {
                    target: self.echo_to.clone(),
                    content: meclaw_core::serde_json::json!({
                        "messages": [],
                        "header": {"marker": marker}
                    }),
                })
                .await;
        }
    }
}

struct GatedEchoFactory {
    gate: Arc<tokio::sync::Notify>,
    latch: Arc<std::sync::atomic::AtomicBool>,
    spawn_count: Arc<AtomicU32>,
}

impl CellFactory for GatedEchoFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        idle_timeout: Option<std::time::Duration>,
        cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let echo_to = meclaw_core::Path::new(
            params
                .get("echo_to")
                .and_then(|v| v.as_str())
                .unwrap_or("/sink"),
        );
        let (sender, receiver) = mpsc::channel::<Message>(1000);
        let me = self.clone();
        let build_path = path.clone();
        let build_outputs = outputs_tx.clone();
        let build_inbox = colony_inbox_tx.clone();
        let build = move |recv: mpsc::Receiver<Message>| {
            me.spawn_count.fetch_add(1, Ordering::Relaxed);
            let conn = meclaw_colony::persist::open_or_create_cell_db(&cell_dir.join("cell.db"))
                .expect("open cell.db");
            let db = DbConn::wrap(conn, None);
            let cell = GatedEchoCell {
                echo_to: echo_to.clone(),
                gate: me.gate.clone(),
                latch: me.latch.clone(),
            };
            build_stateful_task_with_peace(
                build_path.clone(),
                recv,
                build_outputs.clone(),
                build_inbox.clone(),
                idle_timeout,
                message_timeout,
                cell_timeout,
                cell,
                db,
                None,
                None,
            )
        };
        let wake_build = build.clone();
        let wake_inbox = colony_inbox_tx.clone();
        let wake_path = path.clone();
        let wake: WakeFn = Box::new(move |recv| {
            let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = wake_build(recv);
            meclaw_colony::spawn_watcher(
                &wake_inbox,
                wake_path.clone(),
                join,
                peace_rx,
                backstop_rx,
            );
            (stop_tx, death_ack_rx)
        });
        let respawn_build = build;
        let respawn_inbox = colony_inbox_tx.clone();
        let respawn_path = path.clone();
        let respawn: RespawnFn = Box::new(move || {
            let (s, r) = mpsc::channel::<Message>(1000);
            let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = respawn_build(r);
            meclaw_colony::renotify_stop_wiring(
                &respawn_inbox,
                respawn_path.clone(),
                stop_tx,
                death_ack_rx,
            );
            (s, join, peace_rx, backstop_rx)
        });
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn normal_disconnect_still_dlqs_remainder_no_a1_regression() {
    // Generous death-ack term-timeout (60 s): under heavy cargo-parallel load a
    // legitimately-slow death-ack must not spuriously fire `term_timeout` (a
    // premature term_timeout force-kills c1 and changes the remainder handling →
    // R1 would not DLQ as cell_inactive). 60 s tolerates the extra saturation from
    // the β params-message test binaries (CLAUDE.md: failure-marker großzügig).
    set_term_timeout_ms_for_test(60_000);
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/c1/config.json",
        r#"{"cell":{"type":"gated","idle_timeout_ms":60000},"params":{"echo_to":"/sink_old"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/peer/config.json",
        r#"{"cell":{"type":"gated","idle_timeout_ms":60000},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let gate = Arc::new(tokio::sync::Notify::new());
    let latch = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let spawn_count = Arc::new(AtomicU32::new(0));
    let factory: Arc<dyn CellFactory> = Arc::new(GatedEchoFactory {
        gate: gate.clone(),
        latch: latch.clone(),
        spawn_count: spawn_count.clone(),
    });
    let peer_factory: Arc<dyn CellFactory> = Arc::new(GatedEchoFactory {
        gate: Arc::new(tokio::sync::Notify::new()),
        latch: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let h = ColonyHandle::new_with_factories_at(&td, vec![("gated".to_string(), factory.clone())]);

    let c1_dir = td.path().join("main/c1");
    let c1_spawned = factory
        .spawn_cell(
            Path::new("/c1"),
            meclaw_core::serde_json::json!({"echo_to":"/sink_old"}),
            h.runtime().outputs_tx,
            c1_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            Some(std::time::Duration::from_millis(60000)),
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned_typed(Path::new("/c1"), c1_spawned, "gated")
        .await;
    let peer_dir = td.path().join("main/peer");
    let peer_spawned = peer_factory
        .spawn_cell(
            Path::new("/peer"),
            meclaw_core::serde_json::json!({}),
            h.runtime().outputs_tx,
            peer_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            Some(std::time::Duration::from_millis(60000)),
            0,
            None,
            None,
            1000,
        )
        .unwrap();
    h.register_spawned_typed(Path::new("/peer"), peer_spawned, "gated")
        .await;

    // Connect c1 -> peer → c1 active.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope":"/","diff":{"add_edges":[{"from":"c1","to":"peer"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // Wake /c1 with M0 → blocks in handle on the gate.
    h.send(
        MessageBuilder::new(Path::new("/c1"))
            .body(Body::Inline(
                meclaw_core::serde_json::json!({"marker":"M0"}),
            ))
            .build(),
    )
    .await;
    meclaw_testing::wait::wait_for_spawn_count(&spawn_count, 1, std::time::Duration::from_secs(30))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    // Queue a remainder message.
    h.send(
        MessageBuilder::new(Path::new("/c1"))
            .reply_to(Path::new("/src"))
            .body(Body::Inline(
                meclaw_core::serde_json::json!({"marker":"R1"}),
            ))
            .build(),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // DISCONNECT: remove the edge while the cell is blocked. Release the gate so
    // handle(M0) returns; the biased stop fires, the remainder (R1) is DLQ'd.
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: meclaw_core::serde_json::json!({
                "scope":"/","diff":{"remove_edges":[{"match":{"from":"c1","to":"peer"}}]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    // Setup-wait (NOT a semantic discriminator): give colony_task time to receive
    // the disconnect mutation, remove the edge, and SEND the peace-stop to c1
    // BEFORE we release the gate — otherwise handle(M0) returns and c1 processes
    // the remainder (R1) normally instead of DLQ'ing it as cell_inactive. Generous
    // against full-workspace cargo-parallel saturation (CLAUDE.md: failure-marker
    // timeouts großzügig). The colony_task then blocks on the death-ack (which
    // needs this gate), so there is no observable mid-state to poll deterministically.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    gate.notify_one();
    let outcome = ack_rx.await.unwrap();
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "disconnect must commit, got {outcome:?}"
    );

    // R1 was DLQ'd as cell_inactive (NOT re-queued — this is a disconnect).
    // 60 s deadline: generous against full-workspace saturation (the async flow
    // disconnect → peace-stop → mailbox-drain → DLQ-persist can starve under load).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let dlq = h.drain_dead_letters().await;
        if dlq.iter().any(|d| {
            d.resolved_target.as_str() == "/c1"
                && matches!(
                    d.reason,
                    meclaw_colony::dead_letter::DeadLetterReason::CellInactive
                )
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "disconnect remainder was NOT DLQ'd as cell_inactive (A1 regression!)"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    h.shutdown().await;
}
