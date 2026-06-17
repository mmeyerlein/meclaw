//! Phase-14-C Lexical-RAG-Topologie — TDD-Tests.
//! Slice 14-C-1: `retrieve`-Cell isoliert, Keyword-Overlap-Ranking deterministisch.
//! Slice 14-C-2: `rag_question` reist als context durch den store-Retrieval-Hop.
//! Slice 14-C-3: hop verfällt / explizite Promotion überlebt (volle Kette, llm-mock).
//! Slice 14-C-5: Mutations-Validator akzeptiert RAG-Topologie (Positiv + Negativ).
//! Slice 14-C-6: Live-Graph-SVG/DOT aus gebooteter RAG-Topologie.

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

/// Minimal boot: nur `code`-Factory + /sink CaptureCell + bootstrap über `dir`.
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

/// Boot: `code` + `store` Factories + /sink CaptureCell + bootstrap über `dir`.
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

/// Repo-root-relativer Pfad zum eingecheckten 14c-rag-Beispiel-Baum.
fn example_dir_14c() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/14c-rag")
}

/// Kopiert den 14c-rag-Baum in `dst` (rekursiv, ohne SVG/DOT-Artefakte).
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

/// Bounded recv (30 s — robust gegen cargo-parallel-Last).
async fn recv_bounded(
    rx: &mut mpsc::Receiver<meclaw_core::Message>,
) -> Option<meclaw_core::Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Injected corpus: store-select-Ergebnis für 4 Zeilen als JSON-String im tool_result.
/// Format entspricht dem, was die store-Cell bei `select * from corpus` emittiert.
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

/// Baut die minimale Topologie-Verzeichnisse für Slice 14-C-1 in `dir`.
///
/// Erzeugt:
/// - `main/config.json`  (hive, edge retrieve→/sink)
/// - `main/retrieve/config.json`  (code, `script_inline` aus `script`)
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
/// header (= hop nach Colony-Verarbeitung): query, scores, top_k.
/// body: system.context.text = joined top_k chunks, messages[0] = user-Turn mit rag_question.
const RETRIEVE_SCRIPT: &str = r#"
import sys, json, re

d = json.load(sys.stdin)
h = d.get("header", {})
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
/// Boot einer minimalen Topologie (nur `/main/retrieve` code-Cell + `/sink`).
/// Injiziert Corpus-Zeilen als `tool_result` + `context.rag_question`.
/// Beweist: ehrliches Keyword-Overlap-Ranking top_k=2, scores=[4,1] + Body-Bau.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retriever_ranks_top_k_by_keyword_overlap() {
    let td = TempDir::new().unwrap();
    // Grün: echtes Ranking-Script.
    build_minimal_topology(td.path(), RETRIEVE_SCRIPT);

    let (h, mut sink_rx) = boot_code_only(&td).await;

    // Inject: context.rag_question gesetzt, body = Corpus als tool_result.
    let mut ctx = Map::new();
    ctx.insert(
        "rag_question".into(),
        json!("domesticated felines that purr"),
    );
    // Die Cell ist unter /retrieve registriert (root_dir=main/ → / strip).
    h.send(
        MessageBuilder::new(Path::new("/retrieve"))
            .context(ctx)
            .body(Body::Inline(corpus_tool_result_body()))
            .ttl(16)
            .build(),
    )
    .await;

    // Positiver Receipt: retrieve emittiert eine Message an /sink.
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

