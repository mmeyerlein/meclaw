//! Owner ruling 2026-10-16 (wave kurator-fix, strands KA and KA2): every LLM
//! request names its app at the provider.
//!
//! Before: only the conversation brains (`talky/brain`, `cogny/brain`), the
//! memory-hive llm cells and `file-space/embed` carried `http_referer` and
//! `x_title`. Fifteen more llm cells sent their requests without an app
//! attribution, and a set could not give them one either: the mutation door
//! admits an `override_params` key only when the addressed cell's
//! `config.json` carries it under `params` ([`check_override_params`],
//! GH #294), so `x_title` was refused with `names no param of llm`.
//!
//! The cells are not a list. The first fix (strand KA) named nine cells and
//! left six others and four examples without the attribution; a list only
//! holds the cells somebody remembered. So every lock below walks the tree:
//! every `config.json` under `templates/` and `examples/` whose `cell.type` is
//! `llm` is checked, and a new llm cell without the attribution is red the
//! day it lands.
//!
//! Three locks:
//!
//! 1. every llm cell, template or example, carries both keys in exactly the
//!    form of `memory-hive/closer` (`${OPENROUTER_…:-…}`; the boot resolves
//!    the token for an example's `main/` exactly as the mutation does for a
//!    template), so no cell can drift from the reference;
//! 2. the door admits a set's `x_title` and `http_referer` on every template
//!    llm cell, as the cell is parsed from its shipped template (an example is
//!    a colony root, not a template, so no door addresses it);
//! 3. on the wire, for every llm cell: the cell's shipped params send the
//!    default attribution as the `HTTP-Referer` / `X-Title` headers, a set's
//!    values replace them, and neither key leaks into the request body.

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use std::path::PathBuf;
use tempfile::TempDir;

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
use mock_openai::{MockOpenAI, canned_chat_completion};

const KEYS: &[&str] = &["http_referer", "x_title"];

/// The cell whose params are the reference form of the attribution.
const REFERENCE: &str = "templates/memory-hive/closer";

fn core_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_json(rel: &str) -> Value {
    let path = core_root().join(rel);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// One llm cell found on disk.
struct Site {
    /// Directory of the cell relative to the repository, e.g.
    /// `templates/argus/judge` or `examples/hello/main/responder`.
    dir: String,
    /// `(template root, cell path inside the template)` when the cell ships
    /// in a template: the nearest ancestor directory carrying a
    /// `template.json`, both relative to the repository / to that root.
    template: Option<(String, String)>,
}

/// Every directory under `top` (`templates` or `examples`) whose
/// `config.json` declares `cell.type == "llm"`, sorted by path. A file that
/// is not JSON is no cell any boot could start, and is left to the tests of
/// its own tree.
fn llm_sites(top: &str) -> Vec<Site> {
    let root = core_root();
    let top_dir = root.join(top);
    let mut out = Vec::new();
    let mut stack = vec![top_dir.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.file_name().is_none_or(|n| n != "config.json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(config) = meclaw_core::serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if config["cell"]["type"] != "llm" {
                continue;
            }
            let cell_dir = p.parent().expect("a config.json has a directory");
            let rel = |q: &std::path::Path| {
                q.strip_prefix(&root)
                    .unwrap_or(q)
                    .to_string_lossy()
                    .replace('\\', "/")
            };
            let template = cell_dir
                .ancestors()
                .take_while(|a| a.starts_with(&top_dir) && **a != *top_dir)
                .find(|a| a.join("template.json").is_file())
                .map(|t| {
                    let inner = cell_dir
                        .strip_prefix(t)
                        .unwrap_or(cell_dir)
                        .to_string_lossy()
                        .replace('\\', "/");
                    (rel(t), inner)
                });
            out.push(Site {
                dir: rel(cell_dir),
                template,
            });
        }
    }
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// Every llm cell of the tree: the templates' and the examples'.
///
/// Floors for the SMALLER tree: the public export leaves out the templates it
/// does not publish. The reference cell and the examples travel in both.
fn all_sites() -> Vec<Site> {
    let mut sites = llm_sites("templates");
    sites.extend(llm_sites("examples"));
    assert!(
        sites.iter().any(|s| s.dir == REFERENCE),
        "the walk did not find the reference cell {REFERENCE} -- wrong root"
    );
    assert!(
        sites.iter().any(|s| s.dir.starts_with("examples/")),
        "the walk found no llm cell under examples/ -- wrong root"
    );
    assert!(
        sites.len() >= 15,
        "the walk found {} llm cells -- wrong root",
        sites.len()
    );
    sites
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
fn every_llm_cell_carries_the_attribution_in_the_reference_form() {
    let form = read_json(&format!("{REFERENCE}/config.json"))["params"].clone();
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
    for site in all_sites() {
        let config = read_json(&format!("{}/config.json", site.dir));
        for key in KEYS {
            if config["params"][*key] != form[*key] {
                missing.push(format!("{} {key} = {}", site.dir, config["params"][*key]));
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
    let mut checked = 0;
    for site in all_sites() {
        let Some((template_root, cell_key)) = &site.template else {
            continue;
        };
        let template = template_root.trim_start_matches("templates/");
        let parsed = parse_subtree(&core_root().join(template_root), &registry)
            .unwrap_or_else(|e| panic!("the shipped template '{template}' parses: {e:?}"));
        let version = read_json(&format!("{template_root}/template.json"))["version"]
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
        checked += 1;
    }
    assert!(checked > 0, "no template llm cell reached the door");
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
fn shipped_params(dir: &str, base_url: &str, extra: &Value) -> Value {
    let config = read_json(&format!("{dir}/config.json"));
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
async fn captured(dir: &str, extra: Value) -> (Option<String>, Option<String>, Value) {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let raw = shipped_params(dir, &format!("{}/v1", mock.base_url), &extra);
    let mut llm = LlmCell::new(
        LlmParams::parse(&raw).unwrap_or_else(|e| panic!("{dir}: the shipped params parse: {e:?}")),
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
    assert_eq!(snaps.len(), 1, "{dir}: exactly one provider call");
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
    for site in all_sites() {
        let dir = site.dir.as_str();
        let (referer, title, body) = captured(dir, json!({})).await;
        if referer.as_deref() != Some("https://meclaw.ai") || title.as_deref() != Some("MeClaw") {
            wrong.push(format!(
                "{dir} shipped: HTTP-Referer {referer:?}, X-Title {title:?}"
            ));
        }
        for key in KEYS {
            if body.get(*key).is_some() {
                wrong.push(format!("{dir}: {key} leaked into the body"));
            }
        }
        let (referer, title, _) = captured(
            dir,
            json!({"x_title": "Example App", "http_referer": "https://example.com"}),
        )
        .await;
        if referer.as_deref() != Some("https://example.com")
            || title.as_deref() != Some("Example App")
        {
            wrong.push(format!(
                "{dir} set: HTTP-Referer {referer:?}, X-Title {title:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "attribution on the wire:\n{}",
        wrong.join("\n")
    );
}
