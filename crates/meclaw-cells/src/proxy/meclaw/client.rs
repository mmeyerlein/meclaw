//! The outgoing half of the carrier: one POST under one operation timeout,
//! carrying the credential `params.auth` declares (GH #828).
//!
//! `Connection: close` is deliberate: the far mount serves per handed-over
//! connection (`crate::handed::serve_handed`), and a keep-alive outliving a
//! crossing would hold one of sixteen handoff-queue places for a peer with
//! nothing more to say.
//!
//! Redirects are not followed: a 3xx is a non-200 and ends as
//! `peer_unreachable`. Following one would re-POST the frame to a URL the peer
//! chose, and the receipt could come from a third party no operator declared.
//! The same holds for the token endpoint: a 3xx there is `auth_unavailable`,
//! and the client secret is never posted to an address nobody declared.
//!
//! The credential never leaves memory. The header values are marked sensitive,
//! no refusal detail quotes a secret or a token, and the token cache is a field
//! of this client, never a row in `cell.db`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONNECTION, CONTENT_TYPE, HeaderName, HeaderValue};
use serde_json::Value as JsonValue;

use super::lanes::Refusal;
use super::params::{OAuthAuth, PeerAuth};
use super::wire::{AUTH_UNAVAILABLE, INVALID_FRAME, PEER_REFUSED, PEER_TIMEOUT, PEER_UNREACHABLE};

/// At most this much of a token's life is given up to clock skew and the time
/// a request spends on the wire; a short-lived token gives up a tenth.
const EXPIRY_MARGIN: Duration = Duration::from_secs(30);

/// How much of a refusing proxy's body a detail quotes: enough for a sentence,
/// never a page.
const DETAIL_MAX: usize = 200;

/// The HTTP client a `meclaw` proxy posts frames with. `Clone` is cheap and
/// shares the token cache.
#[derive(Clone)]
pub struct PeerClient {
    inner: reqwest::Client,
    credential: Option<Arc<Credential>>,
}

/// The credential in its ready-to-send form.
enum Credential {
    /// One header, its value marked sensitive.
    Header {
        name: HeaderName,
        value: HeaderValue,
    },
    /// The client-credentials grant and the one token it holds.
    OAuth {
        grant: OAuthAuth,
        cache: Mutex<Option<CachedToken>>,
    },
}

/// A token and the instant it stops being offered.
struct CachedToken {
    bearer: HeaderValue,
    until: Instant,
}

impl PeerClient {
    /// Builds an anonymous client. A build failure (e.g. TLS init) is a spawn error.
    pub fn new() -> Result<Self, String> {
        Self::with_auth(None)
    }

