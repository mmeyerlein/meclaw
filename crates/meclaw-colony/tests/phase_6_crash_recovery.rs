//! Phase-6 T26 demo: crash-recovery end-to-end.
//!
//! Isolated in its own test binary: the `AFTER_RENAME` static from
//! `mutation::hook` is binary-local. If this test ran in parallel with other
//! mutation tests in the same binary, the hook set would also park foreign
//! `handle_mutation` calls — they would hang until the hook is cleared.
//! Own binary = own static = no coupling.
//!
//! Only active under `feature = "test-hooks"`; in the default build the file is
//! an empty test-binary stub.

#![cfg(feature = "test-hooks")]

use meclaw_colony::ColonyMsg;
use meclaw_core::Uuid;

/// Write a minimal `echo` template so the mutation's `template: "echo"` resolves.
/// `new_with_echo_at` only registers the echo *factory* (cell-type), not a
/// *template* — without this the mutation rejects as `template_missing` BEFORE the
/// durable in_flight insert, and the in_flight barrier never fires.
fn write_echo_template(root: &std::path::Path) {
    let tpl = root.join("templates").join("echo");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"echo"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/sink"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// Load the templates dir into the running colony (serial inbox → completes
/// before the subsequently-sent mutation is validated).
async fn rescan_templates(
    inbox: &tokio::sync::mpsc::Sender<ColonyMsg>,
    templates_root: std::path::PathBuf,
) {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    inbox
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

/// Chain: set hook → send mutation → parks after rename → the in_flight row is
/// durable (`wait_for_any_in_flight`) → abort the colony task (crash sim) →
/// drop the handle, keep the TempDir → the second boot calls
/// `recover_in_flight_mutations` → the in_flight row transitions to
/// `failed`/`crash_during_commit`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_6_demo_crash_recovery_marks_in_flight_as_failed() {
    use std::sync::Arc;
    use tokio::sync::Notify;

    // The TempDir keeps the FS setup alive beyond h.abort().
    let td = tempfile::TempDir::new().unwrap();
    let db_path = td.path().join("colony.db");

    // Set the hook BEFORE the colony spawn — the first matching mutation parks
    // between rename(2) and the spawn loop.
    let notify = Arc::new(Notify::new());
    meclaw_colony::mutation::hook::set_after_rename(notify.clone());

    // Boot 1: Colony spawnen, Mutation senden, in_flight abwarten, aborten.
    let send_task = {
        let h = meclaw_testing::ColonyHandle::new_with_echo_at(td.path());
        let inbox = h.inbox_tx.clone();

        // Load the `echo` template before the mutation is sent — otherwise the
        // mutation rejects as `template_missing` before the in_flight insert.
        write_echo_template(td.path());
        rescan_templates(&inbox, td.path().join("templates")).await;

        // Send the mutation in the background — the ack never fires because
        // handle_mutation parks on the hook and never reaches the committed step.
        let send_task = tokio::spawn(async move {
            let (ack_tx, _ack_rx) = tokio::sync::oneshot::channel();
            inbox
                .send(ColonyMsg::Mutation {
                    payload: meclaw_core::serde_json::json!({
                        "scope": "/",
                        "diff": {"add_nodes": [{
                            "name": "crash_target",
                            "template": "echo",
                            "override_params": {"echo_to": "/crash_target"}
                        }]}
                    }),
                    reply_to: None,
                    trace_id: Uuid::now_v7(),
                    parent_message_id: Uuid::now_v7(),
                    ack: ack_tx,
                })
                .await
                .unwrap();
            // ack_rx is dropped on purpose — the ack never arrives.
        });

        // Barrier: wait for the durable in_flight insert. Proof that the
        // mutation reached `apply_mutation` (stage+rename) AND the writer
        // committed the in_flight row BEFORE the hook parked.
        meclaw_testing::wait::wait_for_any_in_flight(&db_path, std::time::Duration::from_secs(5))
            .await;

        // Crash simulation: abort the colony task. NO shutdown, NO drain —
        // the writer thread is dropped when the colony future ends.
        h.abort();

        send_task
    };

    // The send_task is obsolete — drop the join handle.
    drop(send_task);

    // Clear the hook so subsequent tests stay unaffected.
    meclaw_colony::mutation::hook::clear_after_rename();

    // Boot 2: recovery against the surviving DB. The TempDir is still alive in
    // this scope, so colony.db and the `.staging/<id>/` remains are still on the
    // filesystem.
    let report =
        meclaw_colony::mutation::recovery::recover_in_flight_mutations(td.path(), &db_path)
            .expect("recovery must succeed against the surviving DB");

    assert_eq!(
        report.failed_mutation_ids.len(),
        1,
        "expected exactly 1 in_flight mutation to be recovered, got {report:?}"
    );

    // The DB row must have transitioned to `failed` with the canonical reason.
    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let (status, reason): (String, Option<String>) = conn
        .query_row(
            "SELECT status, failure_reason FROM mutation_log WHERE id=?",
            [&report.failed_mutation_ids[0]],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "failed", "in_flight row must transition to failed");
    assert_eq!(
        reason.as_deref(),
        Some("crash_during_commit"),
        "failure_reason must be 'crash_during_commit'"
    );

    // The TempDir drops here — staging cleanup is best-effort (audit model).
}
