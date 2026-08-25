//! W8 (GH #380): the `web` cell's params.
//!
//! Three keys, and the two that decide where the cell listens are immutable.
//! Rebinding a live display by mutation would move a running service out from
//! under whatever reverse proxy is pointed at it (R-W8-2 puts auth and TLS in
//! front, forever) — so `port` and `bind` are settled at birth and a params
//! update naming either is refused, loudly and without partial apply.

use crate::params_overlay::OverlayParams;
use meclaw_core::JsonValue;

/// The highest port number there is. Named rather than inlined so the refusal
/// message and the check cannot drift apart.
const MAX_PORT: u64 = 65535;

/// Where a `web` cell listens, and how long it waits on the outside world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebParams {
    /// The TCP port this instance owns. Required — there is no default,
    /// because two instances sharing a default would be a bind race rather
    /// than a configuration (R-W8-1: the type is deliberately multiple).
    pub port: u16,
    /// The address to bind. Loopback by default (R-W8-2): the cell never grows
    /// an auth story, so its default must not be reachable off-host.
    pub bind: String,
    /// Operation-timeout (hard rule 12, A) for I/O this cell initiates.
    pub external_timeout_ms: u64,
}

impl WebParams {
    /// Parse + validate. Shares its path with `validate_params` and
    /// `spawn_cell` (parser invariant, `meclaw_colony::CellFactory`).
    ///
    /// `port` is refused rather than defaulted, and `0` is refused with it: the
    /// OS reads `0` as "assign me anything", which would leave the cell serving
    /// on a port no one can be told in advance — a display nobody can find.
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("params: must be object")?;

        let port_raw = obj
            .get("port")
            .ok_or("port: required (the port this display owns, 1..=65535)")?;
        let port = port_raw
            .as_u64()
            .filter(|p| (1..=MAX_PORT).contains(p))
            .ok_or_else(|| format!("port: must be an integer in 1..={MAX_PORT}, got {port_raw}"))?
            as u16;

        let bind = obj
            .get("bind")
            .map(|b| {
                b.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("bind: must be a string, got {b}"))
            })
            .transpose()?
            .unwrap_or_else(|| "127.0.0.1".to_string());
        if bind.is_empty() {
            return Err("bind: must not be empty".to_string());
        }

        let external_timeout_ms = obj
            .get("external_timeout_ms")
            .map(|t| {
                t.as_u64().filter(|v| *v > 0).ok_or_else(|| {
                    format!("external_timeout_ms: must be a positive integer, got {t}")
                })
            })
            .transpose()?
            .unwrap_or(5000);

        Ok(Self {
            port,
            bind,
            external_timeout_ms,
        })
    }
}

/// The mutable projection of [`WebParams`], for the runtime params-update
/// overlay.
///
/// A projection rather than the whole struct, following the proxy precedent:
/// what round-trips through `cell.db` is only what may actually change, so the
/// cell's identity — where it listens — cannot be carried back in by a restore
/// and quietly differ from what the operator configured.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebOverlay {
    /// Operation-timeout for I/O this cell initiates. Mutable.
    pub external_timeout_ms: u64,
}

impl OverlayParams for WebOverlay {
    /// Every key an update may name at all — including the two immutable ones.
    /// That inclusion is the point: naming `port` here is what makes an update
    /// touching it a loud `Immutable` refusal instead of an `Unknown` one, and
    /// the difference matters to whoever reads the error ("you may not move
    /// this" vs. "there is no such key").
    const KNOWN_KEYS: &'static [&'static str] = &["port", "bind", "external_timeout_ms"];

    /// The two that may not change while the cell lives. Rebinding a running
    /// display would move it out from under the reverse proxy that fronts it
    /// (R-W8-2), which is a service outage dressed up as a config change.
    const IMMUTABLE_KEYS: &'static [&'static str] = &["port", "bind"];

    fn parse(raw: &JsonValue) -> Result<Self, String> {
        // Through the same parser as everything else (parser invariant): the
        // projection is taken from a fully validated `WebParams`, never parsed
        // on its own with looser rules.
        WebParams::parse(raw).map(|p| Self {
            external_timeout_ms: p.external_timeout_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn a_port_is_required() {
        let err = WebParams::parse(&json!({})).unwrap_err();
        assert!(err.starts_with("port: required"), "got {err}");
    }

    #[test]
    fn port_zero_is_not_a_port() {
        // The OS would read it as "pick one", and the operator could not be
        // told in advance where the display went.
        assert!(WebParams::parse(&json!({"port": 0})).is_err());
    }

    #[test]
    fn a_port_above_the_range_is_refused_with_the_bound_named() {
        let err = WebParams::parse(&json!({"port": 70000})).unwrap_err();
        assert!(err.contains("65535"), "the refusal names the bound: {err}");
    }

    #[test]
    fn the_default_bind_is_loopback() {
        let p = WebParams::parse(&json!({"port": 7800})).unwrap();
        assert_eq!(p.bind, "127.0.0.1");
        assert_eq!(p.external_timeout_ms, 5000);
    }

    #[test]
    fn bind_and_port_are_both_known_and_both_immutable() {
        // Both lists, deliberately. `KNOWN_KEYS` decides whether the key exists
        // at all; `IMMUTABLE_KEYS` decides whether it may move. A key that were
        // only in the second would be refused as `Unknown`, which tells the
        // operator the wrong thing.
        for key in ["port", "bind"] {
            assert!(WebOverlay::KNOWN_KEYS.contains(&key), "{key} must be known");
            assert!(
                WebOverlay::IMMUTABLE_KEYS.contains(&key),
                "{key} must be immutable — rebinding a live display by mutation \
                 would move it out from under its reverse proxy"
            );
        }
        assert!(!WebOverlay::IMMUTABLE_KEYS.contains(&"external_timeout_ms"));
    }

    #[test]
    fn the_overlay_projection_carries_only_what_may_change() {
        let o = <WebOverlay as OverlayParams>::parse(&json!({"port": 7800})).unwrap();
        assert_eq!(o.external_timeout_ms, 5000);
        // Serialising the projection must not reproduce the identity keys: a
        // restore replays this document, and it may not be able to say where
        // the cell listens.
        let s = meclaw_core::serde_json::to_string(&o).unwrap();
        assert!(
            !s.contains("port"),
            "the overlay must not round-trip the port: {s}"
        );
        assert!(
            !s.contains("bind"),
            "the overlay must not round-trip the bind: {s}"
        );
    }
}