    /// Builds a client that carries `auth` on every POST. The params were
    /// validated, so a header that does not build here is a spawn error that
    /// names the key and not the value.
    pub fn with_auth(auth: Option<&PeerAuth>) -> Result<Self, String> {
        let inner = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("reqwest build: {e}"))?;
        let credential = match auth {
            None => None,
            Some(PeerAuth::Header(a)) => {
                let name = HeaderName::from_bytes(a.header.as_bytes())
                    .map_err(|_| "auth.header: not a legal HTTP header name".to_string())?;
                let mut value = HeaderValue::from_str(&a.value)
                    .map_err(|_| "auth.value: not a legal HTTP header value".to_string())?;
                value.set_sensitive(true);
                Some(Arc::new(Credential::Header { name, value }))
            }
            Some(PeerAuth::OAuth(a)) => Some(Arc::new(Credential::OAuth {
                grant: a.clone(),
                cache: Mutex::new(None),
            })),
        };
        Ok(Self { inner, credential })
    }

    /// POSTs `frame` to `url` and returns the parsed answer.
    ///
    /// Without a credential this is one POST. With the static form it is one
    /// POST carrying the header. With the OAuth form the token comes first —
    /// from the cache, or from the endpoint under its own `timeout_ms` — and a
    /// `401` from the peer is answered by exactly one fresh token and one
    /// retry; a second `401` is `peer_refused`. A token that cannot be had is
    /// `auth_unavailable` and nothing is posted.
    ///
    /// Every POST is wrapped in `timeout_ms` (hard rule 12): elapsed is
    /// `peer_timeout`, anything the carrier did not finish is
    /// `peer_unreachable`, an answer that is not JSON is `invalid_frame`. The
    /// verdict inside a well-formed answer is `wire::read_receipt`'s to read.
    pub async fn post_frame(
        &self,
        url: &str,
        frame: &JsonValue,
        timeout_ms: u64,
    ) -> Result<JsonValue, Refusal> {
        let body = frame.to_string();
        match self.credential.as_deref() {
            None => self
                .post_once(url, &body, None, timeout_ms)
                .await?
                .json(url),
            Some(Credential::Header { name, value }) => {
                let cred = Some((name.clone(), value.clone()));
                match self.post_once(url, &body, cred, timeout_ms).await? {
                    Posted::Unauthorized(detail) => Err(Refusal::new(
                        PEER_REFUSED,
                        format!("{url} answered 401 to the credential in {name}: {detail}"),
                    )),
                    ok => ok.json(url),
                }
            }
            Some(Credential::OAuth { grant, cache }) => {
                let bearer = self.token(grant, cache, false, timeout_ms).await?;
                let cred = Some((AUTHORIZATION, bearer));
                match self.post_once(url, &body, cred, timeout_ms).await? {
                    Posted::Unauthorized(_) => {
                        // The far side may have revoked or rotated what we
                        // hold: one fresh token, one retry, never a loop.
                        let bearer = self.token(grant, cache, true, timeout_ms).await?;
                        let cred = Some((AUTHORIZATION, bearer));
                        match self.post_once(url, &body, cred, timeout_ms).await? {
                            Posted::Unauthorized(detail) => Err(Refusal::new(
                                PEER_REFUSED,
                                format!(
                                    "{url} answered 401 to a bearer token twice, the second \
                                     time to a fresh one: {detail}"
                                ),
                            )),
                            ok => ok.json(url),
                        }
                    }
                    ok => ok.json(url),
                }
            }
        }
    }

    /// One POST under the operation timeout. A `401` to a sent credential is
    /// returned for the caller to judge; every other non-200 is
    /// `peer_unreachable`.
    async fn post_once(
        &self,
        url: &str,
        body: &str,
        credential: Option<(HeaderName, HeaderValue)>,
        timeout_ms: u64,
    ) -> Result<Posted, Refusal> {
        let credential_sent = credential.is_some();
        let send = async {
            let mut req = self
                .inner
                .post(url)
                .header(CONTENT_TYPE, "application/json")
                .header(CONNECTION, "close")
                .body(body.to_string());
            if let Some((name, value)) = credential {
                req = req.header(name, value);
            }
            let resp = req.send().await.map_err(|e| {
                Refusal::new(PEER_UNREACHABLE, format!("no answer from {url}: {e}"))
            })?;
            let status = resp.status();
            // A 401 is read for the caller to judge only where a credential
            // was sent; every other non-200, and every 401 of an anonymous
            // client, is judged on the status line as before #828 -- a body
            // that stalls or breaks off must not turn it into `peer_timeout`
            // (review M2 of the #828 strand).
            let judged = status == StatusCode::UNAUTHORIZED && credential_sent;
            if status != StatusCode::OK && !judged {
                return Err(Refusal::new(
                    PEER_UNREACHABLE,
                    format!("{url} answered {status}, not 200"),
                ));
            }
            let bytes = resp.bytes().await.map_err(|e| {
                Refusal::new(
                    PEER_UNREACHABLE,
                    format!("the answer from {url} broke off: {e}"),
                )
            })?;
            if judged {
                return Ok(Posted::Unauthorized(excerpt(&bytes)));
            }
            Ok(Posted::Answer(bytes.to_vec()))
        };
        match tokio::time::timeout(Duration::from_millis(timeout_ms), send).await {
            Ok(result) => result,
            Err(_) => Err(Refusal::new(
                PEER_TIMEOUT,
                format!("no receipt from {url} within {timeout_ms} ms"),
            )),
        }
    }

    /// The bearer header: the cached token while it lives, otherwise one grant
    /// against the token endpoint. `fresh` drops the cache first (after a 401).
    async fn token(
        &self,
        grant: &OAuthAuth,
        cache: &Mutex<Option<CachedToken>>,
        fresh: bool,
        timeout_ms: u64,
    ) -> Result<HeaderValue, Refusal> {
        // The lock is never held across an await: the handler is one task, and
        // a clone of this client on a respawned life must not wait on it.
        if let Ok(mut held) = cache.lock() {
            if fresh {
                *held = None;
            } else if let Some(t) = held.as_ref().filter(|t| Instant::now() < t.until) {
                return Ok(t.bearer.clone());
            }
        }
        let url = grant.token_url.as_str();
        let unavailable = |why: String| Refusal::new(AUTH_UNAVAILABLE, why);
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "client_credentials"),
            ("client_id", grant.client_id.as_str()),
            ("client_secret", grant.client_secret.as_str()),
        ];
        if !grant.scope.is_empty() {
            form.push(("scope", grant.scope.as_str()));
        }
        if !grant.audience.is_empty() {
            form.push(("audience", grant.audience.as_str()));
        }
        let ask = async {
            let resp = self
                .inner
                .post(url)
                .header(ACCEPT, "application/json")
                .header(CONNECTION, "close")
                .form(&form)
                .send()
                .await
                // `without_url`: the error text is the carrier's; the URL is
                // named once, by this detail.
                .map_err(|e| {
                    unavailable(format!(
                        "no answer from the token endpoint {url}: {}",
                        e.without_url()
                    ))
                })?;
            let status = resp.status();
            let bytes = resp.bytes().await.map_err(|e| {
                unavailable(format!(
                    "the answer from the token endpoint {url} broke off: {}",
                    e.without_url()
                ))
            })?;
            if !status.is_success() {
                return Err(unavailable(format!(
                    "the token endpoint {url} answered {status}{}",
                    oauth_error(&bytes)
                        .map(|e| format!(" ({e})"))
                        .unwrap_or_default()
                )));
            }
            read_token(&bytes).map_err(|why| {
                unavailable(format!(
                    "the token endpoint {url} answered no usable token: {why}"
                ))
            })
        };
        let (bearer, held_for) =
            match tokio::time::timeout(Duration::from_millis(timeout_ms), ask).await {
                Ok(r) => r?,
                Err(_) => {
                    return Err(unavailable(format!(
                        "no answer from the token endpoint {url} within {timeout_ms} ms"
                    )));
                }
            };
        if let Some(life) = held_for
            && let Ok(mut held) = cache.lock()
        {
            *held = Some(CachedToken {
                bearer: bearer.clone(),
                until: Instant::now() + life,
            });
        }
        Ok(bearer)
    }
}

