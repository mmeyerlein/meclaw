//! The surface machinery: Phoenix-LiveView serving, shared by every consumer.
//!
//! # Why this is its own crate
//!
//! GH #381. This code was `meclaw-api/src/surface/` until a second consumer
//! appeared: the `web` cell (GH #380) speaks the same protocol from inside
//! `meclaw-cells`, and a cell may not depend on the HTTP API — that would be a
//! layering cycle and would put an axum router in every colony that never
//! serves one. So the machinery moved down here, below both, and the axum
//! handler that binds it to the API's router stayed behind in
//! `meclaw-api/src/surface/serve.rs`, which is where router glue belongs.
//!
//! # What GH #383 took back out
//!
//! That handler, and the `/surface/*` route it sat on, are **gone** — with the
//! `cell.surface` declaration they resolved and the `meclaw_colony::surface`
//! module that parsed it. Two modules of this crate went with them, because
//! their type was that declaration: `assets` (a surface's own files) and `page`
//! (the dead render). A display is a `web` cell on a port of its own now; the
//! migration is `templates/canvy/MIGRATION.md`.
//!
//! What remains is what the `web` cell speaks: [`session`], [`frames`],
//! [`bundle`] and the LiveView version in [`socket`]. [`render`] and
//! [`socket::Connection`] are the api-side halves of the retired path and are
//! kept — they carry no consumer today.
//!
//! What follows is the original routing rationale. It describes what
//! [`parse_target`] does and why the two reserved names exist; the URLs it
//! quotes are the retired ones, kept because the two reserved names still hold
//! and the reasoning for them is here and nowhere else.
//!
//! # Surfaces over HTTP: the routing half.
//!
//! # Why the cell path is the URL
//!
//! `/surface/<cell-path>`. Many surfaces are told apart by exactly what tells
//! cells apart, so there is no second namespace to keep in step — no surface id,
//! no registry of pages, nothing that can disagree with the tree. And a reverse
//! proxy in front gets a complete access rule out of one prefix:
//!
//! ```text
//! location /surface/org/acme/member/alice/canvy/ { … }
//! ```
//!
//! covers that surface's page, its own files **and its transport**, without
//! knowing anything about MeClaw.
//!
//! # Why every verb is a suffix
//!
//! A prefix (`/surface/@state/org/acme/…`) would route more easily and would
//! break exactly the promise above: the page and the state would sit under
//! different prefixes, and one `location` block could no longer authorise both.
//! So the verb goes at the end, and two names are reserved to keep that
//! unambiguous:
//!
//! - **`@…`** is ours. A colony path segment may not start with `@`, which is
//!   what keeps a cell named `state` addressable.
//! - **`live`** is the phoenix client's. `LiveSocket` appends exactly
//!   `"/websocket"` to the URL it is handed (`assets/js/phoenix/socket.js:191`),
//!   so the suffix is the bundle's and the prefix is ours. A cell named `live`
//!   is refused here too, so it cannot shadow a transport.
//!
//! # Two owners of static files
//!
//! `/surface/@client/<file>` serves the two vendored LiveView bundles, compiled
//! into the binary: they are the client half of the protocol the binary speaks,
//! and the `liveview_version` reported on join must move with them.
//! `/surface/<cell>/@asset/<file>` served the surface's **own** files out of its
//! own cell directory. **Retracted with GH #383**: the second owner is gone,
//! along with the `assets` module that read it. A `web` cell serves its own
//! files out of its `assets` table instead — rows, not a directory.

/// The one whole path segment the phoenix client owns.
const RESERVED_SEGMENT: &str = "live";

pub mod frames;
pub mod render;
pub mod session;
pub mod socket;

/// What a `/surface/*` URL asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The dead render for a surface.
    Page { cell: String },
    /// The phoenix socket for a surface.
    Socket { cell: String },
    /// One file out of a surface's own asset directory.
    Asset { cell: String, file: String },
    /// One of the vendored bundles, compiled into the binary.
    Client { file: String },
}

/// Parse the path after `/surface/` into a target. Pure; `None` is a 404.
///
/// The order of these checks is the specification:
///
/// 1. `@client/<file>` — the binary's own bundles, first, because the prefix
///    cannot be a cell path (a colony segment may not start with `@`).
/// 2. `…/live/websocket` — the transport. The suffix is the phoenix client's.
/// 3. `…/@asset/<file>` — the surface's own files.
/// 4. otherwise a page, if no segment starts with `@`.
/// 5. else nothing.
pub fn parse_target(rest: &str) -> Option<Target> {
    if let Some(file) = rest.strip_prefix("@client/") {
        return plain_file(file).map(|file| Target::Client { file });
    }
    if let Some(cell) = rest.strip_suffix("/live/websocket") {
        return plain_cell(cell).map(|cell| Target::Socket { cell });
    }
    if let Some((cell, file)) = rest.split_once("/@asset/") {
        let cell = plain_cell(cell)?;
        return plain_file(file).map(|file| Target::Asset { cell, file });
    }
    plain_cell(rest).map(|cell| Target::Page { cell })
}

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

/// A cell path fit to hand to `locate`.
///
/// The heavy checking is `locate`'s — it is the one that touches the filesystem.
/// This rejects what would make the *routing* ambiguous, and it rejects the same
/// two reserved names `locate` does, so the two agree **by construction** rather
/// than by both happening to answer 404. `…/live` alone must miss for the same
/// reason `…/live/websocket` is the transport: one name, one meaning.
fn plain_cell(s: &str) -> Option<String> {
    if s.is_empty() || s.starts_with('/') || s.ends_with('/') {
        return None;
    }
    if s.split('/')
        .any(|seg| seg.is_empty() || seg.starts_with('@') || seg == RESERVED_SEGMENT)
    {
        return None;
    }
    Some(s.to_string())
}

/// One file name. No separator, no traversal, no empty, no NUL.
fn plain_file(s: &str) -> Option<String> {
    if s.is_empty()
        || s.contains('/')
        || s.contains('\\')
        || s.contains('\0')
        || s == "."
        || s == ".."
    {
        return None;
    }
    Some(s.to_string())
}
