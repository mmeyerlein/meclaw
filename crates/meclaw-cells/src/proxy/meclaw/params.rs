//! The declaration of a peer contract class: where it is mounted, what it
//! calls the far end, and which lanes may cross in which direction.
//!
//! Parsed with serde and `deny_unknown_fields` on every level (OR-Peer.L1a.2):
//! a typo in `lanes` would otherwise open or close a lane nobody read. The
//! structural refusals come from serde and already name the key; the value
//! refusals come from [`MeclawParams::validate`] in the house style (field,
//! echoed value, what is allowed).

use meclaw_colony::surfaces::ProxyNet;
use meclaw_core::Path;
use serde::Deserialize;
use serde_json::Value as JsonValue;

/// Every key of a `meclaw` proxy. All of them are immutable (README § 0a A9):
/// a boundary a message can rename is not a boundary.
pub const IMMUTABLE_KEYS: &[&str] = &[
    "platform",
    "mount",
    "identity_header",
    "boundary",
    "emit_to",
    "external_timeout_ms",
    "query_timeout_ms",
    "lanes",
    "auth",
    "trusted_proxies",
];

/// The refusal text for a runtime `params` update aimed at a `meclaw` proxy.
pub fn refuse_params_update() -> String {
    format!(
        "every key of a meclaw proxy is immutable ({}); a boundary a message can rename is \
         not a boundary",
        IMMUTABLE_KEYS.join(", ")
    )
}

/// One lane: a named route and the exact slots allowed to cross on it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lane {
    /// The `hop.route` value that IS the lane; never a cell name.
    pub route: String,
    /// Body allow-list: dotted paths, with one wildcard segment `messages[]`.
    pub fields: Vec<String>,
    /// `context` keys allowed to cross; empty means none.
    #[serde(default)]
    pub context: Vec<String>,
    /// What the lane is for; quoted verbatim in every refusal.
    pub because: String,
}

/// The contract, in both directions.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lanes {
    /// Lanes the peer may send IN.
    #[serde(default)]
    pub accepts: Vec<Lane>,
    /// Lanes this colony may send OUT.
    #[serde(default)]
    pub emits: Vec<Lane>,
}

fn default_timeout_ms() -> u64 {
    5000
}

/// The parsed `params` of a `proxy` cell with `platform: "meclaw"`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeclawParams {
    /// Always `"meclaw"`; kept as a field because it is the key that chose this
    /// branch, and `deny_unknown_fields` would otherwise refuse it (OR-Peer.L1a.1).
    pub platform: String,
    /// The name on the one listener; the peer mount is `/<mount>/`.
    pub mount: String,
    /// The header the reverse proxy fills with the verified sending colony;
    /// empty means this mount accepts nothing.
    #[serde(default)]
    pub identity_header: String,
    /// This side's own name; rides every receipt. Never a URL.
    pub boundary: String,
    /// Where an arrived frame is emitted, as an absolute path.
    pub emit_to: String,
    /// Operation timeout (hard rule 12) around the outgoing POST.
    #[serde(default = "default_timeout_ms")]
    pub external_timeout_ms: u64,
    /// Operation timeout (hard rule 12) around `cell.db` access via `DbConn`.
    #[serde(default = "default_timeout_ms")]
    pub query_timeout_ms: u64,
    /// The contract, in both directions.
    pub lanes: Lanes,
    /// GH #828: the credential every outgoing POST carries. Absent means the
    /// POST goes out anonymously, as before.
    #[serde(default)]
    pub auth: Option<PeerAuth>,
    /// GH #833: the addresses whose `identity_header` this mount believes —
    /// IP addresses or CIDRs. Absent means loopback (`127.0.0.0/8`, `::1/128`,
    /// R-AG-1); an empty list believes nobody. A connection from anywhere else
    /// is judged as if it carried no header. Kept raw, as written, so `Debug`
    /// and a refusal show what the operator wrote; [`Self::trusted`] is the
    /// parsed form.
    #[serde(default)]
    pub trusted_proxies: Option<Vec<String>>,
}

