//! GH #1410 -- the `llm` cell hands its answer out sentence by sentence on
//! the `partial` lane while the provider still streams it.
//!
//! Measured before: the first reaction of a text turn came with the whole
//! answer body. The chat wire collected its stream into one body, the
//! Responses wire read the whole SSE body before parsing, the Messages wire
//! did not stream at all. With `partial_unit: "sentence"` every wire hands its
//! visible text deltas to the cell while it reads, and the cell emits one
//! `route: "partial"` message per sentence boundary, cumulative.
//!
//! The provider is `meclaw_testing::mock_sse`: strict (it refuses a request
//! a real provider would refuse, GH #1283) and gated -- it holds its stream
//! open until the test releases it, so a partial received while the gate is
//! shut was emitted before the stream ended. An event, never a clock.

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::deadline;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use meclaw_testing::mock_sse::{self, Dialect, Script, chat, messages, responses};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The committed chat body of the legacy wire (P10 A0), reused: `off` must
/// leave it byte for byte.
const CHAT_BODY_FIXTURE: &str = "tests/fixtures/expected_chat_completions_body.json";
/// GH #1410: the Responses and Messages bodies with the partials off.
const RESPONSES_BODY_FIXTURE: &str = "tests/fixtures/gh1410_responses_body_off.json";
const MESSAGES_BODY_FIXTURE: &str = "tests/fixtures/gh1410_messages_body_off.json";
/// Review M2: the same three off bodies as the raw bytes on the wire.
const CHAT_BODY_RAW: &str = "tests/fixtures/gh1410_chat_body_off.bin";
const RESPONSES_BODY_RAW: &str = "tests/fixtures/gh1410_responses_body_off.bin";
const MESSAGES_BODY_RAW: &str = "tests/fixtures/gh1410_messages_body_off.bin";

