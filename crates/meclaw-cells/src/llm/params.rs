//! Phase-8 LlmCell params (T5: full struct + serde-Deserialize + parse-validate).

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_temperature() -> f64 {
    0.7
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_external_timeout_ms() -> u64 {
    110_000
}

/// Phase-8 LlmCell parameters (cell-types.md `llm`-params block).
///
/// Required fields: `provider`, `model`, `api_key`. All other fields have
/// defaults. In Phase 8 only `provider == "openai"` is supported; other
/// providers are deferred to a future phase.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmParams {
    /// Provider id. Phase 8: must be `"openai"` (cell-types.md Z.92).
    pub provider: String,
    /// Model id (e.g. `"gpt-4o"`).
    pub model: String,
    /// API key for the provider.
    pub api_key: String,
    /// Optional override of the provider's base URL.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Sampling temperature; default 0.7.
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    /// Hard cap on generated tokens per response; default 4096.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Ordered list of UBF `system`-subtree paths to concatenate into the
    /// system prompt; default `[]`.
    #[serde(default)]
    pub system_order: Vec<String>,
    /// Pass-through map of provider-specific extras; default `{}`.
    #[serde(default)]
    pub provider_extra: serde_json::Map<String, serde_json::Value>,
    /// Operation-timeout (A) for the HTTP call to the provider, in
    /// milliseconds; default 110_000 (= 110 s, llm A-default per
    /// cell-types.md Z.1542).
    #[serde(default = "default_external_timeout_ms")]
    pub external_timeout_ms: u64,
    /// App-attribution: OpenRouter `HTTP-Referer` request header (app page /
    /// model rankings). A regular param (A4 params-uniform ruling): set in
    /// `config.json`, `${VAR}`-substituted from `.env` like any other param.
    /// `None` = no header emitted. The Translate boundary maps this param to a
    /// wire HTTP header (not the request body); see
    /// `translate::build_attribution_headers`.
    #[serde(default)]
    pub http_referer: Option<String>,
    /// App-attribution: OpenRouter `X-Title` request header (app display name
    /// in rankings). Same params-uniform handling as `http_referer`; `None` =
    /// no header emitted.
    #[serde(default)]
    pub x_title: Option<String>,
}

impl LlmParams {
    /// Implementation detail — production entry point is `LlmCellFactory`;
    /// direct construction is `pub` only so tests/integration tests can
    /// drive the cell without the full Colony.
    ///
    /// Validates `provider == "openai"` (Phase-8-Constraint, cell-types.md
    /// Z.92). All other fields are validated structurally by serde. The
    /// returned error message never echoes the `api_key` value
    /// (Plan § 12-API_KEY).
    #[doc(hidden)]
    pub fn parse(raw: &serde_json::Value) -> Result<Self, String> {
        let p: Self =
            serde_json::from_value(raw.clone()).map_err(|e| format!("invalid LlmParams: {e}"))?;
        if p.provider != "openai" {
            return Err(format!(
                "provider must be 'openai' in phase 8, got '{}'",
                p.provider
            ));
        }
        Ok(p)
    }

    /// Apply a runtime params-update (W4b). Pure — no IO.
    ///
    /// `update` is the top-level `params` body-slot of a params-update message
    /// (config.md § Zugriff Z.20): a partial map of param keys to new values,
    /// last-write-wins. Returns the merged `LlmParams` plus the overlay pairs to
    /// persist in `cell.db` (`state::persist_params_overlay`). The caller applies
    /// neither on `Err` — all-or-nothing, no partial apply.
    ///
    /// Reject rules (loud, no partial apply):
    /// - an `IMMUTABLE_PARAM_KEYS` key is present (`provider`, `api_key`) →
    ///   `Immutable` (credential / identity; mirrors the A4 `Authorization`
    ///   secret-hygiene ruling, cell-types.md § llm),
    /// - a key outside `KNOWN_PARAM_KEYS` is present → `Unknown` (a typo'd key
    ///   would otherwise silently no-op),
    /// - the merged result fails `LlmParams::parse` (wrong value type, etc.) →
    ///   `Invalid`.
    pub fn apply_update(
        &self,
        update: &serde_json::Map<String, Value>,
    ) -> Result<(LlmParams, Vec<(String, Value)>), ParamUpdateError> {
        // β: delegate to the generic params-overlay core. The immutable/unknown/
        // invalid reject rules + the merge-then-reparse live there; the per-type
        // key-sets + parse are supplied via `impl OverlayParams for LlmParams`.
        crate::params_overlay::apply_update(self, update)
    }
}

impl crate::params_overlay::OverlayParams for LlmParams {
    const KNOWN_KEYS: &'static [&'static str] = KNOWN_PARAM_KEYS;
    const IMMUTABLE_KEYS: &'static [&'static str] = IMMUTABLE_PARAM_KEYS;
    fn parse(raw: &Value) -> Result<Self, String> {
        LlmParams::parse(raw)
    }
}

