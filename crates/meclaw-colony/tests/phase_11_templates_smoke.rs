//! Phase-11 demo (PROGRESS.md Z.35): "instantiate from a template via mutation".
//!
//! Proof chain:
//!   1. Templates directory `templates/echo-cell@1.0/` with `template.json` + `config.json`.
//!   2. `.env` with `GREETING=hello-templates`.
//!   3. Boot the colony + scan templates via RescanTemplates.
//!   4. Register /sink (CaptureCell) BEFORE the probe (anti-cascade, phase-6.5 lesson).
//!   5. Mutation: add_nodes(name="echo", template="echo-cell@1.0",
//!      override_params={echo_to="/sink", greeting="${GREETING}"})
//!      + add_edges(/echo → /sink).
//!   6. The outcome must be MutationOutcome::Committed.
//!   7. Probe → /echo, check the receipt in /sink via CaptureCell.
//!   8. Read the written echo/config.json: the substituted "hello-templates"
//!      must be in it, the "id" field (UUID v7) must be in it.
//!   9. Shutdown.
//!
//! Demo discipline (phase-7+ lesson): a positive receipt proof through
//! CaptureCell, no negative indicators.

use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Helper: send a mutation and await the outcome.
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

/// Scannt ein Templates-Verzeichnis via RescanTemplates.
async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instanziieren_aus_template_via_mutation() {
    let td = tempfile::TempDir::new().unwrap();

    // 1. Prepare .env with GREETING=hello-templates.
    std::fs::write(td.path().join(".env"), "GREETING=hello-templates\n").unwrap();

    // 2. Create the templates directory.
    //    The directory name only serves organization; the resolve key comes from
    //    template.json "name" (+ an optional "version").
    //    Versioned template: name="echo-cell", version="1.0.0".
    //    Mutation reference: "echo-cell@1.0.0" (versioned, R3-spec conformant).
    let tpl_dir = td.path().join("templates").join("echo-cell@1.0.0");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(
        tpl_dir.join("template.json"),
        r#"{"name":"echo-cell","version":"1.0.0","description":{"purpose":"echo smoke test"}}"#,
    )
    .unwrap();
    // Template config.json: cell.type = "echo" (the EchoCellFactory key), params empty.
    // override_params in the mutation diff supplies echo_to + greeting (with ${GREETING}).
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // 3. Boot the colony with the EchoCellFactory under "echo".
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn meclaw_colony::CellFactory>,
        )],
    );

    // Templates-Scan via RescanTemplates.
    rescan_templates(&h, td.path().join("templates")).await;

    // 4. /sink (CaptureCell) VOR Probe + VOR Mutation registrieren (Anti-Cascade).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // 5. Send the mutation: add_nodes with template="echo-cell@1.0.0" (a versioned
    //    reference, R3-spec conformant — the plan demo prescribes "echo-cell@1.0";
    //    "1.0.0" is the canonical SemVer form that template.json declares).
    //    override_params: echo_to="/sink" (mandatory for EchoCellFactory),
    //                     greeting="${GREETING}" (substitution proof).
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{
                    "name": "echo",
                    "template": "echo-cell@1.0.0",
                    "override_params": {
                        "echo_to": "/sink",
                        "greeting": "${GREETING}"
                    }
                }],
                // W2b (Ruling A1): the identity-fallback is gone — /echo's emission
                // to /sink needs a wired catch-all out-edge, else it no_routes.
                // /echo is created in this same diff (post_state), /sink is live
                // (spawned before the mutation). The doc-comment above already
                // describes this edge as part of the instantiation.
                "add_edges": [{"from": "echo", "to": "sink"}]
            }
        }),
    )
    .await;

    // 6. The outcome must be Committed.
    let committed_id = match outcome {
        MutationOutcome::Committed { id } => id,
        other => panic!("mutation must be Committed, got {other:?}"),
    };

    // 7. Send the probe → /echo.
    //    A UBF-conformant body (origin format, phase-6 lesson) so EchoMockCell
    //    does not produce an InvalidUbfBody DLQ entry.
    let probe = MessageBuilder::new(Path::new("/echo"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(meclaw_core::serde_json::json!({
            "messages": [{"origin": "user", "type": "text", "text": "smoke-probe"}]
        })))
        .build();
    h.inbox_tx
        .send(ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: probe,
        })
        .await
        .unwrap();

    // Check the receipt in /sink via CaptureCell (positive proof).
    let received = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("/sink must receive a message within 30s — proves /echo is live")
        .expect("CaptureCell channel must deliver a message");

    assert_eq!(
        received.target.as_str(),
        "/sink",
        "receipt target must be /sink, got {}",
        received.target.as_str()
    );

    // 8. Read the written echo/config.json and check the proof.
    let cfg_path = td.path().join("echo").join("config.json");
    assert!(cfg_path.exists(), "echo/config.json must have been written");
    let cfg_raw = std::fs::read_to_string(&cfg_path).unwrap();

    // ${GREETING} must be substituted.
    assert!(
        cfg_raw.contains("hello-templates"),
        "${{GREETING}} must be substituted as 'hello-templates': {cfg_raw}"
    );
    // cell.id (UUID v7) must be set.
    assert!(
        cfg_raw.contains("\"id\""),
        "cell.id (UUID v7) must be set in config.json: {cfg_raw}"
    );

    // mutation_log: the committed entry must be present in colony.db.
    let db_path = td.path().join("colony.db");
    h.shutdown().await;

    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM mutation_log WHERE id=?",
            [&committed_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status, "committed",
        "mutation_log.status must be 'committed' for id={committed_id}"
    );

    // Output for --nocapture verification.
    println!(
        "Demo Receipt: target={}, committed_id={}",
        received.target.as_str(),
        committed_id
    );
    println!("echo/config.json Inhalt: {cfg_raw}");
}
