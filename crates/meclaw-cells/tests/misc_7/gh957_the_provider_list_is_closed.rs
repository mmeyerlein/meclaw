//! GH #957 -- `provider` is a closed list, and a decisions cell refuses at
//! birth what its wire cannot carry.
//!
//! - an unknown provider is a loud spawn reject that names the whole list;
//! - `decisions` with a tool, structured-output or stream switch in the
//!   pass-through map, or with a wire dialect, is refused by name
//!   (`decisions_unsupported_param`), never silently ignored;
//! - an `openai` cell parses exactly as before (its wire stays pinned by
//!   `llm_chat_completions_wire_regression`);
//! - `anthropic` (GH #1239) speaks the native Messages wire with an api key
//!   only: a subscription login, a wire dialect, a stream switch or a
//!   length continuation (the Messages wire refuses a prefill) is refused by
//!   name (`anthropic_unsupported_param`).

use meclaw_cells::LlmCellFactory;
use meclaw_cells::llm::params::{LlmParams, PROVIDERS};
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;

#[test]
fn an_unknown_provider_is_refused_with_the_list() {
    let err = LlmCellFactory
        .validate_params(&json!({"provider": "x", "model": "m", "api_key": "k"}))
        .expect_err("x is no provider");
    for name in PROVIDERS {
        assert!(
            err.contains(&format!("'{name}'")),
            "the list is named: {err}"
        );
    }
    assert!(err.contains("'x'"), "{err}");
}

#[test]
fn the_list_is_openai_decisions_and_anthropic() {
    assert_eq!(PROVIDERS, &["openai", "decisions", "anthropic"]);
}

#[test]
fn an_anthropic_cell_takes_an_api_key_and_refuses_the_rest_by_name() {
    let p = LlmParams::parse(&json!({"provider": "anthropic", "model": "claude", "api_key": "k"}))
        .expect("anthropic with an api key is a cell");
    assert!(p.is_anthropic() && !p.is_decisions());
    for (extra, name) in [
        (json!({"wire_dialect": "responses"}), "wire_dialect"),
        (
            json!({"provider_extra": {"stream": true}}),
            "provider_extra.stream",
        ),
        (json!({"length_continuations": 1}), "length_continuations"),
    ] {
        let mut raw = json!({"provider": "anthropic", "model": "claude", "api_key": "k"});
        for (k, v) in extra.as_object().expect("object") {
            raw[k] = v.clone();
        }
        let err = LlmCellFactory.validate_params(&raw).expect_err("refused");
        assert!(
            err.contains("anthropic_unsupported_param") && err.contains(name),
            "{err}"
        );
    }
    let raw = json!({"provider": "anthropic", "model": "claude",
                     "auth": "oauth_subscription", "auth_ref": "/tmp/none.json"});
    let err = LlmParams::parse(&raw).expect_err("api key only");
    assert!(
        err.contains("anthropic_unsupported_param") && err.contains("auth"),
        "{err}"
    );
}

#[test]
fn a_decisions_cell_with_tools_is_refused_by_name() {
    for extra in [
        json!({"tools": []}),
        json!({"tool_choice": "auto"}),
        json!({"response_format": {"type": "json_object"}}),
        json!({"stream": true}),
    ] {
        let raw = json!({"provider": "decisions", "model": "", "api_key": "k",
                         "provider_extra": extra});
        let err = LlmCellFactory.validate_params(&raw).expect_err("refused");
        assert!(err.contains("decisions_unsupported_param"), "{err}");
    }
    let raw = json!({"provider": "decisions", "model": "", "api_key": "k",
                     "wire_dialect": "responses"});
    let err = LlmParams::parse(&raw).expect_err("no dialect on this wire");
    assert!(
        err.contains("decisions_unsupported_param") && err.contains("wire_dialect"),
        "{err}"
    );
}

#[test]
fn an_openai_cell_parses_as_before() {
    let p = LlmParams::parse(&json!({"provider": "openai", "model": "gpt-4o", "api_key": "k"}))
        .expect("unchanged");
    assert!(!p.is_decisions());
    assert_eq!(p.provider, "openai");
}
