//! W4b: params-update messages to the `llm` cell (config.md § Access l.20).
//!
//! A params-update is a normal message carrying a top-level `params` body-slot
//! (1:1 the config.json `params` block). The cell merges it into its live params
//! (last-write-wins) and persists the overlay in its own cell.db; config.json is
//! never touched. These end-to-end tests pin the wire-observable effect:
//! (a) combined — params + messages in ONE message → THIS call already uses a
//! new attribution header, while a model-package key on a turn is not applied
//! at all (GH #853); (a-separate) a params-only message emits nothing, the
//! NEXT inference message uses the updated model.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use tempfile::TempDir;
use tokio::sync::mpsc;

fn mk_cell_and_db(base_url: &str, td: &TempDir) -> (LlmCell, DbConn) {
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key-W4b",
        "base_url": base_url,
    });
    let params = LlmParams::parse(&raw).unwrap();
    let http = reqwest::Client::builder().build().unwrap();
    let cell = LlmCell::new(params, http);
    let raw_conn =
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (cell, DbConn::wrap(raw_conn, None))
}

fn mk_sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

// ───── (a) combined: params + messages in one message → a non-package key applies this call,
// a package key does not apply at all (GH #853: a conversation cannot change its model) ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn combined_params_and_messages_apply_the_header_this_call_but_never_the_model() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("hi back", "stop"),
        canned_chat_completion("again", "stop"),
    ])
    .await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = mk_cell_and_db(&format!("{}/v1", mock.base_url), &td);
    let (sink, _rx) = mk_sink();

    // A non-package key rides on a turn as it always did.
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "params": {"http_referer": "https://example.com"},
            "messages": [{"origin": "user", "type": "text", "text": "Hi"}]
        })))
        .build();
    cell.handle(msg, &sink, &mut db).await;

    // A package key on a turn: the whole slot is not applied (GH #853).
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "params": {"model": "gpt-4o-mini", "x_title": "Never"},
            "messages": [{"origin": "user", "type": "text", "text": "Hi"}]
        })))
        .build();
    cell.handle(msg, &sink, &mut db).await;

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 2, "both turns reached the provider");
    // params applied BEFORE inference → this very call carries the header …
    assert_eq!(
        snaps[0].headers.get("http-referer").map(|s| s.as_str()),
        Some("https://example.com")
    );
    // … while the model a turn names never takes effect, nor anything beside it.
    assert_eq!(snaps[1].model(), Some("gpt-4o"));
    assert!(!snaps[1].headers.contains_key("x-title"));
    let rows: Vec<String> = db
        .call(|conn| {
            let mut stmt = conn.prepare("SELECT key FROM params ORDER BY key").unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        })
        .await;
    assert_eq!(rows, vec!["http_referer".to_string()]);
}

// ───── (a-separate) params-only message emits nothing; the next inference uses the new model ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn params_only_then_inference_uses_updated_model() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("hi back", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = mk_cell_and_db(&format!("{}/v1", mock.base_url), &td);
    let (sink, mut rx) = mk_sink();

    // 1) params-only — no wire call, no emission.
    let upd = MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({"params": {"model": "gpt-4o-mini"}})))
        .build();
    cell.handle(upd, &sink, &mut db).await;
    assert!(rx.try_recv().is_err(), "params-only must not emit");
    assert!(
        mock.recorded_requests().await.is_empty(),
        "params-only must not hit the wire"
    );

    // 2) inference — uses the updated model.
    let infer = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": "Hi"}]
        })))
        .build();
    cell.handle(infer, &sink, &mut db).await;

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].model(), Some("gpt-4o-mini"));
}
