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
    /// P14: like `HttpStatus`, but the body carried a flat `detail` string.
    ///
    /// The subscription backend does NOT use the OpenAI `{"error": {...}}`
    /// envelope for its own rejections — it answers `{"detail": "..."}`. Those
    /// texts are the only actionable diagnosis the operator gets (e.g.
    /// `"The '<model>' model requires a newer version of Codex."` → set
    /// `params.oauth_client_version`, or `"Unsupported parameter: temperature"`).
    /// Collapsing them into a bare `HttpStatus` threw away the one sentence
    /// that says what to do, which is how P10's two defects stayed invisible
    /// until they were reproduced outside the cell.
    ///
    /// Maps to the same `error_code` as `HttpStatus` (`provider_error`): the
    /// spec enum stays closed, only the free-text detail gets richer.
    HttpStatusWithDetail {
        /// The HTTP status that carried the detail.
        status: u16,
        /// Verbatim `detail` string from the provider. Never a credential:
        /// it is the provider's own prose about the request.
        detail: String,
    },
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
    /// GH #75: the gateway answered `HTTP 200` with a body that carries no
    /// `choices` at all, only a top level `error` object.
    ///
    /// That is the normal way an OpenAI-compatible gateway surfaces an upstream
    /// failure, and it is NOT a malformed response. Before #75 the translate
    /// stage saw the missing `choices[0]` and reported a parse defect, so a
    /// transient upstream 429 never reached the `rate_limit` lane and the
    /// provider's own sentence was replaced by a parser complaint.
    ///
    /// `inner` is the classification a real HTTP response with the same status
    /// would get — the identical table (`classify_responses_status`), so an
    /// in-body 429 lands in exactly the lane an HTTP-level 429 lands in.
    InBodyError {
        /// Upstream status the body reported, when it reported one.
        upstream_status: Option<u16>,
        /// Classification of that status. Never `InBodyError` itself.
        inner: Box<WireError>,
        /// The provider's own sentence about what happened. Provider prose,
        /// never a credential.
        message: String,
    },
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
        | WireError::HttpStatusWithDetail { .. }
        | WireError::Network(_)
        | WireError::BodyParse(_)
        | WireError::Transient(_) => "provider_error",
        // GH #75: an error inside a 200 body is the error it says it is. The
        // lane is decided by the wrapped classification, so an in-body 429 and
        // an HTTP-level 429 are indistinguishable to a failover edge.
        WireError::InBodyError { inner, .. } => wire_error_to_code(inner),
    }
}

/// The coarse `meta.error.kind` for a variant the P10 table does not name.
///
/// Only used for the [`WireError::InBodyError`] wrapper (GH #75): the wrapped
/// classification must always carry a `kind`, because that discriminator is the
/// whole point of surfacing the in-body error instead of a parse complaint.
/// The pre-P10 variants keep `None` on their own (byte-identity, see
/// `pre_p10_variants_have_no_extra_meta`).
fn coarse_kind(err: &WireError) -> &'static str {
    match err {
        WireError::Timeout => "timeout",
        WireError::RateLimited => "rate_limited",
        WireError::Unauthorized => "unauthorized",
        WireError::ModelNotFound => "model_not_found",
        _ => "provider_error",
    }
}

/// Lowercase needles that mark a rate-limit sentence when the body carries no
/// numeric status at all. Deliberately short and literal — a gateway that says
/// this is saying 429 in prose.
const RATE_LIMIT_NEEDLES: [&str; 4] = [
    "rate limit",
    "rate-limit",
    "rate_limit",
    "too many requests",
];

