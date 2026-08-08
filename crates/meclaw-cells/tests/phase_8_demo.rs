//! Phase-8 Demo-Topologie tests — production-bootstrap-spawn of `llm`-Cell
//! against MockOpenAI. T27-T31 cover the 5 demo-cases from Plan § 11.
//!
//! T27: `demo_e2e_text_completion` — full end-to-end happy path. Probes the
//! production spawn pipeline (`bootstrap_from_filesystem` → `LlmCellFactory::
//! spawn_cell` → `cell_task_stateful`) and the UBF↔OpenAI translate-edge in
//! both directions. T28-T31 will append to this file using the same Mock-first
//! → TempDir-config → bootstrap order (Plan § 27 K-c).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{json, to_string_pretty};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use mock_openai::{MockOpenAI, canned_chat_completion, canned_error_status, canned_tool_calls};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_e2e_text_completion() {
    // 1. Mock first — bind 127.0.0.1:0, grab the addr → base_url. The
    //    `LlmCell`'s `params.base_url` will point at this in the config.json
    //    written in step 2 (Plan § 27 K-c: Mock-Port-vor-Config-Schreiben).
    let mock = MockOpenAI::start(vec![canned_chat_completion("Hello there!", "stop")]).await;
    let base_url = format!("{}/v1", mock.base_url);

    // 2. TempDir tree:
    //      <td>/main/config.json        (hive scope, empty edges)
    //      <td>/main/llm/config.json    (cell_type="llm", base_url=mock)
    //    `/main` is the single top-level dir under <td> — satisfies
    //    `assert_single_root_dir`. `/sink` is registered via h.spawn (NOT
    //    in the FS tree) so it has no factory requirement.
    let td = tempfile::TempDir::new().unwrap();
    let main_dir = td.path().join("main");
    let llm_dir = main_dir.join("llm");
    std::fs::create_dir_all(&llm_dir).unwrap();
    std::fs::write(
        main_dir.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    let llm_config = json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai",
            "model": "gpt-4o",
            "api_key": "test-key",
            "base_url": base_url,
            "system_order": ["identity", "facts"]
        },
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    });
    std::fs::write(
        llm_dir.join("config.json"),
        to_string_pretty(&llm_config).unwrap(),
    )
    .unwrap();

    // 3. ColonyHandle with LlmCellFactory under name "llm". The Arc is shared
    //    between Colony's runtime (for the Respawn-Pfad) and the registry
    //    passed to `bootstrap_from_filesystem` (for the initial spawn).
    let factory: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("llm".to_string(), factory.clone())]);

    // 4. /sink BEFORE bootstrap (anti-cascade, Phase-6.5 lesson): the
    //    `reply_to=/sink` on the probe must resolve at first emission.
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // 5. bootstrap_from_filesystem — production spawn path for /main/llm.
    //    Eagerly opens cell.db under <td>/main/llm/ + starts cell_task_stateful.
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    // W2 (A1): /llm reply to /sink now needs a wired edge (identity gone).
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/llm"),
        Path::new("/sink"),
    )
    .await;

    // 6. Probe: input UBF carries identity-soul + facts-user_name (system tree)
    //    plus a single user turn. `reply_to = /sink` routes the cell's
    //    assistant-turn emission back to the CaptureCell.
    //    Target = `/llm` — `plan_bootstrap` maps `<td>/main` (root) → `/`, so
    //    the cell at `<td>/main/llm/` registers under `/llm`. Same pattern
    //    as phase_7_5_demo's T4 (`/persist`, not `/main/persist`).
    let probe = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({
            "system": {
                "identity": {"soul": {"text": "You are the assistant."}},
                "facts":    {"user_name": {"text": "Ada"}}
            },
            "messages": [{"origin":"user","type":"text","text":"Hi"}]
        })))
        .build();
    h.send(probe).await;

    // 7. Await the sink emission.
    let em = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("emit timed out")
        .expect("sink channel closed");

    // 8. ASSERT mock received the request with the right UBF→OpenAI shape.
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "mock must receive exactly 1 request");
    let req = &snaps[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/v1/chat/completions");
    assert_eq!(req.model(), Some("gpt-4o"));
    assert_eq!(req.temperature(), Some(0.7));
    let messages = req.messages().expect("body has messages[]");
    assert_eq!(messages.len(), 2, "leading system + user turn expected");
    // system_order = ["identity", "facts"] → identity-text "You are the assistant."
    // first, then facts-text "Ada", joined with "\n\n".
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "You are the assistant.\n\nAda");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "Hi");

    // 9. ASSERT cell emitted assistant-turn at /sink with the right shape.
    //    Colony's `split_content_header` promotes `content.header.*` keys
    //    onto `message.headers` and strips `header` from the body — same
    //    surface as Phase-7.5-demo's `m.headers.hop["counter"]` read.
    assert_eq!(em.target, Path::new("/sink"));
    let body = match &em.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("expected Body::Inline, got Body::Blob"),
    };
    assert_eq!(body["messages"][0]["origin"], "assistant");
    assert_eq!(body["messages"][0]["type"], "text");
    assert_eq!(body["messages"][0]["text"], "Hello there!");
    assert_eq!(em.headers.hop["finish_reason"], "stop");
    assert_eq!(em.headers.hop["tokens_prompt"], 10);
    assert_eq!(em.headers.hop["tokens_completion"], 5);
    assert_eq!(em.headers.hop["model"], "gpt-4o-mock");
    assert_eq!(body["meta"]["provider"], "openai");
    assert_eq!(body["meta"]["model"], "gpt-4o-mock");
    assert!(
        body["meta"]["response_id"].as_str().is_some(),
        "meta.response_id must be set: {}",
        body["meta"]
    );
    assert!(
        body["meta"]["latency_ms"].as_u64().unwrap() < 5000,
        "meta.latency_ms must be < 5s: {}",
        body["meta"]["latency_ms"]
    );

    h.shutdown().await;
}