impl MeclawParams {
    /// Parse + validate. Structural refusals come from serde (prefixed
    /// `params: `), value refusals from [`Self::validate`].
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let p = MeclawParams::deserialize(v).map_err(|e| format!("params: {e}"))?;
        p.validate()?;
        Ok(p)
    }

    /// The value refusals, in a fixed order, each naming the field and echoing
    /// the value.
    pub fn validate(&self) -> Result<(), String> {
        if self.platform != "meclaw" {
            return Err(format!(
                "platform: must be \"meclaw\" for this variant, got {:?}",
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
        // Asked of the param, not of a request: a typo in a config.json must be
        // a refusal at plan time rather than a header that silently never
        // matches (same reasoning as `web/params.rs`).
        if !self.identity_header.is_empty()
            && axum::http::HeaderName::from_bytes(self.identity_header.as_bytes()).is_err()
        {
            return Err(format!(
                "identity_header: must be a legal HTTP header name or empty, got {:?}",
                self.identity_header
            ));
        }
        // GH #833: an entry that is no address is refused at plan time and by
        // index; a list that silently matched nothing would refuse every frame
        // with a detail about the proxy instead of the typo.
        if let Some(list) = &self.trusted_proxies {
            meclaw_colony::surfaces::parse_trusted_proxies(list)?;
        }
        if self.boundary.is_empty() {
            return Err(
                "boundary: required (this side's own name; it rides every receipt)".to_string(),
            );
        }
        if !self.emit_to.starts_with('/') {
            return Err(format!(
                "emit_to: must be an absolute path, got {:?}",
                self.emit_to
            ));
        }
        validate_direction("accepts", &self.lanes.accepts)?;
        validate_direction("emits", &self.lanes.emits)?;
        if let Some(auth) = &self.auth {
            auth.validate()?;
        }
        Ok(())
    }

    /// `emit_to` as a [`Path`].
    pub fn emit_to_path(&self) -> Path {
        Path::new(&self.emit_to)
    }

    /// GH #833: the list the mount judges each connection with — the default
    /// (loopback) when the key is absent, the parsed entries otherwise.
    ///
    /// [`Self::validate`] has already refused an entry that does not parse, so
    /// the error arm is reached only by params that skipped it; it trusts
    /// nobody rather than everybody (fail-closed).
    pub fn trusted(&self) -> Vec<ProxyNet> {
        meclaw_colony::surfaces::trusted_proxies_or_default(self.trusted_proxies.as_deref())
            .unwrap_or_default()
    }
}

/// One direction: every route non-empty and unique, and `system` never named.
fn validate_direction(dir: &str, lanes: &[Lane]) -> Result<(), String> {
    let mut seen: Vec<&str> = Vec::with_capacity(lanes.len());
    for lane in lanes {
        if lane.route.is_empty() {
            return Err(format!("lanes.{dir}: a lane needs a non-empty route"));
        }
        if seen.contains(&lane.route.as_str()) {
            return Err(format!(
                "lanes.{dir}: route {:?} is declared twice; one direction holds one lane per \
                 route",
                lane.route
            ));
        }
        seen.push(&lane.route);
        if let Some(f) = lane
            .fields
            .iter()
            .find(|f| *f == "system" || f.starts_with("system."))
        {
            return Err(format!(
                "lanes.{dir}[{:?}].fields: {f:?} is never allowed to cross; `system` is this \
                 colony's instruction to its own models, not content a peer may read",
                lane.route
            ));
        }
    }
    Ok(())
}

/// GH #828: the two secret keys of `auth`, each with the form it belongs to.
pub const SECRET_KEYS: &[&str] = &["value", "client_secret"];

/// The keys of the static form.
const STATIC_KEYS: &[&str] = &["header", "value"];
/// The keys of the OAuth form.
const OAUTH_KEYS: &[&str] = &[
    "token_url",
    "client_id",
    "client_secret",
    "scope",
    "audience",
];

/// Headers the carrier writes itself; a credential under one of these names
/// would fight the carrier over the same line.
const CARRIER_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "connection",
    "host",
    "transfer-encoding",
];

/// The credential of a `meclaw` proxy (GH #828): one of two standard forms of
/// machine-to-machine authentication, never both.
///
/// `Debug` is written by hand in both forms so the secret cannot reach a log
/// line, a panic message or a receipt (the `slack` precedent).
#[derive(Clone)]
pub enum PeerAuth {
    /// One header with a secret value on every POST.
    Header(StaticAuth),
    /// The OAuth 2.0 client-credentials grant (RFC 6749 § 4.4); the token rides
    /// as `Authorization: Bearer`.
    OAuth(OAuthAuth),
}

/// `{"header": ..., "value": "${VAR}"}`.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticAuth {
    /// The header name, sent verbatim.
    pub header: String,
    /// The secret value; `${VAR}` in the file, resolved in memory.
    pub value: String,
}

