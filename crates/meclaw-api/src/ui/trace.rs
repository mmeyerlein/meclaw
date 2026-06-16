//! GET /ui/trace — Phase 12-D T25.
//!
//! Trace-Tree-Renderer + Search-Form. Wrappt `ColonyMsg::ReadTrace`
//! analog zum 12-B-JSON-Handler. Aus der flachen Liste an
//! `MessageLogDto`-Einträgen wird via `parent_message_id`-Map ein
//! verschachtelter `<ul class="tree">/<li>`-Baum erzeugt — Root-Hops
//! sind solche mit `parent_message_id is None`.
//!
//! Ohne `trace_id`-Query rendert die Seite nur die Search-Form +
//! einen Hinweis-Text. T27 ergänzt 400-HTML bei invalid UUID
//! (statt 422), zentralisiert in `crate::ui::errors`.

use crate::ColonyHandle;
use crate::handlers::clamp_limit;
use crate::ui::layout;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use maud::{Markup, html};
use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::MessageLogDto;
use meclaw_core::Uuid;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Query-Params fuer `/ui/trace`.
#[derive(Debug, Deserialize, Default)]
pub struct TraceUiQuery {
    /// Trace-ID als UUID-String. Pflicht für Tree-Render; ohne diesen Wert
    /// rendert die Seite nur die Search-Form.
    pub trace_id: Option<String>,
    /// Hard-Cap (Default 100, Max 1000).
    pub limit: Option<usize>,
}

/// Handler `GET /ui/trace`.
pub async fn get_trace_ui(
    State(colony): State<Arc<ColonyHandle>>,
    Query(q): Query<TraceUiQuery>,
) -> impl IntoResponse {
    let trace_id_input = q.trace_id.clone().unwrap_or_default();

    // Ohne trace_id: nur Form + Hinweis.
    let Some(trace_id_str) = q.trace_id.as_deref() else {
        let content = html! {
            (render_search_form(&trace_id_input))
            p { "Trace-ID eingeben, um den Hop-Baum zu sehen." }
        };
        return (StatusCode::OK, Html(layout("Trace", content).into_string())).into_response();
    };

    // UUID-Validierung — bei Fail 400 (kein 422 bei Reads, T27).
    let trace_id = match Uuid::parse_str(trace_id_str) {
        Ok(u) => u,
        Err(_) => {
            return crate::ui::errors::render_400("trace_id is not a valid UUID");
        }
    };

    let (ack_tx, ack_rx) = oneshot::channel();
    let msg = ColonyMsg::ReadTrace {
        trace_id: Some(trace_id),
        path_prefix: None,
        correlation_id: None,
        only_error: false,
        since: None,
        limit: clamp_limit(q.limit),
        ack: ack_tx,
    };
    if colony.inbox.send(msg).await.is_err() {
        return crate::ui::errors::render_500("colony unavailable");
    }
    let reply = match ack_rx.await {
        Ok(r) => r,
        Err(_) => return crate::ui::errors::render_500("colony unavailable"),
    };

    let content = html! {
        (render_search_form(&trace_id_input))
        h2 { "Trace " code { (trace_id_str) } " (" (reply.entries.len()) " hops)" }
        @if reply.entries.is_empty() {
            p class="empty" { "Kein Trace gefunden für " code { (trace_id_str) } "." }
        } @else {
            (render_trace_tree(&reply.entries))
        }
    };
    (StatusCode::OK, Html(layout("Trace", content).into_string())).into_response()
}

fn render_search_form(trace_id: &str) -> Markup {
    html! {
        form method="get" action="/ui/trace" {
            label { "Trace-ID: " input type="text" name="trace_id" value=(trace_id) size="40"; }
            button type="submit" { "Suchen" }
        }
    }
}

