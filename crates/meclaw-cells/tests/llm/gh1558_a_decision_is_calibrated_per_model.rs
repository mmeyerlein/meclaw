//! GH #1558 -- the `decisions` lane maps every answer of its model onto one
//! calibrated scale, so an application keeps model-free thresholds and a
//! decider changes without a rebirth.
//!
//! Pinned here, against the in-process HTTP mock (no network, no cost):
//!
//! 1. **The form.** `decision_calibration` is a package key with a fixed form:
//!    per kind (`yes_no`, `choice`) a `default` curve and optional curves
//!    `by_prefix` of the cell key; a curve is a list of knots `[raw, p]`, raw
//!    rising strictly from 0 to 1, p not falling and inside [0, 1]. Anything
//!    else is refused loudly by the param's name, at birth and on a push.
//! 2. **The mapping.** The yes-probability of a `yes_no` answer and every
//!    option probability of a `choice` answer are mapped through the curve of
//!    their kind (the longest matching prefix wins over `default`), linearly
//!    between knots and exactly at a knot; the service's `choice` stays. The
//!    raw answers ride in `decision.raw`, and `decision.calibration` names the
//!    sha256 (12 hex digits) of the calibration that applied.
//! 3. **The switch.** A params-only push of a new model package changes the
//!    model and the curve together; the next answer carries both.
//! 4. **No calibration is loud.** Without the key, without a curve for the
//!    asked kind, or for a `scale` question (the form has no curve for it) the
//!    answer is `decision_uncalibrated`, naming the model -- no request, no
//!    default, no identity curve.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::{LlmParams, MODEL_PACKAGE_KEYS};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::mock_http::MockResponse;
use mock_openai::MockOpenAI;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn mk_sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/decide"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

fn msg(body: Value) -> Message {
    MessageBuilder::new(Path::new("/decide"))
        .reply_to(Path::new("/caller"))
        .body(Body::Inline(body))
        .build()
}

fn drain(rx: &mut mpsc::Receiver<CellEmission>) -> Vec<CellEmission> {
    let mut out = Vec::new();
    while let Ok(em) = rx.try_recv() {
        out.push(em);
    }
    out
}

fn answer(v: Value) -> MockResponse {
    MockResponse::ok_json(&meclaw_core::serde_json::to_vec(&v).unwrap())
}

fn raw_params(base_url: &str, model: &str, calibration: Option<Value>) -> Value {
    let mut raw = json!({"provider": "decisions", "model": model, "api_key": "k",
                         "base_url": base_url});
    if let Some(c) = calibration {
        raw["decision_calibration"] = c;
    }
    raw
}

fn cell_of(raw: &Value) -> (LlmCell, TempDir, DbConn) {
    let cell = LlmCell::new(
        LlmParams::parse(raw).expect("the params parse"),
        reqwest::Client::builder().build().unwrap(),
    );
    let td = TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (cell, td, DbConn::wrap(conn, None))
}

/// A calibration of test numbers -- every value here is test data, chosen so
/// each mapping below is checked by hand.
fn calibration() -> Value {
    json!({
        "yes_no": {
            "default": [[0, 0], [0.5, 0.2], [1, 1]],
            "by_prefix": {
                "needs.": [[0, 0], [0.25, 0.5], [1, 1]],
                "needs.memory": [[0, 0], [0.5, 0.9], [1, 1]]
            }
        },
        "choice": {
            "default": [[0, 0.1], [0.5, 0.5], [1, 0.9]],
            "by_prefix": {"path": [[0, 0], [0.5, 0.25], [1, 1]]}
        }
    })
}

/// Five questions; the wire keys follow the sorted cell keys (GH #1550):
/// q1 needs.memory, q2 needs.tool, q3 path, q4 topic, q5 wants.
fn five_questions() -> Value {
    json!({"messages": [], "decide": {"state": "s", "questions": {
        "needs.memory": {"kind": "yes_no", "instructions": "Memory?"},
        "needs.tool": {"kind": "yes_no", "instructions": "A tool?"},
        "path": {"kind": "choice", "instructions": "Which way?",
                 "options": {"simple": "a quick answer", "talk": "small talk"}},
        "topic": {"kind": "choice", "instructions": "Which topic?",
                  "options": {"weather": "Weather", "none": "Nothing"}},
        "wants": {"kind": "yes_no", "instructions": "Show it?"}
    }}})
}

