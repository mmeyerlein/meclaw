//! GH #890 -- the `llm` cell marks the provider's cache and says when it goes
//! cold.
//!
//! The cell that calls the provider is the one that knows what it sent, so it
//! steers the provider's prompt cache (`cache_mode`) and stamps the moment the
//! written prefix expires (`hop.cache_expires_at`). The curator of an
//! assistant sets its alarm on that stamp and rebuilds its window only when
//! the cache is cold anyway (OR-KX-G2, OR-KX-G4). Measured where it lands:
//!
//! 1. `off` sends the request of before -- the frozen wire fixture, byte for
//!    byte, however the other two knobs are set;
//! 2. `breakpoints` marks exactly two prefix ends: the end of the system
//!    message and the end of the stable history, and nothing else moves
//!    (its own frozen fixture);
//! 3. the answer's hop, read back out of the colony's `message_log`, carries
//!    `cache_expires_at` = the end of the provider call + `cache_ttl_s`, the
//!    cache-write tokens and the window -- and carries no stamp under `off`,
//!    under a TTL of 0, or on an error (a provider that refuses the marks
//!    fails as it always failed);
//! 4. under `breakpoints` two turns of one conversation grow the marked prefix
//!    and never rewrite it;
//! 5. the registry pushes the three keys with the model they belong to, and
//!    clearing the replacement takes all three back out.
//!
//! Free of a paid call by construction: every provider here is the in-process
//! mock. Fixtures live beside the test, not under `plans/`, so the test runs
//! in the published tree (see `llm_chat_completions_wire_regression.rs`).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{CellFactory, CellFactoryRegistry, DbConn, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::MockResponse;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion, canned_error_status};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The legacy wire, frozen by `llm_chat_completions_wire_regression.rs`.
const OFF_FIXTURE: &str = "tests/fixtures/expected_chat_completions_body.json";
/// The same request under `breakpoints` with a five-minute TTL.
const BREAKPOINTS_FIXTURE: &str = "tests/fixtures/expected_chat_completions_body_breakpoints.json";

/// Failure marker, generous per the 30 s convention -- not a timing claim.
const MARKER: Duration = Duration::from_secs(30);

fn sink_at(path: &str) -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(16);
    let sink = OutputSink::new(
        tx,
        Path::new(path),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

fn cell_db(td: &TempDir) -> DbConn {
    let raw = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    DbConn::wrap(raw, None)
}

fn read_fixture(path: &str) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("fixture {path}: {e}"));
    meclaw_core::serde_json::from_str(&text).expect("fixture is valid JSON")
}

/// The broad request of the wire regression -- system, a tool schema, every
/// turn type, attribution, `provider_extra` -- with `extra` params on top.
async fn broad_request(extra: Value) -> Value {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let mut raw = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "api_key": "test-key-a0",
        "base_url": format!("{}/v1", mock.base_url),
        "temperature": 0.3,
        "max_tokens": 512,
        "system_order": ["identity", "instructions"],
        "provider_extra": {"seed": 42},
        "http_referer": "https://example.test",
        "x_title": "Example App",
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        raw[k] = v;
    }
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).unwrap(),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let (sink, _rx) = sink_at("/llm");
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "system": {
                "identity": {"text": "You are a fixture."},
                "instructions": {"text": "Be terse."},
                "tools": {
                    "get_weather": {"text": "{\"name\":\"get_weather\",\"description\":\"w\",\"parameters\":{\"type\":\"object\",\"properties\":{}}}"}
                }
            },
            "messages": [
                {"origin": "user", "type": "text", "text": "first"},
                {"origin": "assistant", "type": "tool_call", "id": "call_1",
                 "text": "{\"name\":\"get_weather\",\"arguments\":\"{}\"}"},
                {"origin": "tool", "type": "tool_result", "id": "call_1", "text": "sunny"},
                {"origin": "assistant", "type": "text", "text": "it is sunny"},
                {"origin": "user", "type": "text", "text": "thanks"}
            ]
        })))
        .build();
    cell.handle(msg, &sink, &mut db).await;
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call");
    snaps[0].body.clone()
}

