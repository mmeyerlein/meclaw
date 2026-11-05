//! GH #1376 (OR-D53-85 (1)) -- **the provider's own error code reaches the
//! error lane as `meta.provider_code`.**
//!
//! A live provider says WHY it refused in a code of its own: the OpenAI
//! Realtime `error` event carries `error.code` (`insufficient_quota`, say)
//! next to the human-readable `error.message`. Until now the `gpt_live`
//! adapter kept only the message, so the colony's error lane said
//! `duplex_failed` with free text in `meta.detail`, and nothing downstream
//! could tell an empty account from a bad minute without reading prose. The
//! argus (`in_error`) names a hint's origin after `meta.provider_code` where
//! there is one (`error:<code>`), so the code has to arrive.
//!
//! The promise, in three pins:
//! - (a) an `error` event with a code -- before `session.started` (fatal,
//!   `duplex_failed`) and inside a running session (`duplex_warning`) --
//!   leaves the cell as `meta.provider_code`, next to the unchanged detail;
//! - (b) without a code (or with one that is not an identifier) the error
//!   message is byte for byte what it was: no field, no `null`;
//! - (c) that very message, handed to the shipped argus door as an
//!   `in_error`, is stored under origin `error:insufficient_quota`.
//!
//! Measured with the shipped `gpt_live` adapter against the strict mock
//! socket of `meclaw_testing::mock_gpt_live`, inside a duplex cell. Loopback
//! sockets, so real time (development rules § 11): every wait is the named
//! hang marker, no claim here is about time, and no model is called.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::{Live, boot_recovering_with, boot_renewing, duplex_params, handover_msg, header};
use meclaw_cells::voice::contract::{DuplexProvider, ProviderTimeouts};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

/// The provider's message, as the OpenAI Realtime API words it.
const QUOTA_MESSAGE: &str =
    "You exceeded your current quota, please check your plan and billing details.";

/// The Realtime `error` event, exactly the documented shape: `type`,
/// `event_id`, and an `error` object with `type`, `code`, `message`, `param`.
fn error_event(code: Value) -> Value {
    json!({
        "type": "error",
        "event_id": "event_mock_error",
        "error": {
            "type": "insufficient_quota",
            "code": code,
            "message": QUOTA_MESSAGE,
            "param": null,
            "event_id": null
        }
    })
}

/// The same event without any `code` key at all.
fn error_event_without_code() -> Value {
    let mut e = error_event(Value::Null);
    e["error"]
        .as_object_mut()
        .expect("an object")
        .remove("code");
    e
}

fn gpt_live_params(mock: &MockGptLive) -> GptLiveParams {
    serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": mock.base_url(),
        "instructions": "Be Egon.",
        "sample_rate": duplex_cell::RATE,
    }))
    .expect("the gpt_live block parses")
}

/// One call against a mock that plays `event` -- in place of
/// `session.started` when `refuse` is set, inside the running session
/// otherwise -- and the cell's first `error` emission.
async fn first_error(event: Value, refuse: bool) -> Value {
    let mock = MockGptLive::start(LiveScript {
        started: !refuse,
        actions: vec![LiveAction::Send(event)],
        ..LiveScript::default()
    })
    .await
    .expect("the mock socket binds");
    let provider: Arc<dyn DuplexProvider> = Arc::new(
        GptLiveDuplex::new(gpt_live_params(&mock)).with_timeouts(ProviderTimeouts::default()),
    );
    let mut live: Live = boot_recovering_with(
        duplex_params(json!({"default_mode": "auto"})),
        provider,
        0,
        0,
    )
    .await;
    let (_client, _hello) = live.connect("session=gh1376-1&mode=auto").await;
    live.emission("error").await
}

/// Pin (a): the code of a refused start and of a mid-session error reaches
/// `meta.provider_code`; the detail is the message it always was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_error_code_reaches_meta_provider_code() {
    let refused = first_error(error_event(json!("insufficient_quota")), true).await;
    assert_eq!(
        header(&refused, "error_code"),
        Some(&json!("duplex_failed")),
        "{refused}"
    );
    assert_eq!(
        refused["meta"],
        json!({
            "detail": format!("duplex protocol error: {QUOTA_MESSAGE}"),
            "provider_code": "insufficient_quota"
        }),
        "the refused start names the provider's code: {refused}"
    );
    assert!(
        header(&refused, "provider_code").is_none(),
        "the code rides the meta, never the hop: {refused}"
    );

    let warned = first_error(error_event(json!("insufficient_quota")), false).await;
    assert_eq!(
        header(&warned, "error_code"),
        Some(&json!("duplex_warning")),
        "{warned}"
    );
    assert_eq!(
        warned["meta"],
        json!({"detail": QUOTA_MESSAGE, "provider_code": "insufficient_quota"}),
        "a mid-session error names the provider's code: {warned}"
    );
}

