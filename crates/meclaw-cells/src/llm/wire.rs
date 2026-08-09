//! Phase-8 LlmCell HTTP-layer (OpenAI wire).
//!
//! T8: `WireError` + `wire_error_to_code`. T11: `call_openai` async I/O
//! function (single `tokio::time::timeout` wrapper for the OpenAI POST)
//! plus URL-constants and `redact_authorization` helper.
//!
//! API-Key-Hygiene (A1, Plan § 12): the Authorization-Bearer-header is set
//! per call inside `call_openai`. The API key is NEVER placed in body, meta,
//! log output, or error messages. `redact_authorization` exists so any
//! future tracing path that wants to debug-log headers can do so safely.

use reqwest::header::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// Default OpenAI base URL. The caller constructs the full URL by appending
/// `OPENAI_CHAT_COMPLETIONS_PATH` (T20 will do this in `LlmCell::handle`).
pub const OPENAI_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// OpenAI Chat-Completions endpoint path.
pub const OPENAI_CHAT_COMPLETIONS_PATH: &str = "/chat/completions";

/// Responses-API endpoint path (P10). Same path segment on the metered
/// endpoint and on the subscription backend; only the base URL differs
/// (reference `openai/codex` @266c6920, `codex-api/src/endpoint/responses.rs:100`).
pub const OPENAI_RESPONSES_PATH: &str = "/responses";

/// HTTP-layer error variants for OpenAI calls. Mapping to UBF `error_code`
/// is `wire_error_to_code`.
#[derive(Debug)]
pub enum WireError {
    /// HTTP request exceeded `params.external_timeout_ms` (A-Timeout).
    Timeout,
    /// HTTP 429 Too Many Requests.
    RateLimited,
    /// HTTP 401/403 — bad/missing API key.
    Unauthorized,
    /// HTTP 404 with OpenAI-style "model not found" error body.
    ModelNotFound,
    /// Other HTTP status (>= 400 not covered above).
    HttpStatus(u16),
    /// Network/IO error wrapping a reqwest text — MUST NEVER include the api_key.
    Network(String),
    /// Response body could not be parsed as JSON in the expected shape.
    BodyParse(String),
    /// P10: HTTP 429 with `error.type == "usage_limit_reached"` — the
    /// subscription's quota is spent. Distinct from ordinary rate limiting:
    /// retrying does not help until `resets_at`.
    QuotaExhausted {
        /// Unix seconds at which the quota window resets, when reported.
        resets_at: Option<i64>,
        /// Subscription plan the limit belongs to, when reported.
        plan_type: Option<String>,
    },
    /// P10: HTTP 429 with `error.type == "usage_not_included"` — the plan does
    /// not cover the requested model at all.
    PlanNotIncluded,
    /// P10: still 401 after a token refresh and one retry.
    AuthExpired,
    /// P10: the credential seam itself failed (store unreadable, refresh
    /// rejected). Carries the typed reason; never a token value.
    Auth(crate::llm::auth::AuthError),
    /// P10: 5xx or an overload signal — retryable in principle, but retrying
    /// is the topology's call, not the cell's.
    Transient(String),
}

/// Map `WireError` to the UBF `error_code`-Enum value (cell-types Z.112).
///
/// Only five `error_code`s exist for `llm`: `rate_limit`, `auth`, `timeout`,
/// `model_not_found`, `provider_error`. `HttpStatus`/`Network`/`BodyParse`
/// all fall into the `provider_error` catch-all bucket.
pub(crate) fn wire_error_to_code(err: &WireError) -> &'static str {
    match err {
        WireError::Timeout => "timeout",
        WireError::RateLimited | WireError::QuotaExhausted { .. } | WireError::PlanNotIncluded => {
            "rate_limit"
        }
        WireError::Unauthorized | WireError::AuthExpired | WireError::Auth(_) => "auth",
        WireError::ModelNotFound => "model_not_found",
        WireError::HttpStatus(_)
        | WireError::Network(_)
        | WireError::BodyParse(_)
        | WireError::Transient(_) => "provider_error",
    }
}

