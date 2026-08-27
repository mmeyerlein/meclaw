//! Phase-6.5 demo gate.
//!
//! Proves the authority separation: cell_task_stateful holds the cell.db
//! connection across `.await`, per-output state carries interleaved emit+write.
//!
//! Sink topology: /sink is a real terminal CaptureCell (phase-3a pattern),
//! registered directly via ColonyHandle::spawn. No re-emit, no cascade loop.
//! The append-log table (step=1, step=2) proves both writes across the await
//! boundary.
//!
//! Phase-11 T16 migration: the mutation uses the templates registry. Before the
//! mutation a `templates/multi_update/` directory is created and loaded via
//! RescanTemplates.

use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_6_5_demo_multi_update_cell_writes_log_between_emits() {
    let td = tempfile::TempDir::new().unwrap();
    let factory: Arc<dyn CellFactory> =
        Arc::new(meclaw_testing::factories::MultiUpdateMockCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("multi_update".to_string(), factory)],
    );

    // Phase-11 T16: create and load a template directory for multi_update.
    {
        let templates_root = td.path().join("templates");
        let tpl = templates_root.join("multi_update");
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(tpl.join("template.json"), r#"{"name":"multi_update"}"#).unwrap();
        std::fs::write(
            tpl.join("config.json"),
            r#"{"cell":{"type":"multi_update"},"params":{"sink_target":"/unset"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        )
        .unwrap();
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        h.inbox_tx
            .send(meclaw_colony::ColonyMsg::RescanTemplates {
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

    // /sink = terminal CaptureCell, registered directly (no mutation path,
    // no re-emit → no cascade loop).
    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // /multi = MultiUpdateMockCell via Mutation
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: meclaw_core::serde_json::json!({
                "scope": "/",
                "diff": {"add_nodes": [{
                    "name": "multi", "template": "multi_update",
                    "override_params": {"sink_target": "/sink"}
                }]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    assert!(matches!(
        ack_rx.await.unwrap(),
        MutationOutcome::Committed { .. }
    ));

    // A1: /multi's two emits to /sink need an explicit catch-all out-edge — the
    // implicit identity-fallback is gone. /multi only ever emits to /sink, so a
    // single unconditional edge carries both emits.
    h.add_edge(Uuid::now_v7(), Path::new("/multi"), Path::new("/sink"))
        .await;

    // Probe an /multi
    let probe = MessageBuilder::new(Path::new("/multi"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(
            meclaw_core::serde_json::json!({"messages": []}),
        ))
        .build();
    h.inbox_tx
        .send(ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: probe,
        })
        .await
        .unwrap();

    // Phase-13.5-A6 follow-up test hygiene: poll-based sync until sink_count==2
    // OR a 2s timeout, THEN shutdown + assert. Before the deterministic-shutdown
    // fix the test relied on a sleep(200ms) + the race-induced slow-shutdown
    // delay as an implicit wait for the outputs_rx drain. With the deterministic
    // fast shutdown the pre-shutdown wait is too tight; poll-based sync is the
    // clean form (late delivery observed via diagnostics, 10/10 green).
    // Diagnostic finding: both emissions arrive, just outside the 200ms sleep
    // window.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut sink_count = 0;
    while sink_count < 2 && std::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(50), sink_rx.recv()).await {
            Ok(Some(_)) => sink_count += 1,
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    h.shutdown().await;
    assert_eq!(
        sink_count, 2,
        "CaptureCell at /sink must receive 2 emits (after 2s poll)"
    );

    // Main proof: read cell.db directly — both append-log rows
    let cell_db_path = td.path().join("multi").join("cell.db");
    let conn = rusqlite::Connection::open_with_flags(
        &cell_db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let rows: Vec<i64> = conn
        .prepare("SELECT step FROM multi_update_log ORDER BY step")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![1, 2],
        "interleaved emit+write must produce both log rows across await; got {rows:?}"
    );
}
