//! Phase-11-Demo (PROGRESS.md Z.35): "Instanziieren aus Template via Mutation".
//!
//! Beweis-Kette:
//!   1. Templates-Verzeichnis `templates/echo-cell@1.0/` mit `template.json` + `config.json`.
//!   2. `.env` mit `GREETING=hello-templates`.
//!   3. Colony hochfahren + Templates-Scan via RescanTemplates.
//!   4. /sink (CaptureCell) VOR Probe registrieren (Anti-Cascade, Phase-6.5-Lesson).
//!   5. Mutation: add_nodes(name="echo", template="echo-cell@1.0",
//!      override_params={echo_to="/sink", greeting="${GREETING}"})
//!      + add_edges(/echo → /sink).
//!   6. Outcome muss MutationOutcome::Committed sein.
//!   7. Probe → /echo, Receipt in /sink via CaptureCell prüfen.
//!   8. Geschriebene echo/config.json lesen: substituiertes "hello-templates"
//!      muss drin sein, "id"-Feld (UUID v7) muss drin sein.
//!   9. Shutdown.
//!
//! Demo-Disziplin (Phase-7+ Lesson): positiver Receipt-Beweis durch CaptureCell,
//! keine Negativ-Indizien.

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

    // 1. .env mit GREETING=hello-templates vorbereiten.
    std::fs::write(td.path().join(".env"), "GREETING=hello-templates\n").unwrap();

    // 2. Templates-Verzeichnis anlegen.
    //    Verzeichnisname dient nur der Organisation; der resolve-Schlüssel kommt
    //    aus template.json "name" (+ optionale "version").
    //    Versioniertes Template: name="echo-cell", version="1.0.0".
    //    Mutation-Referenz: "echo-cell@1.0.0" (versioniert, R3-Spec-konform).
    let tpl_dir = td.path().join("templates").join("echo-cell@1.0.0");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(
        tpl_dir.join("template.json"),
        r#"{"name":"echo-cell","version":"1.0.0","description":{"purpose":"echo smoke test"}}"#,
    )
    .unwrap();
    // Template config.json: cell.type = "echo" (EchoCellFactory-Schlüssel), params leer.
    // override_params im Mutation-Diff liefert echo_to + greeting (mit ${GREETING}).
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // 3. Colony hochfahren mit EchoCellFactory unter "echo".
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

    // 5. Mutation senden: add_nodes mit template="echo-cell@1.0.0" (versionierte Referenz,
    //    R3-Spec-konform — Plan-Demo schreibt "echo-cell@1.0" vor; "1.0.0" ist die kanonische
    //    SemVer-Form, die template.json deklariert).
    //    override_params: echo_to="/sink" (Pflicht für EchoCellFactory),
    //                     greeting="${GREETING}" (Substitutions-Beweis).
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

    // 6. Outcome muss Committed sein.
    let committed_id = match outcome {
        MutationOutcome::Committed { id } => id,
        other => panic!("Mutation muss Committed sein, got {other:?}"),
    };

    // 7. Probe → /echo schicken.
    //    UBF-konformer Body (origin-Format, Phase-6-Lesson) damit EchoMockCell
    //    keine InvalidUbfBody-DLQ erzeugt.
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

    // Receipt in /sink via CaptureCell prüfen (positiver Beweis).
    let received = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("/sink muss eine Nachricht innerhalb von 30s empfangen — beweist /echo ist live")
        .expect("CaptureCell-Channel muss eine Nachricht liefern");

    assert_eq!(
        received.target.as_str(),
        "/sink",
        "Receipt-Target muss /sink sein, got {}",
        received.target.as_str()
    );

    // 8. Geschriebene echo/config.json lesen und Beweis prüfen.
    let cfg_path = td.path().join("echo").join("config.json");
    assert!(
        cfg_path.exists(),
        "echo/config.json muss geschrieben worden sein"
    );
    let cfg_raw = std::fs::read_to_string(&cfg_path).unwrap();

    // ${GREETING} muss substituiert sein.
    assert!(
        cfg_raw.contains("hello-templates"),
        "${{GREETING}} muss als 'hello-templates' substituiert sein: {cfg_raw}"
    );
    // cell.id (UUID v7) muss gesetzt sein.
    assert!(
        cfg_raw.contains("\"id\""),
        "cell.id (UUID v7) muss in config.json gesetzt sein: {cfg_raw}"
    );

    // mutation_log: der Committed-Eintrag muss in colony.db vorhanden sein.
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
        "mutation_log.status muss 'committed' sein für id={committed_id}"
    );

    // Ausgabe für --nocapture Verifikation.
    println!(
        "Demo Receipt: target={}, committed_id={}",
        received.target.as_str(),
        committed_id
    );
    println!("echo/config.json Inhalt: {cfg_raw}");
}
