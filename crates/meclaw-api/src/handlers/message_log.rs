//! `GET /colony/messages` — P1 message browser (read-only).
//!
//! Wraps `ColonyMsg::ReadMessages` the same way `handlers::trace` wraps
//! `ReadTrace`: query → filter, oneshot-ack, JSON out. UUID-shaped fields are
//! validated inline; a parse failure is 400 `{"error":"bad_query", …}` and the
//! detail names the *field*, never the submitted value (secret hygiene).
//!
//! Blob-bodied messages keep their blob id in `body_payload`; the body itself is
//! only fetched when the caller asks for it via `?resolve_blob=true`.

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::{MessageLogCursor, MessageLogFilter};
use meclaw_colony::blob::DiskBlobStore;
use meclaw_core::Uuid;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Default rows read before residual filtering. Deliberately modest: the read
/// stalls the Colony-Inbox loop for its duration.
pub(crate) const DEFAULT_SCAN_BUDGET: usize = 5000;
/// Server-side ceiling on `?scan_budget` — not overridable by the client.
pub(crate) const MAX_SCAN_BUDGET: usize = 50_000;

/// Query params for `GET /colony/messages`.
#[derive(Debug, Deserialize, Default)]
pub struct MessageLogQuery {
    /// Exact message id (UUID string) — single-row lookup, overrides all filters.
    pub id: Option<String>,
    /// Filter by `trace_id` (UUID string).
    pub trace_id: Option<String>,
    /// Filter by `parent_message_id` (UUID string) — the children of one message.
    pub parent_message_id: Option<String>,
    /// Filter by `correlation_id` (UUID string).
    pub correlation_id: Option<String>,
    /// Filter by `to_path` prefix (hive-scope view).
    pub to_path_prefix: Option<String>,
    /// Filter by `from_path` prefix.
    pub from_path_prefix: Option<String>,
    /// Filter by body variant: `inline` | `blob`.
    pub body_kind: Option<String>,
    /// Only rows with `created_at >= since` (Unix seconds).
    pub since: Option<i64>,
    /// Only rows with `created_at <= until` (Unix seconds).
    pub until: Option<i64>,
    /// Keyset cursor part 1: `created_at` of the previous page's last row.
    pub before_created_at: Option<i64>,
    /// Keyset cursor part 2: `id` of the previous page's last row (UUID string).
    pub before_id: Option<String>,
    /// Rows returned (default 100, hard cap 1000).
    pub limit: Option<usize>,
    /// Rows read before residual filtering (default 5000, hard cap 50000).
    pub scan_budget: Option<usize>,
    /// Resolve `blob`-kind bodies through the blob store (off by default).
    pub resolve_blob: Option<bool>,
}

/// Validate an optional UUID-shaped query field.
///
/// `Err(field)` carries the *field name* only — the rejected value never leaves
/// this function, neither into a response nor into a log line.
fn parse_uuid_field(
    value: Option<&str>,
    field: &'static str,
) -> Result<Option<String>, &'static str> {
    match value {
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(_) => Ok(Some(raw.to_string())),
            Err(_) => Err(field),
        },
        None => Ok(None),
    }
}

/// Build the colony-side filter from validated query params.
fn build_filter(q: &MessageLogQuery) -> Result<MessageLogFilter, &'static str> {
    let before = match (q.before_created_at, q.before_id.as_deref()) {
        (Some(created_at), Some(raw)) => {
            let id = parse_uuid_field(Some(raw), "before_id")?.expect("validated above");
            Some(MessageLogCursor { created_at, id })
        }
        _ => None,
    };
    Ok(MessageLogFilter {
        id: parse_uuid_field(q.id.as_deref(), "id")?,
        trace_id: parse_uuid_field(q.trace_id.as_deref(), "trace_id")?,
        parent_message_id: parse_uuid_field(q.parent_message_id.as_deref(), "parent_message_id")?,
        correlation_id: parse_uuid_field(q.correlation_id.as_deref(), "correlation_id")?,
        to_path_prefix: q.to_path_prefix.clone(),
        from_path_prefix: q.from_path_prefix.clone(),
        body_kind: q.body_kind.clone(),
        since: q.since,
        until: q.until,
        before,
        limit: clamp_limit(q.limit),
        scan_budget: q
            .scan_budget
            .unwrap_or(DEFAULT_SCAN_BUDGET)
            .clamp(1, MAX_SCAN_BUDGET),
    })
}

/// Handler for `GET /colony/messages`.
pub async fn get_message_log(
    State(colony): State<Arc<ColonyHandle>>,
    State(blob_store): State<Arc<DiskBlobStore>>,
    Query(q): Query<MessageLogQuery>,
) -> impl IntoResponse {
    let filter = match build_filter(&q) {
        Ok(f) => f,
        Err(field) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "bad_query",
                    "detail": format!("{field} is not a valid UUID"),
                })),
            );
        }
    };

    let (ack_tx, ack_rx) = oneshot::channel();
    if colony
        .inbox
        .send(ColonyMsg::ReadMessages {
            filter,
            ack: ack_tx,
        })
        .await
        .is_err()
    {
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

    let resolve = q.resolve_blob.unwrap_or(false);
    let mut messages = Vec::with_capacity(reply.entries.len());
    for entry in &reply.entries {
        let mut value = serde_json::to_value(entry).unwrap_or_else(|_| json!({}));
        if resolve && entry.body_kind == "blob" {
            attach_blob_body(&blob_store, entry.body_payload.as_deref(), &mut value).await;
        }
        messages.push(value);
    }

    (
        StatusCode::OK,
        Json(json!({
            "messages": messages,
            "next": reply.next,
            "scan_budget": reply.scan_budget,
            "scan_truncated": reply.scan_truncated,
        })),
    )
}

/// Resolve one blob body into `value["blob_body"]`, or record the failure class
/// in `value["blob_error"]`. Never surfaces the store path or the blob content
/// on the error path.
async fn attach_blob_body(
    blob_store: &DiskBlobStore,
    body_payload: Option<&str>,
    value: &mut serde_json::Value,
) {
    let Some(raw) = body_payload else {
        value["blob_error"] = json!("missing_blob_id");
        return;
    };
    let Ok(blob_id) = Uuid::parse_str(raw) else {
        value["blob_error"] = json!("malformed_blob_id");
        return;
    };
    match blob_store.read_body(blob_id).await {
        Ok(body) => value["blob_body"] = body,
        Err(_) => value["blob_error"] = json!("blob_unreadable"),
    }
}
