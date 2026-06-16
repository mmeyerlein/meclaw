//! Phase-7 Slice-3 web_search-Demo.
//!
//! Phase-11 T16 Migration: Mutation nutzt Templates-Registry. Vor der Mutation
//! wird ein `templates/web_search/`-Verzeichnis angelegt und via RescanTemplates geladen.

use meclaw_cells::WebSearchCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, MessageBuilder, Path, Uuid, serde_json::json, validate_ubf_body};
use meclaw_testing::mock_http::{MockResponse, start_mock_server};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;

fn make_tool_call_probe(args: &str, id: &str, reply_to: Path) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/search"))
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

/// Phase-11 T16: Legt ein minimales `web_search`-Template-Verzeichnis an und lädt es
/// via `RescanTemplates` in die Colony-Registry.
async fn setup_web_search_template(td: &tempfile::TempDir, h: &meclaw_testing::ColonyHandle) {
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join("web_search");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"web_search"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"web_search"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
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
async fn phase_7_web_search_demo_results_via_sink() {
    let td = tempfile::TempDir::new().unwrap();
    let json_body = br#"{"results":[{"title":"A","url":"u1","snippet":"s1"},{"title":"B","url":"u2","snippet":"s2"},{"title":"C","url":"u3","snippet":"s3"}]}"#;
    let (addr, _server) = start_mock_server(MockResponse::ok_json(json_body)).await;
    let factory: Arc<dyn CellFactory> = Arc::new(WebSearchCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("web_search".to_string(), factory)],
    );
    setup_web_search_template(&td, &h).await;

    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let endpoint = format!("http://{addr}/search");
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [{
                    "name": "search", "template": "web_search",
                    "override_params": {
                        "endpoint": endpoint,
                        "max_concurrency": 2,
                        "external_timeout_ms": 5000
                    }
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
    // W2 (A1): /search reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/search"), Path::new("/sink"))
        .await;

    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_probe(
                r#"{"query":"rust async"}"#,
                "call-search",
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
    assert_eq!(m.headers.hop["operation"], "web_search");
    assert_eq!(m.headers.hop["result_count"], 3);
    let text = body["messages"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("\"title\":\"A\""),
        "body must contain results"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_7_web_search_demo_non_conforming_is_graceful() {
    let td = tempfile::TempDir::new().unwrap();
    let (addr, _server) = start_mock_server(MockResponse::ok_json(br#"{"hits":[1,2]}"#)).await;
    let factory: Arc<dyn CellFactory> = Arc::new(WebSearchCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("web_search".to_string(), factory)],
    );
    setup_web_search_template(&td, &h).await;

    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let endpoint = format!("http://{addr}/search");
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [{
                    "name": "search", "template": "web_search",
                    "override_params": {"endpoint": endpoint, "external_timeout_ms": 5000}
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
    // W2 (A1): /search reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/search"), Path::new("/sink"))
        .await;

    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_probe(r#"{"query":"x"}"#, "call-nc", Path::new("/sink")),
        })
        .await
        .unwrap();

    let m = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.headers.hop["result_count"], 0);
    // GRACEFUL — KEIN finish_reason=error
    assert!(
        m.headers.hop.get("finish_reason").is_none() || m.headers.hop["finish_reason"] != "error"
    );

    h.shutdown().await;
}
