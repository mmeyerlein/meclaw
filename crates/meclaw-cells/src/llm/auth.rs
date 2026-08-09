//! P10 — OAuth token store + refresh (the vendor-neutral credential seam).
//!
//! Wire pinned against the reference implementation `github.com/openai/codex`
//! @ `266c6920d9b82fe4d68959529565256b12a9be99` (read 2026-08-09):
//!
//! - token endpoint + client id: `login/src/auth/manager.rs:192, :1618`
//! - refresh request is **JSON**, not form-urlencoded: `manager.rs:1506-1524`
//! - refresh response has **no `expires_in`**: `manager.rs:1603-1615`
//! - store layout: `login/src/auth/storage.rs:40-61`, `login/src/token_data.rs:11-26`
//! - permanent refresh failures: `manager.rs:1548-1575`
//!
//! **Secret hygiene (cell-types.md § llm Z.145, extended to this path):** no
//! function here ever puts a token value into an error, a log line, or a
//! returned string. Errors carry a path or an error *code*, never contents.

use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// Default OAuth token endpoint (reference: `manager.rs:192`).
pub const DEFAULT_OAUTH_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";

/// Default OAuth client id (reference: `manager.rs:1618`).
pub const DEFAULT_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// Default `originator` request header (reference: `default_client.rs:40`).
pub const DEFAULT_OAUTH_ORIGINATOR: &str = "codex_cli_rs";

/// Default base URL of the subscription backend (reference:
/// `model-provider-info/src/lib.rs:37`). Note: no `/v1` segment.
pub const DEFAULT_SUBSCRIPTION_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// A-timeout for the refresh POST. Deliberately a constant, not a param: the
/// refresh runs inside the broker actor and must never stall it for long.
pub const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);

/// Failure modes of the credential seam.
///
/// The split that matters to the cell: `RefreshPermanent` means a human must
/// log in again, everything else is retryable in principle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// Store missing, unreadable, malformed, or without a refresh token.
    /// Carries a path and a reason — **never** file contents.
    StoreUnavailable(String),
    /// The refresh token is dead (`refresh_token_expired` / `_reused` /
    /// `_invalidated`). Re-login required; retrying cannot help.
    RefreshPermanent(String),
    /// Refresh failed for a transient reason (network, 5xx, timeout).
    RefreshTransient(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StoreUnavailable(d) => write!(f, "token store unavailable: {d}"),
            Self::RefreshPermanent(c) => {
                write!(
                    f,
                    "token refresh failed permanently ({c}); re-login required"
                )
            }
            Self::RefreshTransient(d) => write!(f, "token refresh failed transiently: {d}"),
        }
    }
}

impl std::error::Error for AuthError {}

/// The token triple this seam cares about, read out of a store file.
///
/// Deliberately NOT `Serialize`/`Debug`-derived with values: see
/// the manual `Debug` below.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredTokens {
    /// Bearer token presented to the provider.
    pub access_token: String,
    /// Token used to obtain the next access token. Rotates on every refresh.
    pub refresh_token: String,
    /// `ChatGPT-Account-ID` header value, when the store carries one.
    pub account_id: Option<String>,
}

/// Redacting `Debug` — a `{:?}` of a token bundle must not leak it, no matter
/// which future call site formats it.
impl std::fmt::Debug for StoredTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredTokens")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("account_id", &self.account_id)
            .finish()
    }
}

/// Read the token store at `path` (Codex `auth.json` layout).
///
/// Blocking filesystem call — callers in async context wrap it in
/// `spawn_blocking` (see `token_broker`).
pub fn read_store(path: &Path) -> Result<StoredTokens, AuthError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| AuthError::StoreUnavailable(format!("{}: {}", path.display(), e.kind())))?;
    let json: Value = serde_json::from_str(&raw)
        .map_err(|_| AuthError::StoreUnavailable(format!("{}: not valid JSON", path.display())))?;
    let tokens = json.get("tokens").ok_or_else(|| {
        AuthError::StoreUnavailable(format!("{}: no 'tokens' object", path.display()))
    })?;
    let field = |name: &str| {
        tokens
            .get(name)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let access_token = field("access_token").ok_or_else(|| {
        AuthError::StoreUnavailable(format!("{}: no tokens.access_token", path.display()))
    })?;
    let refresh_token = field("refresh_token").ok_or_else(|| {
        AuthError::StoreUnavailable(format!("{}: no tokens.refresh_token", path.display()))
    })?;
    Ok(StoredTokens {
        access_token,
        refresh_token,
        account_id: field("account_id"),
    })
}