/// What one POST came back with, before the verdict is read.
enum Posted {
    /// `200`, the answer's bytes.
    Answer(Vec<u8>),
    /// `401`, with an excerpt of the refusing side's body.
    Unauthorized(String),
}

impl Posted {
    /// The answer as JSON; a `401` here is a caller that did not judge it,
    /// which for an anonymous client is what it always was: a non-200.
    fn json(self, url: &str) -> Result<JsonValue, Refusal> {
        match self {
            Posted::Answer(bytes) => serde_json::from_slice::<JsonValue>(&bytes).map_err(|e| {
                Refusal::new(
                    INVALID_FRAME,
                    format!("the answer from {url} is not JSON: {e}"),
                )
            }),
            Posted::Unauthorized(_) => Err(Refusal::new(
                PEER_UNREACHABLE,
                format!("{url} answered 401 Unauthorized, not 200"),
            )),
        }
    }
}

/// The first `DETAIL_MAX` characters of a body, as one line without control
/// characters: the far side's bytes land in our receipt, and an escape
/// sequence or a NUL has no business there (review M6 of the #828 strand).
fn excerpt(bytes: &[u8]) -> String {
    let text: String = String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| c.is_whitespace() || !c.is_control())
        .collect();
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return "(no body)".to_string();
    }
    line.chars().take(DETAIL_MAX).collect()
}

/// The OAuth `error` code of a refusal (RFC 6749 § 5.2), if it is one: a short
/// word from `[a-z_]`, never the free-text description.
fn oauth_error(bytes: &[u8]) -> Option<String> {
    let v: JsonValue = serde_json::from_slice(bytes).ok()?;
    let e = v.get("error")?.as_str()?;
    (e.len() <= 64 && e.chars().all(|c| c.is_ascii_lowercase() || c == '_')).then(|| e.to_string())
}

/// A token response (RFC 6749 § 5.1): the bearer header and how long to hold
/// it. No `expires_in` means the token is used once and not cached.
fn read_token(bytes: &[u8]) -> Result<(HeaderValue, Option<Duration>), String> {
    let v: JsonValue = serde_json::from_slice(bytes).map_err(|_| "not JSON".to_string())?;
    let token = v
        .get("access_token")
        .and_then(JsonValue::as_str)
        .filter(|t| !t.is_empty())
        .ok_or("no access_token")?;
    if let Some(kind) = v.get("token_type").and_then(JsonValue::as_str)
        && !kind.eq_ignore_ascii_case("bearer")
    {
        return Err(format!("token_type {kind:?}, not Bearer"));
    }
    let mut bearer = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "an access_token that is no legal header value".to_string())?;
    bearer.set_sensitive(true);
    // Some providers send the number as a string.
    let expires_in = match v.get("expires_in") {
        Some(JsonValue::Number(n)) => n.as_u64(),
        Some(JsonValue::String(s)) => s.parse::<u64>().ok(),
        _ => None,
    };
    Ok((bearer, expires_in.and_then(lifetime)))
}

