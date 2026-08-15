//! Phase-14-C Lexical-RAG-Topologie — TDD-Tests.
//! Slice 14-C-1: `retrieve`-Cell isoliert, Keyword-Overlap-Ranking deterministisch.
//! Slice 14-C-2: `rag_question` travels as context through the store retrieval hop.
//! Slice 14-C-3: hop decays / an explicit promotion survives (full chain, llm mock).
//! Slice 14-C-5: Mutations-Validator akzeptiert RAG-Topologie (Positiv + Negativ).
//! Slice 14-C-6: live-graph SVG/DOT from the booted RAG topology.

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support_14b.rs"]
mod support;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{
    BootstrapError, CellFactory, CellFactoryRegistry, RegistryOverlay, bootstrap_from_filesystem,
    plan_bootstrap,
};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Minimal boot: only the `code` factory + the /sink CaptureCell + bootstrap over `dir`.
async fn boot_code_only(td: &TempDir) -> (ColonyHandle, mpsc::Receiver<meclaw_core::Message>) {
    let h = ColonyHandle::new_with_factories_at(
        td,
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )],
    );
    let (sink_tx, sink_rx) = mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("code".to_string(), Arc::new(CodeCellFactory));
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

/// Boot: `code` + `store` factories + the /sink CaptureCell + bootstrap over `dir`.
async fn boot_code_and_store(td: &TempDir) -> (ColonyHandle, mpsc::Receiver<meclaw_core::Message>) {
    let h = ColonyHandle::new_with_factories_at(
        td,
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            (
                "store".to_string(),
                Arc::new(StoreCellFactory) as Arc<dyn CellFactory>,
            ),
        ],
    );
    let (sink_tx, sink_rx) = mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("code".to_string(), Arc::new(CodeCellFactory));
    registry.insert("store".to_string(), Arc::new(StoreCellFactory));
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

/// Repo-root-relative path to the checked-in 14c-rag example tree.
fn example_dir_14c() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/14c-rag")
}

/// Copies the 14c-rag tree into `dst` (recursively, without SVG/DOT artefacts).
fn copy_14c_tree(dst: &std::path::Path) {
    copy_dir_recursive_14c(&example_dir_14c(), dst);
}

fn copy_dir_recursive_14c(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".dot") || name.ends_with(".svg") {
            continue;
        }
        if from.is_dir() {
            copy_dir_recursive_14c(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Bounded recv (30 s — robust against cargo's parallel load).
async fn recv_bounded(
    rx: &mut mpsc::Receiver<meclaw_core::Message>,
) -> Option<meclaw_core::Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Injected corpus: a store-select result for 4 rows as a JSON string in the
/// tool_result. The format matches what the store cell emits for
/// `select * from corpus`.
fn corpus_tool_result_body() -> Value {
    let rows = json!([
        {"doc_id": "d1", "text": "Cats are small domesticated felines that purr."},
        {"doc_id": "d2", "text": "Dogs are loyal canines that bark and fetch."},
        {"doc_id": "d3", "text": "The Eiffel Tower is a landmark in Paris France."},
        {"doc_id": "d4", "text": "Python is a programming language used for scripting."}
    ]);
    json!({
        "messages": [{
            "origin": "tool",
            "type": "tool_result",
            "text": meclaw_core::serde_json::to_string(&rows).unwrap(),
            "id": "sel-1"
        }]
    })
}

/// Builds the minimal topology directories for slice 14-C-1 in `dir`.
///
/// Creates:
/// - `main/config.json`  (hive, edge retrieve→/sink)
/// - `main/retrieve/config.json`  (code, `script_inline` from `script`)
fn build_minimal_topology(dir: &std::path::Path, script: &str) {
    let main_dir = dir.join("main");
    std::fs::create_dir_all(&main_dir).unwrap();
    meclaw_core::serde_json::to_writer_pretty(
        std::fs::File::create(main_dir.join("config.json")).unwrap(),
        &json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                {"from": "./retrieve", "to": "/sink"}
            ]}}
        }),
    )
    .unwrap();

    let retrieve_dir = main_dir.join("retrieve");
    std::fs::create_dir_all(&retrieve_dir).unwrap();
    meclaw_core::serde_json::to_writer_pretty(
        std::fs::File::create(retrieve_dir.join("config.json")).unwrap(),
        &json!({
            "cell": {"type": "code"},
            "params": {
                "runner": "python3",
                "script_inline": script,
                "external_timeout_ms": 10000
            },
            "contract": {
                "version": "0.1.0",
                "settings": {},
                "emits": {
                    "hop": {
                        "query":  {"type": "string",  "required": false},
                        "scores": {"type": "array",   "required": false},
                        "top_k":  {"type": "number",  "required": false}
                    },
                    "body": {"messages": {"type": "array"}}
                },
                "consumes": {
                    "body":    {"messages": {"type": "array", "required": true}},
                    "context": {"rag_question": {"type": "string", "required": false}}
                }
            }
        }),
    )
    .unwrap();
}

