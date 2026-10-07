//! GH #1037 — an answer cut on `length` is continued and said, never cut in
//! silence.
//!
//! Measured before: the memory hive's dream step asked for 4 096 tokens and
//! its close step for 8 192; a long consolidation stopped on `length` and the
//! cut answer travelled on as if whole (or was refused as "not JSON"), with no
//! line anywhere. Now the cell continues a cut text answer up to
//! `length_continuations` times, joins the parts into one answer with the
//! usage summed, and writes a line on `meclaw::llm::length` for every
//! continuation and for an answer that stays cut.
//!
//! Events are caught by the hand-rolled collector of
//! `gh853_the_params_line_is_written_at_the_seam.rs`; every test drives its
//! own cell path and reads only events naming it.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use tempfile::TempDir;
use tokio::sync::mpsc;

const TARGET: &str = "meclaw::llm::length";

#[allow(dead_code)]
mod collect {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    /// One captured event: target, level, field name → rendered value.
    #[derive(Clone, Debug)]
    pub struct Event {
        pub target: String,
        pub level: tracing::Level,
        pub fields: HashMap<String, String>,
    }

    impl Event {
        pub fn message(&self) -> &str {
            self.fields.get("message").map(String::as_str).unwrap_or("")
        }
    }

    pub type Events = Arc<Mutex<Vec<Event>>>;

    #[derive(Default)]
    struct Grab(HashMap<String, String>);

    impl tracing::field::Visit for Grab {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .insert(field.name().to_string(), format!("{value:?}"));
        }
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
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
            self.0.lock().unwrap().push(Event {
                target: event.metadata().target().to_string(),
                level: *event.metadata().level(),
                fields: grab.0,
            });
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

    /// The events on `target` that name `path` -- in the message (the params
    /// line and the turn-slot warning carry it there) or in a `path` field
    /// (the restore warning).
    pub fn on(buf: &Events, target: &str, path: &str) -> Vec<Event> {
        let needle = format!("{path} ");
        buf.lock()
            .unwrap()
            .iter()
            .filter(|e| {
                e.target == target
                    && (e.message().contains(&needle)
                        || e.fields.get("path").is_some_and(|p| p == path))
            })
            .cloned()
            .collect()
    }
}

/// One turn through a fresh cell at `path`: the requests the provider saw,
/// the emission, and the length lines naming `path`.
async fn one_turn(
    path: &str,
    extra: Value,
    answers: Vec<meclaw_testing::mock_http::MockResponse>,
) -> (Vec<Value>, Value, Vec<String>) {
    turn_on(TARGET, path, extra, answers).await
}

/// [`one_turn`], reading the lines of `target` instead of the length target.
async fn turn_on(
    target: &str,
    path: &str,
    extra: Value,
    answers: Vec<meclaw_testing::mock_http::MockResponse>,
) -> (Vec<Value>, Value, Vec<String>) {
    turn_backstopped(target, path, extra, answers, None).await
}

