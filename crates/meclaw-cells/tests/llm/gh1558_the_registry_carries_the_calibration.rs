//! GH #1558 -- the model catalogue carries each decider's calibration, and
//! the registry pushes it with the model.
//!
//! Pinned on the shipped `hand` script, run as the code cell runs it (stdin
//! document in, messages out):
//!
//! 1. `model_upsert` takes `package.decision_calibration` in the documented
//!    form and refuses a malformed one by name (`invalid_model_field`) -- a
//!    curve that does not rise, a probability outside [0, 1], a kind the form
//!    has no curve for.
//! 2. A replacement onto a decisions row with a calibration pushes the curve
//!    in the same package as the model, and the llm cell takes that package;
//!    a replacement onto a row without one resets the key, so no curve of the
//!    model before survives.
//! 3. Every decisions row of the shipped seed carries a calibration the cell
//!    takes, so the shipped decider is never only `decision_uncalibrated`.
//! 4. No shipped template sets `decision_calibration` at birth: a curve comes
//!    only from the catalogue row (a birth curve would return with `$reset`).
//!
//! **R2b guard (GH #49 form).** A tree without the template skips.

use meclaw_cells::llm::params::LlmParams;
use meclaw_cells::params_overlay::apply_update_over_start;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_testing::code_wire::{run_shipped_script, shipped_script};

fn hand() -> Option<String> {
    let root =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/llm-registry");
    let cfg = root.join("hand/config.json");
    cfg.exists()
        .then(|| shipped_script(cfg.to_str().expect("utf-8 path")))
}

fn run(script: &str, envelope: Value, body: Value) -> Vec<Value> {
    let doc = json!({"envelope": envelope, "body": body, "params": {}});
    let out = run_shipped_script(script, &doc.to_string());
    assert!(
        out.status.success(),
        "the script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    match meclaw_core::serde_json::from_slice(&out.stdout).expect("json out") {
        Value::Array(a) => a,
        other => vec![other],
    }
}

fn command(hand: &str, cmd: Value) -> Vec<Value> {
    run(
        hand,
        json!({"header": {"hop": {"route": "in_hand"}, "context": {"actor": "operator"}}}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                             "text": cmd.to_string()}]}),
    )
}

fn echo(hand: &str, phase: &str, carry: Value, results: Vec<Value>) -> Vec<Value> {
    let messages: Vec<Value> = results
        .iter()
        .map(|r| json!({"origin": "tool", "type": "tool_result", "id": "x", "text": r.to_string()}))
        .collect();
    run(
        hand,
        json!({"header": {"hop": {"route": "rstore", "operation": "bundle"},
                          "context": {"registry_origin": "hand", "lr_phase": phase,
                                      "lr_carry": carry.to_string()}}}),
        json!({"messages": messages}),
    )
}

fn rejection(out: &[Value]) -> Option<String> {
    out.iter()
        .find(|m| m["header"]["route"] == "ack" && m["header"]["outcome"] == "rejected")
        .map(|m| {
            m["header"]["reason_code"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
}

/// Test numbers, not a fit.
fn calibration() -> Value {
    json!({"yes_no": {"default": [[0, 0], [0.5, 0.3], [1, 1]],
                      "by_prefix": {"needs.": [[0, 0], [1, 0.8]]}},
           "choice": {"default": [[0, 0], [0.4, 0.5], [1, 1]]}})
}

/// Every number as an `f64`, so two curves compare by value.
fn as_floats(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().expect("a finite number")),
        Value::Array(a) => Value::Array(a.iter().map(as_floats).collect()),
        Value::Object(o) => {
            Value::Object(o.iter().map(|(k, x)| (k.clone(), as_floats(x))).collect())
        }
        other => other.clone(),
    }
}

#[test]
fn model_upsert_takes_a_calibration_and_refuses_a_malformed_one() {
    let Some(hand) = hand() else {
        return;
    };
    let upsert = |calib: Value| {
        command(
            &hand,
            json!({"op": "model_upsert",
                   "model": {"model_id": "test/decider", "wire_dialect": "decisions",
                             "package": {"decision_calibration": calib}}}),
        )
    };
    let taken = upsert(calibration());
    assert_eq!(rejection(&taken), None, "{taken:?}");
    for (bad, why) in [
        (
            json!({"yes_no": {"default": [[0, 0], [0.5, 0.6], [1, 0.5]]}}),
            "p falls",
        ),
        (
            json!({"yes_no": {"default": [[0, 0], [0.5, 0.2], [0.5, 0.3], [1, 1]]}}),
            "raw stalls",
        ),
        (
            json!({"yes_no": {"default": [[0, 0], [1, 1.5]]}}),
            "p over 1",
        ),
        (
            json!({"yes_no": {"default": [[0.2, 0], [1, 1]]}}),
            "raw does not start at 0",
        ),
        (
            json!({"scale": {"default": [[0, 0], [1, 1]]}}),
            "an unknown kind",
        ),
        (
            json!({"yes_no": {"by_prefix": {"a.": [[0, 0], [1, 1]]}}}),
            "no default",
        ),
        (json!({}), "no kind"),
        (json!([[0, 0], [1, 1]]), "no object"),
    ] {
        let refused = upsert(bad.clone());
        assert_eq!(
            rejection(&refused).as_deref(),
            Some("invalid_model_field"),
            "{why}: {refused:?}"
        );
        let said = refused
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            said.contains("decision_calibration"),
            "{why}: the refusal names the field: {said}"
        );
    }
}

