//! Phase-6 T26 demo: crash-recovery end-to-end.
//!
//! Isoliert in eigener Test-Binary: der `AFTER_RENAME`-Static aus
//! `mutation::hook` ist binary-lokal. Wenn dieser Test mit anderen
//! Mutation-Tests in der gleichen Binary parallel liefe, würde das hook-set
//! auch fremde `handle_mutation`-Aufrufe parken — sie würden hängen, bis
//! der Hook gecleared wird. Eigene Binary = eigener Static = keine Kopplung.
//!
//! Nur unter `feature = "test-hooks"` aktiv; im Default-Build ist die Datei
//! ein leerer Test-Binary-Stub.

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

/// Chain: set hook → send mutation → parks after rename → in_flight row ist
/// durabel (`wait_for_any_in_flight`) → colony-task aborten (Crash-Sim) →
/// Handle droppen, TempDir behalten → zweiter Boot ruft
/// `recover_in_flight_mutations` → in_flight-Row transitioniert zu
/// `failed`/`crash_during_commit`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_6_demo_crash_recovery_marks_in_flight_as_failed() {
    use std::sync::Arc;
    use tokio::sync::Notify;

    // TempDir hält das FS-Setup über h.abort() hinaus am Leben.
    let td = tempfile::TempDir::new().unwrap();
    let db_path = td.path().join("colony.db");

    // Hook VOR dem Colony-Spawn setzen — die erste passende Mutation parkt
    // zwischen rename(2) und Spawn-Loop.
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

        // Mutation im Hintergrund senden — der Ack feuert nie, weil
        // handle_mutation auf dem Hook parkt und nie zum committed-Step kommt.
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
            // ack_rx wird absichtlich gedroppt — der Ack kommt nie.
        });

        // Barrier: warte auf den durablen in_flight-Insert. Beweis, dass die
        // Mutation `apply_mutation` (stage+rename) erreicht hat UND der Writer
        // die in_flight-Row committet hat, BEVOR der Hook geparkt hat.
        meclaw_testing::wait::wait_for_any_in_flight(&db_path, std::time::Duration::from_secs(5))
            .await;

        // Crash-Simulation: Colony-Task aborten. KEIN shutdown, KEIN drain —
        // der Writer-Thread wird gedroppt, wenn die Colony-Future endet.
        h.abort();

        send_task
    };

    // Der send_task ist obsolet — Join-Handle droppen.
    drop(send_task);

    // Hook löschen, damit nachfolgende Tests unbeeinflusst sind.
    meclaw_colony::mutation::hook::clear_after_rename();

    // Boot 2: Recovery auf der überlebenden DB. Der TempDir lebt noch in
    // diesem Scope, also sind colony.db und `.staging/<id>/`-Reste noch auf
    // dem FS.
    let report =
        meclaw_colony::mutation::recovery::recover_in_flight_mutations(td.path(), &db_path)
            .expect("recovery must succeed against the surviving DB");

    assert_eq!(
        report.failed_mutation_ids.len(),
        1,
        "expected exactly 1 in_flight mutation to be recovered, got {report:?}"
    );

    // DB-Row muss auf `failed` mit kanonischem Grund transitioniert sein.
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

    // TempDir droppt hier — Staging-Cleanup ist best-effort (Audit-Modell).
}
