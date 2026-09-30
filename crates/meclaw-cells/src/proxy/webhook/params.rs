//! The declaration of a webhook mount: where it listens, how a request proves
//! it came from the sender that holds the secret, which request headers travel
//! on, and on which lane the arrival leaves.
//!
//! Parsed with serde and `deny_unknown_fields` on every level, as the `meclaw`
//! platform is (`crate::proxy::meclaw::params`): a typo in `verify` would
//! otherwise open a mount nobody meant to open. The value refusals come from
//! [`WebhookParams::validate`], each naming its field. The secret is never
//! echoed, neither by a refusal nor by `Debug`.

use meclaw_core::Path;
use serde::Deserialize;
use serde_json::Value as JsonValue;

/// Every key of a `webhook` proxy. All of them are immutable, as for `meclaw`:
/// a mount whose secret or lane a message could change is no boundary.
pub const IMMUTABLE_KEYS: &[&str] = &[
    "platform",
    "mount",
    "verify",
    "headers_allow",
    "route",
    "emit_to",
    "max_body_kb",
];

/// The largest body a mount may be declared to take, in KiB. A webhook
/// carries an event, not a file; anything larger belongs behind a store.
pub const MAX_BODY_KB_LIMIT: u64 = 4096;

fn default_max_body_kb() -> u64 {
    256
}

/// How a request proves where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyKind {
    /// HMAC-SHA256 over the raw body, hex in a header, optionally prefixed.
    HmacSha256,
    /// A header that carries the secret itself, optionally prefixed.
    Token,
    /// No proof; allowed only with a written reason.
    None,
}

impl VerifyKind {
    /// The spelling in `params.verify.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            VerifyKind::HmacSha256 => "hmac_sha256",
            VerifyKind::Token => "token",
            VerifyKind::None => "none",
        }
    }

    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "hmac_sha256" => Ok(VerifyKind::HmacSha256),
            "token" => Ok(VerifyKind::Token),
            "none" => Ok(VerifyKind::None),
            other => Err(format!(
                "verify.kind: unknown value {other:?} (accepted: \"hmac_sha256\", \"token\", \
                 \"none\")"
            )),
        }
    }
}

/// `params.verify` as written. `secret` is the RESOLVED value here; the file
/// form is checked by [`validate_declared`].
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verify {
    /// `hmac_sha256`, `token` or `none`.
    pub kind: String,
    /// The shared secret, bound from the environment. Empty means the variable
    /// is set but blank: the cell then answers every POST `503`.
    #[serde(default)]
    pub secret: Option<String>,
    /// The request header that carries the signature or the token.
    #[serde(default)]
    pub header: Option<String>,
    /// A literal the header value starts with (`sha256=`, `Bearer `).
    #[serde(default)]
    pub prefix: Option<String>,
    /// Why a mount takes unproven requests; required with `none`.
    #[serde(default)]
    pub because: Option<String>,
}

impl std::fmt::Debug for Verify {
    /// The secret is shown only as whether it is set.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verify")
            .field("kind", &self.kind)
            .field(
                "secret",
                &self.secret.as_ref().map(|s| {
                    if s.is_empty() {
                        "<empty>"
                    } else {
                        "<redacted>"
                    }
                }),
            )
            .field("header", &self.header)
            .field("prefix", &self.prefix)
            .field("because", &self.because)
            .finish()
    }
}

/// The parsed `params` of a `proxy` cell with `platform: "webhook"`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookParams {
    /// Always `"webhook"`; the key that chose this branch.
    pub platform: String,
    /// The name on the one listener; the mount is `/<mount>/`.
    pub mount: String,
    /// How a request proves itself.
    pub verify: Verify,
    /// Request headers that travel on with the arrival, compared without
    /// regard to case and written lower-case.
    #[serde(default)]
    pub headers_allow: Vec<String>,
    /// The `hop.route` of every arrival: the lane it leaves on.
    pub route: String,
    /// Where an arrival is emitted, as an absolute path.
    pub emit_to: String,
    /// The largest body taken, in KiB; a larger one is answered `413`.
    #[serde(default = "default_max_body_kb")]
    pub max_body_kb: u64,
}

