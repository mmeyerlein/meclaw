//! Owner ruling 2026-10-16 (wave kurator-fix, strand KA): every LLM request
//! names its app at the provider.
//!
//! Before: only the conversation brains (`talky/brain`, `cogny/brain`), the
//! memory-hive llm cells and `file-space/embed` carried `http_referer` and
//! `x_title`. The nine llm cells below sent their requests without an app
//! attribution, and a set could not give them one either: the mutation door
//! admits an `override_params` key only when the addressed cell's
//! `config.json` carries it under `params` ([`check_override_params`],
//! GH #294), so `x_title` was refused with `names no param of llm`.
//!
//! Three locks, for every one of the nine cells:
//!
//! 1. the params carry both keys in exactly the form of `memory-hive/closer`
//!    (`${OPENROUTER_…:-…}`), so the cells cannot drift from the reference;
//! 2. the door admits a set's `x_title` and `http_referer` on the cell, as
//!    the cell is parsed from its shipped template;
//! 3. on the wire: the cell's shipped params send the default attribution as
//!    the `HTTP-Referer` / `X-Title` headers, a set's values replace them, and
//!    neither key leaks into the request body.

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
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

/// `(template, cell)` of every llm cell the ruling adds the attribution to.
const CELLS: &[(&str, &str)] = &[
    ("presenter", "decide"),
    ("display", "judge"),
    ("argus", "judge"),
    ("summarizer", "writer"),
    ("builder", "compose"),
    ("curator", "summarizer"),
    ("file-space", "summarizer"),
    ("llm-registry", "translate"),
    ("steward", "judge"),
];

const KEYS: &[&str] = &["http_referer", "x_title"];

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

fn cell_config(template: &str, cell: &str) -> Value {
    read_json(&format!("templates/{template}/{cell}/config.json"))
}

/// The shipped `templates/` directory as a registry, so refs resolve the way
/// a real mutation resolves them (GH #277).
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