/// Real Retrieve-Script: Keyword-Overlap-Ranking + Body-Bau.
/// tokenize: lowercase + split [^a-z0-9]+ + leere droppen.
/// score = |tokens(frage) ∩ tokens(doc.text)|; rank desc, tie-break doc_id asc; top_k=2.
/// header (= hop after colony processing): query, scores, top_k.
/// body: system.context.text = joined top_k chunks, messages[0] = the user turn
/// carrying rag_question.
const RETRIEVE_SCRIPT: &str = r#"
import sys, json, re

doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
h = envelope.get("header", {})
ctx = h.get("context", {})
question = ctx.get("rag_question", "")

msgs = d.get("messages", [])
rows = []
for m in msgs:
    if m.get("type") == "tool_result":
        try:
            rows = json.loads(m.get("text", "[]"))
        except Exception:
            pass
        break

def tokenize(text):
    return set(t for t in re.split(r'[^a-z0-9]+', text.lower()) if t)

q_tokens = tokenize(question)

scored = []
for row in rows:
    doc_id = row.get("doc_id", "")
    text = row.get("text", "")
    score = len(q_tokens & tokenize(text))
    scored.append((doc_id, text, score))

# rank desc, tie-break doc_id asc
scored.sort(key=lambda x: (-x[2], x[0]))
top = scored[:2]

query = " ".join(sorted(q_tokens))
scores = [x[2] for x in top]
chunks = "\n".join(x[1] for x in top)

out = {
    "header": {
        "query": query,
        "scores": scores,
        "top_k": len(top)
    },
    "system": {"context": {"text": chunks}},
    "messages": [{"origin": "user", "type": "text", "text": question}]
}
sys.stdout.write(json.dumps(out))
"#;

