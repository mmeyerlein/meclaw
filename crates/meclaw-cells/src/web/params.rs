//! W8 (GH #380): the `web` cell's params.
//!
//! Three keys, and none of them is immutable.
//!
//! # A named removal
//!
//! Until `web@1.1.0` a display owned a TCP port: `port` was required, `bind`
//! defaulted to loopback, and a params update naming either moved the running
//! listener. **Both keys are gone since `web@2.0.0`.** A surface cell gets a
//! port only when there is no other way, and a display has another way: the
//! colony's one listener hands it the connections whose first path segment
//! names its [`WebParams::mount`], and the page lives at `/<mount>/`.
//!
//! A document that still carries either key is refused at parse rather than
//! ignored. A display whose `port` was silently dropped would come up at an
//! address nobody expects, and the operator would read the old URL in their own
//! configuration while the browser found nothing there — so the refusal names
//! the migration instead: drop the key, name a mount.

use crate::params_overlay::OverlayParams;
use meclaw_core::JsonValue;

/// What a document carrying `port` is refused with.
const PORT_REMOVED: &str = "port: removed in web 2.0.0 — the cell is reached at /<mount>/ on the \
                            colony's listener; drop the key and name a mount";

/// What a document carrying `bind` is refused with.
const BIND_REMOVED: &str = "bind: removed in web 2.0.0 — the cell is reached at /<mount>/ on the \
                            colony's listener; drop the key and name a mount";

/// Where a `web` cell is reached, and how long it waits on the outside world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebParams {
    /// The name this display is reached under on the colony's listener:
    /// `/<mount>/` is its page, `/<mount>/live/websocket` its socket. Required
    /// — there is no default, because two displays sharing one would be a
    /// mount collision rather than a configuration (R-W8-1: the type is
    /// deliberately multiple). The grammar is
    /// `meclaw_colony::surfaces::mount_is_valid`.
    pub mount: String,
    /// The request header a proxy in front puts the viewer's identity in, or
    /// empty for none. When set, its value on a socket's upgrade request is
    /// stamped as `hop.user_id` on every semantic event of that connection.
    ///
    /// Empty by default, and that is the whole of O-P-4: a header a client can
    /// set without a proxy is not an identity, so nothing is stamped until an
    /// operator names the header their proxy actually writes.
    pub identity_header: String,
    /// Operation-timeout (hard rule 12, A) for I/O this cell initiates.
    pub external_timeout_ms: u64,
}

/// Whether `name` is a header name a request can actually carry.
///
/// Asked of the **param**, not of a request: a typo in a `config.json` must be
/// a refusal at plan time rather than a header that silently never matches.
fn is_header_name(name: &str) -> bool {
    axum::http::HeaderName::from_bytes(name.as_bytes()).is_ok()
}

impl WebParams {
    /// Parse + validate. Shares its path with `validate_params` and
    /// `spawn_cell` (parser invariant, `meclaw_colony::CellFactory`).
    ///
    /// `mount` is refused rather than defaulted: a display nobody named is a
    /// display two instances would fight over.
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("params: must be object")?;

        // Before anything else, so a tree written for `web@1.1.0` reads the
        // migration rather than `mount: required`.
        if obj.contains_key("port") {
            return Err(PORT_REMOVED.to_string());
        }
        if obj.contains_key("bind") {
            return Err(BIND_REMOVED.to_string());
        }

        let mount_raw = obj
            .get("mount")
            .ok_or("mount: required (the name this display is reached under, /<mount>/)")?;
        let mount = mount_raw
            .as_str()
            .ok_or_else(|| format!("mount: must be a string, got {mount_raw}"))?;
        if !meclaw_colony::surfaces::mount_is_valid(mount) {
            return Err(format!(
                "mount: must be 1..=64 characters from [a-z0-9-] and none of the reserved \
                 names, got {mount:?}"
            ));
        }

        let identity_header = obj
            .get("identity_header")
            .map(|h| {
                h.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("identity_header: must be a string, got {h}"))
            })
            .transpose()?
            .unwrap_or_default();
        if !identity_header.is_empty() && !is_header_name(&identity_header) {
            return Err(format!(
                "identity_header: must be a legal HTTP header name or empty, got \
                 {identity_header:?}"
            ));
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
            mount: mount.to_string(),
            identity_header,
            external_timeout_ms,
        })
    }
}

/// The runtime params-update overlay of a `web` cell.
///
/// It carries all three keys, because `apply_update` merges the update over the
/// **serialised current params**: a key that is not serialised here is missing
/// from the merge base, so an update naming only `identity_header` would be
/// re-parsed against a document with no `mount` in it and refused with
/// `mount: required` — a refusal about a key the operator never touched.
///
/// A `mount` update takes effect on the **next life** (O-P-2). The name is
/// registered once per life, at the top of the I/O half, and a live remount
/// would move a running display out from under whichever proxy rule points at
/// it while its viewers hold sockets on the old name. The overlay is what a
/// respawn replays, so the new name is where the display comes back.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebOverlay {
    /// The name this display is reached under. Mutable; effect on the next life.
    pub mount: String,
    /// The proxy's identity header, or empty. Mutable; effect on the next life.
    pub identity_header: String,
    /// Operation-timeout for I/O this cell initiates. Mutable.
    pub external_timeout_ms: u64,
}

