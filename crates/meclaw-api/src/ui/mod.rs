//! Phase 12-D: Server-rendered Operator-UI (`maud`-HTML).
//!
//! Symmetrie zur Phase-12-B-JSON-Schicht: dieselben `ColonyMsg::Read*`-
//! Inbox-Variants, andere Repräsentation. Keine JS, kein Auto-Refresh,
//! keine Mutation-Forms, keine WebSocket. Read-only Operator-Surface.
//!
//! Geteilte Helfer (`style_block`, `layout`) leben hier, damit Dashboard
//! und Entity-Pages konsistent aussehen.

pub mod dashboard;
pub mod dead_letters;
pub mod errors;
pub mod graph;
pub mod registry;
pub mod templates;
pub mod trace;

use maud::{DOCTYPE, Markup, html};

/// Inline `<style>`-Block — ~30 Zeilen, plain CSS, kein Framework.
///
/// Gilt für alle `/ui/*`-Seiten gleich. Pragmatisch: lesbar, funktional,
/// keine Glossy-Layouts. Operator-UI, kein Marketing.
pub(crate) fn style_block() -> Markup {
    html! {
        style {
            r#"
            body { font-family: system-ui, sans-serif; max-width: 1100px; margin: 1em auto; padding: 0 1em; color: #222; }
            h1, h2 { color: #1a4f8c; }
            nav { background: #f0f4f8; padding: 0.5em 1em; margin-bottom: 1em; }
            nav a { margin-right: 1em; color: #1a4f8c; text-decoration: none; }
            nav a:hover { text-decoration: underline; }
            table { border-collapse: collapse; width: 100%; }
            th, td { border: 1px solid #ccc; padding: 0.4em 0.6em; text-align: left; vertical-align: top; }
            th { background: #f4f7fa; }
            tr:nth-child(even) { background: #f9fafc; }
            .empty { color: #888; font-style: italic; }
            .error { color: #b00; }
            footer { color: #888; font-size: 0.9em; margin-top: 2em; border-top: 1px solid #ddd; padding-top: 0.5em; }
            form { margin: 0.5em 0; }
            input, select { padding: 0.3em 0.5em; margin-right: 0.5em; }
            ul.tree { list-style-type: none; padding-left: 1em; border-left: 1px dashed #ccc; }
            code { background: #f4f7fa; padding: 0.1em 0.3em; border-radius: 2px; font-size: 0.9em; }
            "#
        }
    }
}

/// Geteiltes Page-Layout: Doctype + Head + Nav + Main + Footer.
///
/// `title` rendert in `<title>` (mit `" — meclaw"`-Suffix) und als `<h1>`.
/// `content` ist der Seiten-Body unter dem `<h1>`.
pub(crate) fn layout(title: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html {
            head {
                meta charset="utf-8";
                title { (title) " — meclaw" }
                (style_block())
            }
            body {
                nav {
                    a href="/ui/" { "Dashboard" }
                    a href="/ui/registry" { "Registry" }
                    a href="/ui/graph" { "Graph" }
                    a href="/ui/dead_letters" { "Dead Letters" }
                    a href="/ui/trace" { "Trace" }
                    a href="/ui/templates" { "Templates" }
                }
                main {
                    h1 { (title) }
                    (content)
                }
                footer {
                    "meclaw Operator-UI · server-rendered, kein JS, kein Auto-Refresh"
                }
            }
        }
    }
}
