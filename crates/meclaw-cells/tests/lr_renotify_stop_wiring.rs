//! Phase-13.5 Slice 4 T6b (I1): the three production Long-Running factories
//! (`proxy`/`timer`/`mcp`) restore stop-wiring on a `RespawnFn` re-spawn
//! (crash-restart / reconnect-eager) via `renotify_stop_wiring` — the same
//! restoration T6 did for the stateful `store`/`llm` factories.
//!
//! Two layers of proof:
//!   1. Per-factory (proxy/timer/mcp): invoking the `RespawnFn` emits a
//!      `ColonyMsg::StopWiringRestored` on the colony inbox (deterministic, no
//!      network — the proxy/mcp endpoints are unreachable but the re-notify fires
//!      synchronously inside the closure before any I/O).
//!   2. Full-colony (timer, representative): a reconnect-eager re-spawn restores
//!      a LIVE stop pair so a SUBSEQUENT disconnect COMMITS (no
//!      `stop_wiring_unavailable`, no `term_timeout`).

use meclaw_cells::mcp::factory::McpCellFactory;
use meclaw_cells::proxy::factory::ProxyCellFactory;
use meclaw_cells::timer::factory::TimerCellFactory;
use meclaw_colony::{
    CellFactory, ColonyMsg, ContractView, MutationOutcome, set_term_timeout_ms_for_test,
};
use meclaw_core::{CellEmission, JsonValue, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

/// Invoke a Long-Running factory's `RespawnFn` (built via the boot-inactive
/// hook, same construction as `spawn_cell`'s respawn) and assert it emits a
/// `ColonyMsg::StopWiringRestored` for `/ll` on the colony inbox — proving the
/// re-spawn restored a fresh, LIVE `(stop_tx, death_ack_rx)` pair.
async fn assert_respawn_renotifies_stop_wiring(factory: Arc<dyn CellFactory>, params: JsonValue) {
    let td = TempDir::new().unwrap();
    let cell_dir: PathBuf = td.path().join("ll");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, mut inbox_rx) = mpsc::channel::<ColonyMsg>(8);

    let respawn = factory
        .clone()
        .build_boot_inactive_respawn(
            Path::new("/ll"),
            params,
            out_tx,
            cell_dir,
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            1000,
        )
        .expect("Long-Running factory must return Some(respawn)");

    // Invoke the respawn closure (exactly what the reconnect-eager arm does).
    let (sender, join, _peace_rx, _backstop_rx) = respawn();

    // The closure must have re-notified the colony of its fresh stop pair.
    let msg = tokio::time::timeout(Duration::from_secs(30), inbox_rx.recv())
        .await
        .expect("StopWiringRestored must arrive within 30s")
        .expect("colony inbox open");
    match msg {
        ColonyMsg::StopWiringRestored { path, .. } => {
            assert_eq!(
                path.as_str(),
                "/ll",
                "re-notify must carry the cell's own path"
            );
        }
        _ => panic!("expected StopWiringRestored from the LR respawn closure"),
    }

    drop(sender);
    let _ = tokio::time::timeout(Duration::from_secs(30), join).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proxy_respawn_renotifies_stop_wiring() {
    assert_respawn_renotifies_stop_wiring(
        Arc::new(ProxyCellFactory),
        json!({ "bot_token": "T", "emit_to": "/x", "base_url": "http://127.0.0.1:1" }),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timer_respawn_renotifies_stop_wiring() {
    assert_respawn_renotifies_stop_wiring(Arc::new(TimerCellFactory), json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_respawn_renotifies_stop_wiring() {
    assert_respawn_renotifies_stop_wiring(
        Arc::new(McpCellFactory),
        json!({ "endpoint": "http://127.0.0.1:1/rpc" }),
    )
    .await;
}

// ───────────────────────────────────────────────────────────────────────────
// Full-colony proof (timer, representative): reconnect-eager re-spawn restores
// a live stop pair → a subsequent disconnect COMMITS.
// ───────────────────────────────────────────────────────────────────────────

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

async fn lifecycle_status(h: &ColonyHandle, path: &str) -> Option<(bool, String)> {
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
        .map(|e| (e.active, e.lifecycle_status))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timer_reconnect_eager_restores_stop_wiring_then_disconnect_commits() {
    // Generous death-ack term-timeout: under heavy cargo-parallel load a
    // legitimately-slow death-ack must not spuriously fire `term_timeout` — the
    // asserted reject (if the wiring were missing) is `stop_wiring_unavailable`,
    // not the timeout, so this override does not mask the bug under test.
    set_term_timeout_ms_for_test(30_000);

    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/timer/config.json",
        r#"{"cell":{"type":"timer","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/peer/config.json",
        r#"{"cell":{"type":"timer","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let timer_dir = td.path().join("main/timer");
    std::fs::create_dir_all(&timer_dir).unwrap();
    let peer_dir = td.path().join("main/peer");
    std::fs::create_dir_all(&peer_dir).unwrap();

    let factory: Arc<dyn CellFactory> = Arc::new(TimerCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("timer".to_string(), factory.clone())]);

    // Register /peer + /timer as Awake long-running.
    for (path, dir) in [("/peer", &peer_dir), ("/timer", &timer_dir)] {
        let spawned = factory
            .clone()
            .spawn_cell(
                Path::new(path),
                json!({}),
                h.runtime().outputs_tx,
                dir.clone(),
                ContractView::default(),
                h.inbox_tx.clone(),
                None,
                -1,
                None,
                None,
                1000,
            )
            .unwrap();
        h.register_spawned_typed(Path::new(path), spawned, "timer")
            .await;
    }

    // Connect → /timer active + Awake (initial spawn has live stop wiring).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"timer","to":"peer"}]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // First disconnect: peace-stops the live cell (consumes stop_tx → None),
    // parks NotYetSpawned, active=false. Commits cleanly.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"timer","to":"peer"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "first disconnect commits, got {outcome:?}"
    );

    // Reconnect eager: re-spawn via RespawnFn. WITH the I1 fix the respawn calls
    // `renotify_stop_wiring` → stop_tx restored. WITHOUT it, stop_tx stays None.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"timer","to":"peer"}]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    let (active, status) = lifecycle_status(&h, "/timer").await.expect("/timer");
    assert!(active && status == "Awake", "reconnect → active + Awake");

    // SECOND disconnect: this is the I1 discriminator. With restored stop-wiring
    // it peace-stops + COMMITS; without it the guard rejects stop_wiring_unavailable.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"timer","to":"peer"}}]}}),
    )
    .await;
    match &outcome {
        MutationOutcome::Committed { .. } => {}
        MutationOutcome::Rejected { error_code, .. } => panic!(
            "second disconnect after reconnect-eager must COMMIT (stop-wiring restored), \
             got Rejected({error_code})"
        ),
    }
    let (active, status) = lifecycle_status(&h, "/timer").await.expect("/timer");
    assert!(!active, "second disconnect deactivated /timer");
    assert_eq!(
        status, "NotYetSpawned",
        "peace-stopped task parked after the committed disconnect"
    );

    h.shutdown().await;
}
