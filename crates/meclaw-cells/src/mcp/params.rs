//! Phase-10-D: `McpParams`. POC cut — four fields.
//! `endpoint` is mandatory; everything else has defaults.

use serde_json::Value as JsonValue;

/// Parsed mcp params after validation. All fields owned.
#[derive(Debug, Clone)]
pub struct McpParams {
    /// HTTP+JSON-RPC endpoint (POC: no stdio, no SSE stream).
    /// `${VAR}` substitution is done by colony before the hand-off.
    pub endpoint: String,
    /// Optional bearer token (resolved via `${VAR}`). `None` = no Authorization
    /// header.
    pub bearer: Option<String>,
    /// A timeout (`tokio::time::timeout`) around every HTTP op
    /// (initialize, tools/list, tools/call). Default 30000.
    pub external_timeout_ms: u64,
    /// A timeout for `cell.db` calls via `DbConn`. Default 5000.
    pub query_timeout_ms: u64,
}

/// β: the `mcp` runtime-overlay projection. Only the two timeout fields are
/// mutable; `endpoint` + `auth` (bearer) are credential/identity and immutable.
///
/// A minimal projection (not full `McpParams`) because `McpParams::parse` reads
/// `bearer` from a nested `auth.bearer` shape that a flat round-trip would not
/// reconstruct — and `endpoint`/`bearer` never change at runtime anyway, so the
/// overlay round-trip only needs the mutable timeouts. `KNOWN_KEYS` still lists
/// the immutable top-level keys so an update touching them is a loud `Immutable`
/// reject (not a vaguer `Unknown`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpOverlay {
    /// A timeout per HTTP op (tool call). Mutable, path A (handle side, live).
    pub external_timeout_ms: u64,
    /// A timeout for cell.db ops. Mutable, path C (DbConn, live).
    pub query_timeout_ms: u64,
}

impl crate::params_overlay::OverlayParams for McpOverlay {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "endpoint",
        "auth",
        "external_timeout_ms",
        "query_timeout_ms",
    ];
    const IMMUTABLE_KEYS: &'static [&'static str] = &["endpoint", "auth"];
    fn parse(raw: &JsonValue) -> Result<Self, String> {
        let obj = raw.as_object().ok_or("params: must be object")?;
        let external_timeout_ms = obj
            .get("external_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(30_000);
        let query_timeout_ms = obj
            .get("query_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(5_000);
        Ok(Self {
            external_timeout_ms,
            query_timeout_ms,
        })
    }
}

impl McpParams {
    /// Parse + validate. Required fields rejected with explicit name.
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("params: must be object")?;
        let endpoint = obj
            .get("endpoint")
            .and_then(|x| x.as_str())
            .ok_or("endpoint: required (HTTP+JSON-RPC URL)")?
            .to_string();
        let bearer = obj
            .get("auth")
            .and_then(|a| a.as_object())
            .and_then(|a| a.get("bearer"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let external_timeout_ms = obj
            .get("external_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(30_000);
        let query_timeout_ms = obj
            .get("query_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(5_000);
        Ok(Self {
            endpoint,
            bearer,
            external_timeout_ms,
            query_timeout_ms,
        })
    }
}
