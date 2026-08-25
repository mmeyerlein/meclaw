//! The session token: minted per page load, and it names the surface.
//!
//! # What it is for
//!
//! LiveView requires a `data-phx-session` attribute on the container and echoes
//! it back on join. The value is entirely ours to define, so we make it carry the
//! one fact the socket must be able to check: **which surface this page was for**.
//! A join whose token disagrees with the URL it arrived on is refused.
//!
//! # Why it is not signed, and when that stops being enough
//!
//! The token never crosses a trust boundary the path does not also cross. Whoever
//! can read the token from a page could have requested that page, and the socket
//! refuses to serve any surface other than the one the token names — so a token is
//! not a way to reach a surface you could not already reach. Signing becomes
//! necessary the moment a token has to survive being handed to a **third party**
//! (a shared link, an embed), because then possession stops implying
//! authorisation. Recorded as deferred rather than pretended away.
//!
//! The nonce is there so two page loads of the same surface do not produce the
//! same string. It carries no authority on its own.

use meclaw_core::Uuid;

/// Mint a token for one page load of `cell_path`.
///
/// Shape: `<nonce-hex>.<path-hex>`. Hex rather than base64 because it needs no
/// crate, is safe in an HTML attribute without escaping, and round trips without
/// padding rules to get wrong.
pub fn mint(cell_path: &str) -> String {
    let nonce = Uuid::now_v7().as_u128();
    format!("{nonce:032x}.{}", hex(cell_path.as_bytes()))
}

/// The surface a token names, or `None` if it names nothing readable.
pub fn surface_of(token: &str) -> Option<String> {
    let (_nonce, path) = token.split_once('.')?;
    let bytes = unhex(path)?;
    String::from_utf8(bytes).ok()
}

/// Whether `token` was minted for `cell_path`.
///
/// The whole security property of this module is this function returning `false`
/// when a page's token is presented on somebody else's socket.
pub fn names(token: &str, cell_path: &str) -> bool {
    surface_of(token).as_deref() == Some(cell_path)
}

/// The container id for a surface's dead render.
///
/// LiveView needs a unique element id and joins on the topic `lv:<that id>`. It is
/// derived from the path rather than random so a reconnect finds the same
/// container, and every character that is not safe in an id becomes `-`.
pub fn container_id(cell_path: &str) -> String {
    let mut s = String::from("surface");
    for ch in cell_path.chars() {
        s.push(if ch.is_ascii_alphanumeric() { ch } else { '-' });
    }
    s
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || s.is_empty() {
        return None;
    }
    let raw = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in raw.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_round_trips_to_the_surface_it_names() {
        let t = mint("/org/acme/canvy/render");
        assert_eq!(surface_of(&t).as_deref(), Some("/org/acme/canvy/render"));
        assert!(names(&t, "/org/acme/canvy/render"));
    }

    /// The security property. A token from one surface must not open another.
    #[test]
    fn a_token_does_not_name_another_surface() {
        let t = mint("/org/acme/canvy/render");
        assert!(!names(&t, "/org/acme/vault"));
        assert!(!names(&t, "/org/acme/canvy/rende"));
        assert!(!names(&t, "/"));
    }

    #[test]
    fn two_page_loads_of_one_surface_differ() {
        assert_ne!(mint("/s"), mint("/s"), "the nonce must vary");
    }

    #[test]
    fn garbage_names_nothing() {
        for bad in ["", ".", "nodot", "abc.zz", "abc.f", "abc."] {
            assert!(!names(bad, "/s"), "{bad:?} must not name a surface");
        }
    }

    #[test]
    fn a_container_id_is_safe_and_stable() {
        let a = container_id("/org/acme/canvy/render");
        assert_eq!(a, container_id("/org/acme/canvy/render"), "stable");
        assert!(a.starts_with("surface"));
        assert!(
            a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "{a} is not attribute-safe"
        );
        assert_ne!(a, container_id("/org/acme/canvy/other"));
    }
}
