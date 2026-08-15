//! Slice 11-F T18: lazy check + auto-remove (overview Z.1163 path 2).
//!
//! A registry entry without a directory → error + auto-remove.
//!
//! Setup: a ColonyDb with an `UpsertTemplate` op for a template whose
//! `filesystem_path` does not exist (e.g. `/nonexistent/path`).
//! Boot the colony, send a mutation add_nodes(template="ghost").
//! The outcome must be Rejected with error_code "template_missing".
//! After the rejection: db.read_templates() must no longer contain the entry.

use meclaw_colony::persist::writer::ColonyWriteOp;
use meclaw_colony::{ColonyDb, ColonyMsg, MutationOutcome};
use meclaw_core::Uuid;
use meclaw_testing::ColonyHandle;
use tempfile::TempDir;

/// Sends a mutation payload and awaits the outcome.
async fn send_mutation(
    h: &ColonyHandle,
    payload: meclaw_core::serde_json::Value,
) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
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

/// Lazy-Check removes orphan registry entry and rejects the mutation.
///
/// 1. A template "ghost" is written directly into colony.db via UpsertTemplate
///    with a filesystem_path that does not exist on disk (`/nonexistent/ghost_path`).
/// 2. Colony is started with this pre-populated DB.
/// 3. A mutation `add_nodes(template="ghost")` is sent.
/// 4. Expected outcome: `Rejected { error_code: "template_missing" }`.
/// 5. After rejection, a fresh ColonyDb read must show the "ghost" entry is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lazy_check_removes_orphan_registry_entry() {
    // --- Setup: pre-populate colony.db before Colony starts. ---
    let td = TempDir::new().expect("tempdir");
    let db_path = td.path().join("colony.db");

    // Open a temporary ColonyDb, insert a template with a nonexistent filesystem_path.
    {
        let db = ColonyDb::open(&db_path).expect("open colony.db");
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        db.send_op(ColonyWriteOp::UpsertTemplate {
            template_id: Uuid::now_v7().to_string(),
            name: "ghost".into(),
            version: None,
            filesystem_path: "/nonexistent/ghost_path".into(),
            description_json: "{}".into(),
            tags_json: "[]".into(),
            author: None,
            scanned_at: 1,
            ack: Some(ack_tx),
        })
        .await;
        // Wait for durable commit before dropping the DB.
        ack_rx.recv().expect("ack from writer");
        // Drop db (writer thread exits when Sender is dropped).
    }

    // Verify precondition: "ghost" is in the DB.
    {
        let db = ColonyDb::open(&db_path).expect("re-open pre-check");
        let rows = db.read_templates().expect("read_templates");
        assert_eq!(rows.len(), 1, "precondition: one ghost template in DB");
        assert_eq!(rows[0].name, "ghost");
    }

    // --- Start Colony with the pre-populated DB (no new factories needed). ---
    let h = ColonyHandle::new_with_echo_at(td.path());

    // --- Send mutation add_nodes(template="ghost"). ---
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "ghost_cell", "template": "ghost"}]
            }
        }),
    )
    .await;

    // --- Verify outcome is Rejected with "template_missing". ---
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "template_missing",
                "expected template_missing, got {error_code}"
            );
        }
        other => panic!("expected Rejected, got {other:?}"),
    }

    // Graceful shutdown so all fire-and-forget writer ops are flushed.
    h.shutdown().await;

    // --- Verify: ghost entry has been auto-removed from the DB. ---
    let db2 = ColonyDb::open(&db_path).expect("re-open post-check");
    let rows = db2
        .read_templates()
        .expect("read_templates after rejection");
    assert!(
        rows.iter().all(|r| r.name != "ghost"),
        "ghost template must be auto-removed from DB after lazy-check, but rows = {rows:?}"
    );
}