/// `{"token_url": ..., "client_id": ..., "client_secret": "${VAR}", "scope", "audience"}`.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthAuth {
    /// The token endpoint, `http` or `https`.
    pub token_url: String,
    /// The client the grant is for; not a secret.
    pub client_id: String,
    /// The client secret; `${VAR}` in the file, resolved in memory.
    pub client_secret: String,
    /// Optional `scope` form field; empty means not sent.
    #[serde(default)]
    pub scope: String,
    /// Optional `audience` form field, which some providers require; empty
    /// means not sent.
    #[serde(default)]
    pub audience: String,
}

impl std::fmt::Debug for PeerAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerAuth::Header(a) => a.fmt(f),
            PeerAuth::OAuth(a) => a.fmt(f),
        }
    }
}

impl std::fmt::Debug for StaticAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticAuth")
            .field("header", &self.header)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl std::fmt::Debug for OAuthAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthAuth")
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("scope", &self.scope)
            .field("audience", &self.audience)
            .finish()
    }
}

/// The form is chosen by the keys present, before either struct is built, so
/// that "both at once" is refused as that and not as whichever unknown key
/// serde happens to meet first. Every message starts with the key path, and
/// none of them carries a value.
impl<'de> Deserialize<'de> for PeerAuth {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let raw = serde_json::Map::<String, JsonValue>::deserialize(d)
            .map_err(|_| D::Error::custom("auth: must be an object"))?;
        if let Some(k) = raw
            .keys()
            .find(|k| !STATIC_KEYS.contains(&k.as_str()) && !OAUTH_KEYS.contains(&k.as_str()))
        {
            return Err(D::Error::custom(format!(
                "auth: unknown key {k:?}; the static form has {}, the OAuth form {}",
                STATIC_KEYS.join(", "),
                OAUTH_KEYS.join(", ")
            )));
        }
        let has = |set: &[&str]| -> Vec<String> {
            raw.keys()
                .filter(|k| set.contains(&k.as_str()))
                .cloned()
                .collect()
        };
        let (stat, oauth) = (has(STATIC_KEYS), has(OAUTH_KEYS));
        // Every key of both forms is a string. Asked here, key by key, because
        // serde's type error names neither the key nor stays clear of the value.
        if let Some((k, _)) = raw.iter().find(|(_, v)| !v.is_string()) {
            return Err(D::Error::custom(format!("auth.{k}: must be a string")));
        }
        // What is left for serde is a missing key, and its message names it.
        let missing = |e: serde_json::Error| D::Error::custom(format!("auth: {e}"));
        match (stat.is_empty(), oauth.is_empty()) {
            (false, false) => Err(D::Error::custom(format!(
                "auth: declares both forms at once ({} and {}); a peer proxy carries one \
                 credential",
                stat.join(", "),
                oauth.join(", ")
            ))),
            (false, true) => serde_json::from_value(JsonValue::Object(raw))
                .map(PeerAuth::Header)
                .map_err(missing),
            (true, false) => serde_json::from_value(JsonValue::Object(raw))
                .map(PeerAuth::OAuth)
                .map_err(missing),
            (true, true) => Err(D::Error::custom(format!(
                "auth: names neither form; the static form has {}, the OAuth form {}",
                STATIC_KEYS.join(", "),
                OAUTH_KEYS.join(", ")
            ))),
        }
    }
}