/// The upstream status an in-body `error` object reports, if any.
///
/// Gateways are not consistent: some put the HTTP status in `error.code` as a
/// number, some as a string, some in `error.status`, and some only say it in
/// prose. All four are read here so the classification below sees the same
/// number a real HTTP response would have carried.
fn in_body_status(err: &Value, message: &str) -> Option<u16> {
    let numeric = |v: Option<&Value>| -> Option<u16> {
        let v = v?;
        if let Some(n) = v.as_u64() {
            return u16::try_from(n).ok();
        }
        v.as_str()?.trim().parse::<u16>().ok()
    };
    if let Some(s) = numeric(err.get("code"))
        .or_else(|| numeric(err.get("status")))
        .or_else(|| numeric(err.get("http_status")))
    {
        return Some(s);
    }
    // No number anywhere: read the typed strings, then the prose.
    let typed = err
        .get("type")
        .or_else(|| err.get("code"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    if typed.contains("rate_limit") || typed == "insufficient_quota" {
        return Some(429);
    }
    let lower = message.to_lowercase();
    if RATE_LIMIT_NEEDLES.iter().any(|n| lower.contains(n)) {
        return Some(429);
    }
    None
}

/// Longest provider sentence carried into `meta.error`. Bounded so a gateway
/// that answers with a wall of text cannot flood the message log.
const IN_BODY_MESSAGE_MAX: usize = 500;

/// The provider's own sentence out of an in-body `error` value.
fn in_body_message(err: &Value) -> String {
    let raw = match err {
        Value::String(s) => s.clone(),
        _ => match err.get("message").and_then(|v| v.as_str()) {
            Some(m) => m.to_string(),
            None => serde_json::to_string(err).unwrap_or_default(),
        },
    };
    if raw.len() <= IN_BODY_MESSAGE_MAX {
        return raw;
    }
    let mut end = IN_BODY_MESSAGE_MAX;
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated, {} bytes total]", &raw[..end], raw.len())
}

/// Detect an upstream error returned INSIDE a 2xx body (GH #75).
///
/// Returns `None` for every body that is not that shape — in particular for a
/// body that carries `choices`, and for a body that carries neither `choices`
/// nor `error`. The second case stays a genuine parse failure: `missing
/// choices[0]` remains reserved for a response that really has no shape.
pub(crate) fn classify_in_body_error(body: &Value) -> Option<WireError> {
    // A body with `choices` is a completion. Whatever else it carries, the
    // translate stage owns it.
    if body.get("choices").is_some_and(|c| !c.is_null()) {
        return None;
    }
    let err = body.get("error").filter(|e| !e.is_null())?;
    let message = in_body_message(err);
    let upstream_status = in_body_status(err, &message);
    // Same table as a real HTTP status of that number. `0` for "the gateway
    // did not say" falls through to the generic provider_error bucket.
    let inner = classify_responses_status(upstream_status.unwrap_or(0), body);
    Some(WireError::InBodyError {
        upstream_status,
        inner: Box::new(inner),
        message,
    })
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
        // GH #75: the wrapped classification decides the kind (so an in-body
        // 429 says `rate_limited` and an in-body quota signal keeps its P10
        // kind), and the provenance says the error came in a 200 body plus what
        // the provider actually said.
        WireError::InBodyError {
            upstream_status,
            inner,
            message,
        } => {
            m = wire_error_meta(inner).unwrap_or_default();
            m.entry("kind".to_string())
                .or_insert_with(|| Value::String(coarse_kind(inner).into()));
            m.insert("in_body".into(), Value::Bool(true));
            if let Some(s) = upstream_status {
                m.insert("upstream_status".into(), Value::from(*s));
            }
            m.insert("upstream_message".into(), Value::String(message.clone()));
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
        // P14: the subscription backend rejects with a flat `{"detail": ...}`
        // and no `error` envelope, so nothing above matched and the actionable
        // sentence would be dropped. Keep it.
        _ => match body.get("detail").and_then(|v| v.as_str()) {
            Some(detail) => WireError::HttpStatusWithDetail {
                status,
                detail: detail.to_string(),
            },
            None => WireError::HttpStatus(status),
        },
    }
}

/// Wall-clock phases of ONE provider call (GH #124).
///
/// The issue's operator datapoint was "the provider says 2–4.5 s, the message
/// log says 16 s". Answering that needs the roundtrip split in two: the time
/// until the provider's response head arrives (which is the provider's own
/// latency plus the network) and the time the whole call took including body
/// download. A gap between them is ours, not the provider's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WireTimings {
    /// Milliseconds from issuing the request until the response head arrived
    /// (time to first byte). `None` when no head ever arrived — a network
    /// error or an elapsed A-timeout — so a failed call cannot masquerade as
    /// an instant one.
    pub ttfb_ms: Option<u64>,
    /// Milliseconds for the full HTTP roundtrip, body download included, and
    /// including the wait that ended in a timeout.
    pub total_ms: u64,
    /// Provider HTTP attempts inside this call. Always 1 for a single POST;
    /// the Responses lane's auth retry sums two calls into one figure.
    pub attempts: u32,
}