fn five_answers() -> MockResponse {
    answer(json!({"model": "vendor/decider-a", "answers": {
        "q1": {"type": "noul", "noul": 0.5},
        "q2": {"type": "noul", "noul": 0.25},
        "q3": {"type": "choice", "choice": "simple",
               "probabilities": {"simple": 0.75, "talk": 0.25}},
        "q4": {"type": "choice", "choice": "weather",
               "probabilities": {"weather": 0.875, "none": 0.125}},
        "q5": {"type": "noul", "noul": 0.75}
    }}))
}

fn close(v: &Value, want: f64) -> bool {
    v.as_f64().is_some_and(|f| (f - want).abs() < 1e-9)
}

fn is_sha12(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        s.len() == 12
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn the_one_error(ems: &[CellEmission]) -> (String, String) {
    assert_eq!(ems.len(), 1, "one emission: {ems:?}");
    let c = &ems[0].content;
    assert_eq!(c["header"]["finish_reason"], "error", "{c}");
    assert_eq!(c["meta"]["provider"], "decisions", "{c}");
    assert!(c.get("decision").is_none(), "no decision: {c}");
    meclaw_core::validate_ubf_body(c).expect("a UBF body");
    (
        c["header"]["error_code"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        c.to_string(),
    )
}

// ───── 1. the form ─────

#[test]
fn the_calibration_is_a_package_key_of_a_fixed_form() {
    assert!(MODEL_PACKAGE_KEYS.contains(&"decision_calibration"));
    let good = raw_params(
        "https://decide.example/api",
        "vendor/decider-a",
        Some(calibration()),
    );
    LlmParams::parse(&good).expect("the documented form parses");
    let only_yes_no = json!({"yes_no": {"default": [[0, 0], [1, 1]]}});
    LlmParams::parse(&raw_params("https://d.example", "m", Some(only_yes_no)))
        .expect("one kind alone is a form");
    let null = raw_params("https://d.example", "m", Some(Value::Null));
    LlmParams::parse(&null).expect("null is no calibration");

    let curve = |c: Value| json!({"yes_no": {"default": c}});
    for (bad, why) in [
        (
            curve(json!([[0, 0], [0.5, 0.2], [0.5, 0.3], [1, 1]])),
            "raw does not rise strictly",
        ),
        (
            curve(json!([[0, 0], [0.6, 0.2], [0.4, 0.3], [1, 1]])),
            "raw falls",
        ),
        (curve(json!([[0, 0], [0.5, 0.6], [1, 0.5]])), "p falls"),
        (curve(json!([[0, 0], [0.5, 0.5], [1, 1.2]])), "p over 1"),
        (curve(json!([[0, -0.1], [1, 1]])), "p under 0"),
        (curve(json!([[0.1, 0], [1, 1]])), "raw does not start at 0"),
        (curve(json!([[0, 0], [0.9, 1]])), "raw does not end at 1"),
        (curve(json!([[0, 0], [1.5, 1]])), "raw over 1"),
        (curve(json!([[0, 0]])), "one knot"),
        (curve(json!([])), "no knot"),
        (curve(json!([[0, 0, 0], [1, 1]])), "a knot of three numbers"),
        (curve(json!([[0, "0"], [1, 1]])), "a knot that is no number"),
        (curve(json!({"0": 0})), "a curve that is no list"),
        (
            json!({"yes_no": {"by_prefix": {"a": [[0, 0], [1, 1]]}}}),
            "no default",
        ),
        (
            json!({"yes_no": {"default": [[0, 0], [1, 1]], "extra": 1}}),
            "an unknown key",
        ),
        (
            json!({"yes_no": {"default": [[0, 0], [1, 1]], "by_prefix": []}}),
            "by_prefix no object",
        ),
        (
            json!({"yes_no": {"default": [[0, 0], [1, 1]],
                           "by_prefix": {"": [[0, 0], [1, 1]]}}}),
            "an empty prefix",
        ),
        (
            json!({"yes_no": {"default": [[0, 0], [1, 1]],
                           "by_prefix": {"needs.": [[0, 0.5], [1, 0.4]]}}}),
            "a bad prefix curve",
        ),
        (
            json!({"scale": {"default": [[0, 0], [1, 1]]}}),
            "a kind the form has no curve for",
        ),
        (json!({}), "no kind at all"),
        (json!([[0, 0], [1, 1]]), "no object"),
        (json!("identity"), "a word"),
    ] {
        let err = LlmParams::parse(&raw_params("https://d.example", "m", Some(bad.clone())))
            .expect_err(why);
        assert!(
            err.contains("decision_calibration"),
            "{why}: the refusal names the param: {err}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_with_a_malformed_calibration_is_refused_and_moves_nothing() {
    let mock = MockOpenAI::start(vec![five_answers()]).await;
    let base = format!("{}/api", mock.base_url);
    let (mut cell, _td, mut db) =
        cell_of(&raw_params(&base, "vendor/decider-a", Some(calibration())));
    let (sink, mut rx) = mk_sink();
    let bad = json!({"yes_no": {"default": [[0, 0], [0.5, 0.6], [1, 0.5]]}});
    cell.handle(
        msg(json!({"system": {}, "params": {"model": "vendor/decider-b",
                                           "decision_calibration": bad}})),
        &sink,
        &mut db,
    )
    .await;
    let refused = drain(&mut rx);
    assert_eq!(refused.len(), 1, "a refused push is one error");
    assert_eq!(refused[0].content["header"]["error_code"], "invalid_input");
    assert!(
        refused[0]
            .content
            .to_string()
            .contains("decision_calibration"),
        "{}",
        refused[0].content
    );
    assert_eq!(cell.params.model, "vendor/decider-a", "nothing moved");
}

// ───── 2. the mapping ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_probability_is_mapped_through_the_curve_of_its_kind_and_key() {
    let mock = MockOpenAI::start(vec![five_answers()]).await;
    let base = format!("{}/api", mock.base_url);
    let (mut cell, _td, mut db) =
        cell_of(&raw_params(&base, "vendor/decider-a", Some(calibration())));
    let (sink, mut rx) = mk_sink();

    cell.handle(msg(five_questions()), &sink, &mut db).await;

    assert_eq!(mock.recorded_requests().await.len(), 1, "one call");
    let ems = drain(&mut rx);
    assert_eq!(ems.len(), 1, "one emission");
    let d = &ems[0].content["decision"];
    let a = &d["answers"];
    // The longest prefix wins: `needs.memory` over `needs.`, at a knot.
    assert!(close(&a["needs.memory"]["yes"], 0.9), "{a}");
    // `needs.` at its knot 0.25 -> 0.5.
    assert!(close(&a["needs.tool"]["yes"], 0.5), "{a}");
    // No prefix: `default`, between the knots 0.5 and 1 -> 0.2 + 0.5 * 0.8.
    assert!(close(&a["wants"]["yes"], 0.6), "{a}");
    // A choice: every option through the `path` curve; the choice stays.
    assert_eq!(a["path"]["choice"], "simple", "{a}");
    assert!(close(&a["path"]["p"]["simple"], 0.625), "{a}");
    assert!(close(&a["path"]["p"]["talk"], 0.125), "{a}");
    // A choice without a prefix curve: `default` of the choice kind.
    assert_eq!(a["topic"]["choice"], "weather", "{a}");
    assert!(close(&a["topic"]["p"]["weather"], 0.8), "{a}");
    assert!(close(&a["topic"]["p"]["none"], 0.2), "{a}");
    // The raw answers ride beside them for the audit, unchanged.
    let raw = &d["raw"];
    assert!(close(&raw["needs.memory"]["yes"], 0.5), "{raw}");
    assert!(close(&raw["needs.tool"]["yes"], 0.25), "{raw}");
    assert!(close(&raw["wants"]["yes"], 0.75), "{raw}");
    assert!(close(&raw["path"]["p"]["simple"], 0.75), "{raw}");
    assert!(close(&raw["topic"]["p"]["none"], 0.125), "{raw}");
    assert_eq!(raw["topic"]["choice"], "weather", "{raw}");
    // The stamp names the calibration that applied.
    assert!(is_sha12(&d["calibration"]), "{d}");
    assert_eq!(d["model"], "vendor/decider-a");
}

/// The mapping is monotone, so it keeps the order of every pair of answers:
/// the service's argmax is the argmax after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mapping_keeps_the_order_of_the_options() {
    let raws = [0.0, 0.1, 0.3, 0.5, 0.55, 0.9, 1.0];
    let probabilities: Value = raws
        .iter()
        .enumerate()
        .map(|(i, r)| (format!("o{i}"), json!(r)))
        .collect::<meclaw_core::serde_json::Map<String, Value>>()
        .into();
    let options: Value = (0..raws.len())
        .map(|i| (format!("o{i}"), json!(format!("option {i}"))))
        .collect::<meclaw_core::serde_json::Map<String, Value>>()
        .into();
    let mock = MockOpenAI::start(vec![answer(json!({"answers": {"q1": {
        "choice": "o6", "probabilities": probabilities}}}))])
    .await;
    let base = format!("{}/api", mock.base_url);
    let (mut cell, _td, mut db) =
        cell_of(&raw_params(&base, "vendor/decider-a", Some(calibration())));
    let (sink, mut rx) = mk_sink();
    cell.handle(
        msg(
            json!({"messages": [], "decide": {"state": "s", "questions": {
            "topic": {"kind": "choice", "instructions": "Which?", "options": options}}}}),
        ),
        &sink,
        &mut db,
    )
    .await;
    let ems = drain(&mut rx);
    let p = &ems[0].content["decision"]["answers"]["topic"]["p"];
    let mapped: Vec<f64> = (0..raws.len())
        .map(|i| p[format!("o{i}")].as_f64().expect("a probability"))
        .collect();
    assert!(
        mapped.windows(2).all(|w| w[0] < w[1]),
        "a strictly rising curve keeps a strict order: {mapped:?}"
    );
    assert_eq!(
        ems[0].content["decision"]["answers"]["topic"]["choice"],
        "o6"
    );
}

// ───── 3. the switch ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pushed_package_changes_model_and_curve_together() {
    let half = || answer(json!({"answers": {"q1": {"noul": 0.5}}}));
    let mock = MockOpenAI::start(vec![half(), half()]).await;
    let base = format!("{}/api", mock.base_url);
    let curve_a = json!({"yes_no": {"default": [[0, 0], [1, 0.5]]}});
    let curve_b = json!({"yes_no": {"default": [[0, 0.5], [1, 1]]}});
    let (mut cell, _td, mut db) = cell_of(&raw_params(&base, "vendor/decider-a", Some(curve_a)));
    let (sink, mut rx) = mk_sink();
    let ask = || {
        msg(
            json!({"messages": [], "decide": {"state": "s", "questions": {
            "wants": {"kind": "yes_no", "instructions": "Show it?"}}}}),
        )
    };

    cell.handle(ask(), &sink, &mut db).await;
    let before = drain(&mut rx);
    assert!(close(
        &before[0].content["decision"]["answers"]["wants"]["yes"],
        0.25
    ));

    // The registry's push: the package, every other package key reset.
    let reset: Vec<&str> = MODEL_PACKAGE_KEYS
        .iter()
        .copied()
        .filter(|k| !["model", "decision_calibration"].contains(k))
        .collect();
    cell.handle(
        msg(json!({"system": {}, "params": {"model": "vendor/decider-b",
                                           "decision_calibration": curve_b,
                                           "$reset": reset}})),
        &sink,
        &mut db,
    )
    .await;
    assert!(drain(&mut rx).is_empty(), "a push is answered with silence");

    cell.handle(ask(), &sink, &mut db).await;
    let after = drain(&mut rx);
    assert_eq!(after.len(), 1);
    let d = &after[0].content["decision"];
    assert!(close(&d["answers"]["wants"]["yes"], 0.75), "{d}");
    assert!(close(&d["raw"]["wants"]["yes"], 0.5), "{d}");
    assert_ne!(
        d["calibration"], before[0].content["decision"]["calibration"],
        "the stamp names the new curve"
    );
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps[1].body["model"], "vendor/decider-b");
}

