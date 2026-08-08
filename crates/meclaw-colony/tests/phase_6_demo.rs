//! Phase-6 demo (PROGRESS.md Z.28 demo column).
//!
//! Two integration scenarios:
//! 1. Roundtrip: send a valid `add_nodes` mutation, verify Committed, verify
//!    the new cell is REGISTERED AND ADDRESSABLE (not just config.json
//!    on disk — must prove cell-task is live).
//! 2. Validate-Reject: send a mutation with unknown template + reply_to set;
//!    verify Rejected (template_missing), verify mutation_log has the expected
//!    row count, verify reply_to received the error message (cascaded into the
//!    DLQ via the observer echo cell).
//!
//! Phase-11 T16 Migration: Mutation nutzt Templates-Registry. Vor jeder Mutation
//! wird ein Template-Verzeichnis angelegt und via RescanTemplates geladen.

use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;

/// Phase-11 T16: Legt ein Template-Verzeichnis für `name`/`cell_type` an und lädt es.
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

/// Helper: send a mutation and await the outcome.
async fn send_mutation(
    h: &ColonyHandle,
    payload: meclaw_core::serde_json::Value,
    reply_to: Option<Path>,
) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_6_demo_roundtrip_proves_cell_is_addressable() {
    let h = ColonyHandle::new_with_echo();
    // Phase-11 T16: echo-Template-Verzeichnis anlegen und laden.
    setup_template(&h, "echo", "echo").await;

    // Topologie für positives Receipt:
    //   /observer = CaptureCell mit mpsc-tap (direkt via ColonyHandle::spawn,
    //               nicht via Mutation — wir wollen die Observer-Reception
    //               unzweideutig in einem Test-Channel beobachten).
    //   /demo     = Echo-Cell via Mutation (das ist die Cell, deren
    //               Adressierbarkeit der Test beweisen soll).
    //   Probe → /demo → /demo emit → /observer.recv() in unserem Channel.
    //
    // Wenn /demo NICHT adressierbar wäre: receiver_rx blockiert auf timeout,
    // assert fails. Wenn /demo lebt + die Probe empfängt + zu /observer
    // weiterleitet: receiver_rx erhält die emit-Message. Eindeutig positiv.
    let (recv_tx, mut receiver_rx) = tokio::sync::mpsc::channel::<Message>(8);
    h.spawn(Path::new("/observer"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{
                "name": "demo",
                "template": "echo",
                "override_params": {"echo_to": "/observer"}
            }]}
        }),
        None,
    )
    .await;
    let mid = match outcome {
        MutationOutcome::Committed { id } => id,
        other => panic!("expected Committed, got {other:?}"),
    };

    // A1: /demo's echo to /observer needs an explicit catch-all out-edge — the
    // implicit identity-fallback is gone, so without this edge /demo's emission
    // would dead-letter as no_route and /observer would never receive.
    h.add_edge(Uuid::now_v7(), Path::new("/demo"), Path::new("/observer"))
        .await;

    // Probe an /demo. UBF-konformer Body ({"origin": "user", ...}) damit die
    // /demo-Emission ebenfalls valides UBF ist und nicht im InvalidUbfBody-
    // Pfad landet, sondern korrekt zu /observer weitergeleitet wird.
    let probe = MessageBuilder::new(Path::new("/demo"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(meclaw_core::serde_json::json!({
            "messages": [{"origin": "user", "type": "text", "text": "ping"}]
        })))
        .build();
    h.inbox_tx
        .send(ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: probe,
        })
        .await
        .unwrap();

    // Positiver Beweis: /observer erhält die Message via CaptureCell-tap.
    let received = tokio::time::timeout(std::time::Duration::from_secs(30), receiver_rx.recv())
        .await
        .expect("/observer must receive a message within 30s — proves /demo is addressable")
        .expect("CaptureCell channel must yield a message");
    assert_eq!(
        received.target.as_str(),
        "/observer",
        "/observer must have received the cascade from /demo, got target={}",
        received.target.as_str()
    );

    // Verify mutation_log committed-row for the roundtrip mid.
    let db_path = h.tempdir_path().join("colony.db");
    h.shutdown().await;
    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let status: String = conn
        .query_row("SELECT status FROM mutation_log WHERE id=?", [&mid], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "committed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_6_demo_validate_reject_writes_rejected_log_row_and_replies_to_reply_to() {
    let h = ColonyHandle::new_with_echo();
    // Phase-11 T16: echo-Template-Verzeichnis anlegen und laden.
    setup_template(&h, "echo", "echo").await;

    // Pre-spawn a reply-observer at /observer (echo cell, echo_to=/observer_echo_target).
    let setup = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{
                "name": "observer",
                "template": "echo",
                "override_params": {"echo_to": "/observer_echo_target"}
            }]}
        }),
        None,
    )
    .await;
    assert!(
        matches!(setup, MutationOutcome::Committed { .. }),
        "setup add must commit, got {setup:?}"
    );

    // Invalid mutation: unknown template + reply_to=/observer.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "x", "template": "doesnotexist"}]}
        }),
        Some(Path::new("/observer")),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "template_missing");
        }
        other => panic!("expected Rejected with template_missing, got {other:?}"),
    }

    // Give the routed error-reply a tick to traverse /observer → emit → cascade.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Drain DLQ — beweis dass die error-reply tatsächlich an /observer
    // zugestellt wurde. Differenzierung:
    //   - /observer absent → error-reply (reply_to=None) → direct DLQ mit
    //     sender_path=/colony (von send_eda_reject's route-call).
    //   - /observer live → empfängt error-reply, emit zu /observer_echo_target
    //     → DLQ-Entry mit sender_path=/observer (die emittierende Cell).
    // Wir asserten sender_path=/observer = eindeutiger Beweis, dass die Cell
    // die Nachricht verarbeitet UND emittiert hat (reason ist hier egal —
    // InvalidUbfBody oder UnresolvedPath/TtlExpired sind alle valide
    // Cascade-Endpunkte für eine live-emittierende Cell).
    let (drain_tx, drain_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::DrainDeadLetters { ack: drain_tx })
        .await
        .unwrap();
    let drained = drain_rx.await.unwrap();
    let cascade_via_observer = drained
        .iter()
        .any(|dl| dl.sender_path.as_str() == "/observer");
    assert!(
        cascade_via_observer,
        "error-reply must have been routed to live /observer — expected DLQ entry with sender_path=/observer (proves observer-cell emitted); drained={drained:?}"
    );

    // Phase-16 W3 (A6): the validate-reject now DOES write a durable
    // `status='rejected'` row (it used to write nothing). mutation_log holds the
    // committed setup-row PLUS the rejected row — two rows total.
    let db_path = h.tempdir_path().join("colony.db");
    h.shutdown().await;
    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let row_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM mutation_log", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        row_count, 2,
        "A6: the validate-reject writes a rejected row alongside the committed setup-row"
    );
    let (rejected, code): (i64, Option<String>) = conn
        .query_row(
            "SELECT COUNT(*), MAX(error_code) FROM mutation_log WHERE status='rejected'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rejected, 1, "exactly one rejected row");
    assert_eq!(
        code.as_deref(),
        Some("template_missing"),
        "the rejected row carries the validate error_code"
    );
}

// Phase-6 T26 (crash-recovery) lebt in eigener Test-Binary
// `tests/phase_6_crash_recovery.rs` — der `AFTER_RENAME`-Static aus
// `mutation::hook` ist binary-lokal, und das Hook-Set würde parallele
// Tests in derselben Binary blockieren. Eigene Binary = eigener Static.