impl WireTimings {
    /// Fold a follow-up attempt of the same logical call into this one
    /// (the Responses lane's single auth retry).
    ///
    /// `ttfb_ms` becomes the LAST attempt's — that is the answer being
    /// returned — while `total_ms` and `attempts` accumulate, so the summary
    /// line shows what the retry ladder cost in total rather than only its
    /// final rung.
    #[must_use]
    pub(crate) fn plus_attempt(self, next: WireTimings) -> Self {
        Self {
            ttfb_ms: next.ttfb_ms,
            total_ms: self.total_ms.saturating_add(next.total_ms),
            attempts: self.attempts.saturating_add(next.attempts),
        }
    }
}

/// Milliseconds elapsed since `start`, saturated into `u64`.
fn ms_since(start: std::time::Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
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
    call_openai_timed(client, url, api_key, extra_headers, body, timeout)
        .await
        .0
}

/// [`call_openai`] plus the per-phase [`WireTimings`] of that call (GH #124).
///
/// Same request, same classification, same A-timeout — the only addition is
/// that the caller learns how the elapsed time split between waiting for the
/// provider's response head and everything after it. `call_openai` is the
/// timing-free wrapper, so every existing call site is byte-identical.
pub async fn call_openai_timed(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    extra_headers: &[(String, String)],
    body: &Value,
    timeout: Duration,
) -> (Result<Value, WireError>, WireTimings) {
    let started = std::time::Instant::now();
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
        let resp = match req
            .header("Authorization", format!("Bearer {api_key}"))
            .json(body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return (Err(WireError::Network(e.to_string())), None),
        };
        // The response head is here — everything after this point is body
        // download plus our own parsing (GH #124 discriminator).
        let ttfb = Some(ms_since(started));
        let status = resp.status().as_u16();
        let outcome = match status {
            200..=299 => {
                match resp.json::<Value>().await {
                    Ok(json) => {
                        // GH #75: a 2xx that carries an upstream `error` object
                        // instead of `choices` is a provider failure, not a
                        // response. Classify it here, at the wire boundary, so
                        // it reaches the same lane as the same failure
                        // delivered with a real HTTP status.
                        match classify_in_body_error(&json) {
                            Some(e) => Err(e),
                            None => Ok(json),
                        }
                    }
                    Err(e) => Err(WireError::BodyParse(e.to_string())),
                }
            }
            401 => Err(WireError::Unauthorized),
            404 => Err(WireError::ModelNotFound),
            429 => Err(WireError::RateLimited),
            _ => Err(WireError::HttpStatus(status)),
        };
        (outcome, ttfb)
    };
    let (result, ttfb_ms) = match tokio::time::timeout(timeout, request_fut).await {
        Ok(pair) => pair,
        Err(_) => (Err(WireError::Timeout), None),
    };
    let timings = WireTimings {
        ttfb_ms,
        total_ms: ms_since(started),
        attempts: 1,
    };
    (result, timings)
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
    call_responses_timed(client, url, bearer, extra_headers, body, timeout)
        .await
        .0
}

