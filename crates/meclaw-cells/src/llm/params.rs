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

/// How the cell authenticates against the provider (P10).
///
/// Vendor-neutral by design: `OauthSubscription` describes "a rotating OAuth
/// token from a token store", not an OpenAI specialty. OpenAI-via-Codex is
/// instance one, not the shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// Static `api_key` param, sent as `Authorization: Bearer` (the pre-P10
    /// behaviour and the default).
    #[default]
    ApiKey,
    /// Rotating OAuth access token read from the store at `auth_ref`.
    /// Refreshed lazily on 401 by the shared token broker.
    OauthSubscription,
}

/// Which provider wire format the cell speaks (P10).
///
/// Deliberately orthogonal to `provider`: the Responses API is the SAME vendor
/// with a different wire shape, not a different provider. Keeping this a
/// separate axis leaves the `provider == "openai"` constraint untouched
/// (plan D1) and makes `auth × wire_dialect` the vendor-neutral matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireDialect {
    /// `POST /chat/completions` — the pre-P10 dialect.
    ChatCompletions,
    /// `POST /responses` — typed `input[]` items, SSE transport.
    Responses,
}

/// Phase-8 LlmCell parameters (cell-types.md `llm`-params block), extended by
/// the P10 auth dimension.
///
/// Required: `provider`, `model`, plus exactly one credential — `api_key`
/// (for `auth: "api_key"`) or `auth_ref` (for `auth: "oauth_subscription"`).
/// All other fields have defaults. Only `provider == "openai"` is supported.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmParams {
    /// Provider id. Phase 8: must be `"openai"` (cell-types.md Z.92).
    pub provider: String,
    /// Model id (e.g. `"gpt-4o"`).
    pub model: String,
    /// API key for the provider. Required for `auth: "api_key"`, and MUST be
    /// absent for `auth: "oauth_subscription"` (no double credential).
    #[serde(default)]
    pub api_key: Option<String>,
    /// P10: how this cell authenticates. Default `api_key` — every pre-P10
    /// config keeps its exact behaviour.
    #[serde(default)]
    pub auth: AuthMode,
    /// P10: filesystem path to the OAuth token store (Codex `auth.json`
    /// format). Required for `auth: "oauth_subscription"`, forbidden otherwise.
    ///
    /// Deliberately has NO default: an implicit `~/.codex/auth.json` would let
    /// a cell silently rotate the refresh token of a live interactive Codex
    /// session. Pointing at a store is a config decision (plan D7).
    #[serde(default)]
    pub auth_ref: Option<String>,
    /// P10: explicit wire dialect. `None` derives it from `auth`
    /// (`api_key` → chat-completions, `oauth_subscription` → responses).
    /// Read it through `effective_wire_dialect()`, never directly.
    #[serde(default)]
    pub wire_dialect: Option<WireDialect>,
    /// P10: OAuth token endpoint override. `None` → the provider default
    /// (`auth::DEFAULT_OAUTH_TOKEN_ENDPOINT`). Tests point this at a fake.
    #[serde(default)]
    pub oauth_token_endpoint: Option<String>,
    /// P10: OAuth client id override. `None` → the provider default.
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    /// P10: value of the `originator` request header. `None` → the provider
    /// default (`codex_cli_rs`), which is what the pinned reference client
    /// sends.
    #[serde(default)]
    pub oauth_originator: Option<String>,
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
        // P10 auth validation — strictly ADDITIVE, after the untouched provider
        // check. Exactly one credential, and the dialect must be able to carry
        // the chosen auth. No message ever echoes a param VALUE (cell-types
        // Z.145 secret hygiene).
        match p.auth {
            AuthMode::ApiKey => {
                if p.api_key.is_none() {
                    return Err("api_key is required for auth 'api_key'".to_string());
                }
                if p.auth_ref.is_some() {
                    return Err("auth_ref is only valid for auth 'oauth_subscription'".to_string());
                }
            }
            AuthMode::OauthSubscription => {
                if p.auth_ref.is_none() {
                    return Err("auth_ref is required for auth 'oauth_subscription'".to_string());
                }
                if p.api_key.is_some() {
                    return Err("api_key must not be set for auth 'oauth_subscription'".to_string());
                }
                if p.wire_dialect == Some(WireDialect::ChatCompletions) {
                    return Err(
                        "auth 'oauth_subscription' requires wire_dialect 'responses'".to_string(),
                    );
                }
            }
        }
        Ok(p)
    }

    /// The wire dialect this cell actually speaks: the explicit
    /// `wire_dialect` param if set, else derived from `auth`.
    ///
    /// `parse` guarantees the derived value is consistent with `auth`, so this
    /// is total and never panics.
    pub fn effective_wire_dialect(&self) -> WireDialect {
        match self.wire_dialect {
            Some(d) => d,
            None => match self.auth {
                AuthMode::ApiKey => WireDialect::ChatCompletions,
                AuthMode::OauthSubscription => WireDialect::Responses,
            },
        }
    }

    /// Apply a runtime params-update (W4b). Pure — no IO.
    ///
    /// `update` is the top-level `params` body-slot of a params-update message
    /// (config.md § Access l.20): a partial map of param keys to new values,
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
    // P10 auth dimension.
    "auth",
    "auth_ref",
    "wire_dialect",
    "oauth_token_endpoint",
    "oauth_client_id",
    "oauth_originator",
];