fn bytes(v: &Value) -> String {
    meclaw_core::serde_json::to_string(v).expect("serialise")
}

/// The index of every message that carries a `cache_control` mark.
fn marked(body: &Value) -> Vec<usize> {
    body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .enumerate()
        .filter(|(_, m)| m.to_string().contains("cache_control"))
        .map(|(i, _)| i)
        .collect()
}

/// A message with its marks taken off: a one-part text content array becomes
/// the string it wraps. That is what a provider reads either way -- a string
/// content IS one text block -- so this is the content the cache compares.
fn unmarked(message: &Value) -> Value {
    let mut m = message.clone();
    let text = match message["content"].as_array() {
        Some(parts) if parts.len() == 1 && parts[0]["type"] == "text" => parts[0]["text"].clone(),
        _ => return m,
    };
    m["content"] = text;
    m
}

// ═══════════════════════════════════════════════════════════════ 1-2: the wire

/// The most important promise of the strand: a cell that sets `off` -- or
/// nothing -- sends exactly what it sent before, whatever TTL and window it
/// states.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn off_keeps_the_wire_fixture_byte_identical() {
    let body = broad_request(json!({
        "cache_mode": "off", "cache_ttl_s": 3600, "context_window": 128000
    }))
    .await;
    assert_eq!(
        bytes(&body),
        bytes(&read_fixture(OFF_FIXTURE)),
        "off drifted from the legacy wire:\n{}",
        meclaw_core::serde_json::to_string_pretty(&body).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn breakpoints_marks_exactly_two_prefix_ends() {
    let body = broad_request(json!({"cache_mode": "breakpoints", "cache_ttl_s": 300})).await;
    assert_eq!(
        bytes(&body),
        bytes(&read_fixture(BREAKPOINTS_FIXTURE)),
        "breakpoints drifted from its fixture:\n{}",
        meclaw_core::serde_json::to_string_pretty(&body).unwrap()
    );
    // Read as a structure, not only as bytes: the system message and the last
    // message with text before the youngest user turn, and nothing else.
    assert_eq!(marked(&body), vec![0, 4], "{body}");
    assert_eq!(body.to_string().matches("cache_control").count(), 2);
    assert!(body.get("prompt_cache_key").is_none(), "{body}");
    // Everything else byte-identical: take the marks off and it is `off`.
    let mut plain = body.clone();
    let messages: Vec<Value> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(unmarked)
        .collect();
    plain["messages"] = Value::Array(messages);
    assert_eq!(bytes(&plain), bytes(&read_fixture(OFF_FIXTURE)));
}

// ═══════════════════════════════════════════════════ 3: the stamp, in the log

/// A chat completion with a caller-chosen `usage` block.
fn answer_with_usage(usage: Value) -> MockResponse {
    let body = json!({
        "id": "chatcmpl-gh890",
        "model": "gpt-4o-mock",
        "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
        "usage": usage
    });
    MockResponse::ok_json(body.to_string().as_bytes())
}

fn llm_config(base_url: &str, cache: Value) -> String {
    let mut params = json!({
        "provider": "openai", "model": "gpt-4o", "api_key": "test-key", "base_url": base_url
    });
    for (k, v) in cache.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    meclaw_core::serde_json::to_string_pretty(&json!({
        "cell": {"type": "llm"},
        "params": params,
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    }))
    .unwrap()
}

/// One conversation with a history, so both marks have somewhere to sit.
fn conversation_to(cell: &str) -> Message {
    MessageBuilder::new(Path::new(cell))
        .body(Body::Inline(json!({
            "system": {"identity": {"text": "S"}},
            "messages": [
                {"origin": "user", "type": "text", "text": "a"},
                {"origin": "assistant", "type": "text", "text": "b"},
                {"origin": "user", "type": "text", "text": "c"}
            ]
        })))
        .build()
}

/// `(from_path, created_at, hop)` of one logged message.
type LogRow = (String, i64, meclaw_core::serde_json::Map<String, Value>);

/// Every logged message into `/sink`.
fn sink_rows(root: &std::path::Path) -> Vec<LogRow> {
    let Ok(conn) = rusqlite::Connection::open(root.join("colony.db")) else {
        return Vec::new();
    };
    let Ok(mut st) = conn
        .prepare("SELECT from_path, created_at, headers FROM message_log WHERE to_path = '/sink'")
    else {
        return Vec::new();
    };
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })
    .map(|rows| {
        rows.filter_map(Result::ok)
            .map(|(from, at, headers)| {
                let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
                (from, at, h["hop"].as_object().cloned().unwrap_or_default())
            })
            .collect()
    })
    .unwrap_or_default()
}