#[test]
fn a_replacement_pushes_the_curve_with_the_model_and_resets_it_without_one() {
    let Some(hand) = hand() else {
        return;
    };
    const CELL: &str = "/g/talky/decide";
    let models = json!([
        {"model_id": "test/decider-a", "status": "active", "wire_dialect": "decisions",
         "base_url": "https://decide.example/api", "cost_in": 4, "cost_out": 0,
         "context_window": 32000, "package": {"decision_calibration": calibration()}},
        {"model_id": "test/decider-b", "status": "active", "wire_dialect": "decisions",
         "base_url": "https://decide.example/api", "cost_in": 4, "cost_out": 0,
         "context_window": 32000}
    ]);
    let subscribers = json!([
        {"cell_path": CELL, "tier": "", "pinned": 0, "start_model": "test/decider-b",
         "package_hash": "", "model_id": "test/decider-b", "rank": "start",
         "reason": "start_value", "since": "t", "protocol": "decisions"}
    ]);
    let push_to = |model: &str| -> Value {
        let cmd = json!({"op": "override_set", "tier": "", "model_id": model, "reason": "",
                         "scope": "target", "match": "/g", "id": "", "cell_path": "",
                         "start_model": "", "pinned": null, "announce": false,
                         "entries": [], "actor": "operator", "call_id": "c1"});
        let results = vec![
            models.clone(),
            json!([]),
            json!([]),
            subscribers.clone(),
            json!([]),
            json!([{"cell_path": CELL}]),
        ];
        let out = echo(&hand, "world", json!({"cmd": cmd}), results);
        out.iter()
            .filter(|m| m["header"]["route"] == "update" && m["header"]["subscriber"] == CELL)
            .map(|m| m["params"].clone())
            .find(|p| p["model"] == model)
            .unwrap_or_else(|| panic!("the cell is moved onto {model}: {out:?}"))
    };

    let there = push_to("test/decider-a");
    assert_eq!(there["decision_calibration"], calibration(), "{there}");
    // The llm cell takes the package as the registry pushes it: the push goes
    // through the overlay (start value, then `$reset`, then the keys beside
    // it), exactly as a running cell applies a params update.
    let start = json!({"provider": "decisions", "model": "test/decider-b", "api_key": "k",
                       "base_url": "https://decide.example/api"});
    LlmParams::parse(&start).expect("a decisions cell");
    let update = there.as_object().cloned().expect("a params object");
    let moved = apply_update_over_start::<LlmParams>(&start, &Map::new(), &update)
        .unwrap_or_else(|e| panic!("the cell takes the pushed package: {e:?}"));
    assert_eq!(moved.merged.model, "test/decider-a");
    let as_json = meclaw_core::serde_json::to_value(&moved.merged).expect("params serialize");
    // The cell holds its knots as floats (`0` comes back as `0.0`), so the
    // curve is compared by value, not by the JSON number's spelling.
    assert_eq!(
        as_floats(&as_json["decision_calibration"]),
        as_floats(&calibration()),
        "the curve arrived"
    );

    let back = push_to("test/decider-b");
    assert!(back.get("decision_calibration").is_none(), "{back}");
    assert!(
        back["$reset"]
            .as_array()
            .is_some_and(|a| a.iter().any(|k| k == "decision_calibration")),
        "a row without a calibration resets it: {back}"
    );
    // Applied over the overlay the first push left, the reset takes the curve
    // away again: the cell was born without one, so none survives the move.
    let update = back.as_object().cloned().expect("a params object");
    let returned = apply_update_over_start::<LlmParams>(&start, &moved.overlay, &update)
        .unwrap_or_else(|e| panic!("the cell takes the pushed package: {e:?}"));
    assert_eq!(returned.merged.model, "test/decider-b");
    let as_json = meclaw_core::serde_json::to_value(&returned.merged).expect("params serialize");
    assert!(
        as_json["decision_calibration"].is_null(),
        "no curve of the model before survives: {as_json}"
    );
}

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Point 3 of the file: every decisions row the core ships carries a
/// calibration the cell takes. Without one, a colony whose decider the
/// registry moves onto that row answers nothing but `decision_uncalibrated`
/// (review jevr_o1-r2, Minor 6).
/// Checked as the cell checks it at spawn and push (`calibration::checked`),
/// then read into the form the lane maps with.
#[test]
fn every_shipped_decisions_row_carries_a_calibration_the_cell_takes() {
    let seed = templates_root().join("llm-registry/store/seed/models.jsonl");
    if !seed.exists() {
        println!("SKIP: templates/llm-registry does not travel");
        return;
    }
    let text = std::fs::read_to_string(&seed).expect("the seed reads");
    let mut deciders = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let row: Value = meclaw_core::serde_json::from_str(line).expect("a JSON row");
        if row["wire_dialect"] != "decisions" {
            continue;
        }
        deciders += 1;
        let id = row["model_id"].as_str().unwrap_or("?").to_string();
        let curve = &row["package"]["decision_calibration"];
        assert!(
            curve.is_object(),
            "{id}: no package.decision_calibration in its seed row"
        );
        let checked = meclaw_cells::llm::calibration::checked(curve)
            .unwrap_or_else(|e| panic!("{id}: the cell refuses the curve: {e}"));
        let cal: meclaw_cells::llm::calibration::DecisionCalibration =
            meclaw_core::serde_json::from_value(checked)
                .unwrap_or_else(|e| panic!("{id}: the curve reads into no calibration: {e}"));
        assert!(
            cal.yes_no.is_some() || cal.choice.is_some(),
            "{id}: a calibration of no kind"
        );
    }
    assert!(deciders > 0, "the seed ships no decisions row any more");
}