/// T28: `demo_e2e_tool_call` — second demo in the Phase-8 series. Input UBF
/// carries an OpenAI tool-schema under `system.tools.calculator.text` and
/// asks for the tool to be called; mock returns a `tool_calls`-finish_reason
/// response with one `call-xyz`/`calc` tool-call. Verifies the tools-extraction
/// edge (system.tools.* → request `tools[]`, NOT concatenated into the
/// system-message-string) and the inverse: response `tool_calls[]` → UBF
/// `{origin:"assistant", type:"tool_call", id, text}` turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_e2e_tool_call() {
    // 1. Mock first — single canned tool-call response with id "call-xyz",
    //    function "calc", arguments `{"x":2,"y":3}`.
    let mock = MockOpenAI::start(vec![canned_tool_calls(vec![(
        "call-xyz",
        "calc",
        r#"{"x":2,"y":3}"#,
    )])])
    .await;
    let base_url = format!("{}/v1", mock.base_url);

    // 2. Same TempDir/FS-fixture layout as T27 — `/main` (hive) → `/main/llm`
    //    (cell). `plan_bootstrap` maps `<td>/main` → `/`, so the cell
    //    registers under `/llm`.
    let td = tempfile::TempDir::new().unwrap();
    let main_dir = td.path().join("main");
    let llm_dir = main_dir.join("llm");
    std::fs::create_dir_all(&llm_dir).unwrap();
    std::fs::write(
        main_dir.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    let llm_config = json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai",
            "model": "gpt-4o",
            "api_key": "test-key",
            "base_url": base_url
        },
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    });
    std::fs::write(
        llm_dir.join("config.json"),
        to_string_pretty(&llm_config).unwrap(),
    )
    .unwrap();

    // 3. Colony + factory wiring (same Arc shared between runtime and bootstrap registry).
    let factory: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("llm".to_string(), factory.clone())]);

    // 4. /sink BEFORE bootstrap (anti-cascade, Phase-6.5 lesson).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // 5. bootstrap_from_filesystem — production spawn path.
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    // W2 (A1): /llm reply to /sink now needs a wired edge (identity gone).
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/llm"),
        Path::new("/sink"),
    )
    .await;

    // 6. Probe: input UBF carries `system.tools.calculator.text = "<schema-json>"`
    //    only — no identity/facts/etc. The schema-string is the OpenAI tool-object
    //    (`{type:"function", function:{name:"calc", ...}}`). The user-turn asks
    //    for the tool to be called.
    let tool_schema = json!({
        "type": "function",
        "function": {
            "name": "calc",
            "description": "calc x+y",
            "parameters": {
                "type": "object",
                "properties": {
                    "x": {"type": "number"},
                    "y": {"type": "number"}
                }
            }
        }
    });
    let tool_schema_str = meclaw_core::serde_json::to_string(&tool_schema).unwrap();
    let probe = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({
            "system": {"tools": {"calculator": {"text": tool_schema_str}}},
            "messages": [{"origin":"user","type":"text","text":"Was ist 2+3?"}]
        })))
        .build();
    h.send(probe).await;

    // 7. Await the sink emission.
    let em = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("emit timed out")
        .expect("sink channel closed");

    // 8. ASSERT 1: mock received the request — tools[] extracted from
    //    system.tools.*, NOT concatenated into the system-message-string.
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "mock must receive exactly 1 request");
    let req = &snaps[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/v1/chat/completions");
    let tools = req.tools().expect("request must carry tools[] array");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["function"]["name"], "calc");
    // The input's only system-slot is `tools`, which `concat_system_prompt`
    // skips (Plan § 5). Therefore the request must have NO leading
    // system-message at all — `messages[0]` is the user-turn directly.
    let req_messages = req.messages().expect("body has messages[]");
    assert_eq!(
        req_messages[0]["role"], "user",
        "no leading system-message when system has only tools (concat_system_prompt skips tools sub-slot)"
    );
    assert_eq!(req_messages[0]["content"], "Was ist 2+3?");

    // 9. ASSERT 2: cell emits UBF tool_call turn with id pass-through.
    assert_eq!(em.target, Path::new("/sink"));
    let body = match &em.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("expected Body::Inline, got Body::Blob"),
    };
    assert_eq!(body["messages"][0]["origin"], "assistant");
    assert_eq!(body["messages"][0]["type"], "tool_call");
    assert_eq!(body["messages"][0]["id"], "call-xyz");
    // The text is the JSON-stringified `function`-object from the mock —
    // contains the tool name and the arguments-string.
    let text = body["messages"][0]["text"]
        .as_str()
        .expect("tool_call.text must be a string");
    let parsed: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(text).expect("tool_call.text must be JSON");
    assert_eq!(parsed["name"], "calc");
    assert_eq!(parsed["arguments"], r#"{"x":2,"y":3}"#);

    // 10. ASSERT 3: finish_reason promoted onto message headers.
    assert_eq!(em.headers.hop["finish_reason"], "tool_calls");

    h.shutdown().await;
}

