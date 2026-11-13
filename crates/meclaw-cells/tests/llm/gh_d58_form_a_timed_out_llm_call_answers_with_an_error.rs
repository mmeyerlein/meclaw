//! GH #1463 (D58 form, contract point 5) -- a call the `llm` cell gives up
//! on at its operation timeout answers with an error, once.
//!
//! WHY this file exists: a round of the curator ends with the answer of its
//! call, with an error answer, or with the next round of the session (D58
//! form, OR-58-87 point 3). For the error answer the curator's side is
//! pinned; the cell's side was read off the code up to the error path of
//! the call, and no test drove the chain from an expired operation timeout
//! (`external_timeout_ms`, rule A) to the message that leaves the cell. Here
//! the provider holds its answer back far longer than the cell waits: the
//! cell must emit exactly one message to the caller, with `finish_reason`
//! "error", `error_code` "timeout" and the source `wire`, and the turns it
//! was asked about -- the answer a waiting round closes on.
//!
//! The provider's delay is a hang mark (`meclaw_testing::deadline::HANG`),
//! never a verdict: the test asserts what was emitted, not when.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::deadline::HANG;
use mock_openai::{MockOpenAI, canned_chat_completion};
use tempfile::TempDir;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d58_form_a_timed_out_llm_call_answers_with_an_error() {
    // Two held-back answers: whatever the cell does after its timeout, no
    // canned answer reaches it before the hang mark.
    let held = || canned_chat_completion("too late", "stop").with_delay(HANG * 4);
    let mock = MockOpenAI::start(vec![held(), held()]).await;
    let td = TempDir::new().expect("tempdir");
    let params = LlmParams::parse(&json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key-d58",
        "base_url": format!("{}/v1", mock.base_url),
        "external_timeout_ms": 200,
        "x_title": "meclaw-lab d58-form",
        "http_referer": "https://meclaw.ai",
    }))
    .expect("params");
    let mut cell = LlmCell::new(params, reqwest::Client::builder().build().expect("http"));
    let raw = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db"))
        .expect("cell.db");
    let mut db = DbConn::wrap(raw, None);
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let asked = json!([{"origin": "user", "type": "text", "text": "Tell me about the bridge."}]);
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/curator"))
        .body(Body::Inline(json!({
            "system": {"instructions": {"text": "Be brief."}},
            "messages": asked
        })))
        .build();

    tokio::time::timeout(HANG, cell.handle(msg, &sink, &mut db))
        .await
        .expect("the cell gives up on the call at its own timeout, long before the hang mark");
    drop(sink);

    let mut out = Vec::new();
    while let Some(em) = rx.recv().await {
        out.push(em);
    }
    assert_eq!(
        out.len(),
        1,
        "one answer to the caller: {:?}",
        out.iter().map(|e| &e.content).collect::<Vec<_>>()
    );
    let em = &out[0];
    assert_eq!(
        em.target,
        Path::new("/curator"),
        "the answer goes to the caller"
    );
    let c = &em.content;
    assert_eq!(c["header"]["finish_reason"], "error", "{c}");
    assert_eq!(c["header"]["error_code"], "timeout", "{c}");
    assert_eq!(c["meta"]["error"]["source"], "wire", "{c}");
    assert_eq!(
        c["messages"], asked,
        "the answer names the turns it was asked about"
    );
    assert!(
        c["header"]["latency_ms"].is_u64(),
        "a failed call reports the time it took: {c}"
    );
    assert!(
        c["header"].get("tokens_prompt").is_none() && c["header"].get("cost").is_none(),
        "a call that failed reports no tokens and no cost: {c}"
    );
    let asked_provider = mock.recorded_requests().await;
    assert!(
        !asked_provider.is_empty(),
        "the cell asked the provider before it gave up"
    );
}