/// **14-C-1 — Lexical-Retriever isoliert (deterministisch).**
///
/// Boots a minimal topology (only the `/main/retrieve` code cell + `/sink`).
/// Injects corpus rows as a `tool_result` + `context.rag_question`.
/// Beweist: ehrliches Keyword-Overlap-Ranking top_k=2, scores=[4,1] + Body-Bau.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retriever_ranks_top_k_by_keyword_overlap() {
    let td = TempDir::new().unwrap();
    // Green: the real ranking script.
    build_minimal_topology(td.path(), RETRIEVE_SCRIPT);

    let (h, mut sink_rx) = boot_code_only(&td).await;

    // Inject: context.rag_question set, body = the corpus as a tool_result.
    let mut ctx = Map::new();
    ctx.insert(
        "rag_question".into(),
        json!("domesticated felines that purr"),
    );
    // The cell is registered at /retrieve (root_dir=main/ → / stripped).
    h.send(
        MessageBuilder::new(Path::new("/retrieve"))
            .context(ctx)
            .body(Body::Inline(corpus_tool_result_body()))
            .ttl(16)
            .build(),
    )
    .await;

    // Positive receipt: retrieve emits a message to /sink.
    let msg = recv_bounded(&mut sink_rx)
        .await
        .expect("retrieve must emit a message to /sink");

    let body = match &msg.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("expected Inline body from retrieve"),
    };

    // --- hop-Assertions ---
    assert_eq!(msg.headers.hop["top_k"], 2, "top_k must be 2");
    let scores = msg.headers.hop["scores"]
        .as_array()
        .expect("hop.scores must be a JSON array");
    assert_eq!(scores.len(), 2, "scores must have exactly 2 entries");
    assert_eq!(
        scores[0].as_i64().unwrap(),
        4,
        "d1 keyword-overlap score must be 4"
    );
    assert_eq!(
        scores[1].as_i64().unwrap(),
        1,
        "d2 keyword-overlap score must be 1"
    );
    assert!(
        msg.headers.hop.contains_key("query"),
        "hop.query must be present"
    );

    // --- body-Assertions ---
    let sys_str = body["system"]["context"]["text"]
        .as_str()
        .expect("system.context.text must be a string");
    assert!(
        sys_str.contains("domesticated felines"),
        "system.context.text must contain d1 text; got: {sys_str}"
    );
    assert!(
        sys_str.contains("loyal canines"),
        "system.context.text must contain d2 text; got: {sys_str}"
    );

    let messages = body["messages"]
        .as_array()
        .expect("body.messages must be an array");
    assert_eq!(messages.len(), 1, "exactly one user message in body");
    assert_eq!(messages[0]["origin"], "user", "message origin must be user");
    assert_eq!(
        messages[0]["text"], "domesticated felines that purr",
        "user message text must equal context.rag_question"
    );

    h.shutdown().await;
}

/// **14-C-2 — `rag_question` travels as context through the store retrieval hop.**
///
/// Bootet die reale Kette `ask → corpus(store) → retrieve → /sink`.
/// Injects a message carrying the question to `/main/ask`.
/// Proves: at the entrance of `/sink` (after the store hop)
/// `header.context.rag_question == "<question>"` is intact, because store only
/// writes `hop` — which is the structural reason why `rag_question` MUST be
/// context (as hop it would decay at the store emission).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rag_question_survives_store_retrieval_hop() {
    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());

    // Patch the main config: retrieve→/sink (instead of retrieve→llm) so /sink serves as the probe.
    let main_cfg_path = td.path().join("main/config.json");
    let main_cfg_txt = std::fs::read_to_string(&main_cfg_path).unwrap();
    let mut main_cfg: Value = meclaw_core::serde_json::from_str(&main_cfg_txt).unwrap();
    // Replace the `retrieve→llm` edge with `retrieve→/sink`, remove llm+capture.
    main_cfg["params"]["graph"]["edges"] = json!([
        { "from": "./ask", "to": "./corpus",
          "modifier": { "set_context": { "rag_question": "hop.rag_question" } } },
        { "from": "./corpus", "to": "./retrieve" },
        { "from": "./retrieve", "to": "/sink" }
    ]);
    std::fs::write(
        &main_cfg_path,
        meclaw_core::serde_json::to_string_pretty(&main_cfg).unwrap(),
    )
    .unwrap();

    // Remove llm + capture from the TempDir: slice 2 boots only code+store; the
    // directories came along via copy_14c_tree (slice 3 checked them in) but are not
    // part of the probe topology here.
    let _ = std::fs::remove_dir_all(td.path().join("main/llm"));
    let _ = std::fs::remove_dir_all(td.path().join("main/capture"));

    let (h, mut sink_rx) = boot_code_and_store(&td).await;

    // Inject: a message carrying the question to /ask.
    // Bootstrap strips the top-level directory (main/ → /), so the ask cell is
    // registered at /ask (not /main/ask).
    let question = "domesticated felines that purr";
    h.send(
        MessageBuilder::new(Path::new("/ask"))
            .body(Body::Inline(json!({
                "messages": [{"origin": "user", "type": "text", "text": question}]
            })))
            .ttl(16)
            .build(),
    )
    .await;

    // Positive receipt: retrieve emits a message to /sink (after the store hop).
    let msg = recv_bounded(&mut sink_rx)
        .await
        .expect("retrieve must emit a message to /sink after the store hop");

    // Core assertion: context.rag_question is intact after the store hop.
    let rag_q = msg
        .headers
        .context
        .get("rag_question")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        rag_q, question,
        "context.rag_question must survive the store hop unmodified; \
         got: {:?}, context: {:?}",
        rag_q, msg.headers.context
    );

    h.shutdown().await;
}

