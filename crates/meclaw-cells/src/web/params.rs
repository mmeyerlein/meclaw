//! W8 (GH #380): the `web` cell's params.
//!
//! Three keys, and **none of them is immutable** (GH #410).
//!
//! # A named retraction
//!
//! Until `0.22.5` this module said the opposite: `port` and `bind` were settled
//! at birth, and a params update naming either was refused as `Immutable`. The
//! argument was that rebinding a live display would move a running service out
//! from under whatever reverse proxy is pointed at it (R-W8-2 puts auth and TLS
//! in front, forever). **That refusal is withdrawn.**
//!
//! What it protected against was silent divergence between the declared params
//! and the socket the cell actually holds — and an *accepted* params update is
//! the declared params moving, so the protection does not apply to it. What it
//! cost was real: moving a display from loopback to a LAN bind meant
//! re-instantiating the cell and replaying every hand-made object position,
//! because a new instance is a new `cell.db`. The W8 ruling *"the port is the
//! identity"* is untouched by this — it separated two displays at instantiation
//! time, and a rebind changes neither the cell path, nor its database, nor its
//! declared contract. Only the socket moves.

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

/// The runtime params-update overlay of a `web` cell.
///
/// It carries **all three** keys, and that is a consequence of the retraction
/// above rather than a stylistic choice. It used to be a projection holding
/// only `external_timeout_ms`, following the proxy precedent that identity keys
/// must not round-trip through `cell.db`. Two things follow from `port` and
/// `bind` becoming mutable:
///
/// - `apply_update` merges the update over the **serialised current params**,
///   so a key that is not serialised here is missing from the merge base. An
///   update naming only `bind` would be re-parsed against a document with no
///   `port` in it and refused with `port: required` — a refusal about a key the
///   operator never touched.
/// - A move that did not survive a respawn would be exactly the divergence the
///   old immutability was protecting against, only in the other direction: the
///   overlay would say one address and the socket would hold another. The
///   restore in [`crate::web::factory`] replays this document, so a display
///   that was moved comes back where it was moved to.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebOverlay {
    /// The TCP port this instance listens on. Mutable since GH #410.
    pub port: u16,
    /// The address this instance binds. Mutable since GH #410.
    pub bind: String,
    /// Operation-timeout for I/O this cell initiates. Mutable.
    pub external_timeout_ms: u64,
}

impl OverlayParams for WebOverlay {
    /// Every key an update may name.
    const KNOWN_KEYS: &'static [&'static str] = &["port", "bind", "external_timeout_ms"];

    /// **Empty** (GH #410). No param of this cell type is fixed for its
    /// lifetime: a display moves to another address by being told to, not by
    /// being rebuilt. A value that cannot be a listening address is still
    /// refused — by [`WebParams::parse`], as `Invalid`, which is a statement
    /// about the value rather than about the key.
    const IMMUTABLE_KEYS: &'static [&'static str] = &[];

    fn parse(raw: &JsonValue) -> Result<Self, String> {
        // Through the same parser as everything else (parser invariant): the
        // overlay is taken from a fully validated `WebParams`, never parsed on
        // its own with looser rules.
        WebParams::parse(raw).map(|p| Self {
            port: p.port,
            bind: p.bind,
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
    fn no_param_of_a_display_is_immutable() {
        // The inverse of what this file asserted until 0.22.5, and the
        // retraction is the point: `port` and `bind` are known keys that an
        // update may name, and nothing here refuses them for being what they
        // are. A value that is not a port is still refused — but by the parser,
        // and as a statement about the value.
        assert!(
            WebOverlay::IMMUTABLE_KEYS.is_empty(),
            "GH #410: a display moves by being told to, not by being rebuilt"
        );
        for key in ["port", "bind", "external_timeout_ms"] {
            assert!(WebOverlay::KNOWN_KEYS.contains(&key), "{key} must be known");
        }
    }

    #[test]
    fn a_port_update_is_accepted_and_carried_in_the_overlay() {
        use crate::params_overlay::apply_update;
        let current = <WebOverlay as OverlayParams>::parse(&json!({"port": 7800})).unwrap();
        let mut update = meclaw_core::serde_json::Map::new();
        update.insert("port".into(), json!(7801));
        let (merged, overlay) = apply_update(&current, &update).expect("a port update applies");
        assert_eq!(merged.port, 7801);
        assert_eq!(merged.bind, "127.0.0.1", "an untouched key keeps its value");
        assert_eq!(overlay, vec![("port".to_string(), json!(7801))]);
    }

    #[test]
    fn a_bind_only_update_does_not_lose_the_port() {
        // The merge base is the serialised current params. When the overlay was
        // a projection without `port`, this update was refused with
        // `port: required` — a refusal about a key the sender never named.
        use crate::params_overlay::apply_update;
        let current = <WebOverlay as OverlayParams>::parse(&json!({"port": 7800})).unwrap();
        let mut update = meclaw_core::serde_json::Map::new();
        update.insert("bind".into(), json!("0.0.0.0"));
        let (merged, _) = apply_update(&current, &update).expect("a bind update applies");
        assert_eq!(merged.port, 7800);
        assert_eq!(merged.bind, "0.0.0.0");
    }

    #[test]
    fn a_value_that_is_not_an_address_is_still_refused() {
        use crate::params_overlay::{ParamUpdateError, apply_update};
        let current = <WebOverlay as OverlayParams>::parse(&json!({"port": 7800})).unwrap();
        for bad in [json!(0), json!(70000), json!("7801")] {
            let mut update = meclaw_core::serde_json::Map::new();
            update.insert("port".into(), bad.clone());
            let err = apply_update(&current, &update).expect_err("must refuse {bad}");
            assert!(
                matches!(err, ParamUpdateError::Invalid(_)),
                "a bad value is Invalid, not Immutable: {bad} gave {err:?}"
            );
        }
        let mut update = meclaw_core::serde_json::Map::new();
        update.insert("bind".into(), json!(""));
        assert!(apply_update(&current, &update).is_err(), "an empty bind");
    }

    #[test]
    fn the_overlay_round_trips_where_the_cell_listens() {
        // The other half of the retraction. This used to assert the opposite —
        // that the serialised overlay must NOT name `port` or `bind`, so a
        // restore could not say where the cell listens. Now it must: a display
        // that was moved has to come back where it was moved to.
        let o = <WebOverlay as OverlayParams>::parse(&json!({"port": 7801, "bind": "0.0.0.0"}))
            .unwrap();
        let s = meclaw_core::serde_json::to_string(&o).unwrap();
        assert!(s.contains("7801"), "the overlay carries the port: {s}");
        assert!(s.contains("0.0.0.0"), "the overlay carries the bind: {s}");
        // And it round-trips through the same parser it came from.
        let back: JsonValue = meclaw_core::serde_json::from_str(&s).unwrap();
        let again = WebParams::parse(&back).unwrap();
        assert_eq!((again.port, again.bind.as_str()), (7801, "0.0.0.0"));
    }
}
