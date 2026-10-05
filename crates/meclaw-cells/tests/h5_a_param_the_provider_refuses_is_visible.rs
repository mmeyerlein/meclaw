//! GH #999 -- a provider that refuses a request for one of its params says so
//! in `meta.error.kind = "unsupported_param"`.
//!
//! Measured (Loop 13, OR-LP.ME.7): a strict hosted router answered a request
//! carrying `temperature` for a model without it with HTTP 404 "No endpoints
//! found that support the requested parameters". The cell reported
//! `model_not_found` with no further word -- it read like a wrong model name,
//! and the chat wire never even read the body of a 4xx. A local OpenAI-style
//! server refuses an unknown field with 400 "Extra inputs are not permitted".
//!
//! The kind is ADDED; the `error_code` stays what it was (404 ->
//! `model_not_found`, 400 -> `provider_error`), so no failover edge changes
//! its lane (OR-H5-3). Without a needle in the provider's sentence the error
//! is exactly the error of before. The error answer also names the fields the
//! request left out (`hop.dropped`), so the receiver sees what was missing.
//! Measured at the sink.

#[path = "mock_openai.rs"]
mod mock_openai;

#[path = "h5_support/mod.rs"]
mod h5;

use h5::{one_turn, status_json};
use meclaw_core::serde_json::json;

const OPENROUTER_404: &str = "No endpoints found that support the requested parameters";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_openrouter_404_for_parameters_is_unsupported_param() {
    let t = one_turn(
        status_json(
            404,
            json!({"error": {"message": OPENROUTER_404, "code": 404}}),
        ),
        json!({}),
    )
    .await;
    assert_eq!(t.hop["finish_reason"], "error", "{:?}", t.body);
    assert_eq!(
        t.hop["error_code"], "model_not_found",
        "lane unchanged: {:?}",
        t.hop
    );
    assert_eq!(t.kind(), Some("unsupported_param"), "{}", t.body["meta"]);
    assert_eq!(t.body["meta"]["error"]["upstream_status"], 404);
    assert_eq!(
        t.body["meta"]["error"]["upstream_message"], OPENROUTER_404,
        "the provider's own sentence travels"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_a_plain_404_stays_model_not_found() {
    let t = one_turn(
        status_json(
            404,
            json!({"error": {"message": "The model `model-a` does not exist", "code": 404}}),
        ),
        json!({}),
    )
    .await;
    assert_eq!(t.hop["error_code"], "model_not_found", "{:?}", t.hop);
    let error = t.body["meta"]["error"].as_object().expect("meta.error");
    assert!(
        !error.contains_key("kind"),
        "the error of before: {error:?}"
    );
    assert!(!error.contains_key("upstream_message"), "{error:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_vllm_extra_inputs_is_unsupported_param() {
    let message = "[{'type': 'extra_forbidden', 'loc': ('body', 'thinking_token_budget'), \
                   'msg': 'Extra inputs are not permitted', 'input': 512}]";
    let t = one_turn(
        status_json(
            400,
            json!({"object": "error", "message": message, "type": "BadRequestError",
                   "param": null, "code": 400}),
        ),
        json!({}),
    )
    .await;
    assert_eq!(t.hop["error_code"], "provider_error", "{:?}", t.hop);
    assert_eq!(t.kind(), Some("unsupported_param"), "{}", t.body["meta"]);
    assert_eq!(t.body["meta"]["error"]["upstream_message"], message);

    // A 400 without a needle is the 400 of before.
    let t = one_turn(
        status_json(
            400,
            json!({"error": {"message": "messages must not be empty"}}),
        ),
        json!({}),
    )
    .await;
    assert_eq!(t.hop["error_code"], "provider_error", "{:?}", t.hop);
    let error = t.body["meta"]["error"].as_object().expect("meta.error");
    assert!(!error.contains_key("kind"), "{error:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_an_in_body_refusal_is_unsupported_param() {
    // The router also delivers a provider's refusal inside a 200 body (GH #75);
    // the inner 404 must not win the kind as `model_not_found`.
    let t = one_turn(
        status_json(
            200,
            json!({"error": {"message": OPENROUTER_404, "code": 404}}),
        ),
        json!({}),
    )
    .await;
    assert_eq!(t.hop["error_code"], "model_not_found", "{:?}", t.hop);
    assert_eq!(t.kind(), Some("unsupported_param"), "{}", t.body["meta"]);
    assert_eq!(t.body["meta"]["error"]["in_body"], true);
    assert_eq!(t.body["meta"]["error"]["upstream_message"], OPENROUTER_404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_the_error_answer_names_what_was_dropped() {
    let t = one_turn(
        status_json(
            404,
            json!({"error": {"message": OPENROUTER_404, "code": 404}}),
        ),
        json!({"supported_params": ["reasoning"], "reasoning_effort": "low"}),
    )
    .await;
    assert!(
        t.requests[0].get("temperature").is_none(),
        "{}",
        t.requests[0]
    );
    assert_eq!(t.hop["finish_reason"], "error", "{:?}", t.hop);
    assert_eq!(t.hop["dropped"], json!(["temperature"]), "{:?}", t.hop);
    assert!(
        !t.hop.contains_key("unverified"),
        "a list verifies: {:?}",
        t.hop
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5_a_long_flat_refusal_is_bounded() {
    // A local OpenAI-style server's pydantic refusal repeats the `input` of
    // the refused field, so a flat `message` (or `detail`) can be arbitrarily
    // long. It travels into `meta.error.upstream_message` and the log, so it
    // is bounded like the `error` envelope and an in-body message (500 bytes).
    let long = format!(
        "[{{'type': 'extra_forbidden', 'loc': ('body', 'thinking_token_budget'), \
         'msg': 'Extra inputs are not permitted', 'input': '{}'}}]",
        "x".repeat(2000)
    );
    for body in [
        json!({"object": "error", "message": long, "type": "BadRequestError", "code": 400}),
        json!({"detail": long}),
    ] {
        let t = one_turn(status_json(400, body.clone()), json!({})).await;
        assert_eq!(t.hop["error_code"], "provider_error", "{:?}", t.hop);
        assert_eq!(t.kind(), Some("unsupported_param"), "{}", t.body["meta"]);
        let msg = t.body["meta"]["error"]["upstream_message"]
            .as_str()
            .expect("upstream_message");
        let marker = format!("… [truncated, {} bytes total]", long.len());
        assert!(msg.ends_with(&marker), "bounded: {msg}");
        assert_eq!(msg.len(), 500 + marker.len(), "{body}");
    }
}
