//! GH #853: the model package on the run-time path — who may change it, and
//! how its state is made visible.
//!
//! Two small, pure pieces the cell's params step uses:
//!
//! - [`touches_package`]: does a `params` slot name (or reset) a key of
//!   [`MODEL_PACKAGE_KEYS`]? On a turn message such a slot is not applied —
//!   a conversation cannot change the model it talks to (cells know no
//!   sender, so the FORM of the message decides: params-only = operator).
//! - [`params_line`]: the one stderr line with the effective package and each
//!   key's source (`start` | `overlay`). Written on every update and every
//!   restore; a params-only message answers with it instead of an emission,
//!   because every shipped brain's out-edges would take an emission for a
//!   model answer or dead-letter it as `no_route` (OR-SN.L2.1).

use crate::llm::params::{LlmParams, MODEL_PACKAGE_KEYS};
use crate::params_overlay::RESET_KEY;
use meclaw_core::serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// The `tracing` target of the params line — one place to filter it.
pub(crate) const PARAMS_LOG_TARGET: &str = "meclaw::llm::params";

/// True iff `update` sets a package key or resets one.
pub(crate) fn touches_package(update: &Map<String, Value>) -> bool {
    update.iter().any(|(key, value)| {
        if key == RESET_KEY {
            // A malformed `$reset` counts as touching: on a turn it is not
            // applied either way, and the params-only path reports the shape.
            return value.as_array().is_none_or(|names| {
                names
                    .iter()
                    .any(|n| n.as_str().is_none_or(|n| MODEL_PACKAGE_KEYS.contains(&n)))
            });
        }
        MODEL_PACKAGE_KEYS.contains(&key.as_str())
    })
}

/// The package keys a slot names, in the constant's order — for the line
/// that says a turn's slot was not applied.
pub(crate) fn package_keys_in(update: &Map<String, Value>) -> Vec<&'static str> {
    MODEL_PACKAGE_KEYS
        .iter()
        .copied()
        .filter(|k| update.contains_key(*k))
        .chain(update.contains_key(RESET_KEY).then_some(RESET_KEY))
        .collect()
}

/// `llm: params <path> model=<m>[start] … (overlay: <keys>)`.
///
/// Only package keys, and only those with a value. Never a secret: `api_key`
/// and the credential keys are not package keys, a `base_url` loses any
/// userinfo, `provider_extra` shows its keys only (a provider knob may carry
/// anything), and `model_prompt` shows length and the first 8 hex digits of
/// its sha256 — enough to tell two prompts apart, nothing to read.
pub(crate) fn params_line(path: &str, params: &LlmParams, overlay: &Map<String, Value>) -> String {
    let source = |key: &str| {
        if overlay.contains_key(key) {
            "overlay"
        } else {
            "start"
        }
    };
    let mut parts: Vec<String> = Vec::new();
    for key in MODEL_PACKAGE_KEYS {
        let value = match *key {
            "model" => Some(params.model.clone()),
            "base_url" => params.base_url.as_deref().map(without_userinfo),
            "wire_dialect" => Some(
                match params.effective_wire_dialect() {
                    crate::llm::params::WireDialect::ChatCompletions => "chat_completions",
                    crate::llm::params::WireDialect::Responses => "responses",
                }
                .to_string(),
            ),
            "reasoning_effort" => params.reasoning_effort.clone(),
            "reasoning_wire" => Some(
                match params.reasoning_wire {
                    crate::llm::params::ReasoningWire::Nested => "nested",
                    crate::llm::params::ReasoningWire::TopLevel => "top_level",
                }
                .to_string(),
            ),
            "reasoning" => params.reasoning.as_ref().map(|r| r.to_string()),
            "thinking_budget" => params.thinking_budget.map(|b| b.to_string()),
            "max_tokens" => Some(params.max_tokens.to_string()),
            "temperature" => Some(params.temperature.to_string()),
            "external_timeout_ms" => Some(params.external_timeout_ms.to_string()),
            "provider_extra" => (!params.provider_extra.is_empty()).then(|| {
                let keys: Vec<&str> = params.provider_extra.keys().map(String::as_str).collect();
                format!("{{{}}}", keys.join(","))
            }),
            "model_prompt" => params
                .model_prompt
                .as_deref()
                .filter(|m| !m.is_empty())
                .map(|m| {
                    let digest = Sha256::digest(m.as_bytes());
                    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
                    format!("len:{},sha:{hex}", m.len())
                }),
            _ => None,
        };
        if let Some(v) = value {
            parts.push(format!("{key}={v}[{}]", source(key)));
        }
    }
    // GH #858: the requirement is no package key and never in the overlay (it
    // is immutable), but it is what the registry chose the package FOR. Its
    // hash is the one the registry keys its translation by -- sha256 of the
    // trimmed prose -- so a line and a `show` row can be matched by eye; the
    // prose itself is a `show` away and would make every line a paragraph.
    if let Some(req) = params
        .requirement
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        let digest = Sha256::digest(req.as_bytes());
        let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
        parts.push(format!("requirement=len:{},sha:{hex}[start]", req.len()));
    }
    let mut keys: Vec<&str> = overlay.keys().map(String::as_str).collect();
    keys.sort_unstable();
    format!(
        "llm: params {path} {} (overlay: {})",
        parts.join(" "),
        if keys.is_empty() {
            "-".to_string()
        } else {
            keys.join(",")
        }
    )
}

