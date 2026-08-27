//! The Phoenix-LiveView protocol, without a server attached.
//!
//! Three things live here, and nothing else: the channels wire format
//! ([`frames`]), the signed session token and the container id derived from a
//! cell path ([`session`]), and the two vendored client bundles compiled into
//! the binary ([`bundle`]) together with the [`LIVEVIEW_VERSION`] reported on
//! every join. It is protocol arithmetic — it binds no socket, opens no port
//! and holds no colony handle.
//!
//! # Who uses it
//!
//! One consumer: the `web` cell in `meclaw-cells`. It owns its own socket loop
//! and answers `phx_join` out of its own materialised pages; what it takes from
//! here is the codec both sides of a Phoenix connection must agree on, the id
//! its container div carries, and the client it serves.
//!
//! # What was taken out, and when
//!
//! The crate was carved out of `meclaw-api/src/surface/` by GH #381, when the
//! `web` cell appeared as a second speaker of the protocol and a cell may not
//! depend on the HTTP API. GH #383 then retired the `/surface/*` route itself,
//! along with the `cell.surface` declaration and the two modules whose type was
//! that declaration (`assets`, `page`); a display is a `web` cell on a port of
//! its own now, and the migration is `templates/canvy/MIGRATION.md`. GH #396
//! removed what that left stranded: the api-side render cache and diff pusher
//! (`Dispatcher`), the connection that drove it (`Connection`), and the parser
//! for the URL scheme whose routes had already gone (`Target`,
//! `parse_target`). None of them had a consumer. Git is the archive.

/// Reported on every join. Must match the vendored bundle in
/// `client/VERSIONS.md`. A mismatch is only a `console.warn` in the client, which
/// is exactly why it is a constant next to a documented rule and a test rather than
/// something a watchdog would catch.
pub const LIVEVIEW_VERSION: &str = "1.2.9";

pub mod frames;
pub mod session;

/// The LiveView client, compiled in. Content type and body.
///
/// A closed list, not a lookup: the file name comes from a URL, and a list makes
/// traversal impossible rather than guarded.
///
/// `include_str!` rather than a directory next to the binary, because the
/// installer puts **one** file in place (`scripts/install.sh`) — a client read
/// from disk would be missing on every machine except the build host, and the
/// failure would be a page that loads and renders nothing.
pub fn bundle(file: &str) -> Option<(&'static str, &'static str)> {
    const JS: &str = "text/javascript; charset=utf-8";
    Some(match file {
        "phoenix.min.js" => (JS, include_str!("client/phoenix.min.js")),
        "phoenix_live_view.min.js" => (JS, include_str!("client/phoenix_live_view.min.js")),
        _ => return None,
    })
}