impl PeerAuth {
    /// The value refusals. No message echoes a secret; a URL and a header name
    /// are not secrets and are echoed like every other value in this file.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            PeerAuth::Header(a) => {
                if axum::http::HeaderName::from_bytes(a.header.as_bytes()).is_err() {
                    return Err(format!(
                        "auth.header: must be a legal HTTP header name, got {:?}",
                        a.header
                    ));
                }
                if CARRIER_HEADERS.contains(&a.header.to_ascii_lowercase().as_str()) {
                    return Err(format!(
                        "auth.header: {:?} is written by the carrier itself; name a header of \
                         the credential's own",
                        a.header
                    ));
                }
                // An empty variable is not a credential (the GH #270 lesson of
                // the slack variant): it would post a header the far proxy
                // refuses on every crossing while the cell looks healthy.
                if a.value.is_empty() {
                    return Err("auth.value: required and non-empty (use ${VAR})".to_string());
                }
                if axum::http::HeaderValue::from_str(&a.value).is_err() {
                    return Err(
                        "auth.value: not a legal HTTP header value (visible ASCII only); the \
                         value is not echoed"
                            .to_string(),
                    );
                }
            }
            PeerAuth::OAuth(a) => {
                let parsed = reqwest::Url::parse(&a.token_url);
                // Userinfo in the URL is a credential outside `client_secret`:
                // every detail that names the endpoint would carry it into a
                // receipt. Refused, and this once the URL is not echoed
                // (review M5 of the #828 strand).
                if parsed
                    .as_ref()
                    .is_ok_and(|u| !u.username().is_empty() || u.password().is_some())
                {
                    return Err("auth.token_url: must not carry credentials in the URL".to_string());
                }
                let scheme_ok = parsed
                    .map(|u| matches!(u.scheme(), "http" | "https"))
                    .unwrap_or(false);
                if !scheme_ok {
                    return Err(format!(
                        "auth.token_url: must be an http or https URL, got {:?}",
                        a.token_url
                    ));
                }
                if a.client_id.is_empty() {
                    return Err("auth.client_id: required and non-empty".to_string());
                }
                if a.client_secret.is_empty() {
                    return Err(
                        "auth.client_secret: required and non-empty (use ${VAR})".to_string()
                    );
                }
            }
        }
        Ok(())
    }
}

/// GH #828: the secret keys of `auth` as the FILE declares them, before any
/// `${VAR}` is bound. A secret is exactly one `${NAME}` token without a
/// default: a literal would put the credential on disk and into every export
/// of the tree, and a `${NAME:-fallback}` would do the same the moment the
/// variable is unset. The refusal names the key and never the literal.
///
/// Only this check can see the difference: the parse above reads the resolved
/// value, and a resolved value looks the same whether it came from `.env` or
/// from the file.
pub fn validate_declared(declared: &JsonValue) -> Result<(), String> {
    let Some(auth) = declared.get("auth").and_then(JsonValue::as_object) else {
        return Ok(());
    };
    for key in SECRET_KEYS {
        let Some(v) = auth.get(*key) else { continue };
        if !v.as_str().is_some_and(is_env_token) {
            return Err(format!(
                "auth.{key}: a secret is written as ${{VAR}} and bound from the environment, \
                 never as a value in config.json; the value is not echoed"
            ));
        }
    }
    Ok(())
}