/// Top-level param keys recognized by the `llm` cell (the `LlmParams` fields).
/// A params-update key outside this set is rejected (no silent no-op).
pub(crate) const KNOWN_PARAM_KEYS: &[&str] = &[
    "provider",
    "model",
    "api_key",
    "base_url",
    "temperature",
    "max_tokens",
    "system_order",
    "provider_extra",
    "external_timeout_ms",
    "http_referer",
    "x_title",
];

/// Param keys that may NOT be changed at runtime via a params-update message.
/// `provider` (Phase-8 identity) and `api_key` (credential, secret-hygiene).
pub(crate) const IMMUTABLE_PARAM_KEYS: &[&str] = &["provider", "api_key"];

/// β: the reject type now lives in the generic params-overlay core. Re-exported
/// here so existing `llm`-internal references (and W4b tests) are unchanged.
pub use crate::params_overlay::ParamUpdateError;

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn parse_rejects_empty_object() {
        let r = LlmParams::parse(&json!({}));
        assert!(
            r.is_err(),
            "empty params must reject (missing provider/model/api_key)"
        );
    }

    #[test]
    fn parse_minimal_valid_uses_defaults() {
        let raw = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.provider, "openai");
        assert_eq!(p.model, "gpt-4o");
        assert_eq!(p.api_key, "x");
        assert_eq!(p.base_url, None);
        assert_eq!(p.temperature, 0.7);
        assert_eq!(p.max_tokens, 4096);
        assert!(p.system_order.is_empty());
        assert!(p.provider_extra.is_empty());
        assert_eq!(p.external_timeout_ms, 110_000);
    }

    #[test]
    fn parse_attribution_fields_default_to_none() {
        let raw = json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"});
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.http_referer, None);
        assert_eq!(p.x_title, None);
    }

    #[test]
    fn parse_reads_attribution_fields() {
        let raw = json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x",
            "http_referer": "https://gisela.ai",
            "x_title": "Gisela",
        });
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.http_referer.as_deref(), Some("https://gisela.ai"));
        assert_eq!(p.x_title.as_deref(), Some("Gisela"));
    }

    #[test]
    fn apply_update_changes_mutable_model_and_returns_overlay() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"model": "gpt-4o-mini"}).as_object().unwrap().clone();
        let (new, overlay) = base.apply_update(&update).unwrap();
        assert_eq!(new.model, "gpt-4o-mini");
        assert_eq!(new.api_key, "x"); // unchanged
        assert_eq!(overlay, vec![("model".to_string(), json!("gpt-4o-mini"))]);
    }

    #[test]
    fn apply_update_merges_multiple_mutable_keys() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"temperature": 0.2, "max_tokens": 256})
            .as_object()
            .unwrap()
            .clone();
        let (new, _overlay) = base.apply_update(&update).unwrap();
        assert_eq!(new.temperature, 0.2);
        assert_eq!(new.max_tokens, 256);
    }

    #[test]
    fn apply_update_rejects_immutable_api_key() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"api_key": "leaked"}).as_object().unwrap().clone();
        let err = base.apply_update(&update).unwrap_err();
        assert!(matches!(err, super::ParamUpdateError::Immutable(ref k) if k == "api_key"));
        // detail must NOT echo the attempted value.
        assert!(
            !err.detail().contains("leaked"),
            "detail leaks value: {}",
            err.detail()
        );
    }

    #[test]
    fn apply_update_rejects_immutable_provider() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"provider": "openai"}).as_object().unwrap().clone();
        let err = base.apply_update(&update).unwrap_err();
        assert!(matches!(err, super::ParamUpdateError::Immutable(ref k) if k == "provider"));
    }

    #[test]
    fn apply_update_rejects_unknown_key() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"temperatur": 0.2}).as_object().unwrap().clone();
        let err = base.apply_update(&update).unwrap_err();
        assert!(matches!(err, super::ParamUpdateError::Unknown(ref k) if k == "temperatur"));
    }

    #[test]
    fn apply_update_rejects_malformed_value_no_partial() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        // model is valid but temperature is the wrong type → whole update rejects.
        let update = json!({"model": "gpt-4o-mini", "temperature": "hot"})
            .as_object()
            .unwrap()
            .clone();
        let err = base.apply_update(&update).unwrap_err();
        assert!(matches!(err, super::ParamUpdateError::Invalid(_)));
    }

    #[test]
    fn apply_update_immutable_wins_over_otherwise_valid_keys_no_partial() {
        let base = LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o", "api_key": "x"
        }))
        .unwrap();
        let update = json!({"model": "gpt-4o-mini", "api_key": "leaked"})
            .as_object()
            .unwrap()
            .clone();
        let err = base.apply_update(&update).unwrap_err();
        assert!(matches!(err, super::ParamUpdateError::Immutable(ref k) if k == "api_key"));
    }

    #[test]
    fn parse_rejects_non_openai_provider() {
        let raw = json!({"provider": "anthropic", "model": "claude-3", "api_key": "x"});
        let r = LlmParams::parse(&raw);
        assert!(r.is_err());
        let err = r.unwrap_err();
        assert!(
            err.contains("openai"),
            "error must mention provider constraint: {err}"
        );
    }
}
