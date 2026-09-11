//! axum router for the meclaw HTTP API. Phase 12-A: /health only.
//! Phase 12-B adds /colony/*, phase 12-X the /messages multipart path,
//! phase 12-D the /ui/* HTML handlers.

use crate::ColonyHandle;
use crate::handlers::{
    dead_letters, events, graph, health, ledger, message_log, messages, mutations, registry,
    surfaces, templates, trace,
};
use crate::ui;
use axum::Router;
use axum::extract::FromRef;
use axum::response::Redirect;
use axum::routing::{get, post};
use meclaw_colony::SurfaceRegistry;
use meclaw_colony::blob::DiskBlobStore;
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
    /// The process's mount table (ADR-0031), minted by the CLI and shared with
    /// the surface factories and the one listener. `GET /colony/surfaces` reads
    /// it; nothing in this crate writes it.
    pub surfaces: Arc<SurfaceRegistry>,
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

impl FromRef<AppState> for Arc<SurfaceRegistry> {
    fn from_ref(s: &AppState) -> Self {
        s.surfaces.clone()
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
///
/// GH #383: and back to three. The fourth param was `SurfaceState`, the state of
/// the `/surface/*` route; both retired with the serving path they belonged to
/// (`templates/canvy/MIGRATION.md`). A display is a `web` cell on a port of its
/// own now, so this router serves the operator UI and the colony endpoints —
/// exactly what it served before GH #159 — and nothing an application draws.
///
/// GH #644: a fourth param again, `surfaces` — the process's mount table, read
/// by `GET /colony/surfaces`. It is not the retired `SurfaceState` returning: the
/// registry serves no bytes, it only says which name reaches which cell.
pub fn build_router(
    colony: Arc<ColonyHandle>,
    blob_store: Arc<DiskBlobStore>,
    message_default_ttl: u32,
    surfaces: Arc<SurfaceRegistry>,
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
        // GH #644: the mount table of the surface cells. A read of the process's
        // own registry — no message, hence no route() on this path.
        .route("/colony/surfaces", get(surfaces::get_surfaces))
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
        .with_state(state)
}