/// Boot: `code` + `store` + `llm` factories + the /sink CaptureCell + bootstrap over `dir`.
async fn boot_code_store_and_llm(
    td: &TempDir,
) -> (ColonyHandle, mpsc::Receiver<meclaw_core::Message>) {
    let h = ColonyHandle::new_with_factories_at(
        td,
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            (
                "store".to_string(),
                Arc::new(StoreCellFactory) as Arc<dyn CellFactory>,
            ),
            (
                "llm".to_string(),
                Arc::new(LlmCellFactory) as Arc<dyn CellFactory>,
            ),
        ],
    );
    let (sink_tx, sink_rx) = mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("code".to_string(), Arc::new(CodeCellFactory));
    registry.insert("store".to_string(), Arc::new(StoreCellFactory));
    registry.insert("llm".to_string(), Arc::new(LlmCellFactory));
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

/// **14-C-3 — hop decays / an explicit promotion survives.**
///
/// Kette `ask → corpus(store) → retrieve → llm(mock) → /sink` (llm→/sink im Test
/// wired directly, /sink as the CaptureCell probe — the same pattern as slice 2).
/// The committed `capture` node lies in the tree but is replaced by `/sink` for the
/// test, so the llm hop is directly measurable without capture-code-cell decay.
///
/// Assertions at the `/sink` receipt (1 hop downstream of the `retrieve→llm` promotion):
///
/// POSITIV: `context.retrieved_top_k == 2` — per `set_context:{retrieved_top_k:"hop.top_k"}`
///   promoted on the `retrieve→llm` edge, survives the llm hop (context travels through).
/// NEGATIVE: `hop.scores` ABSENT — retrieve emitted hop.scores, it was NEVER
///   promoted and decays at the llm emission (input.hop is structurally replaced
///   fresh). Instead the hop at /sink carries the llm output keys (finish_reason,
///   model).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hop_decays_unless_promoted() {
    // Mock OpenAI: one deterministic response ("rag-answer", finish_reason "stop").
    let mock = MockOpenAI::start(vec![canned_chat_completion("rag-answer", "stop")]).await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());

    // base_url-PLACEHOLDER in llm/config.json ersetzen.
    patch_llm_base_url_14c(td.path(), &base_url);

    // Patch main/config.json: llm→/sink directly (instead of llm→capture) so the
    // llm hop is measurable at /sink without capture-code decay. The `capture` cell
    // lies in the tree (for slice 4) but is not used here.
    let main_cfg_path = td.path().join("main/config.json");
    let main_cfg_txt = std::fs::read_to_string(&main_cfg_path).unwrap();
    let mut main_cfg: Value = meclaw_core::serde_json::from_str(&main_cfg_txt).unwrap();
    main_cfg["params"]["graph"]["edges"] = json!([
        { "from": "./ask", "to": "./corpus",
          "modifier": { "set_context": { "rag_question": "hop.rag_question" } } },
        { "from": "./corpus", "to": "./retrieve" },
        { "from": "./retrieve", "to": "./llm",
          "modifier": { "set_context": { "retrieved_top_k": "hop.top_k" } } },
        { "from": "./llm", "to": "/sink" }
    ]);
    std::fs::write(
        &main_cfg_path,
        meclaw_core::serde_json::to_string_pretty(&main_cfg).unwrap(),
    )
    .unwrap();

    let (h, mut sink_rx) = boot_code_store_and_llm(&td).await;

    let question = "domesticated felines that purr";
    h.send(
        MessageBuilder::new(Path::new("/ask"))
            .body(Body::Inline(json!({
                "messages": [{"origin": "user", "type": "text", "text": question}]
            })))
            .ttl(16)
            .build(),
    )
    .await;

    // Positiver Receipt: die Kette ask→corpus→retrieve→llm→/sink.
    let msg = recv_bounded(&mut sink_rx)
        .await
        .expect("RAG chain ask→corpus→retrieve→llm must deliver a message to /sink");

    // POSITIVE: context.retrieved_top_k == 2 (promoted by the retrieve→llm edge
    // promotion, survives the llm hop because context travels through unchanged).
    let top_k = msg
        .headers
        .context
        .get("retrieved_top_k")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        top_k,
        json!(2),
        "context.retrieved_top_k must be 2 (promoted via retrieve→llm set_context, \
         survives llm hop); context = {:?}",
        msg.headers.context
    );

    // NEGATIVE: hop.scores is ABSENT (retrieve emitted it, it was never promoted →
    // structural decay at the llm emission — input.hop is dropped).
    assert!(
        msg.headers.hop.get("scores").is_none(),
        "hop.scores must be absent at /sink — retrieve's scores were never promoted \
         to context, so they decay at the llm emission; hop = {:?}",
        msg.headers.hop
    );

    // Sharpness: the hop carries llm output keys (finish_reason), NOT retrieve's keys.
    assert!(
        msg.headers.hop.get("finish_reason").is_some(),
        "hop.finish_reason must be present — it is the llm's fresh output hop; \
         hop = {:?}",
        msg.headers.hop
    );
    assert!(
        msg.headers.hop.get("query").is_none(),
        "hop.query must be absent — retrieve's hop.query decayed at llm emission; \
         hop = {:?}",
        msg.headers.hop
    );

    h.shutdown().await;
}