/// RFC 3339, UTC, whole seconds, `Z` -- the form a `timer` takes as `at`.
fn expiry_secs(stamp: &str) -> i64 {
    assert!(
        stamp.ends_with('Z') && !stamp.contains('.') && stamp.len() == 20,
        "not RFC 3339 UTC on seconds: {stamp}"
    );
    chrono::DateTime::parse_from_rfc3339(stamp)
        .unwrap_or_else(|e| panic!("{stamp}: {e}"))
        .timestamp()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hop_says_when_the_cache_goes_cold() {
    let cache_usage = json!({
        "prompt_tokens": 1200, "completion_tokens": 5,
        "prompt_tokens_details": {"cached_tokens": 1024, "cache_write_tokens": 176}
    });
    // One mock per cell: a shared queue would hand the answers out in
    // arrival order, and the cells run concurrently.
    let warm = MockOpenAI::start(vec![answer_with_usage(cache_usage.clone())]).await;
    let cold = MockOpenAI::start(vec![answer_with_usage(cache_usage.clone())]).await;
    let unknown = MockOpenAI::start(vec![answer_with_usage(json!({
        "prompt_tokens": 1200, "completion_tokens": 5,
        "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 64
    }))])
    .await;
    let zero = MockOpenAI::start(vec![answer_with_usage(json!({
        "prompt_tokens": 1200, "completion_tokens": 5,
        "prompt_tokens_details": {"cached_tokens": 0, "cache_write_tokens": 0}
    }))])
    .await;
    // Review focus (1): a provider that refuses the marks answers 400.
    let refused = MockOpenAI::start(vec![canned_error_status(400)]).await;

    let cells: [(&str, &MockOpenAI, Value); 5] = [
        (
            "warm",
            &warm,
            json!({"cache_mode": "breakpoints", "cache_ttl_s": 300, "context_window": 200000}),
        ),
        (
            "cold",
            &cold,
            json!({"cache_mode": "off", "cache_ttl_s": 300}),
        ),
        (
            "unknown",
            &unknown,
            json!({"cache_mode": "implicit", "cache_ttl_s": 0}),
        ),
        (
            "zero",
            &zero,
            json!({"cache_mode": "implicit", "cache_ttl_s": "300"}),
        ),
        (
            "refused",
            &refused,
            json!({"cache_mode": "breakpoints", "cache_ttl_s": 300, "context_window": 1000}),
        ),
    ];

    let td = TempDir::new().unwrap();
    let edges: Vec<Value> = cells
        .iter()
        .map(|(name, _, _)| json!({"from": format!("./{name}"), "to": "/sink"}))
        .collect();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        meclaw_core::serde_json::to_string_pretty(
            &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
        )
        .unwrap(),
    )
    .unwrap();
    for (name, mock, cache) in &cells {
        std::fs::create_dir_all(td.path().join(format!("main/{name}"))).unwrap();
        std::fs::write(
            td.path().join(format!("main/{name}/config.json")),
            llm_config(&format!("{}/v1", mock.base_url), cache.clone()),
        )
        .unwrap();
    }

    let llm_f: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("llm".to_string(), llm_f.clone())]);
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), llm_f);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap");

    for (name, _, _) in &cells {
        h.send(conversation_to(&format!("/{name}"))).await;
    }
    for _ in 0..cells.len() {
        tokio::time::timeout(MARKER, sink_rx.recv())
            .await
            .ok()
            .flatten()
            .expect("every cell answers");
    }
    // The receiver took them; the log is written by its own task. Poll it --
    // the marker is the failure bound, not a timing claim.
    let deadline = std::time::Instant::now() + MARKER;
    let rows = loop {
        let rows = sink_rows(td.path());
        if rows.len() >= cells.len() || std::time::Instant::now() > deadline {
            break rows;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let hop_of = |cell: &str| {
        let hits: Vec<_> = rows
            .iter()
            .filter(|(from, _, _)| from == &format!("/{cell}"))
            .collect();
        assert_eq!(hits.len(), 1, "one logged answer from /{cell}: {rows:?}");
        (hits[0].1, hits[0].2.clone())
    };

    // warm: the stamp, the write tokens, the window -- measured in the log.
    let (logged_at, hop) = hop_of("warm");
    assert_eq!(hop["finish_reason"], "stop", "{hop:?}");
    assert_eq!(hop["tokens_cached"], 1024, "{hop:?}");
    assert_eq!(hop["tokens_cache_write"], 176, "{hop:?}");
    assert_eq!(hop["context_window"], 200_000, "{hop:?}");
    let expires = expiry_secs(hop["cache_expires_at"].as_str().expect("a stamp"));
    assert!(
        (expires - (logged_at + 300)).abs() <= 1,
        "cache_expires_at {expires} is not the answer time {logged_at} + 300 s (±1 s): {hop:?}"
    );
    assert_eq!(
        warm.recorded_requests().await[0]
            .body
            .to_string()
            .matches("cache_control")
            .count(),
        2
    );

    // cold: `off` stamps no expiry, whatever the TTL; the usage is still usage.
    let (_, hop) = hop_of("cold");
    assert!(!hop.contains_key("cache_expires_at"), "{hop:?}");
    assert!(
        !hop.contains_key("context_window"),
        "0 is no window: {hop:?}"
    );
    assert_eq!(hop["tokens_cache_write"], 176, "{hop:?}");
    assert!(
        !cold.recorded_requests().await[0]
            .body
            .to_string()
            .contains("cache_control")
    );

    // unknown: TTL 0 = the expiry is unknown, nothing stamped. The Anthropic
    // spelling of both cache figures lands in the same keys.
    let (_, hop) = hop_of("unknown");
    assert!(!hop.contains_key("cache_expires_at"), "{hop:?}");
    assert_eq!(hop["tokens_cached"], 1000, "{hop:?}");
    assert_eq!(hop["tokens_cache_write"], 64, "{hop:?}");
    assert!(
        unknown.recorded_requests().await[0].body["prompt_cache_key"]
            .as_str()
            .is_some_and(|k| k.starts_with("meclaw-")),
        "implicit sends the cell's key"
    );

    // zero: a TTL from an env substitution ("300") stamps; a zero write is no
    // write (review focus 4).
    let (logged_at, hop) = hop_of("zero");
    assert!(!hop.contains_key("tokens_cache_write"), "{hop:?}");
    let expires = expiry_secs(hop["cache_expires_at"].as_str().expect("a stamp"));
    assert!((expires - (logged_at + 300)).abs() <= 1, "{hop:?}");

    // refused: the error path is the one of before -- no stamp, no window, no
    // usage -- although the cell states a TTL and a window.
    let (_, hop) = hop_of("refused");
    assert_eq!(hop["finish_reason"], "error", "{hop:?}");
    for key in ["cache_expires_at", "context_window", "tokens_cache_write"] {
        assert!(!hop.contains_key(key), "{key} on an error: {hop:?}");
    }

    h.shutdown().await;
}

// ═══════════════════════════════════════════════ 4: the prefix only grows

/// Two turns of one conversation under `breakpoints`: everything up to the
/// second mark of turn 1 is where turn 2 begins, so the provider reads the
/// first prefix back from its cache and writes only what grew. Compared with
/// the marks taken off (`unmarked`): turn 1's history mark sits on a message
/// that is plain history in turn 2, and the mark is no content.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_turns_under_breakpoints_only_grow_the_prefix() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("d", "stop"),
        canned_chat_completion("f", "stop"),
    ])
    .await;
    let raw = json!({
        "provider": "openai", "model": "gpt-4o", "api_key": "k",
        "base_url": format!("{}/v1", mock.base_url),
        "cache_mode": "breakpoints", "cache_ttl_s": 300
    });
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).unwrap(),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let mut db = cell_db(&td);
    let (sink, mut rx) = sink_at("/brain");
    let t = |origin: &str, text: &str| json!({"origin": origin, "type": "text", "text": text});
    for messages in [
        vec![t("user", "a"), t("assistant", "b"), t("user", "c")],
        vec![
            t("user", "a"),
            t("assistant", "b"),
            t("user", "c"),
            t("assistant", "d"),
            t("user", "e"),
        ],
    ] {
        let msg = MessageBuilder::new(Path::new("/brain"))
            .reply_to(Path::new("/observer"))
            .body(Body::Inline(json!({
                "system": {"identity": {"text": "You are the same persona every turn."}},
                "messages": messages
            })))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        let _ = rx.recv().await.expect("an answer");
    }
    let reqs = mock.recorded_requests().await;
    assert_eq!(reqs.len(), 2);
    let (one, two) = (&reqs[0].body, &reqs[1].body);
    assert_eq!(marked(one), vec![0, 2], "{one}");
    assert_eq!(marked(two), vec![0, 4], "{two}");
    let upto = |body: &Value, end: usize| -> Vec<String> {
        body["messages"].as_array().unwrap()[..=end]
            .iter()
            .map(|m| bytes(&unmarked(m)))
            .collect()
    };
    let first = upto(one, marked(one)[1]);
    let second = upto(two, marked(two)[1]);
    assert!(
        second.starts_with(&first),
        "the prefix of turn 2 must begin with the prefix of turn 1:\n{first:?}\n{second:?}"
    );
    assert!(second.len() > first.len(), "and it grew");
    // The system mark is byte-identical, mark included.
    assert_eq!(bytes(&one["messages"][0]), bytes(&two["messages"][0]));
}

