//! `McpParams`: the cell's birth configuration.
//!
//! Two transports since P7 — `http` (the phase-10-D POC, unchanged) and
//! `stdio` (a child process speaking line-JSON). `transport` is optional and
//! defaults to `http`, so every configuration written before P7 keeps parsing
//! to exactly the same result.

use crate::stdio_child::ChildSpec;
use serde_json::Value as JsonValue;

/// How the cell reaches its MCP provider.
#[derive(Debug, Clone)]
pub enum McpTransport {
    /// HTTP + JSON-RPC. One fresh connection per call, no persistent stream.
    Http {
        /// JSON-RPC endpoint. `${VAR}` substitution is done by colony.
        endpoint: String,
        /// Optional bearer token; `None` = no Authorization header.
        bearer: Option<String>,
    },
    /// A child process speaking line-JSON over stdin/stdout.
    Stdio {
        /// How to start that child.
        spec: ChildSpec,
    },
}

/// Parsed mcp params after validation. All fields owned.
#[derive(Debug, Clone)]
pub struct McpParams {
    /// Transport-specific connection identity (immutable at runtime).
    pub transport: McpTransport,
    /// A timeout (`tokio::time::timeout`) around every provider op
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
/// reconstruct — and the transport identity never changes at runtime anyway, so
/// the overlay round-trip only needs the mutable timeouts. `KNOWN_KEYS` still
/// lists the immutable top-level keys so an update touching them is a loud
/// `Immutable` reject (not a vaguer `Unknown`).
///
/// P7 adds the stdio identity keys to both lists for the same reason: swapping
/// the child process under a running cell would be an identity change, not a
/// parameter tweak.
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpOverlay {
    /// A timeout per HTTP op (tool call). Mutable, path A (handle side, live).
    pub external_timeout_ms: u64,
    /// A timeout for cell.db ops. Mutable, path C (DbConn, live).
    pub query_timeout_ms: u64,
}

impl crate::params_overlay::OverlayParams for McpOverlay {
    const KNOWN_KEYS: &'static [&'static str] = &[
        "transport",
        "endpoint",
        "auth",
        "command",
        "args",
        "env",
        "cwd",
        "kill_grace_ms",
        "external_timeout_ms",
        "query_timeout_ms",
    ];
    const IMMUTABLE_KEYS: &'static [&'static str] = &[
        "transport",
        "endpoint",
        "auth",
        "command",
        "args",
        "env",
        "cwd",
        "kill_grace_ms",
    ];
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
    /// Parse + validate. Required fields are rejected by name.
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("params: must be object")?;
        let has_endpoint = obj.contains_key("endpoint");
        let has_command = obj.contains_key("command");
        if has_endpoint && has_command {
            return Err(
                "endpoint and command are mutually exclusive: pick one transport".to_string(),
            );
        }
        let declared = obj.get("transport").and_then(|x| x.as_str());
        let transport = match declared {
            None if has_command => parse_stdio(obj)?,
            None | Some("http") => parse_http(obj)?,
            Some("stdio") => parse_stdio(obj)?,
            Some(other) => return Err(format!("transport: unknown value {other:?}")),
        };
        let external_timeout_ms = obj
            .get("external_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(30_000);
        let query_timeout_ms = obj
            .get("query_timeout_ms")
            .and_then(|x| x.as_u64())
            .unwrap_or(5_000);
        Ok(Self {
            transport,
            external_timeout_ms,
            query_timeout_ms,
        })
    }
}

/// The pre-P7 shape: endpoint plus optional bearer.
fn parse_http(obj: &serde_json::Map<String, JsonValue>) -> Result<McpTransport, String> {
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
    Ok(McpTransport::Http { endpoint, bearer })
}

/// The P7 shape: a child process spec. `command` is the only required key.
fn parse_stdio(obj: &serde_json::Map<String, JsonValue>) -> Result<McpTransport, String> {
    let program = obj
        .get("command")
        .and_then(|x| x.as_str())
        .ok_or("command: required (stdio transport)")?
        .to_string();
    let args = match obj.get("args") {
        None => Vec::new(),
        Some(JsonValue::Array(a)) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "args: must be an array of strings".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("args: must be an array of strings".to_string()),
    };
    let env = match obj.get("env") {
        None => Vec::new(),
        Some(JsonValue::Object(m)) => m
            .iter()
            .map(|(k, v)| {
                v.as_str()
                    .map(|s| (k.clone(), s.to_string()))
                    .ok_or_else(|| "env: values must be strings".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("env: must be an object of string values".to_string()),
    };
    let cwd = obj
        .get("cwd")
        .and_then(|x| x.as_str())
        .map(std::path::PathBuf::from);
    let kill_grace_ms = obj
        .get("kill_grace_ms")
        .and_then(|x| x.as_u64())
        .unwrap_or(2_000);
    Ok(McpTransport::Stdio {
        spec: ChildSpec {
            program,
            args,
            env,
            cwd,
            kill_grace_ms,
        },
    })
}
