//! axum-Router für die meclaw-HTTP-API. Phase 12-A: nur /health.
//! Phase 12-B fügt /colony/* hinzu, Phase 12-X den /messages-multipart-Pfad,
//! Phase 12-D die /ui/*-HTML-Handler.

use crate::ColonyHandle;
use crate::handlers::{
    dead_letters, events, graph, messages, mutations, registry, templates, trace,
};
use crate::ui;
use axum::Router;
use axum::extract::FromRef;
use axum::response::Redirect;
use axum::routing::{get, post};
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

/// Baut den axum-Router mit allen aktivierten Routes für die jeweilige Phase.
/// Phase 12-A: GET /health → 200 (kein Colony-Routing). Alle anderen Pfade
/// fallen auf axum-Default-404. /colony/events-501 ist Phase 14 (U4).
///
/// Phase 12-B T8.1+: `/colony/*`-Read-Handler wandern hier nacheinander rein.
///
/// Phase 12-X T17: zweiter Param `blob_store` für den multipart-Upload-Pfad
/// in `POST /messages` (T18). Beide State-Slots leben in `AppState`; via
/// `FromRef` bleiben bestehende Handler unverändert auf `Arc<ColonyHandle>`.
///
/// TTL slice (2026-06-11): dritter Param `message_default_ttl` — der
/// colony.json-Default für Initial-Messages ohne explizites `ttl`-Request-Feld.
pub fn build_router(
    colony: Arc<ColonyHandle>,
    blob_store: Arc<DiskBlobStore>,
    message_default_ttl: u32,
) -> Router {
    let state = AppState {
        colony,
        blob_store,
        message_default_ttl,
    };
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/colony/registry", get(registry::get_registry))
        .route(
            "/colony/dead_letters",
            get(dead_letters::get_dead_letters).delete(dead_letters::delete_dead_letters),
        )
        .route("/colony/templates", get(templates::get_templates))
        .route("/colony/templates/rescan", post(templates::post_rescan))
        .route("/colony/events", get(events::get_events))
        .route("/colony/trace", get(trace::get_trace))
        .route("/colony/graph", get(graph::get_graph))
        .route(
            "/colony/mutations",
            get(mutations::get_mutations_audit).post(mutations::post_mutation),
        )
        .route("/messages", post(messages::post_messages))
        // Phase 12-D: Operator-UI (server-rendered HTML, kein JS, kein
        // Auto-Refresh, kein Mutations-Pfad). `/` redirected auf das
        // Dashboard, damit der Operator nicht auswendig "/ui/" tippen muss.
        .route("/", get(|| async { Redirect::temporary("/ui/") }))
        .route("/ui/", get(ui::dashboard::get_dashboard))
        .route("/ui/registry", get(ui::registry::get_registry_ui))
        .route("/ui/graph", get(ui::graph::get_graph_ui))
        .route(
            "/ui/dead_letters",
            get(ui::dead_letters::get_dead_letters_ui),
        )
        .route("/ui/trace", get(ui::trace::get_trace_ui))
        .route("/ui/templates", get(ui::templates::get_templates_ui))
        .with_state(state)
}