// ───── 4. no calibration is loud ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_decider_without_a_calibration_answers_uncalibrated_and_asks_nobody() {
    let mock = MockOpenAI::start(vec![five_answers()]).await;
    let base = format!("{}/api", mock.base_url);
    let (mut cell, _td, mut db) = cell_of(&raw_params(&base, "vendor/decider-a", None));
    let (sink, mut rx) = mk_sink();

    cell.handle(msg(five_questions()), &sink, &mut db).await;

    assert!(mock.recorded_requests().await.is_empty(), "no request");
    let (code, all) = the_one_error(&drain(&mut rx));
    assert_eq!(code, "decision_uncalibrated", "{all}");
    assert!(
        all.contains("vendor/decider-a"),
        "the detail names the model: {all}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kind_without_its_curve_answers_uncalibrated() {
    let only_yes_no = json!({"yes_no": {"default": [[0, 0], [1, 1]]}});
    let scale = json!({"messages": [], "decide": {"state": "s", "questions": {
        "urgency": {"kind": "scale", "instructions": "How urgent?",
                    "levels": ["low", "high"]}}}});
    // OR-JEVR-o1-23: a `scale` question beside calibrated ones is no reason
    // to pass it through raw -- the whole call is refused.
    let mut mixed = five_questions();
    mixed["decide"]["questions"]["urgency"] = scale["decide"]["questions"]["urgency"].clone();
    for (calib, body, kind) in [
        (only_yes_no, five_questions(), "choice"),
        (calibration(), scale, "scale"),
        (calibration(), mixed, "scale"),
    ] {
        let mock = MockOpenAI::start(vec![five_answers()]).await;
        let base = format!("{}/api", mock.base_url);
        let (mut cell, _td, mut db) = cell_of(&raw_params(&base, "vendor/decider-a", Some(calib)));
        let (sink, mut rx) = mk_sink();
        cell.handle(msg(body), &sink, &mut db).await;
        assert!(
            mock.recorded_requests().await.is_empty(),
            "{kind}: no request"
        );
        let (code, all) = the_one_error(&drain(&mut rx));
        assert_eq!(code, "decision_uncalibrated", "{kind}: {all}");
        assert!(all.contains("vendor/decider-a"), "{kind}: {all}");
        assert!(
            all.contains(kind),
            "{kind}: the detail names the kind: {all}"
        );
    }
}

