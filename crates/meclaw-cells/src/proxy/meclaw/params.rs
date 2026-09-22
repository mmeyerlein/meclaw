//! The declaration of a peer contract class: where it is mounted, what it
//! calls the far end, and which lanes may cross in which direction.
//!
//! Parsed with serde and `deny_unknown_fields` on every level (OR-Peer.L1a.2):
//! a typo in `lanes` would otherwise open or close a lane nobody read. The
//! structural refusals come from serde and already name the key; the value
//! refusals come from [`MeclawParams::validate`] in the house style (field,
//! echoed value, what is allowed).

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
        Ok(())
    }

    /// `emit_to` as a [`Path`].
    pub fn emit_to_path(&self) -> Path {
        Path::new(&self.emit_to)
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