/// Write rotated tokens back into the store at `path`, **preserving every
/// field we do not own**.
///
/// # Co-ownership
///
/// **The store also belongs to the vendor CLI; we patch only our three fields
/// plus `last_refresh`.** It is not a MeClaw file that a CLI happens to read —
/// it is the CLI's own credential file that MeClaw is a second writer of. It
/// carries fields this crate knows nothing about (`auth_mode`,
/// `OPENAI_API_KEY`, `id_token`, `agent_identity`, …), and a naive rewrite
/// would destroy an interactive login the first time a cell rotates a token.
///
/// So this is a patch, not a rewrite: read the file, replace
/// `tokens.access_token`, `tokens.refresh_token`, `tokens.account_id` and
/// `last_refresh`, write everything else back untouched. Pinned by
/// `write_store_preserves_unknown_fields`.
///
/// Atomic: writes a sibling temp file, `fsync`s it, then `rename`s over the
/// target, so a crash mid-write can never leave a half-written credential.
/// Unix permissions are forced to `0600`.
pub fn write_store(path: &Path, tokens: &StoredTokens) -> Result<(), AuthError> {
    let unavailable = |d: String| AuthError::StoreUnavailable(format!("{}: {}", path.display(), d));

    let mut json: Value = match std::fs::read_to_string(path) {
        Ok(raw) => {
            serde_json::from_str(&raw).map_err(|_| unavailable("not valid JSON".to_string()))?
        }
        Err(e) => return Err(unavailable(format!("{}", e.kind()))),
    };
    let obj = json
        .as_object_mut()
        .ok_or_else(|| unavailable("store root is not an object".to_string()))?;
    let slot = obj
        .entry("tokens")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let slot = slot
        .as_object_mut()
        .ok_or_else(|| unavailable("'tokens' is not an object".to_string()))?;
    slot.insert(
        "access_token".into(),
        Value::String(tokens.access_token.clone()),
    );
    slot.insert(
        "refresh_token".into(),
        Value::String(tokens.refresh_token.clone()),
    );
    if let Some(id) = &tokens.account_id {
        slot.insert("account_id".into(), Value::String(id.clone()));
    }
    obj.insert(
        "last_refresh".into(),
        Value::String(chrono::Utc::now().to_rfc3339()),
    );

    let serialized = serde_json::to_string_pretty(&json).map_err(|e| unavailable(e.to_string()))?;
    write_atomic_0600(path, serialized.as_bytes()).map_err(unavailable)
}

/// Temp-file + fsync + rename, with `0600` on the temp file before it moves
/// into place (so the credential is never briefly world-readable).
fn write_atomic_0600(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let tmp = path.with_extension("tmp-meclaw");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).map_err(|e| e.to_string())?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Exchange a refresh token for a fresh token bundle.
///
/// Request shape is pinned to the reference client: a JSON body with
/// `client_id` / `grant_type` / `refresh_token` (NOT form-urlencoded), and a
/// response whose fields are all optional and carry **no** expiry.
///
/// The rotated `refresh_token` is returned when the server sends one, else the
/// old one is carried forward (the reference treats the field as optional).
pub async fn refresh_tokens(
    client: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    current: &StoredTokens,
) -> Result<StoredTokens, AuthError> {
    let body = serde_json::json!({
        "client_id": client_id,
        "grant_type": "refresh_token",
        "refresh_token": current.refresh_token,
    });
    let fut = async {
        let resp = client
            .post(token_endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|e| AuthError::RefreshTransient(sanitize(&e.to_string())))?;
        let status = resp.status().as_u16();
        let json: Value = resp.json().await.unwrap_or(Value::Null);
        if (200..300).contains(&status) {
            let field = |n: &str| json.get(n).and_then(|v| v.as_str()).map(str::to_string);
            let access_token = field("access_token").ok_or_else(|| {
                AuthError::RefreshTransient("response carried no access_token".to_string())
            })?;
            return Ok(StoredTokens {
                access_token,
                refresh_token: field("refresh_token")
                    .unwrap_or_else(|| current.refresh_token.clone()),
                account_id: current.account_id.clone(),
            });
        }
        let code = json
            .get("error")
            .and_then(|e| e.get("code").or_else(|| e.get("type")))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if is_permanent_refresh_error(&code) {
            Err(AuthError::RefreshPermanent(code))
        } else {
            Err(AuthError::RefreshTransient(format!("http {status}")))
        }
    };
    match tokio::time::timeout(REFRESH_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err(AuthError::RefreshTransient("refresh timed out".to_string())),
    }
}

/// Refresh-error codes that no retry can fix (reference: `manager.rs:1548-1575`).
pub(crate) fn is_permanent_refresh_error(code: &str) -> bool {
    matches!(
        code,
        "refresh_token_expired" | "refresh_token_reused" | "refresh_token_invalidated"
    )
}

