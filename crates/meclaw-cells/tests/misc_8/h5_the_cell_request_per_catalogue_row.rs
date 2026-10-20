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

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

// harness_fold: #[path = "h5_support/mod.rs"]
#[allow(unused_imports)]
use crate::h5; // harness_fold: mod h5;

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

/// GH #1085: the file name the conformance tool expects for a row (its
/// `slug_of`, K8): a `local` row -- a server on the operator's own hardware --
/// under `local__<slug>`, every other row under its slug.
fn record_slug(row: &Map<String, Value>) -> String {
    let s = slug(row["model_id"].as_str().unwrap_or_default());
    if row.get("provider").and_then(|v| v.as_str()) == Some("local") {
        format!("local__{s}")
    } else {
        s
    }
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
    // GH #1037: the model's listed output limit is a column the push carries.
    for key in ["cache_ttl_s", "context_window", "max_output"] {
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
        let file = dir.join(format!("{}.json", record_slug(&row)));
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
                record_slug(&row)
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

/// Every template `llm` cell born with reasoning switched OFF
/// (`params.reasoning.enabled == false`), as `(template path, birth params)`.
fn cells_born_without_reasoning() -> Vec<(String, Value)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, Value)>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().is_some_and(|n| n == "config.json")
                && let Ok(text) = std::fs::read_to_string(&p)
                && let Ok(cfg) = serde_json::from_str::<Value>(&text)
                && cfg["cell"]["type"] == "llm"
                && cfg["params"]["reasoning"]["enabled"] == json!(false)
            {
                out.push((p.display().to_string(), cfg["params"].clone()));
            }
        }
    }
    let mut out = Vec::new();
    walk(&crate_dir().join("../../templates"), &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A `${VAR:-default}` param as the substrate resolves it without the
/// variable: its default.
fn env_default(v: &Value) -> Value {
    match v.as_str() {
        Some(s) if s.starts_with("${") && s.ends_with('}') => match s.find(":-") {
            Some(i) => json!(&s[i + 2..s.len() - 1]),
            None => json!(""),
        },
        _ => v.clone(),
    }
}

/// `reasoning` of the measurement record of a row (`latency.json`, K-KATALOG).
fn measured_reasoning(row: &Map<String, Value>) -> Option<String> {
    let file = crate_dir()
        .join("tests/fixtures/conformance")
        .join(record_slug(row))
        .join("latency.json");
    let lat: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    lat["reasoning"].as_str().map(str::to_string)
}

/// GH #1260 (review C-1): a row the measurement found reasoning MANDATORY on
/// (`latency.json` `reasoning: mandatory_*` -- the endpoint answers the switch
/// with HTTP 400 "Reasoning is mandatory") lands on a cell born with
/// reasoning off as the measured wire, never as the switch.
///
/// A push sets the row's package and `$reset`s every other package key, and a
/// reset restores the BIRTH value: a package that names only
/// `reasoning_effort` leaves the birth `reasoning: {"enabled": false}` in
/// place, and the verbatim object outranks the shorthand (`reasoning_block`).
/// Measured on `templates/talky/quick` with tier `quick` on
/// openai/gpt-oss-120b: every quick turn a 400. The nude cell of
/// `h5_the_cell_request_per_catalogue_row` has no birth `reasoning` and could
/// not see it, so this test boots each such template cell on its own birth
/// params, sends the registry's push of the row, then the probe turn, and
/// holds `reasoning` and `provider` of the body to the measured request
/// (`requests/<slug>.json`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_a_mandatory_reasoning_row_reaches_a_cell_born_without_reasoning_as_measured() {
    use meclaw_cells::llm::LlmCell;
    use meclaw_cells::llm::params::{LlmParams, MODEL_PACKAGE_KEYS};
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};

    let Some(path) = catalogue() else {
        eprintln!("llm-registry did not travel into this tree -- skipped (GH #49)");
        return;
    };
    let cells = cells_born_without_reasoning();
    let Some(quick) = cells
        .iter()
        .find(|(p, _)| p.ends_with("talky/quick/config.json"))
    else {
        eprintln!("talky did not travel into this tree -- skipped (GH #49)");
        return;
    };
    let rows: Vec<_> = chat_rows(&path)
        .into_iter()
        .filter(|r| measured_reasoning(r).is_some_and(|m| m.starts_with("mandatory")))
        .collect();
    // Not vacuous: the catalogue holds a row measured with mandatory
    // reasoning, and the cell born without reasoning is there. Bound to the
    // rows, not to the seed tier `quick` (GH #1260, delta review P): a remap of the tier
    // to a row without the duty is no defect, the lock still holds every row.
    assert!(
        !rows.is_empty(),
        "no chat row of the catalogue is measured with mandatory reasoning"
    );
    assert!(quick.1["reasoning"]["enabled"] == json!(false));

    let mut wrong = Vec::new();
    for (template, birth) in &cells {
        for row in &rows {
            let model_id = row["model_id"].as_str().unwrap_or_default();
            let held: Value = serde_json::from_str(
                &std::fs::read_to_string(requests_dir().join(format!("{}.json", record_slug(row))))
                    .expect("the measured request of the row"),
            )
            .expect("a json fixture");
            let mock =
                mock_openai::MockOpenAI::start(vec![canned_chat_completion("OK", "stop")]).await;
            let base_url = format!("{}/v1", mock.base_url);
            let mut raw = Map::new();
            for (k, v) in birth.as_object().cloned().unwrap_or_default() {
                raw.insert(k, env_default(&v));
            }
            raw.insert("model".into(), json!("openai/gpt-6-luna"));
            raw.insert("api_key".into(), json!("test-key"));
            raw.insert("base_url".into(), json!(base_url));
            let params = LlmParams::parse(&Value::Object(raw)).expect("birth params parse");
            let mut cell = LlmCell::new(params, reqwest::Client::new());
            let td = tempfile::TempDir::new().unwrap();
            let conn =
                meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
            let mut db = DbConn::wrap(conn, None);
            let (tx, mut rx) = tokio::sync::mpsc::channel(8);
            let out = OutputSink::new(
                tx,
                Path::new("/llm"),
                Uuid::now_v7(),
                Uuid::now_v7(),
                32,
                meclaw_core::Headers::new(),
                None,
            );
            let message = |body: Value| {
                MessageBuilder::new(Path::new("/llm"))
                    .reply_to(Path::new("/observer"))
                    .body(Body::Inline(body))
                    .build()
            };
            // The registry's push (`push_params`): the package, the endpoint
            // the mock, every other package key reset to its birth value.
            let mut push = package_of(row);
            push.insert("base_url".into(), json!(base_url));
            let reset: Vec<&str> = MODEL_PACKAGE_KEYS
                .iter()
                .copied()
                .filter(|k| !push.contains_key(*k))
                .collect();
            push.insert("$reset".into(), json!(reset));
            cell.handle(
                message(json!({"system": {}, "params": push})),
                &out,
                &mut db,
            )
            .await;
            cell.handle(
                message(json!({
                    "system": {"identity": {"text": PROBE_SYSTEM}},
                    "messages": [{"origin": "user", "type": "text", "text": PROBE_USER}]
                })),
                &out,
                &mut db,
            )
            .await;
            while rx.try_recv().is_ok() {}
            let reqs = mock.recorded_requests().await;
            assert_eq!(reqs.len(), 1, "{template} on {model_id}: one provider call");
            let body = &reqs[0].body;
            if body.get("reasoning") != held["body"].get("reasoning")
                || body.get("provider") != held["body"].get("provider")
                || body["reasoning"]["enabled"] == json!(false)
            {
                wrong.push(format!(
                    "{template} on {model_id}: reasoning {} provider {} -- measured reasoning {} provider {}",
                    body.get("reasoning").unwrap_or(&Value::Null),
                    body.get("provider").unwrap_or(&Value::Null),
                    held["body"].get("reasoning").unwrap_or(&Value::Null),
                    held["body"].get("provider").unwrap_or(&Value::Null),
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a row with mandatory reasoning reaches a cell born without it unlike its measurement:\n{}",
        wrong.join("\n")
    );
}