impl OverlayParams for WebOverlay {
    /// Every key an update may name.
    const KNOWN_KEYS: &'static [&'static str] =
        &["mount", "identity_header", "external_timeout_ms"];

    /// **Empty.** No param of this cell type is fixed for its lifetime: a
    /// display is renamed by being told to, not by being rebuilt. What a value
    /// cannot be is still refused — by [`WebParams::parse`], as `Invalid`,
    /// which is a statement about the value rather than about the key.
    const IMMUTABLE_KEYS: &'static [&'static str] = &[];

    fn parse(raw: &JsonValue) -> Result<Self, String> {
        // Through the same parser as everything else (parser invariant): the
        // overlay is taken from a fully validated `WebParams`, never parsed on
        // its own with looser rules.
        WebParams::parse(raw).map(|p| Self {
            mount: p.mount,
            identity_header: p.identity_header,
            external_timeout_ms: p.external_timeout_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn a_mount_is_required() {
        let err = WebParams::parse(&json!({})).unwrap_err();
        assert!(err.starts_with("mount: required"), "got {err}");
    }

    /// The migration, and the reason it is a refusal rather than an ignored
    /// key: a display whose `port` was dropped in silence would come up
    /// somewhere the operator's own configuration does not point.
    #[test]
    fn parse_refuses_port_and_bind_since_2_0_0() {
        let with_port = WebParams::parse(&json!({"mount": "web", "port": 7800})).unwrap_err();
        assert_eq!(with_port, PORT_REMOVED, "the refusal names the migration");
        let with_bind =
            WebParams::parse(&json!({"mount": "web", "bind": "127.0.0.1"})).unwrap_err();
        assert_eq!(with_bind, BIND_REMOVED);
        // And the old shape as a whole — a `web@1.1.0` document — reads the
        // port line first, because that is the key an operator has to remove.
        assert_eq!(
            WebParams::parse(&json!({"port": 7800, "bind": "127.0.0.1"})).unwrap_err(),
            PORT_REMOVED
        );
    }

    #[test]
    fn a_mount_outside_the_grammar_is_refused() {
        for bad in [
            json!("Web"),
            json!("a/b"),
            json!("colony"),
            json!(""),
            json!(7),
        ] {
            let err = WebParams::parse(&json!({"mount": bad})).unwrap_err();
            assert!(err.starts_with("mount: "), "{bad}: {err}");
        }
    }

    #[test]
    fn the_identity_header_defaults_to_nothing() {
        let p = WebParams::parse(&json!({"mount": "web"})).unwrap();
        assert_eq!(p.mount, "web");
        assert_eq!(p.identity_header, "");
        assert_eq!(p.external_timeout_ms, 5000);
    }

    #[test]
    fn an_identity_header_that_is_not_a_header_name_is_refused() {
        let err = WebParams::parse(&json!({"mount": "web", "identity_header": "X Forwarded User"}))
            .unwrap_err();
        assert!(err.starts_with("identity_header: "), "got {err}");
        assert!(
            WebParams::parse(&json!({"mount": "web", "identity_header": "X-Forwarded-User"}))
                .is_ok(),
            "a legal header name passes"
        );
    }

    #[test]
    fn no_param_of_a_display_is_immutable() {
        assert!(
            WebOverlay::IMMUTABLE_KEYS.is_empty(),
            "a display is renamed by being told to, not by being rebuilt"
        );
        for key in ["mount", "identity_header", "external_timeout_ms"] {
            assert!(WebOverlay::KNOWN_KEYS.contains(&key), "{key} must be known");
        }
    }

    #[test]
    fn an_identity_header_only_update_does_not_lose_the_mount() {
        // The merge base is the serialised current params. A projection without
        // `mount` would refuse this update with `mount: required` — a refusal
        // about a key the sender never named.
        use crate::params_overlay::apply_update;
        let current = <WebOverlay as OverlayParams>::parse(&json!({"mount": "web"})).unwrap();
        let mut update = meclaw_core::serde_json::Map::new();
        update.insert("identity_header".into(), json!("X-Forwarded-User"));
        let (merged, overlay) = apply_update(&current, &update).expect("the update applies");
        assert_eq!(merged.mount, "web");
        assert_eq!(merged.identity_header, "X-Forwarded-User");
        assert_eq!(
            overlay,
            vec![("identity_header".to_string(), json!("X-Forwarded-User"))]
        );
    }

    #[test]
    fn a_value_that_is_not_a_mount_is_still_refused() {
        use crate::params_overlay::{ParamUpdateError, apply_update};
        let current = <WebOverlay as OverlayParams>::parse(&json!({"mount": "web"})).unwrap();
        for bad in [json!("Web"), json!("live"), json!(7800)] {
            let mut update = meclaw_core::serde_json::Map::new();
            update.insert("mount".into(), bad.clone());
            let err = apply_update(&current, &update).expect_err("must refuse");
            assert!(
                matches!(err, ParamUpdateError::Invalid(_)),
                "a bad value is Invalid, not Immutable: {bad} gave {err:?}"
            );
        }
    }

    #[test]
    fn the_overlay_round_trips_the_name_the_display_answers_to() {
        // A display that was renamed has to come back under the new name, so
        // the serialised overlay carries it.
        let o = <WebOverlay as OverlayParams>::parse(
            &json!({"mount": "screen", "identity_header": "X-User"}),
        )
        .unwrap();
        let s = meclaw_core::serde_json::to_string(&o).unwrap();
        assert!(s.contains("screen"), "the overlay carries the mount: {s}");
        assert!(s.contains("X-User"), "and the identity header: {s}");
        let back: JsonValue = meclaw_core::serde_json::from_str(&s).unwrap();
        let again = WebParams::parse(&back).unwrap();
        assert_eq!(
            (again.mount.as_str(), again.identity_header.as_str()),
            ("screen", "X-User")
        );
    }
}