/// Baut aus der flachen Liste eine Children-Map (parent_message_id → Kinder)
/// und rendert rekursiv. Root-Hops haben `parent_message_id is None`.
///
/// Bei Cycles (sollte nie passieren) wird per `visited`-Set abgebrochen,
/// damit der Renderer nicht in eine Endlos-Schleife läuft.
pub(crate) fn render_trace_tree(entries: &[MessageLogDto]) -> Markup {
    let mut children: HashMap<Option<String>, Vec<&MessageLogDto>> = HashMap::new();
    for e in entries {
        children
            .entry(e.parent_message_id.clone())
            .or_default()
            .push(e);
    }
    let roots: Vec<&MessageLogDto> = children.get(&None).cloned().unwrap_or_default();

    // Falls keine echten Roots vorhanden sind (z.B. Filter schneidet Wurzel
    // weg), fallback: alle Einträge, deren parent NICHT im Set existiert,
    // gelten als orphaned Roots.
    let roots = if roots.is_empty() {
        let ids: std::collections::HashSet<String> = entries.iter().map(|e| e.id.clone()).collect();
        entries
            .iter()
            .filter(|e| {
                e.parent_message_id
                    .as_ref()
                    .map(|p| !ids.contains(p))
                    .unwrap_or(true)
            })
            .collect()
    } else {
        roots
    };

    html! {
        ul class="tree" {
            @for root in &roots {
                (render_node(root, &children, &mut std::collections::HashSet::new()))
            }
        }
    }
}

fn render_node(
    node: &MessageLogDto,
    children: &HashMap<Option<String>, Vec<&MessageLogDto>>,
    visited: &mut std::collections::HashSet<String>,
) -> Markup {
    if !visited.insert(node.id.clone()) {
        return html! { li class="error" { "(cycle in trace, abort)" } };
    }
    let kids = children
        .get(&Some(node.id.clone()))
        .cloned()
        .unwrap_or_default();
    let body_preview = node
        .body_payload
        .as_deref()
        .map(|s| truncate(s, 120))
        .unwrap_or_default();
    let error_code = extract_error_code(&node.headers_json);
    html! {
        li {
            code { (node.to_path) }
            " ← "
            code { (node.from_path) }
            " · ttl=" (node.ttl)
            @if let Some(code) = &error_code {
                " · " span class="error" { "error=" (code) }
            }
            @if !body_preview.is_empty() {
                " · body=" code { (body_preview) }
            }
            @if !kids.is_empty() {
                ul {
                    @for kid in &kids {
                        (render_node(kid, children, visited))
                    }
                }
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Best-effort `error_code` extraction from the raw `headers_json` string.
/// Wenn ungültig oder fehlend: None.
fn extract_error_code(headers_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(headers_json).ok()?;
    v.get("error_code")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_entry(id: &str, parent: Option<&str>, to: &str) -> MessageLogDto {
        MessageLogDto {
            id: id.into(),
            trace_id: "trace-1".into(),
            parent_message_id: parent.map(|s| s.into()),
            correlation_id: None,
            ttl: 32,
            from_path: "/a".into(),
            to_path: to.into(),
            reply_to: None,
            headers_json: "{}".into(),
            body_kind: "inline".into(),
            body_payload: None,
            created_at: 0,
        }
    }

    #[test]
    fn tree_renders_nested_ul_for_two_levels() {
        let entries = vec![
            mk_entry("root", None, "/r"),
            mk_entry("child", Some("root"), "/r/child"),
        ];
        let s = render_trace_tree(&entries).into_string();
        assert!(s.contains("class=\"tree\""));
        // Outer + inner ul: outer is class="tree", inner is plain.
        assert!(s.matches("<ul").count() >= 2);
        assert!(s.contains("/r/child"));
    }

    #[test]
    fn tree_handles_orphan_as_root() {
        // child references unknown parent → child wird selbst zur Root.
        let entries = vec![mk_entry("orphan", Some("ghost"), "/o")];
        let s = render_trace_tree(&entries).into_string();
        assert!(s.contains("/o"));
    }
}