/// **14-C-2 — `rag_question` reist als context durch den store-Retrieval-Hop.**
///
/// Bootet die reale Kette `ask → corpus(store) → retrieve → /sink`.
/// Injiziert eine Message mit der Frage an `/main/ask`.
/// Beweist: am Eingang von `/sink` (nach dem store-Hop) ist
/// `header.context.rag_question == "<frage>"` intakt, weil store nur `hop`
/// schreibt und der strukturelle Grund, warum `rag_question` context sein
/// MUSS (wäre es hop, verfiele es an der store-Emission).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rag_question_survives_store_retrieval_hop() {
    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());

    // Main-Config patchen: retrieve→/sink (statt retrieve→llm), damit /sink als Probe dient.
    let main_cfg_path = td.path().join("main/config.json");
    let main_cfg_txt = std::fs::read_to_string(&main_cfg_path).unwrap();
    let mut main_cfg: Value = meclaw_core::serde_json::from_str(&main_cfg_txt).unwrap();
    // Ersetze den `retrieve→llm`-Edge durch `retrieve→/sink`, llm+capture entfernen.
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

    // llm + capture aus dem TempDir entfernen: Slice 2 bootet nur code+store;
    // die Verzeichnisse wurden per copy_14c_tree mitgebracht (Slice 3 hat sie
    // eingecheckt), sind hier aber nicht Teil der Probe-Topologie.
    let _ = std::fs::remove_dir_all(td.path().join("main/llm"));
    let _ = std::fs::remove_dir_all(td.path().join("main/capture"));

    let (h, mut sink_rx) = boot_code_and_store(&td).await;

    // Inject: eine Message mit der Frage an /ask.
    // Bootstrap strippt das top-level-Verzeichnis (main/ → /),
    // daher ist die ask-Cell unter /ask registriert (nicht /main/ask).
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

    // Positiver Receipt: retrieve emittiert eine Message an /sink (nach dem store-Hop).
    let msg = recv_bounded(&mut sink_rx)
        .await
        .expect("retrieve must emit a message to /sink after the store hop");

    // Kern-Assertion: context.rag_question ist nach dem store-Hop intakt.
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

/// Boot: `code` + `store` + `llm` Factories + /sink CaptureCell + bootstrap über `dir`.
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