/// T29: `demo_error_rate_limit` — the explicit critical demo. Full
/// production-bootstrap-spawn pipeline (TempDir + `/main/llm` config →
/// `ColonyHandle.spawn /sink` → `bootstrap_from_filesystem` → probe to
/// `/llm`) with the mock returning HTTP 429.
///
/// Proves three things end-to-end:
///   1. `WireError::RateLimited` → `wire_error_to_code` mapping ("rate_limit")
///      propagates through `handle()`'s error-branch (T22 unit-level → T29
///      bootstrap-level).
///   2. **GATE-1 ASYMMETRY (critical)**: `output.messages` is BYTE-IDENTICAL
///      to `input.messages` on the error-path. No assistant-turn appended,
///      no mutation, no empty array. This is what makes a Failover-Edge on
///      `finish_reason="error"` work: a Backup-`llm`-Cell receiving the
///      forwarded error-message gets the original conversation to retry.
///   3. Error-meta shape: `provider="openai"`, `error.source="wire"`,
///      `started_at > 0`. `model`/`response_id` absent (no Provider response).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_error_rate_limit() {
    // 1. Mock first — single 429 response.
    let mock = MockOpenAI::start(vec![canned_error_status(429)]).await;
    let base_url = format!("{}/v1", mock.base_url);

    // 2. Same TempDir/FS-fixture layout as T27/T28 — `/main` (hive) →
    //    `/main/llm` (cell). `plan_bootstrap` maps `<td>/main` → `/`, so the
    //    cell registers under `/llm`.
    let td = tempfile::TempDir::new().unwrap();
    let main_dir = td.path().join("main");
    let llm_dir = main_dir.join("llm");
    std::fs::create_dir_all(&llm_dir).unwrap();
    std::fs::write(
        main_dir.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    let llm_config = json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai",
            "model": "gpt-4o",
            "api_key": "test-key",
            "base_url": base_url
        },
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    });
    std::fs::write(
        llm_dir.join("config.json"),
        to_string_pretty(&llm_config).unwrap(),
    )
    .unwrap();

    // 3. Colony + factory wiring (same Arc shared between runtime and bootstrap registry).
    let factory: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("llm".to_string(), factory.clone())]);

    // 4. /sink BEFORE bootstrap (anti-cascade, Phase-6.5 lesson).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // 5. bootstrap_from_filesystem — production spawn path.
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    // W2 (A1): /llm reply to /sink now needs a wired edge (identity gone).
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/llm"),
        Path::new("/sink"),
    )
    .await;

    // 6. Probe: the user-turn we'll verify is passed through UNCHANGED on the
    //    error path. Cloned into `input_messages` so we can byte-compare
    //    against the emitted `output.messages` array later.
    let user_turn = json!({"origin":"user","type":"text","text":"Hi"});
    let input_messages = vec![user_turn.clone()];
    let probe = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({
            "messages": input_messages.clone()
        })))
        .build();
    h.send(probe).await;

    // 7. Await the sink emission.
    let em = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("emit timed out")
        .expect("sink channel closed");

    // ─── Plan § 11.3 final fix-set ───

    // 8. Header: `finish_reason="error"`, `error_code="rate_limit"`. The
    //    mapping comes from `wire_error_to_code(WireError::RateLimited)`
    //    (T8) traversing handle()'s error-branch (T22) through Colony's
    //    `split_content_header` promotion onto `message.headers`.
    assert_eq!(em.target, Path::new("/sink"));
    assert_eq!(em.headers.hop["finish_reason"], "error", "must emit error");
    assert_eq!(
        em.headers.hop["error_code"], "rate_limit",
        "429 must map to rate_limit"
    );

    // 9. Meta-shape on error: `error.source="wire"`, `provider="openai"`,
    //    `started_at > 0`. `model` and `response_id` are absent because the
    //    Provider never returned a usable response.
    let body = match &em.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("expected Body::Inline, got Body::Blob"),
    };
    assert_eq!(body["meta"]["error"]["source"], "wire");
    assert_eq!(body["meta"]["provider"], "openai");
    assert!(
        body["meta"]["started_at"].as_i64().unwrap() > 0,
        "meta.started_at must be > 0: {}",
        body["meta"]
    );

    // ─── GATE-1 (the critical assert): output.messages == input.messages ───
    // The user-turn is UNCHANGED in the output body. No assistant-turn
    // appended, no empty array, no mutation. Concretely: one-element array
    // containing the original user-turn, byte-equal.
    let output_messages = body["messages"].as_array().expect("messages must be array");
    assert_eq!(
        output_messages.len(),
        1,
        "Gate-1: output.messages must be exactly the input — no assistant-turn appended on error. \
         Got {} messages: {:?}",
        output_messages.len(),
        output_messages
    );
    assert_eq!(
        output_messages[0], user_turn,
        "Gate-1: output.messages[0] must be the unchanged user-turn (Failover-Edge-Compatibility)"
    );

    // Bonus deep-check: the whole messages-array is structurally identical to
    // input. Catches any reordering, key-mutation, or array-slot-shuffling
    // that an element-wise check might miss.
    assert_eq!(
        body["messages"],
        meclaw_core::serde_json::Value::Array(input_messages.clone()),
        "Gate-1: messages array must be byte-identical to input.messages"
    );

    // 10. Sanity-check: the mock actually received the request (proves we
    //     went through the full wire-path before failing — not a short-circuit
    //     somewhere before `call_openai`).
    let snaps = mock.recorded_requests().await;
    assert_eq!(
        snaps.len(),
        1,
        "mock must have received 1 request (which returned 429)"
    );

    h.shutdown().await;
}