#[test]
fn the_nine_cells_carry_the_attribution_in_the_reference_form() {
    let form = read_json("templates/memory-hive/closer/config.json")["params"].clone();
    for key in KEYS {
        assert!(
            form[*key]
                .as_str()
                .is_some_and(|v| v.starts_with("${OPENROUTER_")),
            "the reference form moved: closer {key} = {}",
            form[*key]
        );
    }
    let mut missing = Vec::new();
    for (template, cell) in CELLS {
        let config = cell_config(template, cell);
        assert_eq!(
            config["cell"]["type"].as_str(),
            Some("llm"),
            "{template}/{cell} is an llm cell"
        );
        for key in KEYS {
            if config["params"][*key] != form[*key] {
                missing.push(format!(
                    "{template}/{cell} {key} = {}",
                    config["params"][*key]
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "not the memory-hive form:\n{}",
        missing.join("\n")
    );
}

#[test]
fn the_door_admits_a_sets_attribution_on_every_cell() {
    let registry = shipped_registry();
    let mut refused = Vec::new();
    for (template, cell_key) in CELLS {
        let parsed = parse_subtree(&core_root().join("templates").join(template), &registry)
            .unwrap_or_else(|e| panic!("the shipped template '{template}' parses: {e:?}"));
        let version = read_json(&format!("templates/{template}/template.json"))["version"]
            .as_str()
            .unwrap_or_else(|| panic!("{template}/template.json carries a version"))
            .to_string();
        let cell = parsed
            .cells
            .iter()
            .find(|c| c.rel_path == *cell_key)
            .unwrap_or_else(|| panic!("template '{template}' has no cell '{cell_key}'"));
        if let Err(why) = check_override_params(
            cell,
            Some(cell_key),
            &format!("{template}@{version}"),
            &json!({"x_title": "Example App", "http_referer": "https://example.com"}),
        ) {
            refused.push(format!("{template}/{cell_key}: {why:?}"));
        }
    }
    assert!(
        refused.is_empty(),
        "a set's attribution must pass the door:\n{}",
        refused.join("\n")
    );
}

/// The cell's own shipped params with every `${…}` placeholder resolved to its
/// default (or a test value where it has none), pointed at the mock, without
/// a credential grant (the broker is not under test here), and `extra`
/// layered on top the way a set's `override_params` is.
fn shipped_params(template: &str, cell: &str, base_url: &str, extra: &Value) -> Value {
    let config = cell_config(template, cell);
    let mut params: Map<String, Value> = config["params"].as_object().expect("params").clone();
    params.remove("script_inline");
    for value in params.values_mut() {
        let Some(text) = value.as_str() else { continue };
        let Some(inner) = text.strip_prefix("${").and_then(|t| t.strip_suffix('}')) else {
            continue;
        };
        *value = match inner.split_once(":-") {
            Some((_, default)) => json!(default),
            None => json!(""),
        };
    }
    params.insert("base_url".into(), json!(base_url));
    params.insert("model".into(), json!("gpt-4o"));
    params.insert("api_key".into(), json!("sk-test"));
    params.insert("credential_grant_id".into(), json!(""));
    for (key, value) in extra.as_object().expect("extra object") {
        params.insert(key.clone(), value.clone());
    }
    Value::Object(params)
}

/// One request through the cell built from its shipped params plus `extra`;
/// returns the attribution headers and the request body the provider saw.
async fn captured(
    template: &str,
    cell: &str,
    extra: Value,
) -> (Option<String>, Option<String>, Value) {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let raw = shipped_params(template, cell, &format!("{}/v1", mock.base_url), &extra);
    let mut llm = LlmCell::new(
        LlmParams::parse(&raw)
            .unwrap_or_else(|e| panic!("{template}/{cell}: the shipped params parse: {e:?}")),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let mut conn = DbConn::wrap(
        meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap(),
        None,
    );
    let (tx, _rx) = tokio::sync::mpsc::channel::<meclaw_core::CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/cell"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    // The `decisions` wire (presenter/decide, GH #957) asks typed questions;
    // every other cell answers a conversation.
    let body = if raw["provider"] == "decisions" {
        json!({"decide": {"state": "a turn", "questions": {
            "show": {"kind": "yes_no", "instructions": "Show it?"}
        }}})
    } else {
        json!({"messages": [{"origin":"user","type":"text","text":"Hi"}]})
    };
    let msg = MessageBuilder::new(Path::new("/cell"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build();
    llm.handle(msg, &sink, &mut conn).await;
    let snaps = mock.recorded_requests().await;
    assert_eq!(
        snaps.len(),
        1,
        "{template}/{cell}: exactly one provider call"
    );
    let snap = &snaps[0];
    (
        snap.headers.get("http-referer").cloned(),
        snap.headers.get("x-title").cloned(),
        snap.body.clone(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_cell_names_its_app_on_the_wire_and_a_set_renames_it() {
    let mut wrong = Vec::new();
    for (template, cell) in CELLS {
        let (referer, title, body) = captured(template, cell, json!({})).await;
        if referer.as_deref() != Some("https://meclaw.ai") || title.as_deref() != Some("MeClaw") {
            wrong.push(format!(
                "{template}/{cell} shipped: HTTP-Referer {referer:?}, X-Title {title:?}"
            ));
        }
        for key in KEYS {
            if body.get(*key).is_some() {
                wrong.push(format!("{template}/{cell}: {key} leaked into the body"));
            }
        }
        let (referer, title, _) = captured(
            template,
            cell,
            json!({"x_title": "Example App", "http_referer": "https://example.com"}),
        )
        .await;
        if referer.as_deref() != Some("https://example.com")
            || title.as_deref() != Some("Example App")
        {
            wrong.push(format!(
                "{template}/{cell} set: HTTP-Referer {referer:?}, X-Title {title:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "attribution on the wire:\n{}",
        wrong.join("\n")
    );
}
