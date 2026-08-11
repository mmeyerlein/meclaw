//! GET /health — issue #7.
//!
//! Still a health check of the HTTP layer itself: it answers 200 whatever the
//! colony is doing, and it does not route a message through the colony. What it
//! gained is the one signal that was missing when a proxy hung for a quarter of
//! an hour behind a 200: the per-I/O-task progress marks
//! (`ColonyMsg::ReadLiveness`, an in-memory read of the colony's own map, the
//! same inbox-read shape as `/colony/*`).
//!
//! Deliberate non-decisions:
//! - the status code stays 200 for every state, so nothing that probes this
//!   endpoint changes meaning;
//! - `last_success_secs` is an AGE, not a verdict — a `timer` waiting for
//!   tomorrow's cron is quiet on purpose, and only a caller who knows the cell's
//!   cadence can call a silence a fault;
//! - a colony that cannot answer within the deadline below yields
//!   `io_liveness: null` rather than an empty list, because "I could not read
//!   the marks" and "there are no marked cells" are different facts — and a
//!   wedged colony loop is itself a finding.

use crate::ColonyHandle;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use meclaw_colony::ColonyMsg;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// How long `/health` waits for the colony's answer.
///
/// Semantic discriminator, kept tight on purpose: a health probe must return
/// promptly precisely when the colony loop is wedged — the state the marks exist
/// to expose. The read itself is a `HashMap` projection with no I/O, so a
/// healthy colony answers in microseconds; anything near this bound is already
/// the pathology.
const LIVENESS_READ_TIMEOUT: Duration = Duration::from_millis(500);

/// Handler for `GET /health`. Always 200.
pub async fn get_health(State(colony): State<Arc<ColonyHandle>>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "io_liveness": read_liveness(&colony).await,
        })),
    )
}

/// Reads the marks, or `None` when the colony does not answer in time.
///
/// `try_send`, not `send().await`: a full colony inbox must not make the health
/// probe wait on the loop it is reporting about.
async fn read_liveness(colony: &ColonyHandle) -> Option<serde_json::Value> {
    let (ack_tx, ack_rx) = oneshot::channel();
    colony
        .inbox
        .try_send(ColonyMsg::ReadLiveness { ack: ack_tx })
        .ok()?;
    let reply = tokio::time::timeout(LIVENESS_READ_TIMEOUT, ack_rx)
        .await
        .ok()?
        .ok()?;
    serde_json::to_value(reply.entries).ok()
}