/// T30: `demo_system_only_no_emit` — the second explicitly named critical
/// demo (Plan § 11.4 + § 16). Full production-bootstrap-spawn pipeline
/// (TempDir + `/main/llm` config → `ColonyHandle.spawn /sink` →
/// `bootstrap_from_filesystem` → probe to `/llm`) with input that carries
/// ONLY `system.*` and NO `messages` slot.
///
/// Proves three things end-to-end:
///   1. **Q3 system-only-Schweigen** (handle() Schritt-4 early-return):
///      `/sink` receives NOTHING within 500ms. No assistant-turn, no
///      inference, no Provider-call. Verified via `tokio::time::timeout`
///      on `sink_rx.recv()` returning `Err`.
///   2. **DB-Probe-Tiefe** (Plan § 11.4 "tief"): direct
///      `rusqlite::Connection::open` on the cell.db file confirms the
///      new leaf row landed at `system.facts.x` with the UBF-leaf-JSON
///      byte-equal to input (`{"text":"v"}`).
///   3. **No `last_input` write**: T15 `system_first_persist` with `None`
///      for messages skips the last_input INSERT entirely → COUNT == 0.
///
/// **Defense-in-depth**: `base_url = "http://127.0.0.1:1/v1"` is a sentinel
/// unreachable URL. If `handle()` accidentally reached Schritt-5/6 (i.e. Q3
/// regression), the wire call would either timeout or emit a wire-error;
/// both fail the silence-assert below. So this test also proves "system-only
/// MUST NOT call the Provider".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_system_only_no_emit() {
    // 1. TempDir tree (same layout as T27/T28/T29). NO MockOpenAI — the cell
    //    never reaches the wire for system-only input. base_url points at
    //    Port 1 (unreachable) as sentinel: any accidental wire-call would
    //    fail the silence-assert below.
    let td = tempfile::TempDir::new().unwrap();
    let main_dir = td.path().join("main");
    let llm_dir = main_dir.join("llm");
    std::fs::create_dir_all(&llm_dir).unwrap();
    std::fs::write(
        main_dir.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    let llm_config = json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai",
            "model": "gpt-4o",
            "api_key": "test-key",
            "base_url": "http://127.0.0.1:1/v1"
        },"contract":{"version":"0.1.0","settings":{},"consumes":{}}});
    std::fs::write(
        llm_dir.join("config.json"),
        to_string_pretty(&llm_config).unwrap(),
    )
    .unwrap();

    // 2. Colony + factory wiring (same Arc shared between runtime and bootstrap registry).
    let factory: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("llm".to_string(), factory.clone())]);

    // 3. /sink BEFORE bootstrap (anti-cascade, Phase-6.5 lesson).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // 4. bootstrap_from_filesystem — production spawn path.
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    // W2 (A1): /llm reply to /sink now needs a wired edge (identity gone).
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/llm"),
        Path::new("/sink"),
    )
    .await;

    // 5. Probe: system-only, NO messages slot. Q3 → cell persists + returns
    //    silently (handle() Schritt-4 early-return).
    let probe = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({
            "system": {"facts": {"x": {"text": "v"}}}
            // NO messages slot.
        })))
        .build();
    h.send(probe).await;

    // ─── ASSERT 1 (deep-watch requirement): Sink receives NOTHING within 500ms ───
    // Q3 silence: handle() returns after Schritt-2 persist, no inference,
    // no emit. `tokio::time::timeout(...)` returns `Err` on timeout —
    // exactly what we want (no message arrived).
    let sink_result = tokio::time::timeout(Duration::from_millis(500), sink_rx.recv()).await;
    assert!(
        sink_result.is_err(),
        "Q3 silence: Sink MUST NOT receive any emission for system-only input. Got: {sink_result:?}"
    );

    // 6. Sync-on-DB-commit BEFORE direct rusqlite probe: the cell-task must
    //    have committed the persist transaction. `wait_for_cell_db_value`
    //    polls via fresh read-only Connection per iteration (Phase-7.5
    //    helper). The "v" in `{"text":"v"}` is the inner UBF-leaf-JSON-string;
    //    the DB stores the FULL leaf-JSON: `{"text":"v"}`.
    let cell_dir = td.path().join("main").join("llm");
    meclaw_testing::wait::wait_for_cell_db_value(
        &cell_dir,
        "facts.x",
        r#"{"text":"v"}"#,
        Duration::from_secs(5),
    )
    .await;

    // ─── ASSERT 2 (Plan § 11.4 "tief"): direct rusqlite probe on cell.db ───
    // Fresh Connection on the cell.db file. This is the "tief" part: not
    // via the cell, not via the test-helper — straight off disk.
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db"))
        .expect("open cell.db direct via rusqlite");
    let v: String = conn
        .query_row(
            "SELECT value FROM system WHERE slot_path='facts.x'",
            [],
            |r| r.get(0),
        )
        .expect("system['facts.x'] row must exist");
    assert_eq!(
        v, r#"{"text":"v"}"#,
        "cell.db.system['facts.x'] must contain the UBF-leaf-JSON byte-equal to input"
    );

    // ─── ASSERT 3: last_input is untouched ───
    // T15 `system_first_persist` with `None` for messages skips the
    // last_input INSERT entirely. COUNT == 0 proves it.
    let li_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM last_input", [], |r| r.get(0))
        .expect("SELECT COUNT(*) FROM last_input");
    assert_eq!(
        li_count, 0,
        "system-only input must NOT touch last_input (T15 system_first_persist with None messages skips the write)"
    );

    h.shutdown().await;
}

