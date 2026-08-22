//! Phase-7-Close Tool-Chain-Demo.
//!
//! Proves the content flow web_fetch → file → bash across orchestrated message
//! routings. The orchestrator loop (hop-N tool_result → hop-(N+1) tool_call)
//! lives IN THE TEST — a placeholder for the later llm cell (phase 8).
//! Deliberately no anticipation of later phases.
//!
//! Topology (hermetic, no egress):
//!   [mock_http] ──"hello chain"──> /web (web_fetch)
//!                                    │
//!                                    ▼ test orchestrator
//!                                  /file (write out.txt with fetched text)
//!                                    │
//!                                    ▼ test orchestrator
//!                                  /bash (cat <base_path>/out.txt)
//!                                    │
//!                                    ▼ tool_result.text == "hello chain"
//!                                  /sink (CaptureCell)
//!
//! Phase-11 T16 migration: the mutation uses the templates registry. Before the
//! mutation, templates for web_fetch, file and bash are created and loaded via
//! RescanTemplates.

use meclaw_cli::built_in_factories;
use meclaw_colony::{ColonyMsg, MutationOutcome};
use meclaw_core::{Body, MessageBuilder, Path, Uuid, serde_json::json, validate_ubf_body};
use meclaw_testing::mock_http::{MockResponse, start_mock_server};
use meclaw_testing::topologies::phase_3a::CaptureCell;

/// Phase-11 T16: creates minimal templates for the tool-chain cells and loads
/// them into the colony registry via `RescanTemplates`.
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
        // GH #85, the default-deny cut: a template-sourced `bash` cell that
        // declares no `params.sandbox` is instantiated restricted, and this
        // demo's `cat` reads a fresh temp directory no static template could
        // name. So it takes the documented escape hatch, exactly as an
        // operator migrating an existing template would. What this test proves
        // is the CONTENT FLOW web_fetch -> file -> bash; the sandbox boundary
        // is proven where it belongs, in
        // `crates/meclaw-cells/tests/sandbox_isolation.rs`.
        let params = match *cell_type {
            "bash" => r#"{"sandbox":{"trust":"trusted"},"external_timeout_ms":30000}"#,
            // GH #117: the chain starts at a mock server on 127.0.0.1, which
            // the shipped web_fetch default refuses. Documented opt-out, same
            // escape hatch an operator would take.
            "web_fetch" => r#"{"allow_private_networks":true,"external_timeout_ms":30000}"#,
            // GH #294: a template DECLARES the params a mutation may override —
            // `file` gets its `base_path` from the mutation, so the template
            // has to name it.
            "file" => r#"{"base_path":"/tmp"}"#,
            _ => "{}",
        };
        std::fs::write(
            tpl.join("config.json"),
            format!(
                r#"{{"cell":{{"type":"{cell_type}"}},"params":{params},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
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
    // base_path for the file cell is a sub-directory in the tempdir, so it does
    // not collide with the colony's own tree.
    let work_dir = td.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();

    let (addr, _server) = start_mock_server(MockResponse::ok(b"hello chain")).await;
    let mock_url = format!("http://{addr}/data");

    // All five factories — built_in_factories() is the wiring from T1.
    let factories: Vec<(String, std::sync::Arc<dyn meclaw_colony::CellFactory>)> =
        built_in_factories().into_iter().collect();
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(&td, factories);
    // Phase-11 T16: populate the templates registry with the three tool-chain cells.
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

    // === HOP 2: the test orchestrator builds a file-write tool_call ===
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
    // sanity: the file was actually written
    let on_disk = std::fs::read_to_string(work_dir.join("out.txt")).unwrap();
    assert_eq!(on_disk, "hello chain");

    // === HOP 3: the test orchestrator builds a bash tool_call ===
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
    // === CORE PROOF: bash output == the original web_fetch input ===
    assert_eq!(
        bash_output, "hello chain",
        "content flow web_fetch → file → bash broken"
    );

    h.shutdown().await;
}
