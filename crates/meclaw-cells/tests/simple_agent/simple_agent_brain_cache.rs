//! GH #1408 — `simple-agent/brain` and `simple-agent/strong` take an instance
//! `cache_mode` and `cache_ttl_s`, and an unset one changes nothing on the wire.
//!
//! The `llm` cell has marked the provider's prompt cache since GH #890, and
//! both keys travel in the registry's model package. An instance still could
//! not set them: the mutation door admits an `override_params` key only when
//! the addressed cell's `config.json` carries it under `params`
//! ([`check_override_params`], GH #294), and neither model cell of the
//! simple-agent did. Where the package is refused or absent (a lab against a
//! stub `base_url`), the cache then stayed off: measured on one run, 67 calls,
//! the prompt growing 8.6k -> 57.9k tokens, 0 of them cached.
//!
//! Two pins:
//!
//! 1. `an_instance_may_set_the_brain_cache`: the door admits both keys on
//!    both model cells of the shipped template, and a set value reaches the
//!    request body as `cache_control` marks;
//! 2. `the_brain_without_an_instance_cache_keeps_its_bytes`: the declaration
//!    ships the cell's own "not set" (`off`, `0` -- the serde defaults of the
//!    `llm` cell; an empty string, `null` or an empty `${VAR:-}` would be
//!    refused by its parse), so the request body is byte-identical to the one
//!    of a brain that does not declare the keys at all, and a registry package
//!    still decides over both (its `$reset` falls back to this start value).

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::{CacheMode, LlmParams};
use meclaw_colony::DbConn;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use tempfile::TempDir;

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
use mock_openai::{MockOpenAI, canned_chat_completion};

/// The two model cells of the agent that speak the conversation.
const MODEL_CELLS: &[&str] = &["brain", "strong"];

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

/// The cell's shipped params, every `${…}` resolved to a test value, the
/// endpoint pointed at the mock, `extra` on top (`null` removes a key).
fn shipped_params(cell: &str, base_url: &str, extra: &Value) -> Value {
    let config = read_json(&format!("templates/simple-agent/{cell}/config.json"));
    let mut params: Map<String, Value> = config["params"].as_object().expect("params").clone();
    for (key, value) in params.iter_mut() {
        let Some(text) = value.as_str() else { continue };
        let Some(inner) = text.strip_prefix("${").and_then(|t| t.strip_suffix('}')) else {
            continue;
        };
        *value = match (key.as_str(), inner.split_once(":-")) {
            (_, Some((_, default))) => json!(default),
            ("model", None) => json!("gpt-4o"),
            (other, None) => panic!("no test value for the placeholder of '{other}': {text}"),
        };
    }
    params.insert("api_key".into(), json!("sk-test"));
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

/// One inference over a history, so a `breakpoints` cache has a prefix end to
/// mark; returns the request body parsed and as the raw bytes off the wire.
async fn captured_body(cell: &str, extra: Value) -> (Value, Vec<u8>) {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let raw = shipped_params(cell, &format!("{}/v1", mock.base_url), &extra);
    let mut llm = LlmCell::new(
        LlmParams::parse(&raw).expect("the shipped params parse"),
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
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "a"},
            {"origin": "assistant", "type": "text", "text": "b"},
            {"origin": "user", "type": "text", "text": "c"}
        ]})))
        .build();
    llm.handle(msg, &sink, &mut conn).await;
    rx.recv().await.expect("the cell must emit an answer");
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call");
    let raw = mock.captured.lock().await[0].body.clone();
    (snaps[0].body.clone(), raw)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_instance_may_set_the_brain_cache() {
    let set = json!({"cache_mode": "breakpoints", "cache_ttl_s": 300});
    let parsed = parse_subtree(
        &core_root().join("templates/simple-agent"),
        &shipped_registry(),
    )
    .expect("the shipped simple-agent parses with its refs resolved");
    let version = read_json("templates/simple-agent/template.json")["version"]
        .as_str()
        .expect("simple-agent template.json carries a version")
        .to_string();
    let template = format!("simple-agent@{version}");
    for key in MODEL_CELLS {
        let cell = parsed
            .cells
            .iter()
            .find(|c| c.rel_path == *key)
            .unwrap_or_else(|| panic!("the simple-agent has no cell '{key}'"));
        assert_eq!(cell.config["cell"]["type"].as_str(), Some("llm"), "'{key}'");
        if let Err(why) = check_override_params(cell, Some(key), &template, &set) {
            panic!("an instance cache on '{key}' must pass the door: {why:?}");
        }

        // The values reach the cell ...
        let p = LlmParams::parse(&shipped_params(key, "http://127.0.0.1:9/v1", &set))
            .expect("the set params parse");
        assert_eq!(p.cache_mode, CacheMode::Breakpoints, "'{key}'");
        assert_eq!(p.cache_ttl_s, 300, "'{key}'");

        // ... and the request body: the end of the stable history is marked.
        let (body, _) = captured_body(key, set.clone()).await;
        assert_eq!(
            body["messages"][1]["content"],
            json!([{"type": "text", "text": "b", "cache_control": {"type": "ephemeral"}}]),
            "'{key}': the set cache marks the history end: {body}"
        );
        assert_eq!(
            body["messages"][2]["content"], "c",
            "'{key}': the round stays"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_brain_without_an_instance_cache_keeps_its_bytes() {
    // The cell's own "not set": what a params block without the keys parses to.
    let bare = LlmParams::parse(&json!({"provider": "openai", "model": "m", "api_key": "k"}))
        .expect("bare params parse");
    assert_eq!(bare.cache_mode, CacheMode::Off);
    assert_eq!(bare.cache_ttl_s, 0);
    for key in MODEL_CELLS {
        let config = read_json(&format!("templates/simple-agent/{key}/config.json"));
        let params = &config["params"];
        assert_eq!(
            params.get("cache_mode"),
            Some(&json!("off")),
            "'{key}' declares cache_mode and ships the cell's own off"
        );
        assert_eq!(
            params.get("cache_ttl_s"),
            Some(&json!(0)),
            "'{key}' declares cache_ttl_s and ships the cell's own 0"
        );
        for k in ["cache_mode", "cache_ttl_s"] {
            let setting = &config["contract"]["settings"][k];
            assert_eq!(
                setting["default"], params[k],
                "'{key}' settings.{k}.default"
            );
            assert_eq!(setting["secret"], false, "'{key}' settings.{k}.secret");
        }

        let (shipped, shipped_raw) = captured_body(key, json!({})).await;
        let (undeclared, undeclared_raw) =
            captured_body(key, json!({"cache_mode": null, "cache_ttl_s": null})).await;
        assert!(
            !String::from_utf8_lossy(&shipped_raw).contains("cache_control"),
            "'{key}': no mark without an instance value: {shipped}"
        );
        assert!(
            shipped.get("prompt_cache_key").is_none(),
            "'{key}': {shipped}"
        );
        assert_eq!(
            shipped_raw, undeclared_raw,
            "'{key}': the declaration changes no byte on the wire: {shipped} vs {undeclared}"
        );
    }
}