/// T31: `demo_a_timeout` — the last Phase-8 demo. Full production-bootstrap-
/// spawn pipeline (TempDir + `/main/llm` config → `ColonyHandle.spawn /sink` →
/// `bootstrap_from_filesystem` → probe to `/llm`) with the mock holding back
/// its response by 5 seconds while `params.external_timeout_ms = 200`.
///
/// Proves four things end-to-end:
///   1. **A-Timeout (`params.external_timeout_ms`) fires in the production
///      spawn-path**: the cell's `call_openai` wraps the HTTP-future in
///      `tokio::time::timeout(200ms, ...)` and drops it before the mock's
///      5s delay completes. A successful error-emit received within 4s — below
///      the mock's 5s delay — is the externally observable proof: the mock
///      cannot answer before 5s regardless of load, so anything arriving sooner
///      can only be the A-Timeout error-path (no load-sensitive wallclock
///      threshold needed — Test-Hygiene 2026-06-04, Präzedenz e17803a).
///   2. **Wire-error mapping**: `WireError::Timeout → "timeout"` (via
///      `wire_error_to_code`, T8) propagates through `handle()`'s error-branch
///      (T22's mapping verified end-to-end on a fresh error path).
///   3. **Gate-1 pass-through on timeout**: another verification of the
///      Failover-Edge-Compatibility on a different error path (T29 covered
///      `rate_limit`; T31 covers `timeout`). `output.messages` is byte-equal
///      to `input.messages`.
///   4. **`MockResponse::with_delay` works end-to-end with the LlmCell+wire
///      stack** (T9 verified this at the mock_http-level; T31 confirms it
///      through the full Colony→Cell→wire chain).
///
/// **Defensive layering**: B-Backstop (`cell.message_timeout`) is NOT wired
/// per Phase 7.5 (deferred substrate-pass). The only thing protecting from a
/// runaway `handle()` is the A-Timeout in `call_openai`. If A-Timeout fails
/// to fire, the outer `tokio::time::timeout(4s, sink_rx.recv())` (deliberately
/// below the mock's 5s delay) will yield a useful "emit timed out" rather than
/// letting the test hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_a_timeout() {
    // 1. Mock first — single canned chat-completion BUT with a 5-second delay
    //    before sending. The cell's A-Timeout (200ms) MUST fire long before
    //    this response would arrive.
    let slow_response =
        canned_chat_completion("late response", "stop").with_delay(Duration::from_secs(5));
    let mock = MockOpenAI::start(vec![slow_response]).await;
    let base_url = format!("{}/v1", mock.base_url);

    // 2. Same TempDir/FS-fixture layout as T27/T28/T29/T30 — `/main` (hive)
    //    → `/main/llm` (cell). `plan_bootstrap` maps `<td>/main` → `/`, so
    //    the cell registers under `/llm`. Critical: `external_timeout_ms=200`
    //    → A-Timeout fires ~200ms, way before mock's 5s.
    let td = tempfile::TempDir::new().unwrap();
    let main_dir = td.path().join("main");
    let llm_dir = main_dir.join("llm");
    std::fs::create_dir_all(&llm_dir).unwrap();
    std::fs::write(
        main_dir.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    let llm_config = json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai",
            "model": "gpt-4o",
            "api_key": "test-key",
            "base_url": base_url,
            "external_timeout_ms": 200
        },
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    });
    std::fs::write(
        llm_dir.join("config.json"),
        to_string_pretty(&llm_config).unwrap(),
    )
    .unwrap();

    // 3. Colony + factory wiring (same Arc shared between runtime and bootstrap registry).
    let factory: Arc<dyn CellFactory> = Arc::new(LlmCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("llm".to_string(), factory.clone())]);

    // 4. /sink BEFORE bootstrap (anti-cascade, Phase-6.5 lesson).
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // 5. bootstrap_from_filesystem — production spawn path.
    let mut registry = CellFactoryRegistry::new();
    registry.insert("llm".to_string(), factory);
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    // W2 (A1): /llm reply to /sink now needs a wired edge (identity gone).
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/llm"),
        Path::new("/sink"),
    )
    .await;

    // 6. Probe: single user-turn, byte-checked against the emitted output later
    //    (Gate-1 pass-through on the timeout error-path).
    let user_turn = json!({"origin":"user","type":"text","text":"Hi"});
    let probe = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({
            "messages": [user_turn.clone()]
        })))
        .build();

    // 7. Send. The discriminator between "A-Timeout fired" and "didn't" is the
    //    recv-timeout placed BETWEEN the A-Timeout (200ms) and the mock's 5s
    //    delay. The old test added a separate `elapsed < 1s` wallclock assert,
    //    which flaked under Workspace-Last (cross-process Tokio-Scheduler-Druck
    //    schob den ~200ms-Pfad über 1s, ohne dass die A-Timeout-Semantik verletzt
    //    war). Dropped: `error_code=="timeout"` below + the sub-5s recv-timeout
    //    already prove the A-Timeout-path airtight without a tight threshold.
    h.send(probe).await;

    // 8. Outer 4s defensive timeout (deliberately < mock's 5s delay): if the
    //    A-Timeout failed to fire, the cell would block on the mock's 5s response
    //    → this fires first with a useful "emit timed out". A successful recv can
    //    therefore only be the A-Timeout error-emit (mock can't answer before 5s,
    //    load-independent: it's a fixed `tokio::time::sleep` in the mock).
    let em = tokio::time::timeout(Duration::from_secs(4), sink_rx.recv())
        .await
        .expect("emit timed out (cell's A-timeout didn't fire? would have waited for mock's 5s)")
        .expect("sink channel closed");

    // ─── Header assertions ───
    assert_eq!(em.target, Path::new("/sink"));
    assert_eq!(em.headers.hop["finish_reason"], "error", "must emit error");
    assert_eq!(
        em.headers.hop["error_code"], "timeout",
        "A-Timeout must map to error_code=\"timeout\" via wire_error_to_code"
    );

    // ─── Body / meta assertions ───
    let body = match &em.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("expected Body::Inline, got Body::Blob"),
    };
    assert_eq!(body["meta"]["error"]["source"], "wire");
    assert_eq!(body["meta"]["provider"], "openai");

    // ─── GATE-1 pass-through on timeout: output.messages == [user_turn] ───
    let output_messages = body["messages"].as_array().expect("messages must be array");
    assert_eq!(
        output_messages.len(),
        1,
        "Gate-1: output.messages must be exactly the input — no assistant-turn appended on timeout. \
         Got {} messages: {:?}",
        output_messages.len(),
        output_messages
    );
    assert_eq!(
        output_messages[0], user_turn,
        "Gate-1: output.messages[0] must be the unchanged user-turn (Failover-Edge-Compatibility)"
    );

    h.shutdown().await;
}