// ═════════════════════════════════════════ 5: the registry pushes the three

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// R2b / GH #49: a tree without the registry skips.
fn registry_shipped() -> bool {
    ["config.json", "hand/config.json", "store/seed/models.jsonl"]
        .iter()
        .all(|rel| templates_root().join("llm-registry").join(rel).exists())
}

fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_cells(&from, &dst.join(name));
        } else if name == "config.json"
            || src.file_name().is_some_and(|d| d == "seed")
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "jsonl")
        {
            std::fs::copy(&from, dst.join(name)).unwrap();
        }
    }
}

/// A harness text is a registry command, as a tool_call turn; the actor rides
/// the hop so the edge can make it edge truth (the form of GH #855's lock).
const OPERATOR: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
msgs = d.get("messages", [])
raw = str(msgs[-1].get("text", "{}")) if msgs else "{}"
sys.stdout.write(json.dumps({
    "header": {"route": "command", "actor": "operator"},
    "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1", "text": raw}]}))
"#;

const START: &str = "start/model";
const CACHED: &str = "test/cached";
const BRAIN: &str = "/brain";

fn registry_tree(td: &TempDir, provider: &str) {
    let root = td.path();
    let write = |rel: &str, v: &Value| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
    };
    std::fs::write(root.join(".env"), "OPENROUTER_API_KEY=test-key\n").unwrap();
    write(
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": "./operator", "to": "./llm_registry",
             "condition": "has(hop.route) && hop.route == 'command'",
             "modifier": {"set_hop": {"route": "'in_hand'"},
                          "set_context": {"actor": "hop.actor"}}},
            {"from": "./llm_registry", "to": "/acks",
             "condition": "has(hop.route) && hop.route == 'ack'"},
            {"from": "./llm_registry", "to": "/park",
             "condition": "has(hop.route) && hop.route != 'ack' && hop.route != 'update'"},
            // THE ROAD: the registry's update, addressed by the brain's path.
            {"from": "./llm_registry", "to": "./brain",
             "condition": format!("has(hop.route) && hop.route == 'update' && \
                                   has(hop.subscriber) && hop.subscriber == '{BRAIN}'")},
            {"from": "./brain", "to": "/sink"}
        ]}}}),
    );
    write(
        "main/operator/config.json",
        &json!({
            "cell": {"type": "code"},
            "params": {"runner": "python3", "script_inline": OPERATOR,
                       "external_timeout_ms": 15000},
            "contract": {
                "version": "1.0.0", "settings": {}, "multi_send_capable": true,
                "emits": {"body": {"messages": {"type": "array", "required": true}},
                          "hop": {"route": {"type": "string", "values": ["command"],
                                            "required": false},
                                  "actor": {"type": "string", "required": false}}},
                "consumes": {"body": {"messages": {"type": "array", "required": false}}},
                "capabilities": ["shell:exec"]
            },
            "description": {"purpose": "Test stand-in: a registry command.",
                            "use_when": "Test fixture only.", "not_in_scope": "Not a template."}
        }),
    );
    write(
        "main/brain/config.json",
        &json!({
            "cell": {"type": "llm"},
            "params": {"provider": "openai", "model": START, "api_key": "test-key",
                       "base_url": provider},
            "contract": {"version": "0.1.0", "settings": {}, "consumes": {"body": {
                "messages": {"type": "array", "required": false},
                "system": {"type": "object", "required": false},
                "params": {"type": "object", "required": false}}}}
        }),
    );
    copy_cells(
        &templates_root().join("llm-registry"),
        &root.join("main/llm_registry"),
    );
    // The translator is never asked here (no subscriber states a requirement);
    // pointed at a closed port all the same, so nothing can leave the host.
    let translate = root.join("main/llm_registry/translate/config.json");
    let mut t: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(&translate).unwrap()).unwrap();
    t["params"]["base_url"] = json!("http://127.0.0.1:9/v1");
    std::fs::write(
        &translate,
        meclaw_core::serde_json::to_string_pretty(&t).unwrap(),
    )
    .unwrap();
}