/// The fine-grained P10 failure kind, surfaced in `meta.error` (plan D10).
///
/// `docs/cell-types.md` Z.162 defines a CLOSED `error_code` enum without a
/// quota code, and `docs/` is not ours to change. So the taxonomy is two-level:
/// the spec `error_code` stays inside the enum (`wire_error_to_code`), while
/// the discriminator a failover edge actually needs lives here, in `meta`.
/// Returns `None` for the pre-P10 variants, whose meta stays byte-identical.
pub(crate) fn wire_error_meta(err: &WireError) -> Option<serde_json::Map<String, Value>> {
    let mut m = serde_json::Map::new();
    match err {
        WireError::QuotaExhausted {
            resets_at,
            plan_type,
        } => {
            m.insert("kind".into(), Value::String("quota_exhausted".into()));
            if let Some(r) = resets_at {
                m.insert("resets_at".into(), Value::from(*r));
            }
            if let Some(p) = plan_type {
                m.insert("plan_type".into(), Value::String(p.clone()));
            }
        }
        WireError::PlanNotIncluded => {
            m.insert("kind".into(), Value::String("plan_not_included".into()));
        }
        WireError::AuthExpired => {
            m.insert("kind".into(), Value::String("auth_expired".into()));
        }
        WireError::Auth(e) => {
            use crate::llm::auth::AuthError;
            let kind = match e {
                AuthError::StoreUnavailable(_) => "auth_store_unavailable",
                AuthError::RefreshPermanent(_) => "auth_permanent",
                AuthError::RefreshTransient(_) => "auth_refresh_transient",
            };
            m.insert("kind".into(), Value::String(kind.into()));
            if matches!(e, AuthError::RefreshPermanent(_)) {
                m.insert("re_login_required".into(), Value::Bool(true));
            }
        }
        WireError::Transient(_) => {
            m.insert("kind".into(), Value::String("transient".into()));
        }
        _ => return None,
    }
    Some(m)
}