/// How long a token given for `expires_in` seconds is offered: its life minus
/// a margin of a tenth, at most [`EXPIRY_MARGIN`]. Zero is not cached.
fn lifetime(expires_in: u64) -> Option<Duration> {
    let life = Duration::from_secs(expires_in);
    let margin = (life / 10).min(EXPIRY_MARGIN);
    let held = life.saturating_sub(margin);
    (!held.is_zero()).then_some(held)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_held_for_its_life_minus_the_margin() {
        assert_eq!(lifetime(0), None);
        assert_eq!(lifetime(1), Some(Duration::from_millis(900)));
        assert_eq!(lifetime(3600), Some(Duration::from_secs(3570)));
        assert_eq!(lifetime(100), Some(Duration::from_secs(90)));
    }

    #[test]
    fn a_token_response_is_read_strictly() {
        let (b, life) =
            read_token(br#"{"access_token":"abc","token_type":"bearer","expires_in":"60"}"#)
                .expect("reads");
        assert!(b.is_sensitive());
        assert_eq!(b.to_str().ok(), Some("Bearer abc"));
        assert_eq!(life, Some(Duration::from_secs(54)));
        assert!(
            read_token(br#"{"access_token":"abc"}"#)
                .expect("reads")
                .1
                .is_none()
        );
        assert!(read_token(br#"{"access_token":""}"#).is_err());
        assert!(read_token(br#"{"access_token":"abc","token_type":"mac"}"#).is_err());
        assert!(read_token(b"<html>").is_err());
    }

    #[test]
    fn only_a_short_oauth_error_code_is_quoted() {
        assert_eq!(
            oauth_error(br#"{"error":"invalid_client"}"#).as_deref(),
            Some("invalid_client")
        );
        assert_eq!(oauth_error(br#"{"error":"Bad client s3cret"}"#), None);
        assert_eq!(oauth_error(b"nope"), None);
    }

    #[test]
    fn an_excerpt_is_one_bounded_line() {
        assert_eq!(excerpt(b"a\n  b"), "a b");
        assert_eq!(excerpt(b""), "(no body)");
        assert_eq!(excerpt(&[b'x'; 500]).len(), DETAIL_MAX);
    }

    /// GH #828 fix round 1 (review M6): the far side's body goes into our
    /// receipt; a control character (an ANSI escape, a NUL) never does.
    #[test]
    fn an_excerpt_carries_no_control_characters() {
        let e = excerpt(b"a\x1b[31mb\x00c\x7f d\r\n\x07e");
        assert!(!e.chars().any(char::is_control), "{e:?}");
        assert_eq!(e, "a[31mbc d e");
    }

    /// A listener that answers every request with `head` and then holds the
    /// connection open without the body the head promised.
    async fn stalled_answer(head: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = l.local_addr().expect("addr");
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut b = [0u8; 4096];
                    let _ = s.read(&mut b).await;
                    let _ = s.write_all(head.as_bytes()).await;
                    std::future::pending::<()>().await;
                });
            }
        });
        format!("http://{addr}/peer/")
    }

    /// GH #828 fix round 1 (review M2): without a credential a non-200 is
    /// judged on its status line, as before #828 -- the body is never waited
    /// for, so a 503 or 401 whose body stalls is `peer_unreachable` naming
    /// the status, not `peer_timeout` and not "broke off".
    #[tokio::test]
    async fn the_anonymous_path_judges_a_non_200_by_its_status_alone() {
        let client = PeerClient::new().expect("client");
        for (head, status) in [
            (
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 100\r\n\r\n",
                "503",
            ),
            (
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: 100\r\n\r\n",
                "401",
            ),
        ] {
            let url = stalled_answer(head).await;
            let r = client
                .post_frame(&url, &serde_json::json!({}), 2000)
                .await
                .expect_err("refused");
            assert_eq!(r.error_code, PEER_UNREACHABLE, "{}", r.detail);
            assert!(r.detail.contains(status), "{}", r.detail);
            assert!(r.detail.contains("not 200"), "{}", r.detail);
        }
    }
}