/// Every `params` object of `doc` (at any depth) that sets the calibration.
fn births_a_curve(doc: &Value, at: &str, hits: &mut Vec<String>) {
    match doc {
        Value::Object(o) => {
            if o.get("params")
                .and_then(Value::as_object)
                .is_some_and(|p| p.contains_key("decision_calibration"))
            {
                hits.push(at.to_string());
            }
            for (k, v) in o {
                births_a_curve(v, &format!("{at}.{k}"), hits);
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                births_a_curve(v, &format!("{at}[{i}]"), hits);
            }
        }
        _ => {}
    }
}

fn json_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a readable dir") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            json_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}

/// Point 4 of the file: no shipped template births a cell with a curve
/// (OR-jevr.kalib.6, review jevr_o1-r2 Minor 1): `$reset` returns a key to its
/// start value, so a curve
/// set at birth would come back when the registry moves the cell onto a row
/// without one, and the new model would answer on the old model's scale. A
/// curve comes only from the catalogue row. The test fixtures that birth a
/// decider with an identity curve live under `crates/`, not `templates/`.
#[test]
fn no_shipped_template_births_a_cell_with_a_curve() {
    let root = templates_root();
    if !root.join("llm-registry").exists() {
        println!("SKIP: templates/ does not travel");
        return;
    }
    // The sweep sees a curve where a template would set one.
    let mut probe = Vec::new();
    births_a_curve(
        &json!({"cell": {"type": "llm"},
                "params": {"provider": "decisions", "decision_calibration": calibration()}}),
        "probe",
        &mut probe,
    );
    assert_eq!(probe, vec!["probe".to_string()], "the sweep is blind");

    let mut files = Vec::new();
    json_files(&root, &mut files);
    assert!(files.len() > 100, "the sweep found {} files", files.len());
    let mut hits = Vec::new();
    for file in &files {
        let Ok(doc) = meclaw_core::serde_json::from_str::<Value>(
            &std::fs::read_to_string(file).expect("a readable file"),
        ) else {
            continue;
        };
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
        births_a_curve(&doc, &rel, &mut hits);
    }
    assert!(
        hits.is_empty(),
        "a template sets decision_calibration at birth: {hits:?}"
    );
}
