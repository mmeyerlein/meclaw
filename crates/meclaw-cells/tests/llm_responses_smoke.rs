//! P10 step G2 — the ONE paid smoke against the real Responses endpoint.
//!
//! # What this proves, and what it does not
//!
//! **Proves:** the Responses *dialect* is right — the request body is accepted
//! by a real OpenAI endpoint, the SSE event sequence parses, and the
//! `response.model` receipt reaches the emitted header.
//!
//! **Does NOT prove:** the `chatgpt.com` subscription backend's quirks — the
//! header set (`ChatGPT-Account-ID`, `originator`, `session-id`), the
//! `store:false` requirement, `reasoning.encrypted_content`, or Cloudflare's
//! behaviour towards a non-browser client. Those stay pinned against
//! `openai/codex` @`266c6920` and are verified for real only once an operator
//! has completed an interactive login.
//!
//! # Running it
//!
//! Skipped unless BOTH are set — no key, no model, no cost, no default model
//! baked into code (standing rule: models come from `${VAR}` only):
//!
//! ```text
//! MECLAW_SMOKE_OPENAI_KEY=sk-…  MECLAW_SMOKE_MODEL=…  \
//!   cargo test -p meclaw-cells --test llm_responses_smoke -- --nocapture
//! ```
//!
//! One call, `max_tokens` 16, so the cost is a fraction of a cent. Token usage
//! is printed so the run can be costed exactly.

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use tempfile::TempDir;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn responses_smoke_against_real_api() {
    let (Ok(key), Ok(model)) = (
        std::env::var("MECLAW_SMOKE_OPENAI_KEY"),
        std::env::var("MECLAW_SMOKE_MODEL"),
    ) else {
        eprintln!(
            "SKIPPED: set MECLAW_SMOKE_OPENAI_KEY and MECLAW_SMOKE_MODEL to run the paid smoke"
        );
        return;
    };

    let td = TempDir::new().unwrap();
    let raw = json!({
        "provider": "openai",
        "model": model,
        "api_key": key,
        "wire_dialect": "responses",
        "max_tokens": 16,
        "temperature": 0.0,
    });
    let params = LlmParams::parse(&raw).expect("smoke params");
    let mut cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut db = DbConn::wrap(conn, None);

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

    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "system": {"identity": {"text": "Answer with exactly one word."}},
            "messages": [{"origin": "user", "type": "text",
                          "text": "Which planet do humans live on?"}]
        })))
        .build();
    cell.handle(msg, &sink, &mut db).await;

    let out = rx.recv().await.expect("cell emitted nothing").content;
    eprintln!(
        "SMOKE RECEIPT: {}",
        meclaw_core::serde_json::to_string_pretty(&out).unwrap()
    );

    assert_eq!(
        out["header"]["finish_reason"], "stop",
        "smoke failed: {out}"
    );
    // response.model receipt rule: identity comes from the response, never from
    // the config claim.
    let served = out["header"]["model"].as_str().expect("model header");
    assert!(
        served.starts_with(&model) || model.starts_with(served),
        "served model {served:?} does not match requested {model:?}"
    );
    let text = out["messages"][0]["text"].as_str().expect("assistant text");
    assert!(!text.trim().is_empty(), "empty answer");
    assert!(
        out["meta"]["response_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "no response id"
    );
    // Cost receipt: printed so the run can be priced exactly.
    eprintln!(
        "SMOKE COST BASIS: prompt_tokens={} completion_tokens={} model={served}",
        out["header"]["tokens_prompt"], out["header"]["tokens_completion"]
    );
}