impl WebhookParams {
    /// Parse + validate. Structural refusals come from serde (prefixed
    /// `params: `), value refusals from [`Self::validate`].
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let p = WebhookParams::deserialize(v).map_err(|e| format!("params: {e}"))?;
        p.validate()?;
        Ok(p)
    }

    /// The verification kind; `parse` has already refused an unknown one.
    pub fn kind(&self) -> VerifyKind {
        VerifyKind::parse(&self.verify.kind).unwrap_or(VerifyKind::None)
    }

    /// The resolved secret, `""` when the variable is blank or the kind needs none.
    pub fn secret(&self) -> &str {
        self.verify.secret.as_deref().unwrap_or("")
    }

    /// The value refusals, in a fixed order, each naming the field. The
    /// secret is never part of a refusal.
    pub fn validate(&self) -> Result<(), String> {
        if self.platform != "webhook" {
            return Err(format!(
                "platform: must be \"webhook\" for this variant, got {:?}",
                self.platform
            ));
        }
        if !meclaw_colony::surfaces::mount_is_valid(&self.mount) {
            return Err(format!(
                "mount: must be 1..=64 characters from [a-z0-9-] and none of the reserved \
                 names, got {:?}",
                self.mount
            ));
        }
        let kind = VerifyKind::parse(&self.verify.kind)?;
        match kind {
            VerifyKind::HmacSha256 | VerifyKind::Token => {
                if self.verify.secret.is_none() {
                    return Err(format!(
                        "verify.secret: required with kind {:?} (write it as ${{VAR}})",
                        kind.as_str()
                    ));
                }
                let header = self.verify.header.as_deref().unwrap_or("");
                if header.is_empty() {
                    return Err(format!(
                        "verify.header: required with kind {:?} (the request header that \
                         carries the proof)",
                        kind.as_str()
                    ));
                }
                if axum::http::HeaderName::from_bytes(header.as_bytes()).is_err() {
                    return Err(format!(
                        "verify.header: must be a legal HTTP header name, got {header:?}"
                    ));
                }
            }
            VerifyKind::None => {
                if self
                    .verify
                    .because
                    .as_deref()
                    .is_none_or(|b| b.trim().is_empty())
                {
                    return Err(
                        "verify.because: required with kind \"none\" (why this mount takes \
                         requests nobody can prove)"
                            .to_string(),
                    );
                }
                for (key, set) in [
                    ("secret", self.verify.secret.is_some()),
                    ("header", self.verify.header.is_some()),
                    ("prefix", self.verify.prefix.is_some()),
                ] {
                    if set {
                        return Err(format!(
                            "verify.{key}: not used with kind \"none\"; remove it"
                        ));
                    }
                }
            }
        }
        for (i, h) in self.headers_allow.iter().enumerate() {
            if axum::http::HeaderName::from_bytes(h.as_bytes()).is_err() {
                return Err(format!(
                    "headers_allow[{i}]: must be a legal HTTP header name, got {h:?}"
                ));
            }
            // The proof header never travels on: with `token` its value is the
            // secret itself, and an arrival lands in the message log.
            if self
                .verify
                .header
                .as_deref()
                .is_some_and(|p| p.eq_ignore_ascii_case(h))
            {
                return Err(format!(
                    "headers_allow[{i}]: the proof header (verify.header) is never \
                     copied into an arrival"
                ));
            }
        }
        if self.route.trim().is_empty() {
            return Err("route: required (the lane every arrival leaves on)".to_string());
        }
        if self.route == crate::proxy::meclaw::emit::RECEIPT_ROUTE {
            return Err(format!(
                "route: {:?} is the lane of this cell's own receipts; pick another",
                self.route
            ));
        }
        if !self.emit_to.starts_with('/') {
            return Err(format!(
                "emit_to: must be an absolute path, got {:?}",
                self.emit_to
            ));
        }
        if self.max_body_kb == 0 || self.max_body_kb > MAX_BODY_KB_LIMIT {
            return Err(format!(
                "max_body_kb: must be 1..={MAX_BODY_KB_LIMIT}, got {}",
                self.max_body_kb
            ));
        }
        Ok(())
    }

    /// `emit_to` as a [`Path`].
    pub fn emit_to_path(&self) -> Path {
        Path::new(&self.emit_to)
    }

    /// The body limit in bytes.
    pub fn max_body_bytes(&self) -> usize {
        (self.max_body_kb as usize).saturating_mul(1024)
    }
}

