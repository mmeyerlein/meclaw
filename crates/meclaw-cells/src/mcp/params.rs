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
    /// GH #489: no provider was named. `endpoint` absent, or present and empty
    /// (`${MCP_ENDPOINT:-}` with the variable unset), on the `http` transport.
    ///
    /// This is a STATE, not a configuration error, and the distinction is the
    /// whole point: a bridge whose far side has not been named has nothing to
    /// connect to, so it runs no handshake, panics at nothing, and answers
    /// every call with the `endpoint_unset` receipt. What it must never be is
    /// a cell that looks like a fault — the shipped occupant used to carry a
    /// guessed loopback default and turned every colony that grew it into a
    /// colony with a permanently `failed` cell in it.
    ///
    /// GH #270's rule is kept intact rather than reversed: empty and absent
    /// still land in exactly the same state, and that state still has a name
    /// an operator can read. Only the state changed — from "refuse the cell"
    /// to "the cell is idle and says so".
    ///
    /// The `stdio` transport keeps the loud reject for an absent or empty
    /// `command` (below): a config that declares `transport: "stdio"` names a
    /// binary or it names nothing, and nothing in the shipped tree writes that
    /// key through an environment default that could legitimately be blank.
    Unset,
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
        "sandbox",
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
        // GH #96: a containment a runtime params update could switch off is not
        // a containment. Same argument as the store's `write_surface`.
        "sandbox",
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
    // An EMPTY endpoint is not an endpoint (GH #270) — and since GH #489 an
    // ABSENT one is not a refusal either. Both mean the same thing, which is
    // GH #270's actual rule and the half that stays: an operator must never
    // face two different outcomes for `MCP_ENDPOINT=` and for never having set
    // it. What GH #489 changed is what that one outcome IS. It used to be a
    // parse error, which at boot means a cell that cannot spawn; it is now the
    // `Unset` state, which is a cell that spawns, stays idle and answers
    // `endpoint_unset`. The failure mode GH #270 was written against — a cell
    // that looked healthy and failed at URL build on every call, saying nothing
    // an operator could act on — is still gone, and now it has a name.
    let Some(endpoint) = obj
        .get("endpoint")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
    else {
        return Ok(McpTransport::Unset);
    };
    // An empty token is no token (GH #268). `auth.bearer` is written as
    // `${MCP_BEARER:-}` wherever the operator may leave the variable unset, and
    // the substitution turns that into `""`. Sending `Authorization: Bearer `
    // to a provider that would have answered anonymously is a worse failure
    // than sending nothing, and every declaration in the tree describes the
    // empty value as "no Authorization header".
    let bearer = obj
        .get("auth")
        .and_then(|a| a.as_object())
        .and_then(|a| a.get("bearer"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    Ok(McpTransport::Http { endpoint, bearer })
}

/// The P7 shape: a child process spec. `command` is the only required key.
fn parse_stdio(obj: &serde_json::Map<String, JsonValue>) -> Result<McpTransport, String> {
    // An EMPTY command is not a command (GH #270) — see `parse_http` above.
    let program = obj
        .get("command")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
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
    // GH #96: an MCP server is a third-party binary an operator configured, and
    // of the four spawn sites in the tree it is the one least likely to have
    // been written by whoever runs the colony. It reads `params.sandbox` with
    // the SAME schema `bash`, `code` and `harness` use — one profile shape, one
    // parser, one set of mistakes an operator can make.
    //
    // Absent means absent: no profile is the historical behaviour (the child
    // inherits the daemon's rights), which is what every mcp cell on disk has
    // today. The P8 process-group containment switches stay off here as before;
    // a sandbox is a different axis and does not turn them on.
    let sandbox = crate::sandbox::SandboxProfile::parse(&JsonValue::Object(obj.clone()))?;
    Ok(McpTransport::Stdio {
        spec: ChildSpec {
            program,
            args,
            env,
            cwd,
            kill_grace_ms,
            sandbox: sandbox.map(Box::new),
            ..ChildSpec::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// GH #489: the shipped occupant with `MCP_ENDPOINT` unset. Absent and
    /// empty are ONE state, and that state parses.
    #[test]
    fn an_unnamed_provider_parses_to_the_unset_transport() {
        for raw in [json!({}), json!({"endpoint": ""})] {
            let p = McpParams::parse(&raw).expect("an unnamed provider must parse");
            assert!(
                matches!(p.transport, McpTransport::Unset),
                "expected Unset for {raw}, got {:?}",
                p.transport
            );
        }
    }

    /// The unset state keeps its timeouts: they configure `handle()` and the
    /// `cell.db`, both of which still run.
    #[test]
    fn the_unset_transport_keeps_the_timeout_defaults() {
        let p = McpParams::parse(&json!({})).unwrap();
        assert_eq!(p.external_timeout_ms, 30_000);
        assert_eq!(p.query_timeout_ms, 5_000);
    }

    /// The stdio half is untouched (GH #270 as written): a declared child
    /// process that names no binary is still a loud reject.
    #[test]
    fn an_empty_stdio_command_is_still_refused_by_name() {
        let err = McpParams::parse(&json!({"transport": "stdio", "command": ""})).unwrap_err();
        assert!(err.contains("command"), "got: {err}");
    }

    /// A named endpoint still builds the http transport, unchanged.
    #[test]
    fn a_named_endpoint_still_builds_the_http_transport() {
        let p = McpParams::parse(&json!({"endpoint": "https://x.example/rpc"})).unwrap();
        assert!(matches!(p.transport, McpTransport::Http { .. }));
    }
}