/// A URL for a log line: userinfo, query and fragment dropped, the rest as
/// given. An endpoint may carry its key as `?key=…` (review of L2, M-3), and
/// this line goes to stderr on every update and restore.
fn without_userinfo(raw: &str) -> String {
    match reqwest::Url::parse(raw) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => "<unparsable>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn a_package_key_or_its_reset_touches_the_package() {
        assert!(touches_package(&obj(json!({"model": "m"}))));
        assert!(touches_package(&obj(json!({"$reset": ["base_url"]}))));
        assert!(touches_package(&obj(json!({"$reset": "model"}))));
        assert!(!touches_package(&obj(
            json!({"http_referer": "https://example.com"})
        )));
        assert!(!touches_package(&obj(json!({"$reset": ["x_title"]}))));
    }

    #[test]
    fn the_line_hides_secrets_and_names_sources() {
        let p = LlmParams::parse(&json!({
            "provider": "openai", "model": "m", "api_key": "sk-secret",
            "base_url": "http://user:pw@127.0.0.1:1/v1",
            "provider_extra": {"seed": 7},
            "model_prompt": "quirks",
        }))
        .unwrap();
        let line = params_line("/a", &p, &obj(json!({"model": "m"})));
        assert!(
            line.starts_with("llm: params /a model=m[overlay] "),
            "{line}"
        );
        assert!(
            line.contains("base_url=http://127.0.0.1:1/v1[start]"),
            "{line}"
        );
        assert!(line.contains("provider_extra={seed}[start]"), "{line}");
        assert!(line.contains("model_prompt=len:6,sha:"), "{line}");
        for secret in ["sk-secret", "user:", "quirks"] {
            assert!(!line.contains(secret), "{secret} in {line}");
        }
        assert!(line.ends_with("(overlay: model)"), "{line}");
    }

    /// Review of L2, M-3: a key in the query never reaches the line.
    #[test]
    fn the_line_drops_query_and_fragment_of_the_endpoint() {
        assert_eq!(
            without_userinfo("https://u:p@host.example/v1?key=SECRET#frag"),
            "https://host.example/v1"
        );
    }

    #[test]
    fn the_line_names_the_requirement_by_the_registrys_hash() {
        let prose = "Answers one person briefly; fast and cheap.";
        let p = LlmParams::parse(&json!({
            "provider": "openai", "model": "m", "api_key": "k",
            "requirement": format!("  {prose}\n"),
        }))
        .unwrap();
        let line = params_line("/a", &p, &Map::new());
        let digest = Sha256::digest(prose.as_bytes());
        let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
        assert!(
            line.contains(&format!("requirement=len:{},sha:{hex}[start]", prose.len())),
            "{line}"
        );
        assert!(
            !line.contains("briefly"),
            "the prose stays out of the line: {line}"
        );
        let bare =
            LlmParams::parse(&json!({"provider": "openai", "model": "m", "api_key": "k"})).unwrap();
        assert!(!params_line("/a", &bare, &Map::new()).contains("requirement"));
    }
}
