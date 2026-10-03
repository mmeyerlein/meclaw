//! GH #957 -- `provider` is a closed list, and a decisions cell refuses at
//! birth what its wire cannot carry.
//!
//! - an unknown provider is a loud spawn reject that names the whole list;
//! - `decisions` with a tool, structured-output or stream switch in the
//!   pass-through map, or with a wire dialect, is refused by name
//!   (`decisions_unsupported_param`), never silently ignored;
//! - an `openai` cell parses exactly as before (its wire stays pinned by
//!   `llm_chat_completions_wire_regression`).

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
fn the_list_is_openai_and_decisions() {
    assert_eq!(PROVIDERS, &["openai", "decisions"]);
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
