//! GET /colony/ledger — GH #267 (ruling Q14), the HTTP twin of the message
//! endpoint Task 11 built.
//!
//! Every `/colony/<endpoint>` is simultaneously a **message target** for
//! internal senders and an **HTTP route** for the external API
//! (`docs/meclaw-overview.md` § `/colony`, "Symmetrie interne API ↔ externe
//! API"). An endpoint with only one of the two doors is a spec violation, not a
//! smaller surface — this module is the second door of `/colony/ledger`.
//!
//! Symmetry means the same colony-task sequence and the same UBF data model, not
//! a literal `route()` call: this handler translates the query string into
//! `ColonyMsg::ReadLedger` and answers under the endpoint-named `ledger` slot,
//! exactly the body the EDA door emits.
//!
//! The refusals are the same refusals, too. The message door takes them in
//! `colony_dispatch::parse_read_query_ledger_filters`; this handler calls **that
//! same parser** rather than restating its two ledger-specific checks
//! (`group_by` vocabulary, `cycle_id` length bound), so the HTTP door can never
//! become the wider one. Only the wrapping differs: a refusal here is a `400`
//! with this surface's own `{"error": "bad_query", "detail": …}` shape (the
//! precedent `handlers/trace.rs` set), not the in-band `invalid_query` slot the
//! message door has to use for lack of a status code.

use crate::ColonyHandle;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use meclaw_colony::ColonyMsg;
use meclaw_colony::colony_dispatch::parse_read_query_ledger_filters;
use meclaw_core::serde_json::json as core_json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Query params for `GET /colony/ledger`.
///
/// Deliberately **without** `deny_unknown_fields` — a frozen property of the
/// query surface: an unknown parameter is ignored, which makes a new filter an
/// additive rather than a breaking change (`docs/meclaw-overview.en.md`, "Two
/// frozen properties of the query surface").
///
/// Every field is optional here and resolved by the shared parser: `since`
/// defaults to `now - 3600`, `until` to `now`, `scan_budget` to 50 000 clamped
/// into `1..=200_000`, `tag` truncated to 64 characters. The reply echoes the
/// values that were actually used.
#[derive(Debug, Deserialize)]
pub struct LedgerQuery {
    /// Window start, inclusive (Unix seconds).
    pub since: Option<i64>,
    /// Window end, exclusive (Unix seconds).
    pub until: Option<i64>,
    /// Cell path whose traffic the prefix/cycle counters ask about.
    pub path_prefix: Option<String>,
    /// Correlation value of `$.hop.cycle_id`; scopes the arrival counter.
    pub cycle_id: Option<String>,
    /// Requested grouping: `model`, `path` or `error_code` (GH #463); anything
    /// else is refused. `by_model` is answered either way — the parameter says
    /// whether a second group map is computed beside it.
    pub group_by: Option<String>,
    /// Opaque caller correlation token, echoed verbatim.
    pub tag: Option<String>,
    /// Rows each windowed sub-query may read.
    pub scan_budget: Option<i64>,
}

/// Handler for `GET /colony/ledger`.
///
/// Answers `200` with `{"ledger": <ReadLedgerReply>}`, `400` for a filter that
/// cannot be read, and `503` when the colony inbox is gone.
pub async fn get_ledger(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<LedgerQuery>,
) -> impl IntoResponse {
    // One parser for both doors: the query string is folded into the documented
    // EDA `{"query": {…}}` envelope and handed to the endpoint's own reader, so
    // defaults, clamps and refusals are shared rather than re-implemented.
    // Absent params stay absent — a `null` would be a value, and the parser's
    // "present but unreadable is refused" rule must not fire on a param nobody
    // sent.
    let mut fields = meclaw_core::serde_json::Map::new();
    if let Some(v) = q.since {
        fields.insert("since".into(), core_json!(v));
    }
    if let Some(v) = q.until {
        fields.insert("until".into(), core_json!(v));
    }
    if let Some(v) = q.scan_budget {
        fields.insert("scan_budget".into(), core_json!(v));
    }
    for (name, value) in [
        ("path_prefix", q.path_prefix),
        ("cycle_id", q.cycle_id),
        ("group_by", q.group_by),
        ("tag", q.tag),
    ] {
        if let Some(v) = value {
            fields.insert(name.into(), core_json!(v));
        }
    }
    let envelope = core_json!({ "query": meclaw_core::serde_json::Value::Object(fields) });

    let query = match parse_read_query_ledger_filters(&envelope) {
        Ok(q) => q,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "bad_query", "detail": e.details })),
            );
        }
    };

    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadLedger { query, ack: ack_tx };
    if colony.inbox.send(msg).await.is_err() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "colony unavailable" })),
        );
    }
    let reply = match ack_rx.await {
        Ok(r) => r,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "colony unavailable" })),
            );
        }
    };
    // Same slot, same shape as the message door's `build_ledger_reply`: the
    // aggregate is an OBJECT under the endpoint's own name, not a list.
    let ledger = serde_json::to_value(&reply).unwrap_or_default();
    (StatusCode::OK, Json(json!({ "ledger": ledger })))
}