/// Strip anything token-shaped out of a third-party error string before it can
/// reach a log or an emitted message. Belt-and-suspenders: reqwest errors carry
/// URLs, and a token could in principle sit in a query string.
fn sanitize(detail: &str) -> String {
    detail
        .split_whitespace()
        .map(|w| {
            if w.len() > 24 && w.chars().any(|c| c.is_ascii_alphanumeric()) {
                "<redacted>"
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_store_file(dir: &tempfile::TempDir, v: &Value) -> std::path::PathBuf {
        let p = dir.path().join("auth.json");
        std::fs::write(&p, serde_json::to_string_pretty(v).unwrap()).unwrap();
        p
    }

    fn full_store() -> Value {
        json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": "jwt-id-dummy",
                "access_token": "access-dummy-1",
                "refresh_token": "refresh-dummy-1",
                "account_id": "acct-dummy"
            },
            "last_refresh": "2026-08-08T20:51:57Z"
        })
    }

    #[test]
    fn read_store_extracts_tokens() {
        let td = tempfile::TempDir::new().unwrap();
        let p = write_store_file(&td, &full_store());
        let t = read_store(&p).unwrap();
        assert_eq!(t.access_token, "access-dummy-1");
        assert_eq!(t.refresh_token, "refresh-dummy-1");
        assert_eq!(t.account_id.as_deref(), Some("acct-dummy"));
    }

    #[test]
    fn read_store_missing_file_is_store_unavailable() {
        let td = tempfile::TempDir::new().unwrap();
        let err = read_store(&td.path().join("nope.json")).unwrap_err();
        assert!(matches!(err, AuthError::StoreUnavailable(_)));
    }

    #[test]
    fn read_store_without_refresh_token_is_store_unavailable() {
        let td = tempfile::TempDir::new().unwrap();
        let p = write_store_file(&td, &json!({"tokens": {"access_token": "a"}}));
        let err = read_store(&p).unwrap_err();
        match err {
            AuthError::StoreUnavailable(d) => assert!(d.contains("refresh_token"), "{d}"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn debug_of_stored_tokens_redacts_both_secrets() {
        let t = StoredTokens {
            access_token: "access-dummy-1".into(),
            refresh_token: "refresh-dummy-1".into(),
            account_id: Some("acct-dummy".into()),
        };
        let s = format!("{t:?}");
        assert!(!s.contains("access-dummy-1"), "{s}");
        assert!(!s.contains("refresh-dummy-1"), "{s}");
        assert!(s.contains("acct-dummy"), "account id is not a secret: {s}");
    }

    #[test]
    fn write_store_preserves_unknown_fields() {
        let td = tempfile::TempDir::new().unwrap();
        let p = write_store_file(&td, &full_store());
        write_store(
            &p,
            &StoredTokens {
                access_token: "access-dummy-2".into(),
                refresh_token: "refresh-dummy-2".into(),
                account_id: Some("acct-dummy".into()),
            },
        )
        .unwrap();
        let back: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        // rotated …
        assert_eq!(back["tokens"]["access_token"], "access-dummy-2");
        assert_eq!(back["tokens"]["refresh_token"], "refresh-dummy-2");
        // … while everything we do not own survived.
        assert_eq!(back["auth_mode"], "chatgpt");
        assert_eq!(back["tokens"]["id_token"], "jwt-id-dummy");
        assert!(back.get("OPENAI_API_KEY").is_some());
        assert_ne!(back["last_refresh"], "2026-08-08T20:51:57Z");
    }

    #[test]
    fn write_store_leaves_no_temp_file() {
        let td = tempfile::TempDir::new().unwrap();
        let p = write_store_file(&td, &full_store());
        write_store(
            &p,
            &StoredTokens {
                access_token: "a2".into(),
                refresh_token: "r2".into(),
                account_id: None,
            },
        )
        .unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(td.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "auth.json")
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_store_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::TempDir::new().unwrap();
        let p = write_store_file(&td, &full_store());
        // start deliberately world-readable
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_store(
            &p,
            &StoredTokens {
                access_token: "a2".into(),
                refresh_token: "r2".into(),
                account_id: None,
            },
        )
        .unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "store must be owner-only, got {mode:o}");
    }

    #[test]
    fn permanent_refresh_errors_are_classified() {
        for c in [
            "refresh_token_expired",
            "refresh_token_reused",
            "refresh_token_invalidated",
        ] {
            assert!(is_permanent_refresh_error(c), "{c}");
        }
        for c in ["server_error", "slow_down", ""] {
            assert!(!is_permanent_refresh_error(c), "{c}");
        }
    }

    #[test]
    fn sanitize_drops_long_tokenlike_words() {
        let s = sanitize("failed for sk-abcdefghijklmnopqrstuvwxyz0123456789 at endpoint");
        assert!(!s.contains("sk-abcdefghijklmnopqrstuvwxyz"), "{s}");
        assert!(s.contains("endpoint"), "{s}");
    }

    #[test]
    fn auth_error_display_never_carries_a_token() {
        let e = AuthError::RefreshPermanent("refresh_token_reused".into());
        let s = e.to_string();
        assert!(s.contains("re-login"), "{s}");
        assert!(s.contains("refresh_token_reused"), "{s}");
    }
}
