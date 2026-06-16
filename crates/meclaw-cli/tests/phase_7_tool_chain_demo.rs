//! Phase-7-Close Tool-Chain-Demo.
//!
//! Beweist Content-Fluss web_fetch → file → bash über orchestrierte
//! Message-Routings. Der Orchestrator-Loop (Hop-N tool_result → Hop-(N+1)
//! tool_call) lebt IM TEST — Platzhalter für die spätere llm-Cell
//! (Phase 8). Anti-Vorgriff bewusst.
//!
//! Topologie (hermetisch, kein Egress):
//!   [mock_http] ──"hello chain"──> /web (web_fetch)
//!                                    │
//!                                    ▼ test orchestrator
//!                                  /file (write out.txt mit fetched text)
//!                                    │
//!                                    ▼ test orchestrator
//!                                  /bash (cat <base_path>/out.txt)
//!                                    │
//!                                    ▼ tool_result.text == "hello chain"
//!                                  /sink (CaptureCell)
//!
//! Phase-11 T16 Migration: Mutation nutzt Templates-Registry. Vor der Mutation
//! werden Templates für web_fetch, file und bash angelegt und via RescanTemplates geladen.

use meclaw_cli::built_in_factories;
use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::{Body, MessageBuilder, Path, Uuid, serde_json::json, validate_ubf_body};
use meclaw_testing::mock_http::{MockResponse, start_mock_server};
use meclaw_testing::topologies::phase_3a::CaptureCell;

/// Phase-11 T16: Legt minimale Templates für die Tool-Chain-Cells an und lädt sie
/// via `RescanTemplates` in die Colony-Registry.
async fn setup_tool_chain_templates(td: &tempfile::TempDir, h: &meclaw_testing::ColonyHandle) {
    let templates_root = td.path().join("templates");
    for (name, cell_type) in &[
        ("web_fetch", "web_fetch"),
        ("file", "file"),
        ("bash", "bash"),
    ] {
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
    }
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

fn make_tool_call_msg(target: &str, args: &str, id: &str, reply_to: Path) -> meclaw_core::Message {
    MessageBuilder::new(Path::new(target))
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
async fn phase_7_tool_chain_web_fetch_to_file_to_bash() {
    let td = tempfile::TempDir::new().unwrap();
    // base_path für die file-Cell ist ein Unterverzeichnis im Tempdir,
    // damit es nicht mit Colony's eigenem Tree kollidiert.
    let work_dir = td.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();

    let (addr, _server) = start_mock_server(MockResponse::ok(b"hello chain")).await;
    let mock_url = format!("http://{addr}/data");

    // Alle fünf Factories — built_in_factories() ist das Wiring aus T1.
    let factories: Vec<(String, std::sync::Arc<dyn meclaw_colony::CellFactory>)> =
        built_in_factories().into_iter().collect();
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(&td, factories);
    // Phase-11 T16: Templates-Registry mit den drei Tool-Chain-Cells befüllen.
    setup_tool_chain_templates(&td, &h).await;

    // /sink = terminale CaptureCell
    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // Mutation: /web, /file, /bash via add_nodes
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [
                    {"name": "web", "template": "web_fetch",
                     "override_params": {"external_timeout_ms": 5000}},
                    {"name": "file", "template": "file",
                     "override_params": {"base_path": work_dir.to_str().unwrap()}},
                    {"name": "bash", "template": "bash",
                     "override_params": {"external_timeout_ms": 5000}},
                ]}
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
        MutationOutcome::Committed { .. }
    ));

    // W2 (A1): each hop replies to /sink — that reply now needs a wired edge
    // (the implicit identity-fallback is gone).
    h.add_edge(Uuid::now_v7(), Path::new("/web"), Path::new("/sink"))
        .await;
    h.add_edge(Uuid::now_v7(), Path::new("/file"), Path::new("/sink"))
        .await;
    h.add_edge(Uuid::now_v7(), Path::new("/bash"), Path::new("/sink"))
        .await;

    // === HOP 1: web_fetch → /sink ===
    h.inbox_tx
        .send(ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_msg(
                "/web",
                &format!(r#"{{"url":"{mock_url}"}}"#),
                "call-1",
                Path::new("/sink"),
            ),
        })
        .await
        .unwrap();
    let m1 = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let body1 = match m1.body {
        Body::Inline(v) => v,
        _ => panic!("hop 1 body not inline"),
    };
    validate_ubf_body(&body1).expect("hop 1 valid UBF");
    assert_eq!(m1.headers.hop["operation"], "web_fetch");
    assert_eq!(m1.headers.hop["http_status"], 200);
    let fetched_text = body1["messages"][0]["text"].as_str().unwrap().to_string();
    assert_eq!(fetched_text, "hello chain", "hop 1 fetched text mismatch");

    // === HOP 2: test orchestrator baut file-write tool_call ===
    let write_args = json!({
        "op": "write",
        "path": "out.txt",
        "content": fetched_text.clone(),
    });
    h.inbox_tx
        .send(ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_msg(
                "/file",
                &write_args.to_string(),
                "call-2",
                Path::new("/sink"),
            ),
        })
        .await
        .unwrap();
    let m2 = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let body2 = match m2.body {
        Body::Inline(v) => v,
        _ => panic!("hop 2 body not inline"),
    };
    validate_ubf_body(&body2).expect("hop 2 valid UBF");
    assert_eq!(m2.headers.hop["operation"], "write");
    // sanity: file ist tatsächlich geschrieben worden
    let on_disk = std::fs::read_to_string(work_dir.join("out.txt")).unwrap();
    assert_eq!(on_disk, "hello chain");

    // === HOP 3: test orchestrator baut bash tool_call ===
    let cat_cmd = format!("cat {}/out.txt", work_dir.to_str().unwrap());
    let bash_args = json!({"command": cat_cmd});
    h.inbox_tx
        .send(ColonyMsg::Route {
            sender_path: Path::new("/"),
            msg: make_tool_call_msg(
                "/bash",
                &bash_args.to_string(),
                "call-3",
                Path::new("/sink"),
            ),
        })
        .await
        .unwrap();
    let m3 = tokio::time::timeout(std::time::Duration::from_secs(30), sink_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let body3 = match m3.body {
        Body::Inline(v) => v,
        _ => panic!("hop 3 body not inline"),
    };
    validate_ubf_body(&body3).expect("hop 3 valid UBF");
    assert_eq!(m3.headers.hop["operation"], "bash");
    assert_eq!(m3.headers.hop["exit_code"], 0);
    assert_eq!(m3.headers.hop["had_stderr"], false);
    let bash_output = body3["messages"][0]["text"].as_str().unwrap();
    // === KERNBEWEIS: bash output == ursprünglicher web_fetch input ===
    assert_eq!(
        bash_output, "hello chain",
        "content flow web_fetch → file → bash broken"
    );

    h.shutdown().await;
}