/// [`turn_on`] through a cell that knows its `message_timeout` backstop.
async fn turn_backstopped(
    target: &str,
    path: &str,
    extra: Value,
    answers: Vec<meclaw_testing::mock_http::MockResponse>,
    backstop: Option<std::time::Duration>,
) -> (Vec<Value>, Value, Vec<String>) {
    let events = collect::install();
    let mock = MockOpenAI::start(answers).await;
    let mut raw = json!({
        "provider": "openai", "model": "gpt-4o", "api_key": "k",
        "base_url": format!("{}/v1", mock.base_url),
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        raw[k] = v;
    }
    let params = LlmParams::parse(&raw).unwrap();
    let mut cell = LlmCell::new(params, reqwest::Client::new()).with_message_timeout(backstop);
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    let mut conn = DbConn::wrap(conn, None);
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new(path),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new(path))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "system": {},
            "messages": [{"origin": "user", "type": "text", "text": "Consolidate."}]
        })))
        .build();
    cell.handle(msg, &sink, &mut conn).await;
    let em = rx.recv().await.expect("the cell answers");
    let bodies = mock
        .recorded_requests()
        .await
        .into_iter()
        .map(|s| s.body)
        .collect();
    let lines = collect::on(&events, target, path)
        .iter()
        .map(|e| e.message().to_string())
        .collect();
    (bodies, em.content, lines)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cut_answer_is_continued_joined_and_said() {
    let (requests, em, lines) = one_turn(
        "/kf1/continued",
        json!({"length_continuations": 2}),
        vec![
            canned_chat_completion("part one ", "length"),
            canned_chat_completion("part two", "stop"),
        ],
    )
    .await;
    assert_eq!(requests.len(), 2, "one continuation call: {requests:?}");
    assert_eq!(
        requests[0]["max_tokens"], 32_768,
        "no own cap, no package: the modern default"
    );
    let second = requests[1]["messages"].as_array().unwrap();
    let n = second.len();
    assert_eq!(
        second[n - 2],
        json!({"role": "assistant", "content": "part one "}),
        "the continuation joins the text so far: {second:?}"
    );
    assert_eq!(second[n - 1]["role"], "user");
    assert_eq!(
        em["messages"],
        json!([{"origin": "assistant", "type": "text", "text": "part one part two"}]),
        "ONE answer, joined: {em}"
    );
    assert_eq!(em["header"]["finish_reason"], "stop");
    assert_eq!(em["header"]["tokens_prompt"], 20, "usage of both calls");
    assert_eq!(em["header"]["tokens_completion"], 10, "usage of both calls");
    assert_eq!(lines.len(), 1, "one line per continuation: {lines:?}");
    assert!(lines[0].contains("continuation 1/2"), "{lines:?}");
    assert!(lines[0].contains("max_tokens=32768"), "{lines:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_continuations_a_cut_answer_is_still_said() {
    let (requests, em, lines) = one_turn(
        "/kf1/silent",
        json!({"max_output": 65536}),
        vec![canned_chat_completion("cut", "length")],
    )
    .await;
    assert_eq!(requests.len(), 1, "no continuation asked for");
    assert_eq!(
        requests[0]["max_tokens"], 65_536,
        "no own cap: the package's max_output"
    );
    assert_eq!(
        em["header"]["finish_reason"], "length",
        "the reason travels"
    );
    assert_eq!(lines.len(), 1, "never a silent cut: {lines:?}");
    assert!(lines[0].contains("stays cut"), "{lines:?}");
    assert!(lines[0].contains("length_continuations is 0"), "{lines:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spent_budget_keeps_the_joined_text_and_says_so() {
    let (requests, em, lines) = one_turn(
        "/kf1/spent",
        json!({"length_continuations": 1}),
        vec![
            canned_chat_completion("a", "length"),
            canned_chat_completion("b", "length"),
        ],
    )
    .await;
    assert_eq!(requests.len(), 2, "bounded: {requests:?}");
    assert_eq!(
        em["messages"],
        json!([{"origin": "assistant", "type": "text", "text": "ab"}])
    );
    assert_eq!(em["header"]["finish_reason"], "length");
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("continuation 1/1"), "{lines:?}");
    assert!(lines[1].contains("continuation budget spent"), "{lines:?}");
}

/// M1: a continuation is held to the cell's `message_timeout` backstop. With
/// no time left for another call before the backstop, the answer so far is
/// delivered cut and said -- the backstop would otherwise drop all of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_time_left_before_the_backstop_keeps_the_answer_cut() {
    // margin max(10 s, 1 s / 10) = 10 s: the deadline is 600 ms in, the
    // first answer arrives after 800 ms.
    let (requests, em, lines) = turn_backstopped(
        TARGET,
        "/kf1/backstop",
        json!({"length_continuations": 2, "external_timeout_ms": 1_000}),
        vec![
            canned_chat_completion("cut ", "length")
                .with_delay(std::time::Duration::from_millis(800)),
            canned_chat_completion("rest", "stop"),
        ],
        Some(std::time::Duration::from_millis(10_600)),
    )
    .await;
    assert_eq!(requests.len(), 1, "no call past the backstop: {requests:?}");
    assert_eq!(
        em["messages"],
        json!([{"origin": "assistant", "type": "text", "text": "cut "}]),
        "the text so far is delivered: {em}"
    );
    assert_eq!(em["header"]["finish_reason"], "length");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("stays cut"), "{lines:?}");
    assert!(lines[0].contains("backstop"), "{lines:?}");
}

