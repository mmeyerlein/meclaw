//! `GET /ui/messages` — P1 message browser, list view.
//!
//! Newest first, keyset-paginated, filterable. Symmetric to the JSON handler in
//! `handlers::message_log`: same `ColonyMsg::ReadMessages` read, HTML instead of
//! JSON. Read-only — no forms besides the filter GET form, no action buttons.
//!
//! Payloads are truncated here; the full body lives in the detail view
//! (`ui::message`). Blob bodies are never resolved in the list — that is one
//! store read per row and defeats the "lazy" in lazy resolution.

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use crate::handlers::message_log::{DEFAULT_SCAN_BUDGET, MAX_SCAN_BUDGET};
use crate::ui::layout;
use crate::ui::message_render::{render_scan_notice, truncate};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use maud::{Markup, html};
use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::{MessageLogCursor, MessageLogDto, MessageLogFilter};
use meclaw_core::Uuid;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Characters of `body_payload` shown per row.
const PAYLOAD_PREVIEW_CHARS: usize = 120;

/// Query params for `/ui/messages` — mirrors the JSON endpoint's filters.
#[derive(Debug, Deserialize, Default)]
pub struct MessagesUiQuery {
    /// Filter by `to_path` prefix (hive-scope view).
    pub to_path_prefix: Option<String>,
    /// Filter by `from_path` prefix.
    pub from_path_prefix: Option<String>,
    /// Filter by `trace_id` (UUID string).
    pub trace_id: Option<String>,
    /// Filter by `parent_message_id` (UUID string).
    pub parent_message_id: Option<String>,
    /// Filter by `correlation_id` (UUID string).
    pub correlation_id: Option<String>,
    /// Filter by body variant (`inline` | `blob`).
    pub body_kind: Option<String>,
    /// Only rows with `created_at >= since` (Unix seconds).
    pub since: Option<i64>,
    /// Only rows with `created_at <= until` (Unix seconds).
    pub until: Option<i64>,
    /// Keyset cursor part 1 (`created_at` of the previous page's last row).
    pub before_created_at: Option<i64>,
    /// Keyset cursor part 2 (`id` of the previous page's last row).
    pub before_id: Option<String>,
    /// Rows per page (default 100, cap 1000).
    pub limit: Option<usize>,
    /// Rows read before residual filtering (default 5000, cap 50000).
    pub scan_budget: Option<usize>,
}

impl MessagesUiQuery {
    /// Validate the UUID-shaped fields and build the colony-side filter.
    /// `Err(field)` names the offending field — never its value.
    fn to_filter(&self) -> Result<MessageLogFilter, &'static str> {
        let check =
            |v: &Option<String>, field: &'static str| -> Result<Option<String>, &'static str> {
                match v.as_deref() {
                    Some(raw) => Uuid::parse_str(raw)
                        .map(|_| Some(raw.to_string()))
                        .map_err(|_| field),
                    None => Ok(None),
                }
            };
        let before = match (self.before_created_at, self.before_id.as_deref()) {
            (Some(created_at), Some(raw)) => Some(MessageLogCursor {
                created_at,
                id: Uuid::parse_str(raw)
                    .map(|_| raw.to_string())
                    .map_err(|_| "before_id")?,
            }),
            _ => None,
        };
        Ok(MessageLogFilter {
            id: None,
            trace_id: check(&self.trace_id, "trace_id")?,
            parent_message_id: check(&self.parent_message_id, "parent_message_id")?,
            correlation_id: check(&self.correlation_id, "correlation_id")?,
            to_path_prefix: self.to_path_prefix.clone().filter(|s| !s.is_empty()),
            from_path_prefix: self.from_path_prefix.clone().filter(|s| !s.is_empty()),
            body_kind: self.body_kind.clone().filter(|s| !s.is_empty()),
            since: self.since,
            until: self.until,
            before,
            limit: clamp_limit(self.limit),
            scan_budget: self
                .scan_budget
                .unwrap_or(DEFAULT_SCAN_BUDGET)
                .clamp(1, MAX_SCAN_BUDGET),
        })
    }

    /// The filter part of this query as a URL query string (without cursor), so
    /// the "older" link keeps the active filters.
    fn filter_query_string(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut push = |k: &str, v: &str| {
            if !v.is_empty() {
                parts.push(format!("{k}={}", urlencode(v)));
            }
        };
        push(
            "to_path_prefix",
            self.to_path_prefix.as_deref().unwrap_or(""),
        );
        push(
            "from_path_prefix",
            self.from_path_prefix.as_deref().unwrap_or(""),
        );
        push("trace_id", self.trace_id.as_deref().unwrap_or(""));
        push(
            "parent_message_id",
            self.parent_message_id.as_deref().unwrap_or(""),
        );
        push(
            "correlation_id",
            self.correlation_id.as_deref().unwrap_or(""),
        );
        push("body_kind", self.body_kind.as_deref().unwrap_or(""));
        if let Some(v) = self.since {
            parts.push(format!("since={v}"));
        }
        if let Some(v) = self.until {
            parts.push(format!("until={v}"));
        }
        if let Some(v) = self.limit {
            parts.push(format!("limit={v}"));
        }
        if let Some(v) = self.scan_budget {
            parts.push(format!("scan_budget={v}"));
        }
        parts.join("&")
    }
}

