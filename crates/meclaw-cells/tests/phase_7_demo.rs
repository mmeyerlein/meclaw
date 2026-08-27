//! Phase-7 Slice-1 Demo (file-Cell).
//!
//! Topologie: /file (FileCell, base_path=TempDir) + /sink (terminale
//! CaptureCell). A probe to /file with reply_to=/sink (decision 7.1 — the tool
//! answers its caller via the envelope default). The CaptureCell is terminal (no
//! re-emit) → no cascade loop (phase-6.5 lesson).
//!
//! Phase-11 T16 migration: the mutation uses the templates registry. Before the
//! mutation a `templates/file/` directory is created and loaded via RescanTemplates.

use meclaw_cells::FileCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, MessageBuilder, Path, Uuid, serde_json::json, validate_ubf_body};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;

/// Phase-11 T16: Legt ein minimales `file`-Template-Verzeichnis in `root/templates/file/`
/// and loads it into the colony registry via `RescanTemplates`.
async fn setup_file_template(td: &tempfile::TempDir, h: &meclaw_testing::ColonyHandle) {
    let templates_root = td.path().join("templates");
    let file_tpl = templates_root.join("file");
    std::fs::create_dir_all(&file_tpl).unwrap();
    std::fs::write(file_tpl.join("template.json"), r#"{"name":"file"}"#).unwrap();
    std::fs::write(
        file_tpl.join("config.json"),
        r#"{"cell":{"type":"file"},"params":{"base_path":"/tmp","max_concurrency":1},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
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
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

fn make_tool_call_probe(args: &str, id: &str, reply_to: Path) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/file"))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_7_slice_1_demo_write_then_read_roundtrip_via_sink() {
    let td = tempfile::TempDir::new().unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(FileCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("file".to_string(), factory)],
    );
    setup_file_template(&td, &h).await;

    // /sink = terminal CaptureCell
    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // /file = via Mutation
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [{
                    "name": "file", "template": "file",
                    "override_params": {
                        "base_path": td.path().to_str().unwrap(),
                        "max_concurrency": 2
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
    // W2 (A1): /file reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/file"), Path::new("/sink"))
        .await;

    // WRITE
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_probe(
                r#"{"op":"write","path":"hello.txt","content":"world"}"#,
                "call-w",
                Path::new("/sink"),
            ),
        })
        .await
        .unwrap();

    let m = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let body = match &m.body {
        Body::Inline(v) => v.clone(),
        _ => panic!("non-inline body in sink"),
    };
    // Colony's split_content_header extracts `header` from the body and merges
    // it into Message.headers. The body arriving at the sink contains only
    // `messages[]`; operation/bytes live in m.headers.
    validate_ubf_body(&body).expect("write emit must be valid UBF");
    assert_eq!(body["messages"][0]["origin"], "tool");
    assert_eq!(body["messages"][0]["type"], "tool_result");
    assert_eq!(m.headers.hop["operation"], "write");
    assert_eq!(m.headers.hop["bytes"], 5);

    // READ
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_probe(
                r#"{"op":"read","path":"hello.txt"}"#,
                "call-r",
                Path::new("/sink"),
            ),
        })
        .await
        .unwrap();

    let m = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let body = match &m.body {
        Body::Inline(v) => v.clone(),
        _ => panic!("non-inline body in sink"),
    };
    validate_ubf_body(&body).expect("read emit must be valid UBF");
    assert_eq!(
        body["messages"][0]["text"], "world",
        "roundtrip text matches"
    );
    assert_eq!(m.headers.hop["operation"], "read");
    assert_eq!(m.headers.hop["bytes"], 5);

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_7_slice_1_demo_fanout_n_reads_all_arrive() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.txt"), b"AAA").unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(FileCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("file".to_string(), factory)],
    );
    setup_file_template(&td, &h).await;

    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(64);
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
                    "name": "file", "template": "file",
                    "override_params": {
                        "base_path": td.path().to_str().unwrap(),
                        "max_concurrency": 2
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
    // W2 (A1): /file reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/file"), Path::new("/sink"))
        .await;

    const N: usize = 10;
    for i in 0..N {
        h.inbox_tx
            .send(meclaw_colony::ColonyMsg::Route {
                sender_path: Path::new("/"),
                msg: make_tool_call_probe(
                    r#"{"op":"read","path":"a.txt"}"#,
                    &format!("call-{i}"),
                    Path::new("/sink"),
                ),
            })
            .await
            .unwrap();
    }

    let mut received = 0;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while received < N {
        match tokio::time::timeout_at(deadline, sink_rx.recv()).await {
            Ok(Some(m)) => {
                let body = match m.body {
                    Body::Inline(v) => v,
                    _ => panic!("non-inline"),
                };
                validate_ubf_body(&body).unwrap();
                assert_eq!(body["messages"][0]["text"], "AAA");
                received += 1;
            }
            Ok(None) => panic!("sink channel closed early; received={received}"),
            Err(_) => panic!("timeout; received={received}/{N}"),
        }
    }
    assert_eq!(
        received, N,
        "all N reads must arrive at /sink (no loss on the real FileCell path)"
    );

    h.shutdown().await;
}