/// **14-C-3 — hop verfällt / explizite Promotion überlebt.**
///
/// Kette `ask → corpus(store) → retrieve → llm(mock) → /sink` (llm→/sink im Test
/// direkt verdrahtet, /sink als CaptureCell-Probe — analoges Muster wie Slice 2).
/// Der committete `capture`-Knoten liegt im Baum, wird aber für den Test durch
/// `/sink` ersetzt, damit der llm-Hop direkt messbar ist ohne Capture-Code-Cell-Verfall.
///
/// Assertions am `/sink`-Empfang (1 Hop downstream der `retrieve→llm`-Promotion):
///
/// POSITIV: `context.retrieved_top_k == 2` — per `set_context:{retrieved_top_k:"hop.top_k"}`
///   auf der `retrieve→llm`-Edge befördert, überlebt den llm-Hop (context reist durch).
/// NEGATIV: `hop.scores` ABSENT — retrieve emittierte hop.scores, wurde NIE befördert,
///   verfällt an der llm-Emission (input.hop wird structural-fresh ersetzt).
///   Der hop am /sink trägt stattdessen die llm-Output-Keys (finish_reason, model).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hop_decays_unless_promoted() {
    // Mock-OpenAI: eine deterministische Antwort ("rag-answer", finish_reason "stop").
    let mock = MockOpenAI::start(vec![canned_chat_completion("rag-answer", "stop")]).await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());

    // base_url-PLACEHOLDER in llm/config.json ersetzen.
    patch_llm_base_url_14c(td.path(), &base_url);

    // Patch main/config.json: llm→/sink direkt (statt llm→capture),
    // damit der llm-Hop am /sink ohne Capture-Code-Verfall messbar ist.
    // Die `capture`-Cell liegt im Baum (für Slice 4), wird hier aber nicht genutzt.
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

    // POSITIV: context.retrieved_top_k == 2 (per Edge-Promotion von retrieve→llm
    // befördert, überlebt den llm-Hop weil context reist unverändert durch).
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

    // NEGATIV: hop.scores ist ABSENT (retrieve emittierte es, nie befördert →
    // structural verfall an der llm-Emission — input.hop wird fallengelassen).
    assert!(
        msg.headers.hop.get("scores").is_none(),
        "hop.scores must be absent at /sink — retrieve's scores were never promoted \
         to context, so they decay at the llm emission; hop = {:?}",
        msg.headers.hop
    );

    // Schärfe: der hop trägt llm-Output-Keys (finish_reason), NICHT retrieve's Keys.
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
/// Der capturing Mock fängt den OpenAI-Chat-Request, der llm→Mock sendet.
///
/// Assertions:
/// - **Kopplung (Runtime):** user-Turn `content` == rag_question;
///   system-Turn `content` enthält die top_k-Chunk-Texte (d1 + d2).
/// - **Receipt (positiv):** `/sink` empfängt die Assistant-Antwort.
/// - **DLQ leer:** keine Dead-Letters über die volle Kette.
///
/// Beweist: die Inferenz-Eingabe ist strukturell an Frage+Chunks gekoppelt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn final_answer_coupled_to_rag_question() {
    use meclaw_colony::ColonyMsg;
    use meclaw_colony::api_dto::ReadDeadLettersReply;
    use tokio::sync::oneshot;

    // Mock-OpenAI: eine deterministische Antwort, capturing.
    let mock = MockOpenAI::start(vec![canned_chat_completion("rag-coupled-answer", "stop")]).await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());

    // base_url-PLACEHOLDER in llm/config.json ersetzen.
    patch_llm_base_url_14c(td.path(), &base_url);

    // Vollständige Topologie aus dem eingecheckten Baum nutzen:
    // ask → corpus → retrieve → llm → capture → /sink.
    // Main-Config patchen: capture→/sink (statt llm→capture Ende).
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

    // --- Receipt (positiv): /sink empfängt die Antwort ---
    let _msg = recv_bounded(&mut sink_rx)
        .await
        .expect("full RAG chain ask→corpus→retrieve→llm→capture must deliver to /sink");

    // --- Kopplung (Runtime): Request-Inspektion ---
    // Poll bis der Mock den Request empfangen hat (LLM-HTTP-Call läuft asynchron).
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

    // user-Turn: content == rag_question (retrieve baut messages[0] = user-Turn mit rag_question).
    let user_turn = msgs
        .iter()
        .find(|m| m["role"] == "user")
        .expect("llm request must contain a user-role message");
    assert_eq!(
        user_turn["content"], question,
        "user-Turn content must equal rag_question; got: {:?}",
        user_turn["content"]
    );

    // system-Turn: content enthält d1 + d2 Chunk-Texte.
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

    // --- DLQ leer ---
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
/// Bootstrappt den vollständigen `tests/fixtures/14c-rag/`-Baum (alle 5 Knoten +
/// reale Edges) via `plan_bootstrap` (validate-only, kein Spawn).
/// `consumes.context.rag_question` an retrieve + llm ist via der
/// `ask→corpus`-`set_context`-Wurzel reachable; keine required-hop-Verletzung.
///
/// Assertion: `plan_bootstrap` liefert `Ok` — kein `HeaderContractViolation`,
/// kein `EdgeSchema`-Fehler.
#[test]
fn validator_accepts_rag_topology() {
    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());
    // base_url-PLACEHOLDER belassen — plan_bootstrap spawnt nicht, HTTP ist irrelevant.

    let overlay = RegistryOverlay::new(); // Kein colony.db = FirstBoot-Overlay
    let result = plan_bootstrap(td.path(), &factories_14c(), &overlay);
    assert!(
        result.is_ok(),
        "plan_bootstrap must accept the committeted RAG topology (no HeaderContractViolation); \
         errors: {:?}",
        result.err().map(|e| e.items().to_vec())
    );
}