/// Param keys that may NOT be changed at runtime via a params-update message.
///
/// `provider` (Phase-8 identity) and `api_key` (credential, secret-hygiene);
/// P10 adds the whole auth dimension — `auth`/`auth_ref` are credential
/// identity, and `wire_dialect`/`oauth_*` decide which endpoint a credential is
/// presented to. Letting a message repoint any of them would let a params
/// update send an existing token somewhere new.
pub(crate) const IMMUTABLE_PARAM_KEYS: &[&str] = &[
    "provider",
    "api_key",
    "auth",
    "auth_ref",
    "wire_dialect",
    "oauth_token_endpoint",
    "oauth_client_id",
    "oauth_originator",
];

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
        assert_eq!(p.api_key.as_deref(), Some("x"));
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
            "http_referer": "https://example.com",
            "x_title": "Example App",
        });
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.http_referer.as_deref(), Some("https://example.com"));
        assert_eq!(p.x_title.as_deref(), Some("Example App"));
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
        assert_eq!(new.api_key.as_deref(), Some("x")); // unchanged
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

    // ───── P10: auth dimension + wire dialect ─────

    fn api_key_raw() -> serde_json::Value {
        json!({"provider": "openai", "model": "gpt-4o", "api_key": "x"})
    }

    fn oauth_raw() -> serde_json::Value {
        json!({"provider": "openai", "model": "gpt-5", "auth": "oauth_subscription",
               "auth_ref": "/tmp/auth.json"})
    }

    #[test]
    fn parse_auth_defaults_to_api_key() {
        let p = LlmParams::parse(&api_key_raw()).unwrap();
        assert_eq!(p.auth, AuthMode::ApiKey);
        assert_eq!(p.effective_wire_dialect(), WireDialect::ChatCompletions);
    }

    #[test]
    fn parse_rejects_missing_api_key_in_api_key_mode() {
        let raw = json!({"provider": "openai", "model": "gpt-4o"});
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("api_key"), "{err}");
    }

    #[test]
    fn parse_accepts_oauth_subscription_with_auth_ref() {
        let p = LlmParams::parse(&oauth_raw()).unwrap();
        assert_eq!(p.auth, AuthMode::OauthSubscription);
        assert_eq!(p.auth_ref.as_deref(), Some("/tmp/auth.json"));
        assert_eq!(p.api_key, None);
    }

    #[test]
    fn parse_rejects_oauth_without_auth_ref() {
        let raw = json!({"provider": "openai", "model": "gpt-5",
                         "auth": "oauth_subscription"});
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("auth_ref"), "{err}");
    }

    #[test]
    fn parse_rejects_both_credentials() {
        let mut raw = oauth_raw();
        raw["api_key"] = json!("leaked-key");
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("api_key"), "{err}");
        // secret hygiene: the reject must never echo the value.
        assert!(!err.contains("leaked-key"), "error leaks value: {err}");
    }

    #[test]
    fn parse_rejects_auth_ref_in_api_key_mode() {
        let mut raw = api_key_raw();
        raw["auth_ref"] = json!("/tmp/auth.json");
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("auth_ref"), "{err}");
    }

    #[test]
    fn oauth_defaults_to_responses_dialect() {
        let p = LlmParams::parse(&oauth_raw()).unwrap();
        assert_eq!(p.effective_wire_dialect(), WireDialect::Responses);
    }

    #[test]
    fn parse_rejects_oauth_with_chat_completions_dialect() {
        let mut raw = oauth_raw();
        raw["wire_dialect"] = json!("chat_completions");
        let err = LlmParams::parse(&raw).unwrap_err();
        assert!(err.contains("wire_dialect"), "{err}");
    }

    #[test]
    fn api_key_mode_may_opt_into_responses_dialect() {
        // This is the lane the paid smoke uses: api.openai.com/v1/responses.
        let mut raw = api_key_raw();
        raw["wire_dialect"] = json!("responses");
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(p.effective_wire_dialect(), WireDialect::Responses);
    }

    #[test]
    fn apply_update_rejects_immutable_auth_keys() {
        let base = LlmParams::parse(&api_key_raw()).unwrap();
        for key in [
            "auth",
            "auth_ref",
            "wire_dialect",
            "oauth_token_endpoint",
            "oauth_client_id",
            "oauth_originator",
        ] {
            let update = json!({key: "whatever"}).as_object().unwrap().clone();
            let err = base.apply_update(&update).unwrap_err();
            assert!(
                matches!(err, super::ParamUpdateError::Immutable(ref k) if k == key),
                "key {key} must be immutable, got {err:?}"
            );
        }
    }

    #[test]
    fn oauth_endpoint_overrides_are_read() {
        let mut raw = oauth_raw();
        raw["oauth_token_endpoint"] = json!("http://127.0.0.1:9/token");
        raw["oauth_client_id"] = json!("client-abc");
        raw["oauth_originator"] = json!("meclaw_test");
        let p = LlmParams::parse(&raw).unwrap();
        assert_eq!(
            p.oauth_token_endpoint.as_deref(),
            Some("http://127.0.0.1:9/token")
        );
        assert_eq!(p.oauth_client_id.as_deref(), Some("client-abc"));
        assert_eq!(p.oauth_originator.as_deref(), Some("meclaw_test"));
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
