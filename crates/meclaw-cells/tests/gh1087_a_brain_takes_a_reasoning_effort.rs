//! GH #1087 — `talky/brain` and `talky-chat/brain` of an assistant take a
//! `reasoning_effort`, and an unset one changes nothing on the wire.
//!
//! The `llm` cell has translated `reasoning_effort` into the provider request
//! since GH #124 (`"reasoning": {"effort": …}`). The assistant still could not
//! pass one: the mutation door admits an `override_params` key only when the
//! addressed cell's `config.json` carries it under `params`
//! ([`check_override_params`], GH #294), and the brain the assistant's two
//! keepers reference did not. A set that asked for a low effort was refused
//! with `names no param of llm`.
//!
//! Three locks:
//!
//! 1. the door admits `reasoning_effort` on both brains of the shipped
//!    assistant (the refusal the issue quotes, turned around);
//! 2. the declaration ships UNSET (`null`, the form `memory-hive/recall`
//!    ships its derived bounds in) and no ref marker of the assistant sets it;
//! 3. on the wire: the brain's own params, declared as shipped, produce a
//!    request byte-identical to the same params without the key, and a set
//!    `low` adds exactly `"reasoning": {"effort": "low"}` and nothing else.

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use tempfile::TempDir;

#[path = "mock_openai.rs"]
mod mock_openai;
use mock_openai::{MockOpenAI, canned_chat_completion};

/// The two keepers of an assistant whose brain answers a person.
const BRAINS: &[&str] = &["talky/brain", "talky-chat/brain"];

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_json(rel: &str) -> Value {
    let path = core_root().join(rel);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// The shipped `templates/` directory as a registry, so the assistant's `ref`
/// keepers resolve the way a real mutation resolves them (GH #277).
fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

fn assistant_ref() -> String {
    let version = read_json("templates/assistant/template.json")["version"]
        .as_str()
        .expect("assistant template.json carries a version")
        .to_string();
    format!("assistant@{version}")
}

#[test]
fn the_door_admits_a_reasoning_effort_on_both_brains_of_an_assistant() {
    let parsed = parse_subtree(
        &core_root().join("templates/assistant"),
        &shipped_registry(),
    )
    .expect("the shipped assistant parses with its refs resolved");
    let template = assistant_ref();
    for key in BRAINS {
        let cell = parsed
            .cells
            .iter()
            .find(|c| c.rel_path == *key)
            .unwrap_or_else(|| panic!("the assistant has no cell '{key}'"));
        let kind = cell.config["cell"]["type"].as_str();
        assert_eq!(kind, Some("llm"), "'{key}' is the llm brain");
        if let Err(why) = check_override_params(
            cell,
            Some(key),
            &template,
            &json!({"reasoning_effort": "low"}),
        ) {
            panic!("a set's reasoning_effort on '{key}' must pass the door: {why:?}");
        }
    }
}

#[test]
fn the_brain_declares_the_effort_and_ships_it_unset() {
    let brain = read_json("templates/talky/brain/config.json");
    let params = brain["params"].as_object().expect("brain params");
    assert!(
        params.contains_key("reasoning_effort"),
        "talky/brain must declare reasoning_effort under params"
    );
    assert!(
        params["reasoning_effort"].is_null(),
        "it ships unset (null), got {}",
        params["reasoning_effort"]
    );
    let setting = &brain["contract"]["settings"]["reasoning_effort"];
    assert_eq!(setting["type"], "string", "declared as a string setting");
    assert!(setting["default"].is_null(), "its default is unset");
    assert_eq!(setting["secret"], false);

    // No keeper of the assistant sets it: unset is what a generation gets.
    for keeper in ["talky", "talky-chat"] {
        let marker = read_json(&format!("templates/assistant/{keeper}/config.json"));
        assert!(
            marker["override_params"]["brain"]
                .get("reasoning_effort")
                .is_none(),
            "the '{keeper}' ref marker must leave reasoning_effort unset"
        );
    }
}

/// The brain's own shipped params with every `${…}` placeholder resolved to a
/// test value, the endpoint pointed at the mock, and `extra` layered on top
/// (`null` removes a key, as a test of "absent" needs).
fn brain_params(base_url: &str, extra: &Value) -> Value {
    let brain = read_json("templates/talky/brain/config.json");
    let mut params: Map<String, Value> = brain["params"].as_object().expect("params").clone();
    for (key, value) in params.iter_mut() {
        let Some(text) = value.as_str() else { continue };
        let Some(inner) = text.strip_prefix("${").and_then(|t| t.strip_suffix('}')) else {
            continue;
        };
        *value = match (key.as_str(), inner.split_once(":-")) {
            (_, Some((_, default))) => json!(default),
            ("model", None) => json!("gpt-4o"),
            ("api_key", None) => json!("sk-test"),
            (other, None) => panic!("no test value for the placeholder of '{other}': {text}"),
        };
    }
    params.insert("base_url".into(), json!(base_url));
    for (key, value) in extra.as_object().expect("extra object") {
        if value.is_null() {
            params.remove(key);
        } else {
            params.insert(key.clone(), value.clone());
        }
    }
    Value::Object(params)
}

/// One inference through a brain built from its shipped params plus `extra`;
/// returns the request body as the provider received it, parsed and as the raw
/// bytes off the wire.
async fn captured_body(extra: Value) -> (Value, Vec<u8>) {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let raw = brain_params(&format!("{}/v1", mock.base_url), &extra);
    let mut cell = LlmCell::new(
        LlmParams::parse(&raw).expect("the brain's shipped params parse"),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let mut conn = DbConn::wrap(
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap(),
        None,
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/brain"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new("/brain"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(
            json!({"messages": [{"origin":"user","type":"text","text":"Hi"}]}),
        ))
        .build();
    cell.handle(msg, &sink, &mut conn).await;
    rx.recv().await.expect("the brain must emit an answer");
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call");
    let raw = mock.captured.lock().await[0].body.clone();
    (snaps[0].body.clone(), raw)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unset_effort_sends_the_request_byte_for_byte() {
    let shipped = read_json("templates/talky/brain/config.json");
    assert!(
        shipped["params"]
            .as_object()
            .is_some_and(|p| p.contains_key("reasoning_effort")),
        "the comparison is vacuous unless the brain declares the key"
    );
    let (declared, declared_raw) = captured_body(json!({})).await;
    let (_, without_raw) = captured_body(json!({"reasoning_effort": null})).await;
    assert!(
        declared.get("reasoning").is_none(),
        "unset: the provider sees no reasoning field: {declared}"
    );
    assert_eq!(
        String::from_utf8_lossy(&declared_raw),
        String::from_utf8_lossy(&without_raw),
        "the declared-but-unset brain sends the same bytes as a brain without the key"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_low_effort_reaches_the_provider_and_nothing_else_moves() {
    let (unset, _) = captured_body(json!({})).await;
    let (low, _) = captured_body(json!({"reasoning_effort": "low"})).await;
    assert_eq!(low["reasoning"], json!({"effort": "low"}), "{low}");
    let mut rest = low.clone();
    rest.as_object_mut().unwrap().remove("reasoning");
    assert_eq!(rest, unset, "the effort is the only difference on the wire");
}
