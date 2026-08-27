//! Phase-7 Slice-2 edit-Demo.
//!
//! Topologie: /edit (EditCell) + /sink (terminale CaptureCell).
//!
//! Phase-11 T16 migration: the mutation uses the templates registry. Before the
//! mutation a `templates/edit/` directory is created and loaded via RescanTemplates.

use meclaw_cells::EditCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, MessageBuilder, Path, Uuid, serde_json::json, validate_ubf_body};
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;

fn make_tool_call_probe(args: &str, id: &str, reply_to: Path) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/edit"))
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

/// Phase-11 T16: creates a minimal `edit` template directory and loads it
/// via `RescanTemplates` in die Colony-Registry.
async fn setup_edit_template(td: &tempfile::TempDir, h: &meclaw_testing::ColonyHandle) {
    let templates_root = td.path().join("templates");
    let edit_tpl = templates_root.join("edit");
    std::fs::create_dir_all(&edit_tpl).unwrap();
    std::fs::write(edit_tpl.join("template.json"), r#"{"name":"edit"}"#).unwrap();
    std::fs::write(
        edit_tpl.join("config.json"),
        r#"{"cell":{"type":"edit"},"params":{"base_path":"/tmp","max_concurrency":1},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_7_edit_demo_find_replace_and_insert() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.txt"), b"foo bar\n").unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(EditCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("edit".to_string(), factory)],
    );
    setup_edit_template(&td, &h).await;

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
                    "name": "edit", "template": "edit",
                    "override_params": {
                        "base_path": td.path().to_str().unwrap(),
                        "max_concurrency": 4
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
    // W2 (A1): /edit reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/edit"), Path::new("/sink"))
        .await;

    // FIND_REPLACE
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_probe(
                r#"{"op":"find_replace","path":"a.txt","find":"foo","replace":"BAZ"}"#,
                "call-1",
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
    assert_eq!(m.headers.hop["operation"], "find_replace");
    assert_eq!(m.headers.hop["matches_changed"], 1);
    let written = std::fs::read_to_string(td.path().join("a.txt")).unwrap();
    assert_eq!(written, "BAZ bar\n");

    // INSERT_AT_LINE
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_probe(
                r#"{"op":"insert_at_line","path":"a.txt","line":1,"content":"HEADER\n"}"#,
                "call-2",
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
    assert_eq!(m.headers.hop["operation"], "insert_at_line");
    let written = std::fs::read_to_string(td.path().join("a.txt")).unwrap();
    assert_eq!(written, "HEADER\nBAZ bar\n");

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_7_edit_demo_pattern_not_found_emits_error() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.txt"), b"foo\n").unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(EditCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("edit".to_string(), factory)],
    );
    setup_edit_template(&td, &h).await;

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
                    "name": "edit", "template": "edit",
                    "override_params": {"base_path": td.path().to_str().unwrap()}
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
    // W2 (A1): /edit reply to /sink now needs a wired edge (identity gone).
    h.add_edge(Uuid::now_v7(), Path::new("/edit"), Path::new("/sink"))
        .await;

    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_probe(
                r#"{"op":"find_replace","path":"a.txt","find":"NOPE","replace":"x"}"#,
                "call-3",
                Path::new("/sink"),
            ),
        })
        .await
        .unwrap();

    let m = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.headers.hop["finish_reason"], "error");
    assert_eq!(m.headers.hop["error_code"], "pattern_not_found");

    h.shutdown().await;
}
