//! Phase-6 T12: ColonyMsg::Mutation variant skeleton arm.
//!
//! Smoke-test that the new variant is wired into both select!-loop arms
//! (main + Shutdown-Drain) and that the inline skeleton acks `Committed`
//! for any input. T13+ replaces the inline arm with a real `handle_mutation`
//! that does substitute / validate / apply.
//!
//! Phase-11 T16 migration: the mutation uses the templates registry. Tests that
//! use the `echo` template create a registry entry up front via RescanTemplates.

use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::Uuid;
use meclaw_testing::ColonyHandle;

/// Phase-11 T16: creates a template directory for `name`/`cell_type` and loads it.
async fn setup_template(h: &ColonyHandle, name: &str, cell_type: &str) {
    let root = h.tempdir_path();
    let templates_root = root.join("templates");
    let tpl = templates_root.join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        format!(
            r#"{{"cell":{{"type":"{cell_type}"}},"params":{{}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
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
    ack_rx.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn colony_msg_mutation_returns_committed_outcome_for_empty_diff() {
    let h = ColonyHandle::new();
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: meclaw_core::serde_json::json!({"scope": "/", "diff": {}}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let outcome = ack_rx.await.unwrap();
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_mutation_emits_rejected_and_writes_rejected_mutation_log_row() {
    let h = ColonyHandle::new();
    let db_path = h.tempdir_path().join("colony.db");
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: meclaw_core::serde_json::json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": "x", "template": "doesnotexist"}]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let outcome = ack_rx.await.unwrap();
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "template_missing")
        }
        _ => panic!("expected Rejected"),
    }
    h.shutdown().await; // BARRIER — writer-thread flushed.

    // Phase-16 W3 (A6): an invalid (validate-rejected) mutation never gets an
    // `in_flight` row, but now DOES get a single terminal `status='rejected'`
    // audit row (error_code + trace_id + reason). It must NOT carry a committed_at.
    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let (row_count, status, code, committed): (i64, String, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT COUNT(*), MAX(status), MAX(error_code), MAX(committed_at) FROM mutation_log",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row_count, 1, "the validate-reject writes exactly one row");
    assert_eq!(status, "rejected");
    assert_eq!(code.as_deref(), Some("template_missing"));
    assert!(committed.is_none(), "a reject never commits");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn substitute_error_rejects_before_insert() {
    let h = ColonyHandle::new();
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: meclaw_core::serde_json::json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": "x", "template": "${MISSING_ENV}"}]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    match ack_rx.await.unwrap() {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "env_var_missing")
        }
        _ => panic!("expected Rejected"),
    }
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn valid_mutation_writes_committed_row() {
    let h = ColonyHandle::new();
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: meclaw_core::serde_json::json!({"scope": "/", "diff": {}}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let id = match ack_rx.await.unwrap() {
        MutationOutcome::Committed { id } => id,
        _ => panic!("expected Committed"),
    };
    let db_path = h.tempdir_path().join("colony.db");
    h.shutdown().await; // BARRIER — writer-thread flushed.

    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let status: String = conn
        .query_row("SELECT status FROM mutation_log WHERE id=?", [&id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "committed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_node_mutation_spawns_addressable_cell() {
    let h = meclaw_testing::ColonyHandle::new_with_echo(); // T18 adds this helper
    setup_template(&h, "echo", "echo").await;
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: meclaw_core::serde_json::json!({
                "scope": "/",
                "diff": {"add_nodes": [{
                    "name": "n",
                    "template": "echo",
                    "override_params": {"echo_to": "/n"}
                }]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let outcome = ack_rx.await.unwrap();
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "expected Committed, got {outcome:?}"
    );
    // After commit, /n must be in registry. Probe via direct SQLite read of
    // the registry table after writer-thread is flushed by shutdown().
    let db_path = h.tempdir_path().join("colony.db");
    h.shutdown().await; // flush writer

    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let cnt: i64 = conn
        .query_row("SELECT COUNT(*) FROM registry WHERE path='/n'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(cnt, 1, "/n must be registered in colony.db.registry");
}

/// Phase-13.5-Lifecycle-3b Task 6 (SCOPE 4, spec Z.260): `remove_nodes` is a
/// **Disconnect, not a Delete**. The original Phase-6 test
/// `remove_then_add_at_same_path_works` encoded the OLD T19 contract
/// (`registry.remove` → the slot is free → re-add at the same path no longer
/// collides). Under the new contract the registry entry STAYS (No-Delete:
/// `cell_id`, FS, `cell.db` untouched) and only goes `active = false`. The slot
/// therefore remains occupied, so a re-add of the SAME name no longer clears the
/// way — this test now asserts the surviving-entry contract directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_nodes_keeps_entry_inactive_not_deleted() {
    use meclaw_colony::api_dto::ReadRegistryReply;

    let h = meclaw_testing::ColonyHandle::new_with_echo();
    setup_template(&h, "echo", "echo").await;

    // Read the RAM registry entry for `/n_rm` (or `None`).
    async fn entry(
        h: &meclaw_testing::ColonyHandle,
    ) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<ReadRegistryReply>();
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
            .find(|e| e.path == "/n_rm")
    }

    // Add /n_rm.
    {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        h.inbox_tx
            .send(ColonyMsg::Mutation {
                payload: meclaw_core::serde_json::json!({
                    "scope": "/",
                    "diff": {"add_nodes": [{
                        "name": "n_rm",
                        "template": "echo",
                        "override_params": {"echo_to": "/n_rm"}
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
    }
    let cell_id_before = entry(&h).await.expect("/n_rm registered").cell_id;

    // remove_nodes /n_rm → Disconnect (entry STAYS, active = false).
    {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        h.inbox_tx
            .send(ColonyMsg::Mutation {
                payload: meclaw_core::serde_json::json!({
                    "scope": "/",
                    "diff": {"remove_nodes": [{"match": {"name": "n_rm"}}]}
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
    }

    // New contract: the entry STAYS, same cell_id, active == false.
    let after = entry(&h)
        .await
        .expect("/n_rm entry must STAY per No-Delete (Disconnect, not Delete)");
    assert_eq!(after.cell_id, cell_id_before, "cell_id must be unchanged");
    assert!(!after.active, "/n_rm must be inactive after remove_nodes");

    // Re-add of the SAME name is now rejected — the slot is still occupied
    // (No-Delete). The OLD contract expected the slot to be free; that is exactly
    // what changed. We only assert the re-add does NOT commit (the precise
    // error_code — naming_collision vs. resume_requires_stopped_cell — is not the
    // point of this test).
    let third = {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        h.inbox_tx
            .send(ColonyMsg::Mutation {
                payload: meclaw_core::serde_json::json!({
                    "scope": "/",
                    "diff": {"add_nodes": [{
                        "name": "n_rm",
                        "template": "echo",
                        "override_params": {"echo_to": "/n_rm"}
                    }]}
                }),
                reply_to: None,
                trace_id: Uuid::now_v7(),
                parent_message_id: Uuid::now_v7(),
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap()
    };
    assert!(
        matches!(third, MutationOutcome::Rejected { .. }),
        "re-add at the still-occupied path must be rejected (No-Delete), got {third:?}"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_edge_mutation_persists_to_db_by_committed_ack() {
    let h = meclaw_testing::ColonyHandle::new_with_echo();
    setup_template(&h, "echo", "echo").await;
    let send = |payload| {
        let inbox = h.inbox_tx.clone();
        async move {
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            inbox
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
    };
    // Add two echo cells.
    assert!(matches!(
        send(meclaw_core::serde_json::json!({
            "scope": "/", "diff": {"add_nodes": [
                {"name": "a_e", "template": "echo", "override_params": {"echo_to": "/a_e"}},
                {"name": "b_e", "template": "echo", "override_params": {"echo_to": "/b_e"}}
            ]}
        }))
        .await,
        MutationOutcome::Committed { .. }
    ));
    // Add edge a_e -> b_e.
    assert!(matches!(
        send(meclaw_core::serde_json::json!({
            "scope": "/", "diff": {"add_edges": [{"from": "a_e", "to": "b_e"}]}
        }))
        .await,
        MutationOutcome::Committed { .. }
    ));

    // Verify durability via fresh connection AFTER shutdown.
    let db_path = h.tempdir_path().join("colony.db");
    h.shutdown().await;
    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let cnt: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE from_path='/a_e' AND to_path='/b_e'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        cnt, 1,
        "edge must be durable in colony.db by FIFO ordering before committed ack"
    );
}
