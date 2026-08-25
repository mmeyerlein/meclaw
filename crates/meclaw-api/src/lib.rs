//! HTTP API + operator web UI for meclaw (phase 12).
//!
//! Symmetry with the /colony/* endpoints is a data-plane statement: the same
//! typed ColonyMsg inbox variant + oneshot ack, NOT literally route(). See
//! docs/meclaw-overview.md § `/colony` as a virtual endpoint.
//!
//! Internal crate — the public contract is the HTTP API and the template DSL;
//! no SemVer guarantee on Rust items. See README.md § Stability.

pub mod handlers;
pub mod router;
pub mod ui;

pub use router::AppState;

/// Re-export of `axum` for consumers that need `serve` + `with_graceful_shutdown`
/// without carrying axum as their own top-level dep (e.g. `meclaw-cli`).
/// Consistent with the plan's rule that "meclaw-api is the HTTP layer".
pub use axum;

use meclaw_colony::ColonyMsg;

/// Handle for HTTP handlers to send into colony's inbox.
/// `inbox` is Send+Sync (mpsc::Sender), so it may live in an Arc<ColonyHandle>.
///
/// `templates_root` is needed for `POST /colony/templates/rescan` (phase 12-B
/// T8.5), which passes the path through in `ColonyMsg::RescanTemplates`. Tests
/// that do not drive a rescan may leave `PathBuf::new()` as a stub.
#[derive(Clone)]
pub struct ColonyHandle {
    pub inbox: tokio::sync::mpsc::Sender<ColonyMsg>,
    pub templates_root: std::path::PathBuf,
}