/// **14-C-4 — llm gekoppelt an context.rag_question, end-to-end + Receipt.**
///
/// Volle Kette `ask → corpus(store) → retrieve → llm(capturing-mock) → capture → /sink`.
/// The capturing mock intercepts the OpenAI chat request that llm sends to the mock.
///
/// Assertions:
/// - **Kopplung (Runtime):** user-Turn `content` == rag_question;
///   the system turn's `content` contains the top_k chunk texts (d1 + d2).
/// - **Receipt (positive):** `/sink` receives the assistant answer.
/// - **DLQ empty:** no dead letters across the full chain.
///
/// Proves: the inference input is structurally coupled to question + chunks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn final_answer_coupled_to_rag_question() {
    use meclaw_colony::ColonyMsg;
    use meclaw_colony::api_dto::ReadDeadLettersReply;
    use tokio::sync::oneshot;

    // Mock OpenAI: one deterministic response, capturing.
    let mock = MockOpenAI::start(vec![canned_chat_completion("rag-coupled-answer", "stop")]).await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());

    // base_url-PLACEHOLDER in llm/config.json ersetzen.
    patch_llm_base_url_14c(td.path(), &base_url);

    // Use the complete topology from the checked-in tree:
    // ask → corpus → retrieve → llm → capture → /sink.
    // Patch the main config: capture→/sink (instead of ending at llm→capture).
    let main_cfg_path = td.path().join("main/config.json");
    let main_cfg_txt = std::fs::read_to_string(&main_cfg_path).unwrap();
    let mut main_cfg: Value = meclaw_core::serde_json::from_str(&main_cfg_txt).unwrap();
    main_cfg["params"]["graph"]["edges"] = json!([
        { "from": "./ask", "to": "./corpus",
          "modifier": { "set_context": { "rag_question": "hop.rag_question" } } },
        { "from": "./corpus", "to": "./retrieve" },
        { "from": "./retrieve", "to": "./llm",
          "modifier": { "set_context": { "retrieved_top_k": "hop.top_k" } } },
        { "from": "./llm", "to": "./capture" },
        { "from": "./capture", "to": "/sink" }
    ]);
    std::fs::write(
        &main_cfg_path,
        meclaw_core::serde_json::to_string_pretty(&main_cfg).unwrap(),
    )
    .unwrap();

    let (h, mut sink_rx) = boot_code_store_and_llm(&td).await;

    let question = "domesticated felines that purr";
    h.send(
        MessageBuilder::new(Path::new("/ask"))
            .body(Body::Inline(json!({
                "messages": [{"origin": "user", "type": "text", "text": question}]
            })))
            .ttl(16)
            .build(),
    )
    .await;

    // --- Receipt (positive): /sink receives the answer ---
    let _msg = recv_bounded(&mut sink_rx)
        .await
        .expect("full RAG chain ask→corpus→retrieve→llm→capture must deliver to /sink");

    // --- Kopplung (Runtime): Request-Inspektion ---
    // Poll until the mock has received the request (the LLM HTTP call runs asynchronously).
    let snaps = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let s = mock.recorded_requests().await;
            if !s.is_empty() || std::time::Instant::now() > deadline {
                break s;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    assert_eq!(snaps.len(), 1, "exactly one llm call over the full chain");

    let msgs = snaps[0]
        .messages()
        .expect("llm request must carry messages");

    // User turn: content == rag_question (retrieve builds messages[0] = the user turn with rag_question).
    let user_turn = msgs
        .iter()
        .find(|m| m["role"] == "user")
        .expect("llm request must contain a user-role message");
    assert_eq!(
        user_turn["content"], question,
        "user-Turn content must equal rag_question; got: {:?}",
        user_turn["content"]
    );

    // System turn: content contains the d1 + d2 chunk texts.
    let system_turn = msgs
        .iter()
        .find(|m| m["role"] == "system")
        .expect("llm request must contain a system-role message");
    let sys_content = system_turn["content"]
        .as_str()
        .expect("system content must be a string");
    assert!(
        sys_content.contains("domesticated felines"),
        "system content must contain d1 text (top_k chunk); got: {sys_content:?}"
    );
    assert!(
        sys_content.contains("loyal canines"),
        "system content must contain d2 text (top_k chunk); got: {sys_content:?}"
    );

    // --- DLQ empty ---
    let (ack_tx, ack_rx) = oneshot::channel::<ReadDeadLettersReply>();
    h.runtime()
        .inbox_tx
        .send(ColonyMsg::ReadDeadLetters {
            since: None,
            error_code: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let dlq = ack_rx.await.unwrap();
    assert_eq!(
        dlq.entries.len(),
        0,
        "DLQ empty — no dead-letter over the full RAG chain: {:?}",
        dlq.entries
            .iter()
            .map(|e| e.error_code.clone())
            .collect::<Vec<_>>()
    );

    h.shutdown().await;
}

/// Builds a `CellFactoryRegistry` with `code`, `store`, and `llm` factories —
/// exactly the set required to validate the full 14c-rag topology.
fn factories_14c() -> CellFactoryRegistry {
    let mut reg = CellFactoryRegistry::new();
    reg.insert(
        "code".to_string(),
        Arc::new(meclaw_cells::code::CodeCellFactory) as Arc<dyn CellFactory>,
    );
    reg.insert(
        "store".to_string(),
        Arc::new(meclaw_cells::store::StoreCellFactory) as Arc<dyn CellFactory>,
    );
    reg.insert(
        "llm".to_string(),
        Arc::new(meclaw_cells::LlmCellFactory) as Arc<dyn CellFactory>,
    );
    reg
}

/// **14-C-5 Positiv — Validator akzeptiert die committete RAG-Topologie.**
///
/// Bootstraps the complete `tests/fixtures/14c-rag/` tree (all 5 nodes + the real
/// edges) via `plan_bootstrap` (validate only, no spawn).
/// `consumes.context.rag_question` at retrieve + llm is reachable via the
/// `ask→corpus` `set_context` root; no required-hop violation.
///
/// Assertion: `plan_bootstrap` returns `Ok` — no `HeaderContractViolation`, no
/// `EdgeSchema` error.
#[test]
fn validator_accepts_rag_topology() {
    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());
    // Leave the base_url PLACEHOLDER — plan_bootstrap does not spawn, HTTP is irrelevant.

    let overlay = RegistryOverlay::new(); // no colony.db = FirstBoot overlay
    let result = plan_bootstrap(td.path(), &factories_14c(), &overlay);
    assert!(
        result.is_ok(),
        "plan_bootstrap must accept the committeted RAG topology (no HeaderContractViolation); \
         errors: {:?}",
        result.err().map(|e| e.items().to_vec())
    );
}

/// **14-C-5 negative — the validator rejects without the `rag_question` promotion.**
///
/// Copies the 14c-rag tree into a TempDir and removes `set_context:{rag_question}`
/// from the `ask→corpus` edge (patching main/config.json). Afterwards
/// `consumes.context.rag_question` at retrieve + llm is no longer reachable.
///
/// Assertion: `plan_bootstrap` returns `Err` with a `HeaderContractViolation` whose
/// `reason` contains both `"rag_question"` and `"context presence not reachable"`.
/// Proves: the promotion is not optional — without it the build-time check breaks.
#[test]
fn validator_rejects_without_rag_question_promotion() {
    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());

    // Remove set_context:{rag_question} from the ask→corpus edge.
    let main_cfg_path = td.path().join("main/config.json");
    let main_cfg_txt = std::fs::read_to_string(&main_cfg_path).unwrap();
    let mut main_cfg: Value = meclaw_core::serde_json::from_str(&main_cfg_txt).unwrap();
    // Replace ask→corpus without the modifier (promotion removed), rest unchanged.
    main_cfg["params"]["graph"]["edges"] = json!([
        { "from": "./ask", "to": "./corpus" },
        { "from": "./corpus", "to": "./retrieve" },
        { "from": "./retrieve", "to": "./llm",
          "modifier": { "set_context": { "retrieved_top_k": "hop.top_k" } } },
        { "from": "./llm", "to": "./capture" }
    ]);
    std::fs::write(
        &main_cfg_path,
        meclaw_core::serde_json::to_string_pretty(&main_cfg).unwrap(),
    )
    .unwrap();

    let overlay = RegistryOverlay::new();
    let result = plan_bootstrap(td.path(), &factories_14c(), &overlay);
    let errs = result.expect_err(
        "plan_bootstrap must REJECT topology without rag_question promotion (context not reachable)",
    );

    // At least one HeaderContractViolation error must mention rag_question + the
    // kanonische Reject-Message nennen.
    let found = errs.items().iter().any(|e| {
        if let BootstrapError::HeaderContractViolation { reason } = e {
            reason.contains("rag_question") && reason.contains("context presence not reachable")
        } else {
            false
        }
    });
    assert!(
        found,
        "expected HeaderContractViolation mentioning 'rag_question' and \
         'context presence not reachable'; got: {:?}",
        errs.items()
    );
}

