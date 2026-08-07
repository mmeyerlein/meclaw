//! Shared maud render helpers for the P1 message browser (list + detail).
//!
//! `truncate` is deliberately a local copy of the private helper in `ui::trace` —
//! refactoring a working module is out of scope for this package (AGENTS.md
//! rule 8).

use maud::{Markup, html};

/// Cut `s` to `max` characters, appending `…` when it was shortened.
/// Character-based, not byte-based: never splits a UTF-8 sequence.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Pretty-print a raw JSON string; `None` when it does not parse.
pub(crate) fn pretty_json(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    serde_json::to_string_pretty(&v).ok()
}

/// Render the two header compartments as separate, labelled tables.
///
/// `message_log.headers` holds a serialized `meclaw_core::Headers`, i.e.
/// `{"context": {...}, "hop": {...}}`. The split is the point: `context` is
/// persistent and only edges write it, `hop` is exactly one hop old and decays
/// on the next cell emission. Debugging a message means knowing which is which.
///
/// Broken or unexpected JSON falls back to the raw string in a `<pre>` block —
/// an operator surface must never hide what is actually stored.
pub(crate) fn render_headers_split(headers_json: &str) -> Markup {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(headers_json) else {
        return html! {
            p class="error" { "Headers sind kein gültiges JSON — Rohwert:" }
            pre { (headers_json) }
        };
    };
    let (Some(context), Some(hop)) = (value.get("context"), value.get("hop")) else {
        // Either compartment missing: still render what is there, then the raw
        // value, rather than pretending the model matched.
        return html! {
            @if let Some(ctx) = value.get("context") {
                h3 { "context" }
                (render_kv_table(ctx))
                h3 { "hop" }
                p class="empty" { "(leer)" }
            } @else if let Some(h) = value.get("hop") {
                h3 { "context" }
                p class="empty" { "(leer)" }
                h3 { "hop" }
                (render_kv_table(h))
            } @else {
                p class="error" { "Headers ohne context/hop-Fächer — Rohwert:" }
                pre { (headers_json) }
            }
        };
    };
    html! {
        h3 { "context" }
        p class="empty" { "persistent — nur Edges schreiben dieses Fach" }
        (render_kv_table(context))
        h3 { "hop" }
        p class="empty" { "genau ein Hop — verfällt bei der nächsten Cell-Emission" }
        (render_kv_table(hop))
    }
}

/// Render a JSON object as a two-column key/value table; scalar values inline,
/// nested values pretty-printed.
fn render_kv_table(value: &serde_json::Value) -> Markup {
    let Some(map) = value.as_object() else {
        return html! { pre { (value.to_string()) } };
    };
    if map.is_empty() {
        return html! { p class="empty" { "(leer)" } };
    }
    html! {
        table {
            thead { tr { th { "Key" } th { "Value" } } }
            tbody {
                @for (k, v) in map {
                    tr {
                        td { code { (k) } }
                        td {
                            @match v.as_str() {
                                Some(s) => code { (s) },
                                None => pre {
                                    (serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()))
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Scan-budget disclosure. Empty markup when the scan covered everything,
/// otherwise an unconditional, visible notice.
///
/// `message_log` has no index on `from_path`, `body_kind` or `correlation_id`;
/// those filters run inside a bounded window. Staying silent about an exhausted
/// window would make "found nothing" indistinguishable from "did not look far
/// enough".
pub(crate) fn render_scan_notice(scanned: usize, truncated: bool) -> Markup {
    html! {
        @if truncated {
            p class="error" {
                (scanned) " Rows gescannt, evtl. unvollständig — nicht-indizierte "
                "Filter (from_path, body_kind, correlation_id) wirkten nur auf "
                "dieses Fenster. Zeitraum eingrenzen oder scan_budget erhöhen."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_appends_ellipsis_beyond_limit() {
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(truncate("ab", 3), "ab");
    }

    #[test]
    fn pretty_json_indents_valid_json_and_gives_up_on_garbage() {
        let out = pretty_json(r#"{"a":1}"#).expect("valid json");
        assert!(out.contains('\n'), "pretty output is multi-line");
        assert!(pretty_json("nope{").is_none());
    }

    /// The two-compartment header model is exactly what one debugs — `context`
    /// (persistent, edge-written) and `hop` (one hop, decays) must be visibly
    /// separate, and each key must sit in its own compartment.
    #[test]
    fn headers_split_renders_hop_and_context_separately() {
        let raw = r#"{"context":{"turn_id":"t1"},"hop":{"finish_reason":"stop"}}"#;
        let s = render_headers_split(raw).into_string();
        let ctx_at = s.find("context").expect("context section");
        let hop_at = s.find("hop").expect("hop section");
        assert!(ctx_at < hop_at, "context section renders before hop");
        assert!(s.contains("turn_id") && s.contains("finish_reason"));
        // Assignment must be right, not merely "both strings occur somewhere".
        let ctx_block = &s[ctx_at..hop_at];
        assert!(
            ctx_block.contains("turn_id"),
            "turn_id belongs to the context compartment"
        );
        assert!(
            !ctx_block.contains("finish_reason"),
            "finish_reason must not leak into the context compartment"
        );
    }

    #[test]
    fn headers_split_marks_an_absent_compartment_as_empty() {
        let s = render_headers_split(r#"{"context":{"turn_id":"t1"}}"#).into_string();
        assert!(s.contains("turn_id"));
        assert!(s.contains("(leer)"), "missing hop compartment is labelled");
    }

    #[test]
    fn headers_split_falls_back_to_raw_on_broken_json() {
        let s = render_headers_split("not json").into_string();
        assert!(
            s.contains("not json"),
            "raw fallback keeps the value visible"
        );
        assert!(s.contains("<pre"), "raw fallback renders as pre-block");
    }

    /// Plan § F3 ruling 3: truncation is disclosed whenever it happened — also
    /// when the result list is empty.
    #[test]
    fn scan_notice_is_shown_whenever_the_budget_was_exhausted() {
        let shown = render_scan_notice(5000, true).into_string();
        assert!(shown.contains("5000"), "scanned row count must be visible");
        assert!(
            shown.contains("evtl. unvollständig"),
            "honest truncation wording"
        );
        let quiet = render_scan_notice(12, false).into_string();
        assert!(
            quiet.is_empty(),
            "no notice when the whole window was covered"
        );
    }
}
