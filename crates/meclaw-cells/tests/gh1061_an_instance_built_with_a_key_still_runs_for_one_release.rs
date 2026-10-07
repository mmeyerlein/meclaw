//! GH #1061 (#801), lock T5 (A8) — an instance built with a key still runs for
//! one release.
//!
//! 0.62 ships no `${…_KEY}` in any template, but instances grown before it
//! carry one in their own `config.json`: an `llm` cell with
//! `api_key: "${OPENROUTER_API_KEY}"`, no grant, and the value in
//! `{root}/.env`. A8 promises those instances run unchanged until the literal
//! road is removed in 0.63.0 — the boot substitutes the token from `.env`, and
//! the cell presents it.
//!
//! Overlap: `gh1058_a_literal_key_wins_while_no_grant_is_set` pins the CELL
//! half (a literal `api_key` without a grant is presented, nothing is asked).
//! This lock adds the leg that file does not reach — the 0.61 instance as it
//! lies on disk, booted through `bootstrap_from_filesystem`, with the token
//! resolved from `{root}/.env` — and keeps the harness thin.
//!
//! The stub value must reach no log line and no `message_log` row.

use meclaw_cells::LlmCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const SECRET: &str = "stub-secret-5";

/// Every tracing event of this test process, formatted with its field values.
/// Global, because the cell runs on worker threads; nextest gives every test
/// its own process.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

struct Fields(String);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut f = Fields(event.metadata().target().to_string());
        event.record(&mut f);
        self.0.lock().unwrap().push(f.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

fn capture() -> Capture {
    static C: OnceLock<Capture> = OnceLock::new();
    C.get_or_init(|| {
        let c = Capture::default();
        let _ = tracing::subscriber::set_global_default(c.clone());
        c
    })
    .clone()
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, v.to_string()).unwrap();
}

/// The instance as 0.61 grew it: the key as a late-bound token, no grant.
fn instance(root: &std::path::Path, base_url: &str) {
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    write(
        root,
        "main/brain/config.json",
        &json!({
            "cell": {"type": "llm"},
            "params": {
                "provider": "openai", "model": "gpt-4o-mini",
                "api_key": "${OPENROUTER_API_KEY}",
                "base_url": base_url, "external_timeout_ms": 10_000
            },
            "contract": {
                "version": "1.0.0", "settings": {},
                "emits": {"body": {"messages": {"type": "array", "required": false},
                                   "meta": {"type": "object", "required": false}}},
                "consumes": {"body": {"messages": {"type": "array", "required": false}}},
                "capabilities": ["network:llm", "db:own"]
            }
        }),
    );
    std::fs::write(root.join(".env"), format!("OPENROUTER_API_KEY={SECRET}\n")).unwrap();
}

fn chat_answer() -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1,
            "model": "gpt-4o-mini",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": "pong"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string()
        .as_bytes(),
    )
}

fn log_rows(root: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT headers, COALESCE(body_payload, '') FROM message_log")
        .expect("message_log");
    st.query_map([], |r| {
        Ok(format!(
            "{} {}",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1061_an_instance_built_with_a_key_still_runs_for_one_release() {
    let logs = capture();
    let (addr, _server, captured) = start_mock_server_capturing(vec![chat_answer()]).await;
    let td = tempfile::TempDir::new().unwrap();
    instance(td.path(), &format!("http://{addr}/v1"));

    let fs: Vec<(String, Arc<dyn CellFactory>)> = vec![(
        "llm".to_string(),
        Arc::new(LlmCellFactory) as Arc<dyn CellFactory>,
    )];
    let h = ColonyHandle::new_with_factories_at(&td, fs.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the 0.61 instance boots");

    h.send(
        MessageBuilder::new(Path::new("/brain"))
            .body(Body::Inline(
                json!({"messages": [{"origin": "user", "type": "text", "text": "ping"}]}),
            ))
            .ttl(400)
            .build(),
    )
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while captured.lock().await.is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the instance never called its provider"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let auth: Vec<String> = captured
        .lock()
        .await
        .iter()
        .map(|r| r.headers.get("authorization").cloned().unwrap_or_default())
        .collect();
    assert_eq!(
        auth,
        [format!("Bearer {SECRET}")],
        "the key from {{root}}/.env reaches the provider"
    );
    h.shutdown().await;

    let rows = log_rows(td.path());
    assert!(
        rows.iter().all(|r| !r.contains("credential_request")),
        "no grant, no request"
    );
    assert!(
        rows.iter().all(|r| !r.contains(SECRET)),
        "the key is on record"
    );
    let lines = logs.0.lock().unwrap().clone();
    assert!(
        lines.iter().all(|l| !l.contains(SECRET)),
        "the key is in a log line"
    );
}