fn params(base_url: &str, provider: &str, extra: Value) -> Value {
    let mut raw = json!({
        "provider": provider, "model": "sse-model", "api_key": "k-1410",
        "base_url": base_url, "max_tokens": 256,
        "x_title": "meclaw-lab gh1410", "http_referer": "https://meclaw.ai",
        // Far beyond every hang mark: an answer inside the mark is never
        // this timeout.
        "external_timeout_ms": deadline::product_wait_ms(),
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        raw[k] = v;
    }
    raw
}

fn on(extra: Value) -> Value {
    let mut v = json!({"partial_unit": "sentence"});
    for (k, x) in extra.as_object().cloned().unwrap_or_default() {
        v[k] = x;
    }
    v
}

/// One turn of a fresh cell on its own task; the input's id and the
/// emissions as they come.
async fn turn(raw: Value, body: Value) -> (Uuid, mpsc::Receiver<CellEmission>) {
    let params = LlmParams::parse(&raw).expect("params");
    let (tx, rx) = mpsc::channel::<CellEmission>(32);
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build();
    let id = msg.id;
    tokio::spawn(async move {
        let td = TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = DbConn::wrap(conn, None);
        let sink = OutputSink::new(
            tx,
            Path::new("/llm"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            32,
            meclaw_core::Headers::new(),
            None,
        );
        let mut cell = LlmCell::new(params, reqwest::Client::new());
        cell.handle(msg, &sink, &mut db).await;
    });
    (id, rx)
}

fn ask(text: &str) -> Value {
    json!({"messages": [{"origin": "user", "type": "text", "text": text}]})
}

/// The next emission's body.
async fn next(rx: &mut mpsc::Receiver<CellEmission>) -> Value {
    deadline::hang("the cell emits", rx.recv())
        .await
        .expect("the cell emits")
        .content
}

/// Every emission up to and including the answer (the first one that is not
/// a partial), then the end of the turn (the sink closes).
async fn all(rx: &mut mpsc::Receiver<CellEmission>) -> (Vec<Value>, Value) {
    let mut partials = Vec::new();
    loop {
        let e = next(rx).await;
        if e["header"]["route"] == "partial" {
            partials.push(e);
            continue;
        }
        assert!(
            deadline::hang("the turn ends", rx.recv()).await.is_none(),
            "nothing leaves after the answer"
        );
        return (partials, e);
    }
}

fn texts(partials: &[Value]) -> Vec<String> {
    partials
        .iter()
        .map(|p| p["messages"][0]["text"].as_str().unwrap().to_string())
        .collect()
}

fn answer_text(answer: &Value) -> String {
    answer["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["type"] == "text")
        .filter_map(|t| t["text"].as_str())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sentence_partial_is_emitted_at_the_first_boundary() {
    let mut p = mock_sse::start(
        Dialect::Chat,
        vec![Script::gated(
            vec![chat::text("Sure, gladly."), chat::text(" Let me")],
            [
                vec![chat::text(" look it up for you.")],
                chat::end("stop", 9, 7),
            ]
            .concat(),
        )],
    )
    .await
    .expect("bind a loopback provider");
    let (id, mut rx) = turn(params(&p.base_url, "openai", on(json!({}))), ask("hi")).await;
    // The gate is shut: the rest of the answer has not been sent.
    let first = next(&mut rx).await;
    let sent = p.request().await.expect("the provider read a request");
    assert_eq!(sent.body["stream"], true, "the partials stream the call");
    assert_eq!(first["header"]["route"], "partial", "{first}");
    assert_eq!(first["header"]["seq"], 1, "{first}");
    assert_eq!(first["header"]["final"], false, "{first}");
    assert_eq!(first["header"]["partial_of"], id.to_string(), "{first}");
    assert!(
        first["header"].get("finish_reason").is_none(),
        "an edge on finish_reason never fires on a partial: {first}"
    );
    assert_eq!(
        first["messages"],
        json!([{"origin": "assistant", "type": "text", "text": "Sure, gladly."}])
    );
    p.release(0);
    let (rest, answer) = all(&mut rx).await;
    assert!(
        rest.is_empty(),
        "the rest after the last boundary comes only with the answer: {rest:?}"
    );
    assert_eq!(answer["header"]["finish_reason"], "stop", "{answer}");
    assert!(answer["header"].get("route").is_none(), "{answer}");
    assert_eq!(
        answer_text(&answer),
        "Sure, gladly. Let me look it up for you."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partials_are_cumulative_prefixes_of_the_final_answer() {
    let frames = [
        vec![
            chat::text("The weather is"),
            chat::text(" mild. It will"),
            chat::text(" rain at 3.5 degrees."),
            chat::text(" Take an"),
            chat::text(" umbrella! Bye"),
        ],
        chat::end("stop", 9, 20),
    ]
    .concat();
    let p = mock_sse::start(Dialect::Chat, vec![Script::whole(frames)])
        .await
        .expect("bind a loopback provider");
    let raw = params(&p.base_url, "openai", on(json!({"partial_min_chars": 1})));
    let (_, mut rx) = turn(raw, ask("weather?")).await;
    let (partials, answer) = all(&mut rx).await;
    let full = answer_text(&answer);
    assert_eq!(
        full,
        "The weather is mild. It will rain at 3.5 degrees. Take an umbrella! Bye"
    );
    assert_eq!(
        texts(&partials),
        vec![
            "The weather is mild.",
            "The weather is mild. It will rain at 3.5 degrees.",
            "The weather is mild. It will rain at 3.5 degrees. Take an umbrella!",
        ]
    );
    for (i, p) in partials.iter().enumerate() {
        assert_eq!(p["header"]["seq"], i as u64 + 1, "{p}");
    }
    let t = texts(&partials);
    for w in t.windows(2) {
        assert!(w[1].starts_with(&w[0]), "{w:?}");
    }
    assert!(t.iter().all(|x| full.starts_with(x)), "{t:?} / {full}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partials_count_on_through_a_length_continuation() {
    let p = mock_sse::start(
        Dialect::Chat,
        vec![
            Script::whole(
                [
                    vec![chat::text("First part here. Sec")],
                    chat::end("length", 5, 5),
                ]
                .concat(),
            ),
            Script::whole([vec![chat::text("ond part. Third")], chat::end("stop", 9, 5)].concat()),
        ],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "openai",
        on(json!({"partial_min_chars": 1, "length_continuations": 1})),
    );
    let (_, mut rx) = turn(raw, ask("go")).await;
    let (partials, answer) = all(&mut rx).await;
    assert_eq!(answer_text(&answer), "First part here. Second part. Third");
    assert_eq!(
        texts(&partials),
        vec!["First part here.", "First part here. Second part."]
    );
    let seqs: Vec<&Value> = partials.iter().map(|p| &p["header"]["seq"]).collect();
    assert_eq!(seqs, vec![&json!(1), &json!(2)], "seq counts on");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_partial_is_cut_inside_tool_call_arguments() {
    let tool = json!({"type": "function", "function": {"name": "fetch", "parameters": {}}});
    let mut p = mock_sse::start(
        Dialect::Chat,
        vec![Script::gated(
            vec![
                chat::reasoning("I think. Then I act. "),
                chat::text("Let me check that for you."),
                chat::call_start(0, "call_1", "fetch"),
                chat::call_args(0, "{\"q\": \"Sentence one. Sentence two. \"}"),
            ],
            chat::end("tool_calls", 9, 9),
        )],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "openai",
        on(json!({"partial_min_chars": 1, "tool_call_limit": 16})),
    );
    let body = json!({
        "system": {"tools": {"fetch": {"text": tool.to_string()}}},
        "messages": [{"origin": "user", "type": "text", "text": "look"}]
    });
    let (_, mut rx) = turn(raw, body).await;
    // The sentence before the call comes while the stream is still open.
    let first = next(&mut rx).await;
    assert_eq!(first["header"]["route"], "partial", "{first}");
    assert_eq!(first["messages"][0]["text"], "Let me check that for you.");
    p.release(0);
    let (rest, answer) = all(&mut rx).await;
    assert!(
        rest.is_empty(),
        "nothing from the arguments or the thinking: {rest:?}"
    );
    assert_eq!(answer["header"]["finish_reason"], "tool_calls", "{answer}");
    assert!(
        answer["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["type"] == "tool_call"),
        "{answer}"
    );
}

/// The captured request body of one call through the one-body mock.
async fn body_of(raw: Value, response: MockResponse, input: Value) -> (Vec<u8>, Vec<Value>) {
    let (addr, _join, captured) = start_mock_server_capturing(vec![response]).await;
    let mut raw = raw;
    let base = raw["base_url"]
        .as_str()
        .unwrap()
        .replace("BASE", &format!("http://{addr}"));
    raw["base_url"] = json!(base);
    let (_, mut rx) = turn(raw, input).await;
    let mut out = Vec::new();
    while let Some(e) = deadline::hang("the turn ends", rx.recv()).await {
        out.push(e.content);
    }
    let reqs = captured.lock().await;
    assert_eq!(reqs.len(), 1, "one call");
    (reqs[0].body.clone(), out)
}

/// Review M2: the off body as the bytes on the wire, not as a JSON value --
/// a reordered key or a changed number form moves the bytes, never the
/// value. `GH277_WRITE_GOLDEN=1` writes the golden (an explicit act).
fn raw_golden(path: &str, actual: &[u8]) {
    if std::env::var("GH277_WRITE_GOLDEN").as_deref() == Ok("1") {
        std::fs::write(path, actual).expect("write the golden");
        return;
    }
    let want = std::fs::read(path).unwrap_or_else(|e| {
        panic!(
            "missing golden {path}: {e}\nactual bytes were:\n{}",
            String::from_utf8_lossy(actual)
        )
    });
    assert!(
        want == actual,
        "{path}: the wire bytes moved\nwant: {}\nhave: {}",
        String::from_utf8_lossy(&want),
        String::from_utf8_lossy(actual)
    );
}

fn json_of(bytes: &[u8]) -> Value {
    meclaw_core::serde_json::from_slice(bytes).expect("a JSON body")
}

fn fixture(path: &str, actual: &Value) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "missing fixture {path}: {e}\nactual body was:\n{}",
            meclaw_core::serde_json::to_string_pretty(actual).unwrap()
        )
    });
    meclaw_core::serde_json::from_str(&text).expect("fixture is JSON")
}

fn sse_body(events: &[Value]) -> Vec<u8> {
    events
        .iter()
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect::<String>()
        .into_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn off_emits_nothing_and_the_wire_bytes_do_not_move() {
    // (1) chat: the legacy request of P10 A0, with the knob named `off`.
    let raw = json!({
        "provider": "openai", "model": "gpt-4o", "api_key": "test-key-a0",
        "base_url": "BASE/v1", "temperature": 0.3, "max_tokens": 512,
        "system_order": ["identity", "instructions"],
        "provider_extra": {"seed": 42},
        "http_referer": "https://example.test", "x_title": "Example App",
        "partial_unit": "off", "partial_min_chars": 1,
    });
    let input = json!({
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
    });
    let answer = json!({
        "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-4o",
        "choices": [{"index": 0, "finish_reason": "stop",
                     "message": {"role": "assistant", "content": "Fine. Thanks. Bye."}}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 3}
    });
    let (sent, out) = body_of(
        raw,
        MockResponse::ok_json(answer.to_string().as_bytes()),
        input,
    )
    .await;
    raw_golden(CHAT_BODY_RAW, &sent);
    let sent = json_of(&sent);
    assert_eq!(
        sent,
        fixture(CHAT_BODY_FIXTURE, &sent),
        "the chat body moved"
    );
    assert!(sent.get("stream").is_none(), "{sent}");
    assert_eq!(out.len(), 1, "one emission, the answer: {out:?}");
    assert!(out[0]["header"].get("route").is_none(), "{}", out[0]);

    // (2) Responses.
    let raw = params(
        "BASE/v1",
        "openai",
        json!({"wire_dialect": "responses", "partial_unit": "off"}),
    );
    let sse = sse_body(&[
        json!({"type": "response.created", "response": {"id": "r", "model": "sse-model"}}),
        json!({"type": "response.output_text.delta", "delta": "Fine. Thanks. "}),
        json!({"type": "response.output_item.done", "item": {"type": "message",
               "role": "assistant", "content": [{"type": "output_text", "text": "Fine. Thanks. "}]}}),
        json!({"type": "response.completed", "response": {"id": "r", "model": "sse-model",
               "usage": {"input_tokens": 3, "output_tokens": 2}}}),
    ]);
    let (sent, out) = body_of(
        raw,
        MockResponse {
            status: 200,
            body: sse,
            content_type: "text/event-stream".into(),
            delay: None,
        },
        ask("hello"),
    )
    .await;
    raw_golden(RESPONSES_BODY_RAW, &sent);
    let sent = json_of(&sent);
    assert_eq!(
        sent,
        fixture(RESPONSES_BODY_FIXTURE, &sent),
        "the responses body moved"
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(out[0]["header"].get("route").is_none(), "{}", out[0]);

    // (3) Messages: no `stream` key at all.
    let raw = params("BASE/v1", "anthropic", json!({"partial_unit": "off"}));
    let answer = json!({
        "id": "msg_1", "type": "message", "role": "assistant", "model": "sse-model",
        "content": [{"type": "text", "text": "Fine. Thanks. "}], "stop_reason": "end_turn",
        "usage": {"input_tokens": 3, "output_tokens": 2}
    });
    let (sent, out) = body_of(
        raw,
        MockResponse::ok_json(answer.to_string().as_bytes()),
        ask("hello"),
    )
    .await;
    raw_golden(MESSAGES_BODY_RAW, &sent);
    let sent = json_of(&sent);
    assert_eq!(
        sent,
        fixture(MESSAGES_BODY_FIXTURE, &sent),
        "the messages body moved"
    );
    assert!(sent.get("stream").is_none(), "{sent}");
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(out[0]["header"].get("route").is_none(), "{}", out[0]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anthropic_messages_stream_text_and_tool_use_under_a_strict_mock() {
    let tool = json!({"type": "function", "function": {"name": "get_weather",
                      "description": "w", "parameters": {"type": "object", "properties": {}}}});
    let mut p = mock_sse::start(
        Dialect::Messages,
        vec![Script::gated(
            vec![
                messages::start(20),
                messages::ping(),
                messages::text_start(0),
                messages::text(0, "I will check. "),
            ],
            [
                vec![
                    messages::text(0, "One moment"),
                    messages::stop(0),
                    messages::tool_start(1, "toolu_1", "get_weather"),
                    messages::tool_args(1, "{\"city\":"),
                    messages::tool_args(1, " \"Berlin. Paris. \"}"),
                    messages::stop(1),
                ],
                messages::end("tool_use", 30),
            ]
            .concat(),
        )],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "anthropic",
        on(json!({"partial_min_chars": 1})),
    );
    let body = json!({
        "system": {"tools": {"get_weather": {"text": tool.to_string()}}},
        "messages": [{"origin": "user", "type": "text", "text": "weather?"}]
    });
    let (_, mut rx) = turn(raw, body).await;
    let first = next(&mut rx).await;
    let sent = p.request().await.expect("the provider read a request");
    assert_eq!(sent.body["stream"], true, "{}", sent.body);
    assert_eq!(
        sent.headers.get("x-api-key").map(String::as_str),
        Some("k-1410")
    );
    assert_eq!(first["header"]["route"], "partial", "{first}");
    assert_eq!(first["messages"][0]["text"], "I will check.");
    p.release(0);
    let (rest, answer) = all(&mut rx).await;
    assert!(rest.is_empty(), "no sentence from the tool input: {rest:?}");
    assert_eq!(answer["header"]["finish_reason"], "tool_calls", "{answer}");
    assert_eq!(answer["header"]["tokens_prompt"], 20, "{answer}");
    assert_eq!(answer["header"]["tokens_completion"], 30, "{answer}");
    assert_eq!(answer_text(&answer), "I will check. One moment");
    let call = answer["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["type"] == "tool_call")
        .expect("the tool call")
        .clone();
    assert_eq!(call["id"], "toolu_1", "{call}");
    let function: Value =
        meclaw_core::serde_json::from_str(call["text"].as_str().unwrap()).unwrap();
    assert_eq!(function["name"], "get_weather", "{call}");
    let args: Value = match &function["arguments"] {
        Value::String(s) => meclaw_core::serde_json::from_str(s).unwrap(),
        other => other.clone(),
    };
    assert_eq!(args, json!({"city": "Berlin. Paris. "}), "{call}");

    // A stream that ends before `message_stop` is an error, never an answer;
    // the partial before it stays the only one.
    let p = mock_sse::start(
        Dialect::Messages,
        vec![Script::whole(vec![
            messages::start(20),
            messages::text_start(0),
            messages::text(0, "Half an answer. And"),
        ])],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "anthropic",
        on(json!({"partial_min_chars": 1})),
    );
    let (_, mut rx) = turn(raw, ask("weather?")).await;
    let (partials, answer) = all(&mut rx).await;
    assert_eq!(texts(&partials), vec!["Half an answer."]);
    assert_eq!(answer["header"]["finish_reason"], "error", "{answer}");
    assert_eq!(answer["header"]["error_code"], "provider_error", "{answer}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn responses_partials_arrive_before_the_stream_ends() {
    let mut p = mock_sse::start(
        Dialect::Responses,
        vec![Script::gated(
            vec![responses::created(), responses::text("Here it is. ")],
            vec![
                responses::text("All done"),
                responses::message_done("Here it is. All done"),
                responses::completed(7, 5),
            ],
        )],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "openai",
        on(json!({"wire_dialect": "responses", "partial_min_chars": 1})),
    );
    let (_, mut rx) = turn(raw, ask("go")).await;
    let first = next(&mut rx).await;
    assert_eq!(first["header"]["route"], "partial", "{first}");
    assert_eq!(first["messages"][0]["text"], "Here it is.");
    let _ = p.request().await.expect("the provider read a request");
    p.release(0);
    let (rest, answer) = all(&mut rx).await;
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(answer["header"]["finish_reason"], "stop", "{answer}");
    assert_eq!(answer_text(&answer), "Here it is. All done");

    // Cut short (no `response.completed`): the error of before.
    let p = mock_sse::start(
        Dialect::Responses,
        vec![Script::whole(vec![
            responses::created(),
            responses::text("Here it is. And"),
            responses::message_done("Here it is. And"),
        ])],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "openai",
        on(json!({"wire_dialect": "responses", "partial_min_chars": 1})),
    );
    let (_, mut rx) = turn(raw, ask("go")).await;
    let (partials, answer) = all(&mut rx).await;
    assert_eq!(texts(&partials), vec!["Here it is."]);
    assert_eq!(answer["header"]["finish_reason"], "error", "{answer}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_short_fragment_waits_for_the_next_boundary() {
    // Default `partial_min_chars` (12): "Yes." alone is no partial.
    let mut p = mock_sse::start(
        Dialect::Chat,
        vec![Script::gated(
            vec![chat::text("Yes. "), chat::text("Let me look it up. ")],
            [vec![chat::text("Done")], chat::end("stop", 3, 3)].concat(),
        )],
    )
    .await
    .expect("bind a loopback provider");
    let (_, mut rx) = turn(params(&p.base_url, "openai", on(json!({}))), ask("hi")).await;
    let first = next(&mut rx).await;
    assert_eq!(first["header"]["route"], "partial", "{first}");
    assert_eq!(first["messages"][0]["text"], "Yes. Let me look it up.");
    p.release(0);
    let (rest, answer) = all(&mut rx).await;
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(answer_text(&answer), "Yes. Let me look it up. Done");

    // An answer of short sentences only: no partial, the answer as ever.
    let p = mock_sse::start(
        Dialect::Chat,
        vec![Script::whole(
            [
                vec![chat::text("Yes. "), chat::text("Sure.")],
                chat::end("stop", 3, 3),
            ]
            .concat(),
        )],
    )
    .await
    .expect("bind a loopback provider");
    let (_, mut rx) = turn(params(&p.base_url, "openai", on(json!({}))), ask("hi")).await;
    let (partials, answer) = all(&mut rx).await;
    assert!(partials.is_empty(), "{partials:?}");
    assert_eq!(answer_text(&answer), "Yes. Sure.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_latency_line_names_the_first_token_and_the_partials() {
    let log = meclaw_testing::log_recorder::global();
    let p = mock_sse::start(
        Dialect::Chat,
        vec![Script::whole(
            [
                vec![
                    chat::text("First sentence here. "),
                    chat::text("Second one too. End"),
                ],
                chat::end("stop", 3, 3),
            ]
            .concat(),
        )],
    )
    .await
    .expect("bind a loopback provider");
    let mut raw = params(&p.base_url, "openai", on(json!({"partial_min_chars": 1})));
    raw["model"] = json!("model-gh1410-latency");
    let (_, mut rx) = turn(raw, ask("hi")).await;
    let (partials, _) = all(&mut rx).await;
    assert_eq!(partials.len(), 2);
    let lines = log.lines(
        tracing::Level::INFO,
        &[
            "llm provider call phases",
            "model=model-gh1410-latency outcome",
        ],
    );
    assert_eq!(lines.len(), 1, "{lines:?}");
    for field in [" first_token_ms=", " first_partial_ms=", " partials=2"] {
        assert!(lines[0].contains(field), "{field} in {}", lines[0]);
    }
    // Off: none of the three on the line.
    let answer = json!({
        "id": "c", "object": "chat.completion", "model": "x",
        "choices": [{"index": 0, "finish_reason": "stop",
                     "message": {"role": "assistant", "content": "Fine. Thanks."}}],
    });
    let mut raw = params("BASE/v1", "openai", json!({}));
    raw["model"] = json!("model-gh1410-latency-off");
    let _ = body_of(
        raw,
        MockResponse::ok_json(answer.to_string().as_bytes()),
        ask("hi"),
    )
    .await;
    let lines = log.lines(
        tracing::Level::INFO,
        &[
            "llm provider call phases",
            "model=model-gh1410-latency-off outcome",
        ],
    );
    assert_eq!(lines.len(), 1, "{lines:?}");
    for field in ["first_token_ms", "first_partial_ms", "partials"] {
        assert!(!lines[0].contains(field), "{field} in {}", lines[0]);
    }
}

/// Review I1 (promises 1 and 6): a `length` continuation that fails after
/// it emitted partials ends in the error message, never in an answer cut
/// short of a text the partials already handed out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_continuation_after_partials_is_an_error() {
    let p = mock_sse::start(
        Dialect::Chat,
        vec![
            Script::whole(
                [
                    vec![chat::text("First part here. Sec")],
                    chat::end("length", 5, 5),
                ]
                .concat(),
            ),
            Script::whole(vec![
                chat::text("ond part. And"),
                chat::error("upstream went away"),
            ]),
        ],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "openai",
        on(json!({"partial_min_chars": 1, "length_continuations": 1})),
    );
    let (_, mut rx) = turn(raw, ask("go")).await;
    let (partials, answer) = all(&mut rx).await;
    assert_eq!(
        texts(&partials),
        vec!["First part here.", "First part here. Second part."]
    );
    assert_eq!(answer["header"]["finish_reason"], "error", "{answer}");

    // The continuation fails before it said anything: the cut answer of
    // before, byte for byte -- nothing it handed out is missing from it.
    let p = mock_sse::start(
        Dialect::Chat,
        vec![
            Script::whole(
                [
                    vec![chat::text("First part here. Sec")],
                    chat::end("length", 5, 5),
                ]
                .concat(),
            ),
            Script::whole(vec![chat::error("upstream went away")]),
        ],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "openai",
        on(json!({"partial_min_chars": 1, "length_continuations": 1})),
    );
    let (_, mut rx) = turn(raw, ask("go")).await;
    let (partials, answer) = all(&mut rx).await;
    assert_eq!(texts(&partials), vec!["First part here."]);
    assert_eq!(answer["header"]["finish_reason"], "length", "{answer}");
    assert_eq!(answer_text(&answer), "First part here. Sec");
}

/// Review M1 (promise 6 on the chat wire): an error chunk after partials is
/// the error message, and no partial follows it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chat_error_after_partials_is_an_error() {
    let p = mock_sse::start(
        Dialect::Chat,
        vec![Script::whole(vec![
            chat::text("Half an answer. And"),
            chat::error("upstream went away"),
            chat::text(" more. Even more. "),
        ])],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(&p.base_url, "openai", on(json!({"partial_min_chars": 1})));
    let (_, mut rx) = turn(raw, ask("go")).await;
    let (partials, answer) = all(&mut rx).await;
    assert_eq!(texts(&partials), vec!["Half an answer."]);
    assert_eq!(answer["header"]["finish_reason"], "error", "{answer}");
}

/// Review M1 (promise 2 on the Responses wire): reasoning summaries and
/// call arguments are no visible text; the sentence before the call comes
/// while the stream is still open.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_responses_partial_comes_from_reasoning_or_call_arguments() {
    let tool = json!({"type": "function", "function": {"name": "fetch", "parameters": {}}});
    let mut p = mock_sse::start(
        Dialect::Responses,
        vec![Script::gated(
            vec![
                responses::created(),
                responses::reasoning("I think. Then I act. "),
                responses::text("Let me check that for you."),
                responses::call_added("call_1", "fetch"),
                responses::call_args("{\"q\": \"Sentence one. Sentence two. \"}"),
            ],
            vec![
                responses::call_done(
                    "call_1",
                    "fetch",
                    "{\"q\": \"Sentence one. Sentence two. \"}",
                ),
                responses::message_done("Let me check that for you."),
                responses::completed(7, 5),
            ],
        )],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "openai",
        on(json!({"wire_dialect": "responses", "partial_min_chars": 1})),
    );
    let body = json!({
        "system": {"tools": {"fetch": {"text": tool.to_string()}}},
        "messages": [{"origin": "user", "type": "text", "text": "look"}]
    });
    let (_, mut rx) = turn(raw, body).await;
    let first = next(&mut rx).await;
    assert_eq!(first["header"]["route"], "partial", "{first}");
    assert_eq!(first["messages"][0]["text"], "Let me check that for you.");
    let _ = p.request().await.expect("the provider read a request");
    p.release(0);
    let (rest, answer) = all(&mut rx).await;
    assert!(
        rest.is_empty(),
        "nothing from the arguments or the reasoning: {rest:?}"
    );
    assert_eq!(answer["header"]["finish_reason"], "tool_calls", "{answer}");
}

/// Review M1 (promise 2 on the Messages wire): a `thinking_delta` is no
/// visible text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_messages_partial_comes_from_thinking() {
    let p = mock_sse::start(
        Dialect::Messages,
        vec![Script::whole(
            [
                vec![
                    messages::start(20),
                    messages::thinking_start(0),
                    messages::thinking(0, "I think. Then I answer. "),
                    messages::stop(0),
                    messages::text_start(1),
                    messages::text(1, "Here it is. Done"),
                    messages::stop(1),
                ],
                messages::end("end_turn", 30),
            ]
            .concat(),
        )],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "anthropic",
        on(json!({"partial_min_chars": 1})),
    );
    let (_, mut rx) = turn(raw, ask("go")).await;
    let (partials, answer) = all(&mut rx).await;
    assert_eq!(texts(&partials), vec!["Here it is."]);
    assert_eq!(answer["header"]["finish_reason"], "stop", "{answer}");
    assert_eq!(answer_text(&answer), "Here it is. Done");
}

/// Re-check (strict mock): with a reasoning effort the cell sends
/// `output_config`, a key the Messages API takes -- the strict mock lets the
/// request through and the stream answers as without it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reasoning_effort_passes_the_strict_messages_mock() {
    let mut p = mock_sse::start(
        Dialect::Messages,
        vec![Script::whole(
            [
                vec![
                    messages::start(20),
                    messages::text_start(0),
                    messages::text(0, "Here it is. Done"),
                    messages::stop(0),
                ],
                messages::end("end_turn", 30),
            ]
            .concat(),
        )],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "anthropic",
        on(json!({"partial_min_chars": 1, "reasoning_effort": "medium"})),
    );
    let (_, mut rx) = turn(raw, ask("go")).await;
    let (partials, answer) = all(&mut rx).await;
    let seen = p.request().await.expect("the request");
    assert_eq!(
        seen.body["output_config"]["effort"], "medium",
        "{}",
        seen.body
    );
    assert_eq!(answer["header"]["finish_reason"], "stop", "{answer}");
    assert_eq!(texts(&partials), vec!["Here it is."]);
    assert_eq!(answer_text(&answer), "Here it is. Done");
}

/// Review M1 (promise 6 on the Messages wire): an `error` event in the
/// middle of the stream is the error message, and no partial follows it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_anthropic_error_event_mid_stream_is_an_error() {
    let p = mock_sse::start(
        Dialect::Messages,
        vec![Script::whole(vec![
            messages::start(20),
            messages::text_start(0),
            messages::text(0, "Half an answer. And"),
            messages::error("overloaded_error", "Overloaded"),
            messages::text(0, " more. Even more. "),
        ])],
    )
    .await
    .expect("bind a loopback provider");
    let raw = params(
        &p.base_url,
        "anthropic",
        on(json!({"partial_min_chars": 1})),
    );
    let (_, mut rx) = turn(raw, ask("go")).await;
    let (partials, answer) = all(&mut rx).await;
    assert_eq!(texts(&partials), vec!["Half an answer."]);
    assert_eq!(answer["header"]["finish_reason"], "error", "{answer}");
}

/// Review M3: the mock refuses what the providers refuse -- an unknown top
/// level key on Messages, a chat or Responses call without a bearer, any
/// method but POST.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_strict_mock_refuses_what_the_providers_refuse() {
    let client = reqwest::Client::new();
    let ok_messages = json!({"model": "m", "max_tokens": 8, "stream": true,
                             "messages": [{"role": "user", "content": "hi"}]});
    let cases: Vec<(Dialect, &str, &str, Value, bool, &str)> = vec![
        (
            Dialect::Messages,
            "POST",
            "/messages",
            {
                let mut b = ok_messages.clone();
                b["stream_options"] = json!({"include_usage": true});
                b
            },
            true,
            "stream_options",
        ),
        (
            Dialect::Chat,
            "POST",
            "/chat/completions",
            json!({"model": "m", "stream": true, "stream_options": {"include_usage": true},
                   "messages": [{"role": "user", "content": "hi"}]}),
            false,
            "authorization",
        ),
        (
            Dialect::Responses,
            "POST",
            "/responses",
            json!({"model": "m", "stream": true, "input": "hi"}),
            false,
            "authorization",
        ),
        (
            Dialect::Messages,
            "PUT",
            "/messages",
            ok_messages.clone(),
            true,
            "POST",
        ),
    ];
    for (dialect, method, path, body, keyed, rule) in cases {
        let mut p = mock_sse::start(dialect, vec![Script::whole(vec![])])
            .await
            .expect("bind a loopback provider");
        let mut req = client
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).expect("a method"),
                format!("{}{path}", p.base_url),
            )
            .json(&body);
        if keyed {
            req = req
                .header("x-api-key", "k")
                .header("anthropic-version", "2023-06-01");
        }
        let resp = deadline::hang("the mock answers", req.send())
            .await
            .expect("the mock answers");
        let status = resp.status().as_u16();
        let text = deadline::hang("the mock body", resp.text())
            .await
            .expect("a body");
        assert_eq!(status, 400, "{method} {path} ({rule}): {text}");
        assert!(text.contains(rule), "{method} {path}: {text}");
        let _ = p.request().await;
    }
}