struct Ports {
    answers: mpsc::Receiver<Message>,
    acks: mpsc::Receiver<Message>,
    _park: mpsc::Receiver<Message>,
}

async fn recv(rx: &mut mpsc::Receiver<Message>, what: &str) -> Message {
    tokio::time::timeout(MARKER, rx.recv())
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("{what} never arrived"))
}

fn first_turn_json(m: &Message) -> Value {
    let Body::Inline(b) = &m.body else {
        panic!("inline expected")
    };
    meclaw_core::serde_json::from_str(b["messages"][0]["text"].as_str().unwrap_or("null"))
        .unwrap_or(Value::Null)
}

async fn command(h: &ColonyHandle, p: &mut Ports, cmd: Value) -> Value {
    h.send(
        MessageBuilder::new(Path::new("/operator"))
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": cmd.to_string()}]})))
            .ttl(200)
            .build(),
    )
    .await;
    loop {
        let m = recv(&mut p.acks, "the ack").await;
        if m.headers.hop.get("route").and_then(|v| v.as_str()) == Some("ack") {
            return first_turn_json(&m);
        }
    }
}

/// One turn into the brain: the request it cost and the hop of its answer.
async fn a_turn(h: &ColonyHandle, p: &mut Ports, mock: &MockOpenAI) -> (Value, Message) {
    let before = mock.recorded_requests().await.len();
    h.send(conversation_to(BRAIN)).await;
    let answer = recv(&mut p.answers, "the answer").await;
    let reqs = mock.recorded_requests().await;
    assert_eq!(
        reqs.len(),
        before + 1,
        "one provider call per turn, none per push"
    );
    (reqs[before].body.clone(), answer)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_registry_pushes_cache_mode_ttl_and_window() {
    if !registry_shipped() {
        return;
    }
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("one", "stop"),
        canned_chat_completion("two", "stop"),
    ])
    .await;
    let td = TempDir::new().unwrap();
    registry_tree(&td, &format!("{}/v1", mock.base_url));
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (a_tx, a_rx) = mpsc::channel::<Message>(64);
    let (k_tx, k_rx) = mpsc::channel::<Message>(64);
    let (p_tx, p_rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(a_tx.clone()))
        .await;
    h.spawn(Path::new("/acks"), move || CaptureCell::new(k_tx.clone()))
        .await;
    h.spawn(Path::new("/park"), move || CaptureCell::new(p_tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap");
    let mut p = Ports {
        answers: a_rx,
        acks: k_rx,
        _park: p_rx,
    };

    // The catalogue row, kept by the operator on the hand's own lane: no
    // endpoint of its own (the brain keeps its), and the cache of its provider.
    let ack = command(
        &h,
        &mut p,
        json!({"op": "model_upsert", "model": {
            "model_id": CACHED, "provider": "gateway", "base_url": "", "wire_dialect": "",
            "context_window": 64000, "cost_in": 1, "cost_out": 1, "status": "active",
            "note": "TEST ROW", "cache_mode": "breakpoints", "cache_ttl_s": 3600}}),
    )
    .await;
    assert_eq!(ack["outcome"], "accepted", "{ack}");
    let ack = command(
        &h,
        &mut p,
        json!({"op": "subscribe", "cell_path": BRAIN, "start_model": START}),
    )
    .await;
    assert_eq!(ack["outcome"], "accepted", "{ack}");

    // 1. The targeted replacement moves the brain -- with its cache package.
    let ack = command(
        &h,
        &mut p,
        json!({"op": "override_set", "scope": "target", "match": BRAIN, "model_id": CACHED}),
    )
    .await;
    assert_eq!(ack["pushed"], 1, "{ack}");
    let id = ack["id"].as_str().unwrap_or_default().to_string();
    let (body, answer) = a_turn(&h, &mut p, &mock).await;
    assert_eq!(body["model"], CACHED, "{body}");
    assert_eq!(marked(&body), vec![0, 2], "cache_mode arrived: {body}");
    assert_eq!(
        body["messages"][0]["content"][0]["cache_control"],
        json!({"type": "ephemeral", "ttl": "1h"}),
        "cache_ttl_s 3600 arrived: {body}"
    );
    let hop = &answer.headers.hop;
    assert_eq!(hop.get("context_window"), Some(&json!(64000)), "{hop:?}");
    let stamp = hop
        .get("cache_expires_at")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("no expiry stamp: {hop:?}"));
    let lead = expiry_secs(stamp) - chrono::Utc::now().timestamp();
    assert!(
        (3600 - 30..=3600 + 1).contains(&lead),
        "the stamp is an hour out: {stamp} ({lead} s)"
    );

    // 2. Cleared: the start value is a `$reset` of every package key, and no
    //    cache setting of the old package stays behind (review focus 3).
    let ack = command(&h, &mut p, json!({"op": "override_clear", "id": id})).await;
    assert_eq!(ack["pushed"], 1, "{ack}");
    let (body, answer) = a_turn(&h, &mut p, &mock).await;
    assert_eq!(body["model"], START, "{body}");
    assert!(!body.to_string().contains("cache_control"), "{body}");
    assert!(body.get("prompt_cache_key").is_none(), "{body}");
    let hop = &answer.headers.hop;
    assert!(
        hop.get("cache_expires_at").is_none() && hop.get("context_window").is_none(),
        "{hop:?}"
    );

    h.shutdown().await;
}
