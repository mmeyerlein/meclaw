//! Phase-7 Slice-3 web_fetch-Demo.
//!
//! Topologie: /web (WebFetchCell) + /sink (terminale CaptureCell). HTTP-Endpoint
//! via Mock-HTTP-Server (127.0.0.1:0, kein Egress).
//!
//! Phase-11 T16 Migration: Mutation nutzt Templates-Registry. Vor der Mutation
//! wird ein `templates/web_fetch/`-Verzeichnis angelegt und via RescanTemplates geladen.

use meclaw_cells::WebFetchCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, MessageBuilder, Path, Uuid, serde_json::json, validate_ubf_body};
use meclaw_testing::mock_http::{MockResponse, start_mock_server};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;

fn make_tool_call_probe(args: &str, id: &str, reply_to: Path) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/web"))
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

/// Phase-11 T16: Legt ein minimales `web_fetch`-Template-Verzeichnis an und lädt es
/// via `RescanTemplates` in die Colony-Registry.
async fn setup_web_fetch_template(td: &tempfile::TempDir, h: &meclaw_testing::ColonyHandle) {
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join("web_fetch");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"web_fetch"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"web_fetch"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
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
async fn phase_7_web_fetch_demo_200_via_sink() {
    let td = tempfile::TempDir::new().unwrap();
    let (addr, _server) = start_mock_server(MockResponse::ok(b"hello demo")).await;
    let factory: Arc<dyn CellFactory> = Arc::new(WebFetchCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("web_fetch".to_string(), factory)],
    );
    setup_web_fetch_template(&td, &h).await;

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
                    "name": "web", "template": "web_fetch",
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
    // W2 (A1): /web reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/web"), Path::new("/sink"))
        .await;

    let url = format!("http://{addr}/foo");
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_probe(
                &format!(r#"{{"url":"{url}"}}"#),
                "call-200",
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
    assert_eq!(body["messages"][0]["text"], "hello demo");
    assert_eq!(m.headers.hop["operation"], "web_fetch");
    assert_eq!(m.headers.hop["http_status"], 200);

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_7_web_fetch_demo_404_is_normal_not_error() {
    let td = tempfile::TempDir::new().unwrap();
    let (addr, _server) = start_mock_server(MockResponse::not_found()).await;
    let factory: Arc<dyn CellFactory> = Arc::new(WebFetchCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("web_fetch".to_string(), factory)],
    );
    setup_web_fetch_template(&td, &h).await;

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
                    "name": "web", "template": "web_fetch",
                    "override_params": {"external_timeout_ms": 5000}
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
    // W2 (A1): /web reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/web"), Path::new("/sink"))
        .await;

    let url = format!("http://{addr}/nope");
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_probe(
                &format!(r#"{{"url":"{url}"}}"#),
                "call-404",
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
    assert_eq!(m.headers.hop["http_status"], 404);
    // NORMAL — kein finish_reason=error
    assert!(
        m.headers.hop.get("finish_reason").is_none() || m.headers.hop["finish_reason"] != "error"
    );

    h.shutdown().await;
}
