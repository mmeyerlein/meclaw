//! Phase-16 W2d (Substrat, ruling 2026-06-12): the LLM cell no longer
//! hard-codes `/colony/dead_letters` as its reply-target fallback. With no
//! `reply_to` AND no matching out-edge, its assistant-turn emission travels from
//! its own path (`msg.target`) and dead-letters as `no_route` — NOT a write to
//! the READ endpoint, NOT a source loop. Companion to W2a's no_route pins, but
//! for the empirically-isolated `llm` source (W2c STOPP root).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{json, to_string_pretty};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::time::Duration;

use mock_openai::{MockOpenAI, canned_chat_completion};

fn llm_config(base_url: &str) -> String {
    to_string_pretty(&json!({
        "cell": {"type": "llm"},
        "params": {"provider": "openai", "model": "gpt-4o", "api_key": "test-key", "base_url": base_url},
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    }))
    .unwrap()
}

/// A user-turn probe to `/llm` that sets intentionally NO `reply_to`.
fn probe_no_reply_to() -> Message {
    MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": "sag hallo"}]
        })))
        .build()
}

/// (b) `/llm` emits without `reply_to` and matches NO out-edge ⇒ `no_route`
/// dead-letter (self-locating, sender `/llm`), bounded — no `/colony/dead_letters`
/// write, no loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_emission_without_reply_to_and_no_edge_dead_letters_as_no_route() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("hallo", "stop")]).await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main/llm")).unwrap();
    // Root hive with NO edges — /llm's assistant-turn emission is unrouted.
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.path().join("main/llm/config.json"),
        llm_config(&base_url),
    )
    .unwrap();

    let llm_f: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("llm".to_string(), llm_f.clone())]);
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), llm_f);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap must succeed");

    h.send(probe_no_reply_to()).await;

    // Bounded wait for the terminating no_route DLQ entry.
    let mut snapshot = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        snapshot = h.drain_dead_letters().await;
        if snapshot.iter().any(|d| d.reason.as_code() == "no_route") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let no_route: Vec<_> = snapshot
        .iter()
        .filter(|d| d.reason.as_code() == "no_route")
        .collect();
    assert_eq!(
        no_route.len(),
        1,
        "exactly one no_route DLQ for the unrouted /llm emission; got {:?}",
        snapshot
            .iter()
            .map(|d| (
                d.sender_path.as_str(),
                d.resolved_target.as_str(),
                d.reason.as_code()
            ))
            .collect::<Vec<_>>()
    );
    let dl = no_route[0];
    assert_eq!(dl.sender_path.as_str(), "/llm", "dying edge starts at /llm");
    // The fallback target is the cell's own path, NOT the /colony/dead_letters
    // READ endpoint (the W2d anchor fix).
    assert_eq!(
        dl.resolved_target.as_str(),
        "/llm",
        "fallback target is the cell's own path, not /colony/dead_letters"
    );

    // Exactly one llm inference — no self-sustaining loop.
    let snaps = mock.recorded_requests().await;
    assert_eq!(
        snaps.len(),
        1,
        "single llm call, no loop; got {}",
        snaps.len()
    );

    h.shutdown().await;
}
