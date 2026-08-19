//! GH #124 — phase instrumentation of the `llm` cell.
//!
//! The operator datapoint behind the issue: the provider dashboard reported
//! 2–4.5 s per request while the message log showed ~16 s between the
//! collector's emission and the brain's answer. Roughly 12 s were unaccounted
//! for INSIDE our own path, and no log line could say where. These tests pin
//! the seams that make that question answerable — the wire layer reports
//! time-to-first-byte separately from the full roundtrip, so "the provider was
//! slow" and "we were slow around the provider" stop looking alike.
//!
//! All timings are wall clock against a local mock; the asserts are deliberately
//! one-sided (a floor from an injected delay, an ordering between phases) so
//! they stay robust under cargo-parallel load.

use meclaw_cells::llm::wire::call_openai_timed;
use serde_json::json;
use std::time::Duration;

#[path = "mock_openai.rs"]
mod mock_openai;
use mock_openai::{MockOpenAI, canned_chat_completion};

/// The injected server delay. Large enough to dominate scheduling noise, small
/// enough not to slow the suite down.
const SERVER_DELAY: Duration = Duration::from_millis(400);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_call_reports_ttfb_and_total_for_a_slow_provider() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("hi", "stop").with_delay(SERVER_DELAY),
    ])
    .await;
    let client = reqwest::Client::builder().build().unwrap();
    let url = format!("{}/chat/completions", mock.base_url);
    let (result, timings) = call_openai_timed(
        &client,
        &url,
        Some("sk-test"),
        &[],
        &json!({"model": "gpt-4o", "messages": []}),
        Duration::from_secs(10),
    )
    .await;
    assert!(result.is_ok(), "mock must answer: {result:?}");

    let ttfb = timings
        .ttfb_ms
        .expect("a completed roundtrip always has a time-to-first-byte");
    // Semantic discriminator: the server slept before answering, so both
    // numbers must have observed that sleep. The floor is the injected delay
    // minus a small tolerance for coarse millisecond truncation.
    let floor = SERVER_DELAY.as_millis() as u64 - 50;
    assert!(
        ttfb >= floor,
        "ttfb_ms={ttfb} must observe the server delay"
    );
    assert!(
        timings.total_ms >= ttfb,
        "total_ms={} must not precede ttfb_ms={ttfb}",
        timings.total_ms
    );
    assert_eq!(timings.attempts, 1, "one POST, one attempt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_call_reports_no_ttfb_when_no_response_head_arrived() {
    // Unbound loopback port — the request never gets a response head, so the
    // instrumentation must say so instead of reporting a fake zero.
    let client = reqwest::Client::builder().build().unwrap();
    let (result, timings) = call_openai_timed(
        &client,
        "http://127.0.0.1:1/chat/completions",
        Some("sk-test"),
        &[],
        &json!({"model": "gpt-4o", "messages": []}),
        Duration::from_millis(500),
    )
    .await;
    assert!(result.is_err(), "an unbound port cannot answer");
    assert_eq!(
        timings.ttfb_ms, None,
        "no response head ⇒ no time-to-first-byte"
    );
    assert_eq!(timings.attempts, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_wire_call_reports_ttfb_and_total_too() {
    // The Responses lane streams its body, so the same split has to exist
    // there — otherwise the SSE drain time would hide inside "the provider".
    let sse = "event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n";
    let mock = MockOpenAI::start(vec![
        meclaw_testing::mock_http::MockResponse::ok(sse.as_bytes()).with_delay(SERVER_DELAY),
    ])
    .await;
    let client = reqwest::Client::builder().build().unwrap();
    let url = format!("{}/responses", mock.base_url);
    let (result, timings) = meclaw_cells::llm::wire::call_responses_timed(
        &client,
        &url,
        Some("sk-test"),
        &[],
        &json!({"model": "gpt-4o", "input": []}),
        Duration::from_secs(10),
    )
    .await;
    assert!(result.is_ok(), "mock must answer: {result:?}");
    let ttfb = timings.ttfb_ms.expect("a streamed answer still has a head");
    assert!(
        ttfb >= SERVER_DELAY.as_millis() as u64 - 50,
        "ttfb_ms={ttfb} must observe the server delay"
    );
    assert!(timings.total_ms >= ttfb);
    assert_eq!(timings.attempts, 1);
}

// ───── GH #124: the summary line an operating log is grepped for ─────

/// Minimal hand-rolled collector. `tracing-subscriber` is not a dependency of
/// this crate and `Cargo.toml` is frozen for this track, so the events are
/// captured through the `Subscriber` trait directly — which is all this test
/// needs: one event stream, no spans, no formatting.
mod collect {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    /// Field name → rendered value, per captured event.
    pub type Fields = HashMap<String, String>;

    /// Shared capture buffer: `(target, fields)` in emission order.
    pub type Events = Arc<Mutex<Vec<(String, Fields)>>>;

    #[derive(Default)]
    struct Grab(Fields);

    impl tracing::field::Visit for Grab {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .insert(field.name().to_string(), format!("{value:?}"));
        }
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_string(), value.to_string());
        }
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            self.0.insert(field.name().to_string(), value.to_string());
        }
    }

    struct Collector(Events);

    impl tracing::Subscriber for Collector {
        fn enabled(&self, _m: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _a: &tracing::span::Attributes<'_>) -> tracing::Id {
            tracing::Id::from_u64(1)
        }
        fn record(&self, _i: &tracing::Id, _v: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _i: &tracing::Id, _f: &tracing::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            let mut grab = Grab::default();
            event.record(&mut grab);
            let target = event.metadata().target().to_string();
            self.0.lock().unwrap().push((target, grab.0));
        }
        fn enter(&self, _i: &tracing::Id) {}
        fn exit(&self, _i: &tracing::Id) {}
    }

    static EVENTS: OnceLock<Events> = OnceLock::new();

    /// Install the collector once per test binary and return the shared buffer.
    pub fn install() -> Events {
        EVENTS
            .get_or_init(|| {
                let buf: Events = Arc::new(Mutex::new(Vec::new()));
                let _ = tracing::subscriber::set_global_default(Collector(buf.clone()));
                buf
            })
            .clone()
    }

    /// Captured events on the latency target with the given message AND
    /// dialect. Tests in one binary share the global collector, so every query
    /// is scoped to the lane the asking test drives.
    pub fn latency_events(buf: &Events, message: &str, dialect: &str) -> Vec<Fields> {
        buf.lock()
            .unwrap()
            .iter()
            .filter(|(t, f)| {
                t == "meclaw::llm::latency"
                    && f.get("message").is_some_and(|m| m == message)
                    && f.get("dialect").is_some_and(|d| d == dialect)
            })
            .map(|(_, f)| f.clone())
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_finished_call_logs_one_summary_line_that_accounts_for_the_whole_handle() {
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::LlmParams;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};

    let events = collect::install();
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("hi back", "stop").with_delay(SERVER_DELAY),
    ])
    .await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
    });
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).unwrap(),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = tempfile::TempDir::new().unwrap();
    let mut conn = DbConn::wrap(
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap(),
        None,
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel::<meclaw_core::CellEmission>(8);
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
            "system": {"identity": {"soul": {"text": "P"}}},
            "messages": [{"origin":"user","type":"text","text":"Hi"}]
        })))
        .build();
    cell.handle(msg, &sink, &mut conn).await;
    rx.recv().await.expect("the cell must emit an answer");

    let lines = collect::latency_events(&events, "llm provider call phases", "chat_completions");
    assert_eq!(
        lines.len(),
        1,
        "exactly one summary line per provider call: {lines:?}"
    );
    let f = &lines[0];
    assert_eq!(f.get("outcome").map(String::as_str), Some("ok"));
    assert_eq!(
        f.get("dialect").map(String::as_str),
        Some("chat_completions")
    );
    assert_eq!(f.get("model").map(String::as_str), Some("gpt-4o"));

    // The point of the line: every phase is named, and the provider's own share
    // is visible next to the total. The mock slept, so both must have seen it.
    let num = |k: &str| -> u64 {
        f.get(k)
            .unwrap_or_else(|| panic!("field {k} missing: {f:?}"))
            .trim_start_matches("Some(")
            .trim_end_matches(')')
            .parse()
            .unwrap_or_else(|e| panic!("field {k} not a number ({e}): {f:?}"))
    };
    let floor = SERVER_DELAY.as_millis() as u64 - 50;
    assert!(num("provider_ttfb_ms") >= floor, "{f:?}");
    assert!(num("wire_total_ms") >= floor, "{f:?}");
    assert!(num("handle_ms") >= num("wire_total_ms"), "{f:?}");
    assert_eq!(num("wire_attempts"), 1, "{f:?}");
    // The residue is what the issue asks about — it must be present and, on a
    // healthy call against a local mock, small.
    assert!(num("unaccounted_ms") < 1_000, "{f:?}");
    for k in ["persist_ms", "translate_ms"] {
        num(k);
    }

    // The DEBUG companion explains a large translate phase with sizes only —
    // never with a byte of the conversation.
    let detail = collect::latency_events(&events, "llm request build detail", "chat_completions");
    assert_eq!(
        detail.len(),
        1,
        "one detail line per built request: {detail:?}"
    );
    let d = &detail[0];
    assert_eq!(d.get("input_turns").map(String::as_str), Some("1"));
    assert_eq!(d.get("tools").map(String::as_str), Some("0"));
    assert_eq!(d.get("image_parts").map(String::as_str), Some("0"));
    assert!(
        d["request_bytes"].parse::<usize>().unwrap() > 0,
        "the built request has a size: {d:?}"
    );
    assert!(
        !d.values().any(|v| v.contains("Hi")),
        "no conversation content may reach the log: {d:?}"
    );
}