/// **14-C-5 Negativ — Validator rejectet ohne `rag_question`-Promotion.**
///
/// Kopiert den 14c-rag-Baum in TempDir und entfernt `set_context:{rag_question}`
/// von der `ask→corpus`-Edge (Patch der main/config.json). Danach ist
/// `consumes.context.rag_question` an retrieve + llm nicht mehr reachable.
///
/// Assertion: `plan_bootstrap` liefert `Err` mit einem `HeaderContractViolation`,
/// dessen `reason` sowohl `"rag_question"` als auch `"context presence not reachable"`
/// enthält. Beweist: die Promotion ist nicht optional — ohne sie bricht die
/// Bauzeit-Prüfung.
#[test]
fn validator_rejects_without_rag_question_promotion() {
    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());

    // Entferne set_context:{rag_question} von der ask→corpus-Edge.
    let main_cfg_path = td.path().join("main/config.json");
    let main_cfg_txt = std::fs::read_to_string(&main_cfg_path).unwrap();
    let mut main_cfg: Value = meclaw_core::serde_json::from_str(&main_cfg_txt).unwrap();
    // Ersetze ask→corpus ohne modifier (Promotion entfernt), rest unverändert.
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

    // Mind. ein HeaderContractViolation-Fehler muss rag_question + die
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

/// Patcht die `base_url` in JEDER `llm`-config.json im Teilbaum `dir` auf `base_url`.
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

/// **14-C-6 — Live-Graph-SVG/DOT aus gebooteter RAG-Topologie.**
///
/// Bootet den vollständigen `tests/fixtures/14c-rag/`-Baum (TempDir-Kopie, llm gegen Mock),
/// liest den Live-Graph via `ColonyMsg::ReadGraph` für den Scope `/main`,
/// rendert `graph.dot` + `graph.svg` mit dem zero-dep-Generator (identisch zu 14a/14b)
/// und schreibt sie nach `tests/fixtures/14c-rag/` (unter Env-Gate `MECLAW_EMIT_DOT=1`).
///
/// Assertions:
/// - Alle 5 Knoten (`/ask`, `/corpus`, `/retrieve`, `/llm`, `/capture`) erscheinen im DOT.
/// - Beide Edge-Promotions (`rag_question`, `retrieved_top_k`) erscheinen im DOT.
/// - Das generierte SVG ist ein gültiges SVG-Dokument (`<svg`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn topology_svg_from_live_graph() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("rag-svg-answer", "stop")]).await;
    let base_url = format!("{}/v1", mock.base_url);

    let td = TempDir::new().unwrap();
    copy_14c_tree(td.path());
    patch_llm_base_url_14c(td.path(), &base_url);

    // Main-Config patchen: capture→/sink hinzufügen (positiver Receipt für den Test).
    // Der committete Baum endet bei llm→capture; hier wird capture→/sink ergänzt
    // damit der Test den vollständigen Durchlauf als positives Receipt messen kann.
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

    // Inject: volle Kette starten damit alle 5 Knoten im Live-Graph registriert sind.
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

    // Positiver Receipt: Kette ask→corpus→retrieve→llm→capture→/sink läuft vollständig.
    let _msg = recv_bounded(&mut sink_rx)
        .await
        .expect("full RAG chain ask→corpus→retrieve→llm→capture must deliver to /sink");

    // Live-Graph lesen: Scope "/" liefert alle 5 Knoten + Edges
    // (bootstrap_from_filesystem strippt das top-level-Verzeichnis → Pfade: /ask, /corpus, …).
    let (nodes, edges) = support::live_graph(&h, &["/"]).await;

    // SVG/DOT unter Env-Gate `MECLAW_EMIT_DOT=1` nach tests/fixtures/14c-rag/ schreiben.
    support::emit_dot_if_requested("14c-rag", &nodes, &edges);

    // --- DOT/SVG für Assertions lokal rendern (unabhängig von MECLAW_EMIT_DOT) ---
    let dot = support::render_dot(&nodes, &edges);
    let svg = support::render_svg(&nodes, &edges);

    // Alle 5 RAG-Knoten müssen im DOT erscheinen.
    for node in &["/ask", "/corpus", "/retrieve", "/llm", "/capture"] {
        assert!(
            dot.contains(node),
            "DOT must contain node {node}; dot = {dot:?}"
        );
    }

    // Beide Edge-Promotions müssen als Modifier in den Live-Graph-DTOs erscheinen.
    // Der DOT-Renderer kodiert Modifier nicht als Edge-Label (nur condition), daher
    // direkt gegen die DTO-Struktur testen.
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

    // SVG muss ein gültiges SVG-Dokument sein und alle 5 Knoten enthalten.
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
