//! HTTP-API + Operator-Web-UI für meclaw (Phase 12).
//!
//! Symmetrie zu /colony/*-Endpunkten ist eine Daten-Ebenen-Aussage:
//! gleiche typisierte ColonyMsg-Inbox-Variante + oneshot-ack, NICHT literal
//! route(). Siehe docs/meclaw-overview.md § /colony als virtueller Endpunkt.

pub mod handlers;
pub mod router;
pub mod ui;

pub use router::AppState;

/// Re-export von `axum` für Konsumenten, die `serve` + `with_graceful_shutdown`
/// brauchen, ohne axum als eigene Top-Level-Dep zu führen (z.B. `meclaw-cli`).
/// Konsistent mit der Plan-Vorgabe „meclaw-api ist die HTTP-Schicht".
pub use axum;

use meclaw_colony::ColonyMsg;

/// Handle für HTTP-Handler zum Senden in Colonys Inbox.
/// `inbox` ist Send+Sync (mpsc::Sender), darf in Arc<ColonyHandle> leben.
///
/// `templates_root` wird benötigt für `POST /colony/templates/rescan` (Phase 12-B T8.5),
/// das den Pfad in `ColonyMsg::RescanTemplates` durchreicht. In Tests, die
/// Rescan nicht treiben, darf `PathBuf::new()` als Stub stehen.
#[derive(Clone)]
pub struct ColonyHandle {
    pub inbox: tokio::sync::mpsc::Sender<ColonyMsg>,
    pub templates_root: std::path::PathBuf,
}