/// A minimal but faithful Responses SSE stream (the `event:` line before each
/// `data:` line is what makes the cell recognise it as a stream at all).
fn sse_text_answer(text: &str, model: &str) -> meclaw_testing::mock_http::MockResponse {
    let events = [
        json!({"type":"response.created","response":{"id":"resp_1","model":model}}),
        json!({"type":"response.output_item.done","item":{
            "type":"message","role":"assistant",
            "content":[{"type":"output_text","text":text}]}}),
        json!({"type":"response.completed","response":{
            "id":"resp_1","model":model,"usage":{"input_tokens":3,"output_tokens":2}}}),
    ];
    let body: String = events
        .iter()
        .map(|e| {
            format!(
                "event: {}\ndata: {e}\n\n",
                e["type"].as_str().unwrap_or("message")
            )
        })
        .collect();
    meclaw_testing::mock_http::MockResponse {
        status: 200,
        body: body.into_bytes(),
        content_type: "text/event-stream".into(),
        delay: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_responses_lane_logs_the_same_summary_line() {
    // The SSE lane must be instrumented too — otherwise the stream-drain time
    // would sit invisibly between "provider answered" and "cell emitted", which
    // is exactly the blind spot GH #124 is about.
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::LlmParams;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};

    let events = collect::install();
    let mock = MockOpenAI::start(vec![sse_text_answer("hi back", "gpt-4o")]).await;
    let raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "sk-test",
        "wire_dialect": "responses",
        "base_url": mock.base_url,
    });
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).unwrap(),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = tempfile::TempDir::new().unwrap();
    let mut conn = DbConn::wrap(
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap(),
        None,
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel::<meclaw_core::CellEmission>(8);
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
        .body(Body::Inline(
            json!({"messages": [{"origin":"user","type":"text","text":"Hi"}]}),
        ))
        .build();
    cell.handle(msg, &sink, &mut conn).await;
    let em = rx.recv().await.expect("the cell must emit an answer");
    assert_eq!(em.content["header"]["finish_reason"], "stop");

    let lines = collect::latency_events(&events, "llm provider call phases", "responses");
    assert_eq!(
        lines.len(),
        1,
        "one summary line on this lane too: {lines:?}"
    );
    let f = &lines[0];
    assert_eq!(f.get("outcome").map(String::as_str), Some("ok"));
    assert_eq!(
        f.get("wire_attempts").map(String::as_str),
        Some("1"),
        "one POST, no auth retry: {f:?}"
    );
    assert!(f.contains_key("unaccounted_ms"), "{f:?}");
    assert!(f.contains_key("provider_ttfb_ms"), "{f:?}");
}
