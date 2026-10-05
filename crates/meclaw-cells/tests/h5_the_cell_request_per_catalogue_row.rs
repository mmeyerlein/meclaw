//! GH #999 -- the request the `llm` cell builds for each catalogue row, held
//! as a file the conformance tool sends unchanged to the real endpoint (K8).
//!
//! A mock can only check the cell against the cell's own idea of the wire.
//! So for every `active` or `explicit` chat-wire row of the shipped catalogue this test
//! boots a real cell with that row's package (what the registry would push:
//! the row's columns plus its `package` json), sends the tool's fixed probe
//! turn, and compares the body the mock RECEIVED with
//! `fixtures/conformance/requests/<slug>.json` (`slug` = `model_id` with `/`
//! as `__`). The tool then sends that very body to the real endpoint, so the
//! measurement checks the cell, not a re-creation of it. `expected_dropped` is
//! the `hop.dropped` the answer carried at the receiver.
//!
//! `MECLAW_UPDATE_CONFORMANCE=1` writes the files anew (and removes the file
//! of a row that left the catalogue); without it any difference is red, and
//! each differing record is printed on one `CONFORMANCE-RECORD <file> <json>`
//! line, so a run on a build host can be written back from its log.
//! Guarded like every other template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;

#[path = "h5_support/mod.rs"]
mod h5;

use meclaw_core::serde_json::{self, Map, Value, json};
use mock_openai::canned_chat_completion;
use std::path::PathBuf;

/// The fixed probe turn of the conformance tool (P1 § 3).
const PROBE_SYSTEM: &str = "Answer with the single word OK.";
const PROBE_USER: &str = "Ping.";

/// The model-package keys the registry takes from catalogue COLUMNS, as
/// `(param, column)` -- `package_of` in the `llm-registry` hand.
const COLUMN_KEYS: &[(&str, &str)] = &[
    ("base_url", "base_url"),
    ("wire_dialect", "wire_dialect"),
    ("cache_mode", "cache_mode"),
];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn catalogue() -> Option<PathBuf> {
    let p = crate_dir().join("../../templates/llm-registry/store/seed/models.jsonl");
    p.is_file().then_some(p)
}

fn requests_dir() -> PathBuf {
    crate_dir().join("tests/fixtures/conformance/requests")
}

fn slug(model_id: &str) -> String {
    model_id.replace('/', "__")
}

/// The params a registry push sets from one row -- the same rule as
/// `package_of`: empty strings and zero counts are unset, `prompt` becomes
/// `model_prompt`, the `package` json carries the rest.
fn package_of(row: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("model".into(), row["model_id"].clone());
    for (param, column) in COLUMN_KEYS {
        if let Some(s) = row.get(*column).and_then(|v| v.as_str())
            && !s.trim().is_empty()
        {
            out.insert((*param).into(), json!(s));
        }
    }
    for key in ["cache_ttl_s", "context_window"] {
        if let Some(n) = row.get(key).and_then(|v| v.as_u64())
            && n > 0
        {
            out.insert(key.into(), json!(n));
        }
    }
    for (k, v) in row
        .get("package")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default()
    {
        if !v.is_null() && v != json!("") {
            out.insert(k, v);
        }
    }
    if let Some(p) = row.get("prompt").and_then(|v| v.as_str())
        && !p.trim().is_empty()
    {
        out.insert("model_prompt".into(), json!(p));
    }
    out
}

/// The reachable (`active` or `explicit`) chat-wire rows of the shipped catalogue.
fn chat_rows(path: &PathBuf) -> Vec<Map<String, Value>> {
    std::fs::read_to_string(path)
        .expect("catalogue")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Map<String, Value>>(l).expect("a json row"))
        .filter(|r| !r.contains_key("schema"))
        // GH #1025: an `explicit` row is pushed to a brain like an active one
        // (override, tier, model id), so its params are held to the same lock.
        .filter(|r| {
            matches!(
                r.get("status").and_then(|v| v.as_str()),
                Some("active" | "explicit")
            )
        })
        .filter(|r| r.get("wire_dialect").and_then(|v| v.as_str()) == Some("chat_completions"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_the_cell_request_per_catalogue_row() {
    let Some(path) = catalogue() else {
        eprintln!("llm-registry did not travel into this tree -- skipped (GH #49)");
        return;
    };
    let update = std::env::var("MECLAW_UPDATE_CONFORMANCE").as_deref() == Ok("1");
    let rows = chat_rows(&path);
    assert!(!rows.is_empty(), "the catalogue has active chat rows");
    let dir = requests_dir();
    let mut wanted = Vec::new();
    let mut drift = Vec::new();
    for row in rows {
        let model_id = row["model_id"].as_str().expect("model_id").to_string();
        let package = package_of(&row);
        let t = h5::turn_with(
            canned_chat_completion("OK", "stop"),
            |base_url| {
                let mut params = json!({"provider": "openai", "api_key": "test-key"});
                for (k, v) in &package {
                    params[k] = v.clone();
                }
                // The endpoint is the mock; the file never names a host.
                params["base_url"] = json!(base_url);
                params
            },
            json!({
                "system": {"identity": {"text": PROBE_SYSTEM}},
                "messages": [{"origin": "user", "type": "text", "text": PROBE_USER}]
            }),
        )
        .await;
        assert_eq!(t.requests.len(), 1, "{model_id}: one provider call");
        assert_eq!(t.hop["finish_reason"], "stop", "{model_id}: {:?}", t.body);
        let file = dir.join(format!("{}.json", slug(&model_id)));
        let record = json!({
            "model_id": model_id,
            "endpoint_class": row.get("provider").cloned().unwrap_or(Value::Null),
            "wire_dialect": "chat_completions",
            "body": t.requests[0],
            "expected_dropped": t.hop.get("dropped").cloned().unwrap_or(json!([])),
        });
        wanted.push(file.clone());
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            let text = serde_json::to_string_pretty(&record).unwrap() + "\n";
            std::fs::write(&file, text).unwrap();
            continue;
        }
        // One line per row a lane run can be read back from: a test on a
        // build host cannot write into this tree, its log can carry the file.
        if std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .as_ref()
            != Some(&record)
        {
            eprintln!(
                "CONFORMANCE-RECORD requests/{}.json {record}",
                slug(&model_id)
            );
        }
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                let held: Value = serde_json::from_str(&text).expect("a json fixture");
                if held != record {
                    drift.push(format!("{}: held {held}\n  built {record}", file.display()));
                }
            }
            Err(_) => drift.push(format!("{}: missing", file.display())),
        }
    }
    // A file whose row left the catalogue (or the chat wire) is stale.
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|e| e == "json") && !wanted.contains(&p) {
            if update {
                std::fs::remove_file(&p).unwrap();
            } else {
                drift.push(format!("{}: no active chat row", p.display()));
            }
        }
    }
    assert!(
        drift.is_empty(),
        "the cell's request moved -- MECLAW_UPDATE_CONFORMANCE=1 writes the files anew:\n{}",
        drift.join("\n")
    );
}
