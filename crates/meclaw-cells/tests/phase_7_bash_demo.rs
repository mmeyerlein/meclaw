//! Phase-7 Slice-2 bash-Demo.
//!
//! Topologie: /bash (BashCell) + /sink (terminale CaptureCell). Probe an
//! /bash mit reply_to=/sink (Decision 7.1). CaptureCell ist terminal (kein
//! Re-Emit) → kein Cascade-Loop (Phase-6.5-Lesson).
//!
//! Phase-11 T16 Migration: Mutation nutzt Templates-Registry. Vor der Mutation
//! wird ein `templates/bash/`-Verzeichnis angelegt und via RescanTemplates geladen.

use meclaw_cells::BashCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, MessageBuilder, Path, Uuid, serde_json::json, validate_ubf_body};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;

fn make_tool_call_probe(args: &str, id: &str, reply_to: Path) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/bash"))
        .reply_to(reply_to)
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({
            "messages": [{
                "origin": "assistant", "type": "tool_call",
                "text": args, "id": id
            }]
        })))
        .build()
}

/// Phase-11 T16: Legt ein minimales `bash`-Template-Verzeichnis in `root/templates/bash/`
/// an und lädt es via `RescanTemplates` in die Colony-Registry.
async fn setup_bash_template(td: &tempfile::TempDir, h: &meclaw_testing::ColonyHandle) {
    let templates_root = td.path().join("templates");
    let bash_tpl = templates_root.join("bash");
    std::fs::create_dir_all(&bash_tpl).unwrap();
    std::fs::write(bash_tpl.join("template.json"), r#"{"name":"bash"}"#).unwrap();
    std::fs::write(
        bash_tpl.join("config.json"),
        r#"{"cell":{"type":"bash"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
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
async fn phase_7_bash_demo_echo_via_sink() {
    let td = tempfile::TempDir::new().unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(BashCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("bash".to_string(), factory)],
    );
    setup_bash_template(&td, &h).await;

    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [{
                    "name": "bash", "template": "bash",
                    "override_params": {"max_concurrency": 2, "external_timeout_ms": 5000}
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
        meclaw_colony::MutationOutcome::Committed { .. }
    ));
    // W2 (A1): /bash reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/bash"), Path::new("/sink"))
        .await;

    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_probe(
                r#"{"command": "echo hello"}"#,
                "call-echo",
                Path::new("/sink"),
            ),
        })
        .await
        .unwrap();

    let m = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let body = match m.body {
        Body::Inline(v) => v,
        _ => panic!("non-inline"),
    };
    validate_ubf_body(&body).expect("valid UBF");
    assert_eq!(body["messages"][0]["text"], "hello\n");
    assert_eq!(m.headers.hop["operation"], "bash");
    assert_eq!(m.headers.hop["exit_code"], 0);
    assert_eq!(m.headers.hop["had_stderr"], false);

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_7_bash_demo_timeout_emits_err_timeout() {
    let td = tempfile::TempDir::new().unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(BashCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("bash".to_string(), factory)],
    );
    setup_bash_template(&td, &h).await;

    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [{
                    "name": "bash", "template": "bash",
                    "override_params": {"max_concurrency": 2, "external_timeout_ms": 100}
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
        meclaw_colony::MutationOutcome::Committed { .. }
    ));
    // W2 (A1): /bash reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/bash"), Path::new("/sink"))
        .await;

    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_probe(
                r#"{"command": "sleep 30"}"#,
                "call-sleep",
                Path::new("/sink"),
            ),
        })
        .await
        .unwrap();

    let m = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let body = match m.body {
        Body::Inline(v) => v,
        _ => panic!("non-inline"),
    };
    validate_ubf_body(&body).expect("valid UBF");
    assert_eq!(m.headers.hop["finish_reason"], "error");
    assert_eq!(m.headers.hop["error_code"], "timeout");

    h.shutdown().await;
}