/// M1: a continuation gets what is left before the backstop, not a fresh
/// `external_timeout_ms`; when that runs out, the text so far stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_continuation_waits_only_for_the_time_left() {
    // deadline 11.5 s - 10 s = 1.5 s in; the continuation would take 3 s.
    let started = std::time::Instant::now();
    let (requests, em, lines) = turn_backstopped(
        TARGET,
        "/kf1/clamped",
        json!({"length_continuations": 2, "external_timeout_ms": 5_000}),
        vec![
            canned_chat_completion("part ", "length"),
            canned_chat_completion("rest", "stop")
                .with_delay(std::time::Duration::from_millis(3_000)),
        ],
        Some(std::time::Duration::from_millis(11_500)),
    )
    .await;
    assert!(
        started.elapsed() < std::time::Duration::from_millis(2_800),
        "the continuation was cut at the time left: {:?}",
        started.elapsed()
    );
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(
        em["messages"],
        json!([{"origin": "assistant", "type": "text", "text": "part "}]),
        "the text so far is delivered: {em}"
    );
    assert_eq!(em["header"]["finish_reason"], "length");
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[1].contains("continuation call failed"), "{lines:?}");
}

/// R-HK-15/16: the window target -- refusals over `input_hard` and the clamp
/// of the output budget to what the window has left.
const WINDOW: &str = "meclaw::llm::window";

/// A prompt above `input_hard` is refused before any provider call, with an
/// error the sender can route and a line in the journal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn over_input_hard_the_call_is_refused_and_said() {
    let (requests, em, lines) = turn_on(
        WINDOW,
        "/kf1b/hard",
        json!({"input_soft": 2, "input_hard": 5}),
        vec![canned_chat_completion("never", "stop")],
    )
    .await;
    assert!(
        requests.is_empty(),
        "no model call over the hard bound: {requests:?}"
    );
    assert_eq!(em["header"]["finish_reason"], "error", "{em}");
    assert_eq!(em["header"]["error_code"], "invalid_input", "{em}");
    assert_eq!(em["meta"]["error"]["kind"], "input_over_hard", "{em}");
    assert_eq!(em["meta"]["error"]["input_hard"], 5, "{em}");
    assert!(
        em["meta"]["error"]["input_estimate"].as_u64().unwrap() > 5,
        "{em}"
    );
    assert_eq!(lines.len(), 1, "one journal line: {lines:?}");
    assert!(lines[0].contains("input_hard=5"), "{lines:?}");
}

/// The output budget never asks past the window: a local server refuses
/// `max_tokens` above `context_window - prompt` with a 400 (KF1 finding 4).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn max_tokens_is_clamped_to_what_the_window_has_left() {
    let (requests, em, lines) = turn_on(
        WINDOW,
        "/kf1b/clamp",
        json!({"context_window": 4000, "max_output": 128000}),
        vec![canned_chat_completion("ok", "stop")],
    )
    .await;
    assert_eq!(requests.len(), 1, "{em}");
    let asked = requests[0]["max_tokens"].as_u64().unwrap();
    assert!(
        asked < 4000 && asked > 3000,
        "clamped below the window: {asked}"
    );
    assert_eq!(lines.len(), 1, "the clamp is said: {lines:?}");
    assert!(lines[0].contains("max_tokens"), "{lines:?}");
}

/// The hop of every answer carries the bounds and prices of the package --
/// the interface the curator (KF2, #1038) reads -- next to the cached tokens.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hop_carries_bounds_and_prices() {
    let (_, em, _) = turn_on(
        WINDOW,
        "/kf1b/hop",
        json!({"input_soft": 250000, "input_hard": 1000000,
               "cost_in": 10, "cost_cached_in": 1.6}),
        vec![canned_chat_completion("ok", "stop")],
    )
    .await;
    let h = &em["header"];
    assert_eq!(h["input_soft"], 250_000, "{h}");
    assert_eq!(h["input_hard"], 1_000_000, "{h}");
    assert_eq!(h["cost_in"], 10.0, "{h}");
    assert_eq!(h["cost_cached_in"], 1.6, "{h}");
}