/// Classify a non-2xx Responses reply (plan § 3.6, reference
/// `codex-api/src/api_bridge.rs:60-165`).
///
/// `body` is the parsed error payload, which may be `Null` when the server
/// answered with something unparseable.
pub(crate) fn classify_responses_status(status: u16, body: &Value) -> WireError {
    let err = body.get("error");
    let error_type = err
        .and_then(|e| e.get("type").or_else(|| e.get("code")))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match status {
        401 | 403 => WireError::Unauthorized,
        404 => WireError::ModelNotFound,
        429 => match error_type {
            "usage_limit_reached" => WireError::QuotaExhausted {
                resets_at: err
                    .and_then(|e| e.get("resets_at"))
                    .and_then(|v| v.as_i64()),
                plan_type: err
                    .and_then(|e| e.get("plan_type"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            },
            "usage_not_included" => WireError::PlanNotIncluded,
            _ => WireError::RateLimited,
        },
        500..=599 => WireError::Transient(format!("http {status}")),
        _ => WireError::HttpStatus(status),
    }
}

/// Execute the OpenAI Chat-Completions POST.
///
/// Single async I/O function for the LlmCell — every Translate result flows
/// through here. Wraps the full HTTP roundtrip in `tokio::time::timeout`
/// (A-Timeout) and maps every outcome to a `WireError` variant or the
/// parsed JSON response body.
///
/// `url` is the full URL (caller joins `base_url` + `OPENAI_CHAT_COMPLETIONS_PATH`).
/// `api_key` becomes the `Authorization: Bearer <key>` header — never logged.
/// `extra_headers` are provider-attribution request headers produced by the
/// Translate boundary (`translate::build_attribution_headers`, A4). They are
/// applied verbatim with one exception: an `Authorization` entry is refused
/// (case-insensitive) — `Authorization` is the single auth header, set from
/// `api_key`, and params can NOT override it (secret hygiene, A4 ruling).
/// `body` is the request payload as JSON. `timeout` is the A-Timeout budget.
pub async fn call_openai(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    extra_headers: &[(String, String)],
    body: &Value,
    timeout: Duration,
) -> Result<Value, WireError> {
    let request_fut = async {
        let mut req = client.post(url);
        for (name, value) in extra_headers {
            // Secret-hygiene guard: never let an attribution header clobber the
            // api_key Bearer. The attribution mapping is a closed allowlist that
            // never emits Authorization; this is belt-and-suspenders for any
            // future caller.
            if name.eq_ignore_ascii_case("authorization") {
                tracing::warn!("ignoring attempt to set Authorization via attribution header");
                continue;
            }
            req = req.header(name.as_str(), value.as_str());
        }
        // Authorization set LAST so it is authoritative regardless of input.
        let resp = req
            .header("Authorization", format!("Bearer {api_key}"))
            .json(body)
            .send()
            .await
            .map_err(|e| WireError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        match status {
            200..=299 => {
                let json = resp
                    .json::<Value>()
                    .await
                    .map_err(|e| WireError::BodyParse(e.to_string()))?;
                Ok(json)
            }
            401 => Err(WireError::Unauthorized),
            404 => Err(WireError::ModelNotFound),
            429 => Err(WireError::RateLimited),
            _ => Err(WireError::HttpStatus(status)),
        }
    };
    match tokio::time::timeout(timeout, request_fut).await {
        Ok(result) => result,
        Err(_) => Err(WireError::Timeout),
    }
}

/// Execute a Responses-API POST and return the raw response body as text.
///
/// Text, not JSON, because this wire is streamed: the body is an SSE event
/// stream. The cell hands it to `translate_responses::parse_responses_sse`
/// (or the JSON parser when a server answered non-streamed). Nothing is parsed
/// here — the wire layer stays free of provider semantics.
///
/// The whole roundtrip including body download sits inside one
/// `tokio::time::timeout` (A-timeout), so a stalled stream cannot hang the cell.
///
/// `bearer` is either the static `api_key` or a broker-issued access token; it
/// becomes `Authorization: Bearer <bearer>` and is set LAST so no entry in
/// `extra_headers` can displace it (same secret-hygiene guard as `call_openai`).
pub async fn call_responses(
    client: &reqwest::Client,
    url: &str,
    bearer: &str,
    extra_headers: &[(String, String)],
    body: &Value,
    timeout: Duration,
) -> Result<String, WireError> {
    let request_fut = async {
        let mut req = client.post(url);
        for (name, value) in extra_headers {
            if name.eq_ignore_ascii_case("authorization") {
                tracing::warn!("ignoring attempt to set Authorization via a mapped header");
                continue;
            }
            req = req.header(name.as_str(), value.as_str());
        }
        let resp = req
            .header("Authorization", format!("Bearer {bearer}"))
            .json(body)
            .send()
            .await
            .map_err(|e| WireError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| WireError::Network(e.to_string()))?;
        if (200..300).contains(&status) {
            return Ok(text);
        }
        let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Err(classify_responses_status(status, &parsed))
    };
    match tokio::time::timeout(timeout, request_fut).await {
        Ok(result) => result,
        Err(_) => Err(WireError::Timeout),
    }
}

/// Copy a `HeaderMap` into a `HashMap<String, String>` with the
/// `authorization` header value replaced by `<redacted>`.
///
/// Header names are kept case-as-iterated (reqwest yields them lowercased),
/// and non-UTF-8 header values fall back to an empty string. Intended for
/// any future `tracing::debug!`/`tracing::warn!` path that wants to inspect
/// headers on error without leaking the API key.
pub fn redact_authorization(headers: &HeaderMap) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (name, value) in headers.iter() {
        let key = name.as_str().to_string();
        if key.eq_ignore_ascii_case("authorization") {
            out.insert(key, "<redacted>".to_string());
        } else {
            out.insert(key, value.to_str().unwrap_or("").to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{WireError, redact_authorization, wire_error_to_code};
    use reqwest::header::HeaderMap;

    #[test]
    fn wire_error_timeout_maps_to_timeout() {
        assert_eq!(wire_error_to_code(&WireError::Timeout), "timeout");
    }

    #[test]
    fn wire_error_rate_limited_maps_to_rate_limit() {
        assert_eq!(wire_error_to_code(&WireError::RateLimited), "rate_limit");
    }

    #[test]
    fn wire_error_unauthorized_maps_to_auth() {
        assert_eq!(wire_error_to_code(&WireError::Unauthorized), "auth");
    }

    #[test]
    fn wire_error_model_not_found_maps_to_model_not_found() {
        assert_eq!(
            wire_error_to_code(&WireError::ModelNotFound),
            "model_not_found"
        );
    }

    #[test]
    fn wire_error_http_status_maps_to_provider_error() {
        assert_eq!(
            wire_error_to_code(&WireError::HttpStatus(500)),
            "provider_error"
        );
    }

    #[test]
    fn wire_error_network_maps_to_provider_error() {
        assert_eq!(
            wire_error_to_code(&WireError::Network("connection refused".into())),
            "provider_error"
        );
    }

    #[test]
    fn wire_error_body_parse_maps_to_provider_error() {
        assert_eq!(
            wire_error_to_code(&WireError::BodyParse("not json".into())),
            "provider_error"
        );
    }

    // ───── P10: the two-level taxonomy ─────

    use super::{classify_responses_status, wire_error_meta};
    use serde_json::json;

    #[test]
    fn maps_usage_limit_reached_to_quota_exhausted() {
        let body = json!({"error": {"type": "usage_limit_reached",
                                    "plan_type": "plus", "resets_at": 1786000000i64}});
        let e = classify_responses_status(429, &body);
        match &e {
            WireError::QuotaExhausted {
                resets_at,
                plan_type,
            } => {
                assert_eq!(*resets_at, Some(1786000000));
                assert_eq!(plan_type.as_deref(), Some("plus"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
        // spec enum stays closed …
        assert_eq!(wire_error_to_code(&e), "rate_limit");
        // … while meta carries the discriminator a failover edge needs.
        let m = wire_error_meta(&e).unwrap();
        assert_eq!(m["kind"], "quota_exhausted");
        assert_eq!(m["resets_at"], 1786000000i64);
        assert_eq!(m["plan_type"], "plus");
    }

    #[test]
    fn maps_usage_not_included_and_plain_429() {
        let not_included =
            classify_responses_status(429, &json!({"error": {"type": "usage_not_included"}}));
        assert!(matches!(not_included, WireError::PlanNotIncluded));
        assert_eq!(wire_error_to_code(&not_included), "rate_limit");
        assert_eq!(
            wire_error_meta(&not_included).unwrap()["kind"],
            "plan_not_included"
        );

        let plain = classify_responses_status(429, &serde_json::Value::Null);
        assert!(matches!(plain, WireError::RateLimited));
    }

    #[test]
    fn maps_auth_and_server_statuses() {
        assert!(matches!(
            classify_responses_status(401, &serde_json::Value::Null),
            WireError::Unauthorized
        ));
        let transient = classify_responses_status(503, &serde_json::Value::Null);
        assert!(matches!(transient, WireError::Transient(_)));
        assert_eq!(wire_error_to_code(&transient), "provider_error");
        assert_eq!(wire_error_meta(&transient).unwrap()["kind"], "transient");
    }

    #[test]
    fn auth_errors_map_to_auth_code_with_kinds() {
        use crate::llm::auth::AuthError;
        let cases = [
            (
                WireError::Auth(AuthError::StoreUnavailable("p".into())),
                "auth_store_unavailable",
            ),
            (
                WireError::Auth(AuthError::RefreshPermanent("refresh_token_reused".into())),
                "auth_permanent",
            ),
            (WireError::AuthExpired, "auth_expired"),
        ];
        for (e, kind) in cases {
            assert_eq!(wire_error_to_code(&e), "auth", "{e:?}");
            assert_eq!(wire_error_meta(&e).unwrap()["kind"], kind);
        }
        let permanent =
            WireError::Auth(AuthError::RefreshPermanent("refresh_token_expired".into()));
        assert_eq!(
            wire_error_meta(&permanent).unwrap()["re_login_required"],
            true
        );
    }

    #[test]
    fn pre_p10_variants_have_no_extra_meta() {
        // The legacy path's emitted message must stay byte-identical.
        for e in [
            WireError::Timeout,
            WireError::RateLimited,
            WireError::Unauthorized,
            WireError::ModelNotFound,
            WireError::HttpStatus(500),
            WireError::Network("x".into()),
            WireError::BodyParse("x".into()),
        ] {
            assert!(wire_error_meta(&e).is_none(), "{e:?} gained meta");
        }
    }

    #[test]
    fn error_debug_never_carries_a_token() {
        use crate::llm::auth::AuthError;
        let e = WireError::Auth(AuthError::RefreshPermanent("refresh_token_reused".into()));
        let s = format!("{e:?}");
        assert!(s.contains("refresh_token_reused"), "{s}");
        assert!(!s.to_lowercase().contains("bearer"), "{s}");
    }

    #[test]
    fn redact_authorization_replaces_value_in_unit() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer sk-secret-12345".parse().unwrap());
        h.insert("content-type", "application/json".parse().unwrap());
        let r = redact_authorization(&h);
        assert_eq!(r.get("authorization"), Some(&"<redacted>".to_string()));
        assert_eq!(r.get("content-type"), Some(&"application/json".to_string()));
    }
}
