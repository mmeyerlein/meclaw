//! axum router for the meclaw HTTP API. Phase 12-A: /health only.
//! Phase 12-B adds /colony/*, phase 12-X the /messages multipart path,
//! phase 12-D the /ui/* HTML handlers.

use crate::ColonyHandle;
use crate::handlers::{
    dead_letters, events, graph, health, ledger, message_log, messages, mutations, registry,
    templates, trace,
};
use crate::ui;
use axum::Router;
use axum::extract::FromRef;
use axum::response::Redirect;
use axum::routing::{get, post};
use meclaw_colony::blob::DiskBlobStore;
use std::path::PathBuf;
use std::sync::Arc;

/// Shared HTTP-handler state. Phase 12-X T17 introduces `blob_store` alongside
/// `colony` so the new multipart-handler at `POST /messages` (T18) can stream
/// uploads into the blob store. Existing handlers extract `State<Arc<ColonyHandle>>`
/// unchanged via `FromRef` — only the multipart-handler needs
/// `State<Arc<DiskBlobStore>>`.
#[derive(Clone)]
pub struct AppState {
    pub colony: Arc<ColonyHandle>,
    pub blob_store: Arc<DiskBlobStore>,
    /// Colony-wide default TTL for initial messages (TTL slice 2026-06-11):
    /// `colony.json::message_default_ttl`, seeded from
    /// `meclaw_core::MESSAGE_DEFAULT_TTL`. `POST /messages` uses it whenever the
    /// request carries no explicit `ttl` field.
    pub message_default_ttl: u32,
    /// GH #159: everything the `/surface/*` routes need, and nothing they do not.
    pub surfaces: SurfaceState,
}

/// GH #159: the surface routes' own state.
///
/// Two things, and the absence of a third is the point: a colony root to resolve a
/// cell path against, and the dispatcher that carries a cell's answer back. **No
/// database handle** — see `docs/meclaw-overview.md` § Datenbank-Isolation. A
/// surface's data is the surface cell's business, obtained by message.
#[derive(Clone)]
pub struct SurfaceState {
    /// The colony tree root. `<root>/main/<cell-path>` is a surface's directory.
    pub colony_root: Arc<PathBuf>,
    /// Message out, HTML back.
    pub dispatcher: Arc<crate::surface::render::Dispatcher>,
}

impl FromRef<AppState> for SurfaceState {
    fn from_ref(s: &AppState) -> Self {
        s.surfaces.clone()
    }
}

impl SurfaceState {
    /// A surface state that can serve nothing: a root that does not exist and a
    /// dispatcher whose colony is gone.
    ///
    /// For every caller that does not exercise a surface. Deliberately **not** a
    /// stub that quietly succeeds — a test that starts reaching a surface by
    /// accident gets a 404 and a timeout rather than a plausible answer.
    pub fn disabled() -> Self {
        let (colony_tx, _colony_rx) = tokio::sync::mpsc::channel(1);
        let (_egress_tx, egress_rx) = tokio::sync::mpsc::channel(1);
        let (dispatcher, _join) = crate::surface::render::Dispatcher::new(
            colony_tx,
            egress_rx,
            meclaw_core::MESSAGE_DEFAULT_TTL,
        );
        Self {
            colony_root: Arc::new(PathBuf::from("/nonexistent-surface-root")),
            dispatcher,
        }
    }
}

impl FromRef<AppState> for Arc<ColonyHandle> {
    fn from_ref(s: &AppState) -> Self {
        s.colony.clone()
    }
}

impl FromRef<AppState> for Arc<DiskBlobStore> {
    fn from_ref(s: &AppState) -> Self {
        s.blob_store.clone()
    }
}

/// Builds the axum router with all routes enabled for the respective phase.
/// Phase 12-A: GET /health → 200 (no colony routing). Every other path falls
/// through to axum's default 404. The /colony/events 501 is phase 14 (U4).
///
/// Phase 12-B T8.1+: the `/colony/*` read handlers move in here one by one.
///
/// Phase 12-X T17: a second param `blob_store` for the multipart upload path in
/// `POST /messages` (T18). Both state slots live in `AppState`; via `FromRef`
/// existing handlers stay unchanged on `Arc<ColonyHandle>`.
///
/// TTL slice (2026-06-11): a third param `message_default_ttl` — the colony.json
/// default for initial messages without an explicit `ttl` request field.
pub fn build_router(
    colony: Arc<ColonyHandle>,
    blob_store: Arc<DiskBlobStore>,
    message_default_ttl: u32,
    surfaces: SurfaceState,
) -> Router {
    let state = AppState {
        colony,
        blob_store,
        message_default_ttl,
        surfaces,
    };
    Router::new()
        // Issue #7: still the HTTP layer's own health check (always 200, no
        // message routed through the colony) — plus the per-I/O-task liveness
        // marks, read from the colony's in-memory map.
        .route("/health", get(health::get_health))
        .route("/colony/registry", get(registry::get_registry))
        .route(
            "/colony/dead_letters",
            get(dead_letters::get_dead_letters).delete(dead_letters::delete_dead_letters),
        )
        .route("/colony/templates", get(templates::get_templates))
        .route("/colony/templates/rescan", post(templates::post_rescan))
        .route("/colony/events", get(events::get_events))
        .route("/colony/trace", get(trace::get_trace))
        // GH #267: the ledger's second door. Counts and sums over one window —
        // never rows, never header content.
        .route("/colony/ledger", get(ledger::get_ledger))
        // P1 message browser — read-only surface over colony.db::message_log.
        .route("/colony/messages", get(message_log::get_message_log))
        .route("/colony/graph", get(graph::get_graph))
        .route(
            "/colony/mutations",
            get(mutations::get_mutations_audit).post(mutations::post_mutation),
        )
        .route("/messages", post(messages::post_messages))
        // Phase 12-D: operator UI (server-rendered HTML, no JS, no auto-refresh,
        // no mutation path). `/` redirects to the dashboard so the operator does
        // not have to type "/ui/" from memory.
        .route("/", get(|| async { Redirect::temporary("/ui/") }))
        .route("/ui/", get(ui::dashboard::get_dashboard))
        .route("/ui/registry", get(ui::registry::get_registry_ui))
        .route("/ui/graph", get(ui::graph::get_graph_ui))
        .route(
            "/ui/dead_letters",
            get(ui::dead_letters::get_dead_letters_ui),
        )
        .route("/ui/messages", get(ui::messages::get_messages_ui))
        .route("/ui/message", get(ui::message::get_message_ui))
        .route("/ui/trace", get(ui::trace::get_trace_ui))
        .route("/ui/templates", get(ui::templates::get_templates_ui))
        // GH #159: surfaces. ONE wildcard route for the page, a surface's own
        // assets and the vendored bundles, plus one for the socket — axum 0.7
        // wildcard syntax is `*rest`, not `{rest}`. The socket route is listed
        // first so its more specific suffix wins.
        .route("/surface/*rest", get(crate::surface::serve::get_surface))
        .with_state(state)
}