/// A push of a row without a calibration resets the key: the next call is
/// uncalibrated, never answered on the curve of the model before.
///
/// The registry's case: the cell is born without a curve (its start row has
/// none), a push moves it onto a row with one, and a push onto a row without
/// one resets the key. `$reset` returns a key to its START value (GH #853,
/// `apply_update_over_start`), so the curve the first push brought is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_push_without_a_calibration_leaves_the_decider_uncalibrated() {
    let mock = MockOpenAI::start(vec![five_answers()]).await;
    let base = format!("{}/api", mock.base_url);
    let (mut cell, _td, mut db) = cell_of(&raw_params(&base, "vendor/decider-b", None));
    let (sink, mut rx) = mk_sink();
    let reset_but = |kept: &[&str]| -> Vec<String> {
        MODEL_PACKAGE_KEYS
            .iter()
            .filter(|k| !kept.contains(k))
            .map(|k| k.to_string())
            .collect()
    };

    // Onto a row with a curve: the next answer is mapped through it.
    cell.handle(
        msg(json!({"system": {}, "params": {"model": "vendor/decider-a",
                                           "decision_calibration": calibration(),
                                           "$reset": reset_but(&["model", "decision_calibration"])}})),
        &sink,
        &mut db,
    )
    .await;
    assert!(drain(&mut rx).is_empty(), "a push is answered with silence");
    cell.handle(msg(five_questions()), &sink, &mut db).await;
    let calibrated = drain(&mut rx);
    assert_eq!(calibrated.len(), 1, "{calibrated:?}");
    assert!(
        is_sha12(&calibrated[0].content["decision"]["calibration"]),
        "the answer is calibrated: {}",
        calibrated[0].content
    );
    assert_eq!(mock.recorded_requests().await.len(), 1);

    // Onto a row without one: the key is reset, nothing is asked.
    cell.handle(
        msg(json!({"system": {}, "params": {"model": "vendor/decider-b",
                                           "$reset": reset_but(&["model"])}})),
        &sink,
        &mut db,
    )
    .await;
    assert!(drain(&mut rx).is_empty(), "a push is answered with silence");
    cell.handle(msg(five_questions()), &sink, &mut db).await;
    assert_eq!(
        mock.recorded_requests().await.len(),
        1,
        "no request after the reset"
    );
    let (code, all) = the_one_error(&drain(&mut rx));
    assert_eq!(code, "decision_uncalibrated", "{all}");
    assert!(all.contains("vendor/decider-b"), "{all}");
}