/// Percent-encode the handful of characters that can appear in a path or UUID
/// filter and would otherwise break the query string. Deliberately minimal —
/// no new dependency for a link builder.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' => out.push(c),
            _ => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).as_bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}

/// Format Unix seconds as `YYYY-MM-DD HH:MM:SS` UTC.
///
/// Plain civil-date arithmetic (days-from-epoch, Howard Hinnant's algorithm) —
/// the workspace has no date crate on its allow-list and this is display-only.
pub(crate) fn format_ts(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let (h, m, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    // days since 1970-01-01 -> civil y/m/d
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

/// Handler `GET /ui/messages`.
pub async fn get_messages_ui(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<MessagesUiQuery>,
) -> impl IntoResponse {
    let filter = match q.to_filter() {
        Ok(f) => f,
        Err(field) => {
            return crate::ui::errors::render_400(&format!("{field} is not a valid UUID"));
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
        return crate::ui::errors::render_500("colony unavailable");
    }
    let reply = match ack_rx.await {
        Ok(r) => r,
        Err(_) => return crate::ui::errors::render_500("colony unavailable"),
    };

    let older_link = reply.next.as_ref().map(|c| {
        let filters = q.filter_query_string();
        let sep = if filters.is_empty() { "" } else { "&" };
        format!(
            "/ui/messages?{filters}{sep}before_created_at={}&before_id={}",
            c.created_at,
            urlencode(&c.id)
        )
    });

    let content = html! {
        (render_filter_form(&q))
        (render_scan_notice(reply.scan_budget, reply.scan_truncated))
        h2 { "Messages (" (reply.entries.len()) ")" }
        (render_table(&reply.entries))
        @if let Some(link) = &older_link {
            p { a href=(link) { "Older →" } }
        }
    };
    (
        StatusCode::OK,
        Html(layout("Messages", content).into_string()),
    )
        .into_response()
}

fn render_filter_form(q: &MessagesUiQuery) -> Markup {
    let text = |v: &Option<String>| v.clone().unwrap_or_default();
    let num = |v: &Option<i64>| v.map(|n| n.to_string()).unwrap_or_default();
    let usz = |v: &Option<usize>| v.map(|n| n.to_string()).unwrap_or_default();
    html! {
        form method="get" action="/ui/messages" {
            label { "to_path: " input type="text" name="to_path_prefix" value=(text(&q.to_path_prefix)) size="18"; }
            label { "from_path: " input type="text" name="from_path_prefix" value=(text(&q.from_path_prefix)) size="18"; }
            label { "trace_id: " input type="text" name="trace_id" value=(text(&q.trace_id)) size="34"; }
            label { "correlation_id: " input type="text" name="correlation_id" value=(text(&q.correlation_id)) size="34"; }
            label { "body_kind: " input type="text" name="body_kind" value=(text(&q.body_kind)) size="8"; }
            label { "since: " input type="text" name="since" value=(num(&q.since)) size="12"; }
            label { "until: " input type="text" name="until" value=(num(&q.until)) size="12"; }
            label { "limit: " input type="text" name="limit" value=(usz(&q.limit)) size="6"; }
            button type="submit" { "Filter" }
        }
    }
}

fn render_table(entries: &[MessageLogDto]) -> Markup {
    if entries.is_empty() {
        return html! { p class="empty" { "No messages for this filter." } };
    }
    html! {
        table {
            thead {
                tr {
                    th { "Time (UTC)" }
                    th { "from" }
                    th { "to" }
                    th { "kind" }
                    th { "ttl" }
                    th { "Payload" }
                    th { "Detail" }
                }
            }
            tbody {
                @for e in entries {
                    tr {
                        td { (format_ts(e.created_at)) }
                        td { code { (e.from_path) } }
                        td { code { (e.to_path) } }
                        td { (e.body_kind) }
                        td { (e.ttl) }
                        td {
                            @match e.body_payload.as_deref() {
                                Some(p) => code { (truncate(p, PAYLOAD_PREVIEW_CHARS)) },
                                None => span class="empty" { "—" },
                            }
                        }
                        td { a href=(format!("/ui/message?id={}", urlencode(&e.id))) { "open" } }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_ts_renders_utc_civil_time() {
        assert_eq!(format_ts(0), "1970-01-01 00:00:00");
        // 2026-08-07 12:34:56 UTC (cross-checked against a reference impl)
        assert_eq!(format_ts(1_786_106_096), "2026-08-07 12:34:56");
        // leap-year boundary: 2024-02-29 is a real date, 2100 is not a leap year
        assert_eq!(format_ts(1_709_164_800), "2024-02-29 00:00:00");
        assert_eq!(format_ts(4_107_542_400), "2100-03-01 00:00:00");
    }

    #[test]
    fn urlencode_keeps_paths_readable_and_escapes_the_rest() {
        assert_eq!(urlencode("/mem/a"), "/mem/a");
        assert_eq!(urlencode("a b&c"), "a%20b%26c");
    }

    #[test]
    fn filter_query_string_drops_empty_fields() {
        let q = MessagesUiQuery {
            to_path_prefix: Some("/mem".into()),
            limit: Some(25),
            ..Default::default()
        };
        let s = q.filter_query_string();
        assert!(s.contains("to_path_prefix=/mem"));
        assert!(s.contains("limit=25"));
        assert!(!s.contains("trace_id"), "empty fields are omitted");
    }
}
