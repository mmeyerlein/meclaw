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
}

/// Map `WireError` to the UBF `error_code`-Enum value (cell-types Z.112).
///
/// Only five `error_code`s exist for `llm`: `rate_limit`, `auth`, `timeout`,
/// `model_not_found`, `provider_error`. `HttpStatus`/`Network`/`BodyParse`
/// all fall into the `provider_error` catch-all bucket.
pub(crate) fn wire_error_to_code(err: &WireError) -> &'static str {
    match err {
        WireError::Timeout => "timeout",
        WireError::RateLimited => "rate_limit",
        WireError::Unauthorized => "auth",
        WireError::ModelNotFound => "model_not_found",
        WireError::HttpStatus(_) | WireError::Network(_) | WireError::BodyParse(_) => {
            "provider_error"
        }
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