/// `${NAME}`, the whole string, `NAME` from `[A-Za-z_][A-Za-z0-9_]*`.
fn is_env_token(s: &str) -> bool {
    let Some(name) = s.strip_prefix("${").and_then(|r| r.strip_suffix('}')) else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base(auth: JsonValue) -> JsonValue {
        json!({
            "platform": "meclaw", "mount": "peer", "identity_header": "X-Meclaw-Peer",
            "boundary": "north", "emit_to": "/",
            "lanes": {"accepts": [], "emits": []},
            "auth": auth,
        })
    }

    const SECRET: &str = "s3cret-value-828";

    #[test]
    fn no_auth_parses_as_before() {
        let mut v = base(json!(null));
        if let Some(o) = v.as_object_mut() {
            o.remove("auth");
        }
        let p = MeclawParams::parse(&v).expect("parses");
        assert!(p.auth.is_none());
    }

    #[test]
    fn the_static_form_parses() {
        let p = MeclawParams::parse(&base(
            json!({"header": "X-Peer-Credential", "value": SECRET}),
        ))
        .expect("parses");
        assert!(matches!(p.auth, Some(PeerAuth::Header(ref a)) if a.header == "X-Peer-Credential"));
    }

    #[test]
    fn the_oauth_form_parses_with_scope_and_audience_optional() {
        let p = MeclawParams::parse(&base(json!({
            "token_url": "https://idp.example/token", "client_id": "north", "client_secret": SECRET
        })))
        .expect("parses");
        let Some(PeerAuth::OAuth(a)) = p.auth else {
            panic!("the OAuth form")
        };
        assert_eq!((a.scope.as_str(), a.audience.as_str()), ("", ""));
    }

    #[test]
    fn both_forms_at_once_are_refused_naming_the_keys() {
        let e = MeclawParams::parse(&base(json!({
            "header": "X-Peer-Credential", "value": SECRET, "token_url": "https://idp.example/t"
        })))
        .expect_err("refused");
        assert!(e.contains("auth") && e.contains("both forms"), "{e}");
        assert!(e.contains("token_url") && e.contains("header"), "{e}");
        assert!(!e.contains(SECRET), "{e}");
    }

    #[test]
    fn an_unknown_key_on_either_level_is_refused_by_name() {
        let e = MeclawParams::parse(&base(
            json!({"header": "X-A", "value": SECRET, "vaule": "x"}),
        ))
        .expect_err("refused");
        assert!(e.contains("vaule"), "{e}");
        let e = MeclawParams::parse(&base(json!({}))).expect_err("refused");
        assert!(e.contains("neither form"), "{e}");
    }

    #[test]
    fn a_missing_key_of_a_form_is_refused_by_name() {
        let e = MeclawParams::parse(&base(json!({"header": "X-A"}))).expect_err("refused");
        assert!(e.contains("value"), "{e}");
        let e = MeclawParams::parse(&base(json!({"token_url": "https://i/t", "client_id": "n"})))
            .expect_err("refused");
        assert!(e.contains("client_secret"), "{e}");
    }

    #[test]
    fn a_wrong_type_on_a_secret_key_does_not_echo_the_value() {
        let e = MeclawParams::parse(&base(
            json!({"header": "X-A", "value": ["s3cret-in-array"]}),
        ))
        .expect_err("refused");
        assert!(!e.contains("s3cret-in-array"), "{e}");
    }

    #[test]
    fn value_refusals_name_the_key_and_never_the_secret() {
        for (auth, key) in [
            (
                json!({"header": "bad header", "value": SECRET}),
                "auth.header",
            ),
            (
                json!({"header": "Content-Type", "value": SECRET}),
                "auth.header",
            ),
            (json!({"header": "X-A", "value": ""}), "auth.value"),
            (
                json!({"header": "X-A", "value": "s3cret\nvalue"}),
                "auth.value",
            ),
            (
                json!({"token_url": "ftp://i/t", "client_id": "n", "client_secret": SECRET}),
                "auth.token_url",
            ),
            (
                json!({"token_url": "https://i/t", "client_id": "", "client_secret": SECRET}),
                "auth.client_id",
            ),
            (
                json!({"token_url": "https://i/t", "client_id": "n", "client_secret": ""}),
                "auth.client_secret",
            ),
        ] {
            let e = MeclawParams::parse(&base(auth)).expect_err("refused");
            assert!(e.starts_with(key), "{key}: {e}");
            assert!(!e.contains(SECRET) && !e.contains("s3cret"), "{e}");
        }
    }

    /// GH #828 fix round 1 (review M5): a `token_url` with userinfo would put
    /// a credential into every detail that names the endpoint. Refused, and
    /// the URL is not echoed in this case.
    #[test]
    fn a_token_url_with_credentials_in_it_is_refused_without_an_echo() {
        for url in [
            "https://id:pw-828@idp.example/token",
            "https://pw-828@idp.example/t",
        ] {
            let e = MeclawParams::parse(&base(
                json!({"token_url": url, "client_id": "n", "client_secret": SECRET}),
            ))
            .expect_err("refused");
            assert_eq!(e, "auth.token_url: must not carry credentials in the URL");
            assert!(!e.contains("pw-828"), "{e}");
        }
    }

    #[test]
    fn debug_never_shows_a_secret() {
        for auth in [
            json!({"header": "X-A", "value": SECRET}),
            json!({"token_url": "https://i/t", "client_id": "n", "client_secret": SECRET}),
        ] {
            let p = MeclawParams::parse(&base(auth)).expect("parses");
            let d = format!("{p:?}");
            assert!(!d.contains(SECRET), "{d}");
            assert!(d.contains("<redacted>"), "{d}");
        }
    }

    #[test]
    fn auth_is_immutable() {
        assert!(IMMUTABLE_KEYS.contains(&"auth"));
        assert!(refuse_params_update().contains("auth"));
    }

    /// GH #833: whose header counts is part of the boundary.
    #[test]
    fn trusted_proxies_is_immutable_and_debug_shows_it_as_written() {
        assert!(IMMUTABLE_KEYS.contains(&"trusted_proxies"));
        assert!(refuse_params_update().contains("trusted_proxies"));
        let mut v = base(json!(null));
        v["trusted_proxies"] = json!(["192.0.2.0/24"]);
        if let Some(o) = v.as_object_mut() {
            o.remove("auth");
        }
        let p = MeclawParams::parse(&v).expect("parses");
        assert!(format!("{p:?}").contains("192.0.2.0/24"));
    }

    #[test]
    fn a_secret_must_be_declared_as_one_env_token() {
        let ok = base(json!({"header": "X-A", "value": "${PEER_CREDENTIAL}"}));
        assert_eq!(validate_declared(&ok), Ok(()));
        let ok = base(
            json!({"token_url": "https://i/t", "client_id": "lit", "client_secret": "${S_1}"}),
        );
        assert_eq!(validate_declared(&ok), Ok(()), "client_id is no secret");
        for (auth, key) in [
            (json!({"header": "X-A", "value": SECRET}), "auth.value"),
            (
                json!({"header": "X-A", "value": "${PEER:-fallback}"}),
                "auth.value",
            ),
            (
                json!({"header": "X-A", "value": "Bearer ${PEER}"}),
                "auth.value",
            ),
            (json!({"header": "X-A", "value": "$${PEER}"}), "auth.value"),
            (
                json!({"token_url": "https://i/t", "client_id": "n", "client_secret": SECRET}),
                "auth.client_secret",
            ),
        ] {
            let e = validate_declared(&base(auth)).expect_err("refused");
            assert!(e.starts_with(key), "{key}: {e}");
            assert!(!e.contains(SECRET) && !e.contains("fallback"), "{e}");
        }
        assert_eq!(
            validate_declared(&base(json!(null))),
            Ok(()),
            "no auth, nothing to check"
        );
    }
}