/// [`call_responses`] plus the per-phase [`WireTimings`] of that call (GH #124).
///
/// On this wire the split matters even more than on chat-completions: the body
/// is an SSE stream that is consumed to the end, so `ttfb_ms` is when the model
/// started answering and `total_ms - ttfb_ms` is how long the generation took
/// to stream out.
pub async fn call_responses_timed(
    client: &reqwest::Client,
    url: &str,
    bearer: &str,
    extra_headers: &[(String, String)],
    body: &Value,
    timeout: Duration,
) -> (Result<String, WireError>, WireTimings) {
    let started = std::time::Instant::now();
    let request_fut = async {
        let mut req = client.post(url);
        for (name, value) in extra_headers {
            if name.eq_ignore_ascii_case("authorization") {
                tracing::warn!("ignoring attempt to set Authorization via a mapped header");
                continue;
            }
            req = req.header(name.as_str(), value.as_str());
        }
        let resp = match req
            .header("Authorization", format!("Bearer {bearer}"))
            .json(body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return (Err(WireError::Network(e.to_string())), None),
        };
        // Response head in hand — the SSE body still has to stream out, and
        // that remainder is what `total_ms - ttfb_ms` shows (GH #124).
        let ttfb = Some(ms_since(started));
        let status = resp.status().as_u16();
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => return (Err(WireError::Network(e.to_string())), ttfb),
        };
        if (200..300).contains(&status) {
            return (Ok(text), ttfb);
        }
        let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        (Err(classify_responses_status(status, &parsed)), ttfb)
    };
    let (result, ttfb_ms) = match tokio::time::timeout(timeout, request_fut).await {
        Ok(pair) => pair,
        Err(_) => (Err(WireError::Timeout), None),
    };
    let timings = WireTimings {
        ttfb_ms,
        total_ms: ms_since(started),
        attempts: 1,
    };
    (result, timings)
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

    /// P14 — verbatim shape observed live on 2026-08-10. The subscription
    /// backend sends no `error` envelope at all, just a flat `detail`; before
    /// P14 this collapsed to a bare `HttpStatus(400)` and the one actionable
    /// sentence was lost.
    #[test]
    fn flat_detail_payload_survives_classification() {
        let body = json!({"detail":
            "The 'gpt-5.6-luna' model requires a newer version of Codex."});
        let e = classify_responses_status(400, &body);
        match &e {
            WireError::HttpStatusWithDetail { status, detail } => {
                assert_eq!(*status, 400);
                assert!(detail.contains("newer version of Codex"));
            }
            other => panic!("the diagnosable text was dropped: {other:?}"),
        }
        // The closed spec enum is unchanged; only the free text got richer.
        assert_eq!(wire_error_to_code(&e), "provider_error");
        // …and it reaches the operator, because meta.error.detail is built
        // from the Debug rendering of this value (cell.rs:32).
        assert!(format!("{e:?}").contains("newer version of Codex"));
    }

    #[test]
    fn a_400_without_any_text_stays_a_plain_http_status() {
        assert!(matches!(
            classify_responses_status(400, &serde_json::Value::Null),
            WireError::HttpStatus(400)
        ));
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

    // ───── GH #75: an upstream error inside a 200 body ─────

    use super::classify_in_body_error;

    /// Verbatim shape observed live on 2026-08-12: an OpenAI-compatible gateway
    /// answers `HTTP 200`, the body has no `choices` at all, only `error`, and
    /// the numeric code is the upstream status.
    #[test]
    fn in_body_429_lands_in_the_rate_limit_lane_like_a_real_429() {
        let body = json!({"error": {
            "message": "openai/gpt-x is temporarily rate-limited upstream. Please retry shortly.",
            "code": 429
        }});
        let e = classify_in_body_error(&body).expect("an in-body error must be classified");
        // Identical lane to the HTTP-level 429 (`call_openai`'s 429 arm).
        assert_eq!(wire_error_to_code(&e), "rate_limit");
        assert_eq!(
            wire_error_to_code(&e),
            wire_error_to_code(&WireError::RateLimited),
            "in-body and HTTP-level 429 must be indistinguishable to a failover edge"
        );
        let m = wire_error_meta(&e).expect("in-body errors always carry meta");
        assert_eq!(m["kind"], "rate_limited");
        assert_eq!(m["in_body"], true);
        assert_eq!(m["upstream_status"], 429);
        assert!(
            m["upstream_message"]
                .as_str()
                .unwrap()
                .contains("rate-limited upstream"),
            "the provider's own sentence must survive: {m:?}"
        );
        // …and it reaches meta.error.detail, which is built from the Debug form.
        assert!(format!("{e:?}").contains("rate-limited upstream"));
    }

    #[test]
    fn in_body_5xx_is_transient_provider_error() {
        let body = json!({"error": {"message": "upstream is having a moment", "code": 503}});
        let e = classify_in_body_error(&body).unwrap();
        assert_eq!(wire_error_to_code(&e), "provider_error");
        let m = wire_error_meta(&e).unwrap();
        assert_eq!(m["kind"], "transient");
        assert_eq!(m["upstream_status"], 503);
    }

    #[test]
    fn in_body_auth_and_model_errors_keep_their_lanes() {
        let auth =
            classify_in_body_error(&json!({"error": {"message": "bad key", "code": 401}})).unwrap();
        assert_eq!(wire_error_to_code(&auth), "auth");
        assert_eq!(wire_error_meta(&auth).unwrap()["kind"], "unauthorized");

        let missing =
            classify_in_body_error(&json!({"error": {"message": "no such model", "code": "404"}}))
                .unwrap();
        assert_eq!(wire_error_to_code(&missing), "model_not_found");
        assert_eq!(
            wire_error_meta(&missing).unwrap()["upstream_status"],
            404,
            "a stringified status is still a status"
        );
    }

    #[test]
    fn a_rate_limit_stated_only_in_prose_is_still_a_rate_limit() {
        // No numeric code anywhere — the sentence is all the gateway gives.
        let e = classify_in_body_error(&json!({"error": {
            "message": "Too Many Requests: please slow down"
        }}))
        .unwrap();
        assert_eq!(wire_error_to_code(&e), "rate_limit");
        assert_eq!(wire_error_meta(&e).unwrap()["kind"], "rate_limited");
    }

    #[test]
    fn an_in_body_quota_signal_keeps_its_p10_kind() {
        let e = classify_in_body_error(&json!({"error": {
            "type": "usage_limit_reached", "code": 429,
            "plan_type": "plus", "resets_at": 1786000000i64
        }}))
        .unwrap();
        assert_eq!(wire_error_to_code(&e), "rate_limit");
        let m = wire_error_meta(&e).unwrap();
        assert_eq!(m["kind"], "quota_exhausted");
        assert_eq!(m["resets_at"], 1786000000i64);
        assert_eq!(m["in_body"], true);
    }

    #[test]
    fn an_untyped_in_body_error_is_a_provider_error_not_a_parse_failure() {
        let e = classify_in_body_error(&json!({"error": {"message": "something went sideways"}}))
            .unwrap();
        assert_eq!(wire_error_to_code(&e), "provider_error");
        let m = wire_error_meta(&e).unwrap();
        assert_eq!(m["kind"], "provider_error");
        assert!(m.get("upstream_status").is_none(), "no status was reported");
        assert_eq!(m["upstream_message"], "something went sideways");
    }

    /// `missing choices[0]` stays reserved for a body that genuinely has no
    /// shape. This is the discriminator the whole fix hangs on.
    #[test]
    fn a_body_without_choices_and_without_error_is_not_classified_here() {
        assert!(classify_in_body_error(&json!({"id": "x", "object": "chat.completion"})).is_none());
        assert!(classify_in_body_error(&json!({})).is_none());
        assert!(
            classify_in_body_error(&json!({"choices": [], "error": null})).is_none(),
            "a null error next to choices is a normal completion"
        );
    }

    #[test]
    fn a_completion_that_also_carries_an_error_key_stays_a_completion() {
        let body = json!({"choices": [{"message": {"content": "hi"}, "finish_reason": "stop"}],
                          "error": {"message": "trailing junk"}});
        assert!(
            classify_in_body_error(&body).is_none(),
            "choices win — the translate stage owns a body that has them"
        );
    }

    #[test]
    fn an_error_given_as_a_bare_string_still_carries_its_text() {
        let e = classify_in_body_error(&json!({"error": "rate limit exceeded"})).unwrap();
        assert_eq!(wire_error_to_code(&e), "rate_limit");
        assert_eq!(
            wire_error_meta(&e).unwrap()["upstream_message"],
            "rate limit exceeded"
        );
    }

    #[test]
    fn a_wall_of_provider_text_is_bounded_in_meta() {
        let long = "x".repeat(5000);
        let e = classify_in_body_error(&json!({"error": {"message": long, "code": 500}})).unwrap();
        let m = wire_error_meta(&e).unwrap();
        let carried = m["upstream_message"].as_str().unwrap();
        assert!(
            carried.len() < 700,
            "message must be bounded: {}",
            carried.len()
        );
        assert!(carried.contains("[truncated,"), "the cut must be marked");
    }

    // ───── GH #124: phase timings ─────

    use super::WireTimings;

    /// The Responses lane's auth retry is two POSTs for one logical call. The
    /// summary must show what BOTH cost, and the time-to-first-byte of the
    /// answer that is actually returned — the second one.
    #[test]
    fn folding_a_retry_sums_the_cost_and_keeps_the_last_ttfb() {
        let first = WireTimings {
            ttfb_ms: Some(120),
            total_ms: 140,
            attempts: 1,
        };
        let second = WireTimings {
            ttfb_ms: Some(3_000),
            total_ms: 3_200,
            attempts: 1,
        };
        let folded = first.plus_attempt(second);
        assert_eq!(folded.attempts, 2, "a retry is a second attempt");
        assert_eq!(folded.total_ms, 3_340, "both attempts cost wall clock");
        assert_eq!(
            folded.ttfb_ms,
            Some(3_000),
            "the returned answer's own ttfb, not the rejected one's"
        );
    }

    #[test]
    fn folding_saturates_instead_of_overflowing() {
        let huge = WireTimings {
            ttfb_ms: None,
            total_ms: u64::MAX,
            attempts: u32::MAX,
        };
        let folded = huge.plus_attempt(huge);
        assert_eq!(folded.total_ms, u64::MAX);
        assert_eq!(folded.attempts, u32::MAX);
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