/// Patches the `base_url` in EVERY `llm` config.json under the subtree `dir` to `base_url`.
fn patch_llm_base_url_14c(dir: &std::path::Path, base_url: &str) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            patch_llm_base_url_14c(&p, base_url);
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            let txt = std::fs::read_to_string(&p).unwrap();
            let mut v: Value = meclaw_core::serde_json::from_str(&txt).unwrap();
            if v["cell"]["type"] == "llm" {
                v["params"]["base_url"] = Value::String(base_url.to_string());
                std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
            }
        }
    }
}

/// **14-C-6 — live-graph SVG/DOT from the booted RAG topology.**
///
/// Boots the complete `tests/fixtures/14c-rag/` tree (a TempDir copy, llm against
/// the mock), reads the live graph via `ColonyMsg::ReadGraph` for the scope `/main`,
/// renders `graph.dot` + `graph.svg` with the zero-dep generator (identical to
/// 14a/14b) and writes them to `tests/fixtures/14c-rag/` (under the env gate
/// `MECLAW_EMIT_DOT=1`).
///
/// Assertions:
/// - All 5 nodes (`/ask`, `/corpus`, `/retrieve`, `/llm`, `/capture`) appear in the DOT.
/// - Both edge promotions (`rag_question`, `retrieved_top_k`) appear in the DOT.
/// - The generated SVG is a valid SVG document (`<svg`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn topology_svg_from_live_graph() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("rag-svg-answer", "stop")]).await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());
    patch_llm_base_url_14c(td.path(), &base_url);

    // Patch the main config: add capture→/sink (a positive receipt for the test).
    // The committed tree ends at llm→capture; capture→/sink is added here so the
    // test can measure the complete run as a positive receipt.
    let main_cfg_path = td.path().join("main/config.json");
    let main_cfg_txt = std::fs::read_to_string(&main_cfg_path).unwrap();
    let mut main_cfg: Value = meclaw_core::serde_json::from_str(&main_cfg_txt).unwrap();
    main_cfg["params"]["graph"]["edges"] = json!([
        { "from": "./ask", "to": "./corpus",
          "modifier": { "set_context": { "rag_question": "hop.rag_question" } } },
        { "from": "./corpus", "to": "./retrieve" },
        { "from": "./retrieve", "to": "./llm",
          "modifier": { "set_context": { "retrieved_top_k": "hop.top_k" } } },
        { "from": "./llm", "to": "./capture" },
        { "from": "./capture", "to": "/sink" }
    ]);
    std::fs::write(
        &main_cfg_path,
        meclaw_core::serde_json::to_string_pretty(&main_cfg).unwrap(),
    )
    .unwrap();

    let (h, mut sink_rx) = boot_code_store_and_llm(&td).await;

    // Inject: start the full chain so all 5 nodes are registered in the live graph.
    let question = "domesticated felines that purr";
    h.send(
        MessageBuilder::new(Path::new("/ask"))
            .body(Body::Inline(json!({
                "messages": [{"origin": "user", "type": "text", "text": question}]
            })))
            .ttl(16)
            .build(),
    )
    .await;

    // Positive receipt: the chain ask→corpus→retrieve→llm→capture→/sink runs completely.
    let _msg = recv_bounded(&mut sink_rx)
        .await
        .expect("full RAG chain ask→corpus→retrieve→llm→capture must deliver to /sink");

    // Read the live graph: scope "/" returns all 5 nodes + edges
    // (bootstrap_from_filesystem strips the top-level directory → paths: /ask, /corpus, …).
    let (nodes, edges) = support::live_graph(&h, &["/"]).await;

    // Write SVG/DOT to tests/fixtures/14c-rag/ under the env gate `MECLAW_EMIT_DOT=1`.
    support::emit_dot_if_requested("14c-rag", &nodes, &edges);

    // --- Render DOT/SVG locally for the assertions (independent of MECLAW_EMIT_DOT) ---
    let dot = support::render_dot(&nodes, &edges);
    let svg = support::render_svg(&nodes, &edges);

    // All 5 RAG nodes must appear in the DOT.
    for node in &["/ask", "/corpus", "/retrieve", "/llm", "/capture"] {
        assert!(
            dot.contains(node),
            "DOT must contain node {node}; dot = {dot:?}"
        );
    }

    // Both edge promotions must appear as modifiers in the live-graph DTOs. The DOT
    // renderer does not encode modifiers as an edge label (only the condition), so we
    // test directly against the DTO structure.
    let modifier_str: String = edges
        .iter()
        .filter_map(|e| e.modifier.as_ref().map(|m| m.to_string()))
        .collect::<Vec<_>>()
        .join("|");
    assert!(
        modifier_str.contains("rag_question"),
        "Live-graph edges must carry rag_question modifier; modifiers = {modifier_str:?}"
    );
    assert!(
        modifier_str.contains("retrieved_top_k"),
        "Live-graph edges must carry retrieved_top_k modifier; modifiers = {modifier_str:?}"
    );

    // The SVG must be a valid SVG document and contain all 5 nodes.
    assert!(
        svg.contains("<svg"),
        "rendered output must be an SVG document; got = {svg:.200?}"
    );
    for node in &["/ask", "/corpus", "/retrieve", "/llm", "/capture"] {
        assert!(
            svg.contains(node),
            "SVG must contain node {node}; svg = {svg:.200?}"
        );
    }

    h.shutdown().await;
}