/// The refusal text for a runtime `params` update aimed at a `webhook` proxy.
pub fn refuse_params_update() -> String {
    format!(
        "every key of a webhook proxy is immutable ({}); change the cell, not the message",
        IMMUTABLE_KEYS.join(", ")
    )
}

/// `verify.secret` as the FILE declares it, before any `${VAR}` is bound:
/// exactly one `${NAME}` token without a default, as for the `meclaw`
/// platform's `auth` (`crate::proxy::meclaw::params::validate_declared`). A
/// literal would put the secret on disk and into every export of the tree.
/// The refusal names the key and never the value.
pub fn validate_declared(declared: &JsonValue) -> Result<(), String> {
    let Some(secret) = declared.get("verify").and_then(|v| v.get("secret")) else {
        return Ok(());
    };
    if secret
        .as_str()
        .is_some_and(crate::proxy::meclaw::params::is_env_token)
    {
        Ok(())
    } else {
        Err(
            "verify.secret: a secret is written as ${VAR} and bound from the environment, \
             never as a value in config.json; the value is not echoed"
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SECRET: &str = "s3cret-value-921";

    fn base() -> JsonValue {
        json!({
            "platform": "webhook", "mount": "probe-hook",
            "verify": {"kind": "hmac_sha256", "secret": SECRET,
                       "header": "X-Probe-Signature", "prefix": "sha256="},
            "headers_allow": ["X-Probe-Event"],
            "route": "probe", "emit_to": "/",
        })
    }

    fn with(path: &[&str], v: JsonValue) -> JsonValue {
        let mut out = base();
        let mut cur = &mut out;
        for k in &path[..path.len() - 1] {
            cur = &mut cur[*k];
        }
        let last = path[path.len() - 1];
        if v.is_null() {
            cur.as_object_mut().expect("object").remove(last);
        } else {
            cur[last] = v;
        }
        out
    }

    #[test]
    fn the_base_parses_with_the_defaults() {
        let p = WebhookParams::parse(&base()).expect("parses");
        assert_eq!(p.kind(), VerifyKind::HmacSha256);
        assert_eq!(p.max_body_kb, 256);
        assert_eq!(p.max_body_bytes(), 256 * 1024);
        assert_eq!(p.secret(), SECRET);
    }

    /// Every refusal names its field, and none repeats the secret.
    #[test]
    fn refusals_name_the_field_and_never_the_secret() {
        let cases: Vec<(JsonValue, &str)> = vec![
            (with(&["platform"], json!("meclaw")), "platform"),
            (with(&["mount"], json!("Probe Hook")), "mount"),
            (with(&["mount"], json!("colony")), "mount"),
            (with(&["verify", "kind"], json!("rsa")), "verify.kind"),
            (with(&["verify", "secret"], json!(null)), "verify.secret"),
            (with(&["verify", "header"], json!(null)), "verify.header"),
            (
                with(&["verify", "header"], json!("bad header")),
                "verify.header",
            ),
            (
                with(&["headers_allow"], json!(["ok", "no pe"])),
                "headers_allow[1]",
            ),
            // The proof header is never copied into an arrival: with `token`
            // its value IS the secret (review H, I1), whatever the case.
            (
                with(
                    &["headers_allow"],
                    json!(["X-Probe-Event", "x-probe-SIGNATURE"]),
                ),
                "headers_allow[1]",
            ),
            (
                json!({"platform": "webhook", "mount": "m", "route": "r", "emit_to": "/",
                       "verify": {"kind": "token", "secret": SECRET,
                                  "header": "Authorization", "prefix": "Bearer "},
                       "headers_allow": ["authorization"]}),
                "headers_allow[0]",
            ),
            (with(&["route"], json!("")), "route"),
            (with(&["route"], json!("  ")), "route"),
            // An arrival on the receipts' own lane could not be told apart
            // from them on a `hop.route == 'receipt'` edge (review H, M1).
            (with(&["route"], json!("receipt")), "route"),
            (with(&["emit_to"], json!("sink")), "emit_to"),
            (with(&["max_body_kb"], json!(0)), "max_body_kb"),
            (with(&["max_body_kb"], json!(4097)), "max_body_kb"),
            (
                json!({"platform": "webhook", "mount": "m", "route": "r", "emit_to": "/",
                       "verify": {"kind": "none"}}),
                "verify.because",
            ),
            (
                json!({"platform": "webhook", "mount": "m", "route": "r", "emit_to": "/",
                       "verify": {"kind": "none", "because": "   "}}),
                "verify.because",
            ),
            (
                json!({"platform": "webhook", "mount": "m", "route": "r", "emit_to": "/",
                       "verify": {"kind": "none", "because": "x", "secret": SECRET}}),
                "verify.secret",
            ),
            (with(&["verify", "extra"], json!(1)), "extra"),
            (with(&["surprise"], json!(1)), "surprise"),
        ];
        for (v, field) in cases {
            let e = WebhookParams::parse(&v).expect_err(field);
            assert!(e.contains(field), "{field}: {e}");
            assert!(!e.contains(SECRET), "{field}: the secret leaked: {e}");
        }
    }

    #[test]
    fn the_limit_itself_and_a_reasoned_none_parse() {
        assert!(WebhookParams::parse(&with(&["max_body_kb"], json!(4096))).is_ok());
        let none = json!({"platform": "webhook", "mount": "m", "route": "r", "emit_to": "/",
                          "verify": {"kind": "none", "because": "a loopback test source"}});
        assert_eq!(
            WebhookParams::parse(&none).expect("parses").kind(),
            VerifyKind::None
        );
        let token = with(&["verify", "kind"], json!("token"));
        assert_eq!(
            WebhookParams::parse(&token).expect("parses").kind(),
            VerifyKind::Token
        );
    }

    /// A blank variable parses: the cell then answers `503` rather than
    /// refusing to boot (the contract's `secret_missing`).
    #[test]
    fn a_blank_secret_parses() {
        let p = WebhookParams::parse(&with(&["verify", "secret"], json!(""))).expect("parses");
        assert_eq!(p.secret(), "");
    }

    #[test]
    fn debug_never_shows_the_secret() {
        let p = WebhookParams::parse(&base()).expect("parses");
        let d = format!("{p:?}");
        assert!(!d.contains(SECRET), "{d}");
        assert!(d.contains("<redacted>"), "{d}");
    }

    /// The file form: exactly one `${VAR}` without a default.
    #[test]
    fn the_declared_secret_must_be_one_env_token() {
        for ok in [json!("${PROBE_SECRET}"), json!("${_X1}")] {
            let v = with(&["verify", "secret"], ok.clone());
            assert_eq!(validate_declared(&v), Ok(()), "{ok}");
        }
        for bad in [
            json!(SECRET),
            json!("${PROBE_SECRET:-fallback}"),
            json!("x${PROBE_SECRET}"),
            json!("${PROBE_SECRET}${OTHER}"),
            json!(42),
        ] {
            let v = with(&["verify", "secret"], bad.clone());
            let e = validate_declared(&v).expect_err("refused");
            assert!(e.starts_with("verify.secret"), "{bad}: {e}");
            assert!(!e.contains(SECRET), "{e}");
        }
        assert_eq!(
            validate_declared(&with(&["verify", "secret"], json!(null))),
            Ok(()),
            "no secret key: the parse decides whether one is needed"
        );
    }
}