/// Pin (b): without a code -- absent, `null`, empty, or not an identifier
/// (`[A-Za-z0-9_.-]{1,64}`, the argus' own form) -- the error message does
/// not move: `meta` is `{detail}` and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_provider_code_the_error_bytes_do_not_move() {
    let long = "x".repeat(65);
    for event in [
        error_event_without_code(),
        error_event(Value::Null),
        error_event(json!("")),
        error_event(json!(42)),
        error_event(json!("https://u:pw@h.example")),
        error_event(json!(long)),
    ] {
        let refused = first_error(event.clone(), true).await;
        assert_eq!(
            refused["meta"],
            json!({"detail": format!("duplex protocol error: {QUOTA_MESSAGE}")}),
            "no provider code, no field: {event} -> {refused}"
        );
        let warned = first_error(event.clone(), false).await;
        assert_eq!(
            warned["meta"],
            json!({"detail": QUOTA_MESSAGE}),
            "no provider code, no field: {event} -> {warned}"
        );
    }
}

/// Pin (c), end to end: the refused start's error message, taken over an
/// `in_error` edge (route and sender stamped as the edge stamps them), makes
/// the shipped argus door store a hint of origin `error:insufficient_quota`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_provider_code_names_the_argus_origin() {
    let door = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/argus/door/config.json");
    let Ok(raw) = std::fs::read_to_string(&door) else {
        eprintln!("templates/argus did not travel into this tree -- skipped (GH #49)");
        return;
    };
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("no python3 -- skipped");
        return;
    }
    let cfg: Value = serde_json::from_str(&raw).expect("the door parses");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();

    let refused = first_error(error_event(json!("insufficient_quota")), true).await;
    let mut hop = refused["header"].clone();
    hop["route"] = json!("in_error");
    hop["error_sender"] = json!("voice");
    let mut body = refused.clone();
    body.as_object_mut().expect("an object").remove("header");
    let doc = json!({
        "envelope": {"header": {"hop": hop, "context": {}}},
        "body": body,
        "params": {},
    });

    let mut child = Command::new("python3")
        .arg("-c")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3 runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("the document goes in");
    let done = child.wait_with_output().expect("the door ends");
    assert!(
        done.status.success(),
        "the door exits 0: {}",
        String::from_utf8_lossy(&done.stderr)
    );
    let out: Vec<Value> = serde_json::from_slice(&done.stdout).expect("a list of emissions");
    let insert: Value = out
        .first()
        .and_then(|m| m["messages"][0]["text"].as_str())
        .map(|t| serde_json::from_str(t).expect("the store operation parses"))
        .unwrap_or_else(|| panic!("the door asks its store to keep the hint: {out:?}"));
    assert_eq!(insert["operation"], json!("insert"), "{insert}");
    assert_eq!(
        insert["row"]["origin"],
        json!("error:insufficient_quota"),
        "the provider's code names the origin: {insert}"
    );
}

/// Pin (a'): a renewal the provider refuses with its own code (a spent quota
/// on the new session) names that code on the warning of the failed renewal,
/// like a refused start does (review m1, GH #1376). Played with the renewing
/// fake: its second session ends with `DuplexError::Refused`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_renewal_names_the_provider_code() {
    let params = duplex_params(json!({"default_mode": "auto"}));
    let mut live = boot_renewing(params, 300, 20_000).await;
    let (_client, _hello) = live.connect("session=gh1376-renew&mode=auto").await;
    let _first = live.session().await;
    let _renewed = live.emission("renewed").await;
    let mut second = live.session().await;
    second.refuse("insufficient_quota").await;
    live.send(handover_msg("gh1376-renew", "Handover.", 1))
        .await;
    let warning = live.emission("error").await;
    assert_eq!(
        header(&warning, "error_code"),
        Some(&json!("duplex_warning")),
        "{warning}"
    );
    assert_eq!(
        warning["meta"]["provider_code"],
        json!("insufficient_quota"),
        "the failed renewal names the provider's code: {warning}"
    );
    drop(second);
}
