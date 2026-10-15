//! GH #867 -- the operator UI under `/ui/` carries no inline code a strict
//! `script-src 'self'` would block.
//!
//! The UI is server-rendered (`maud`) and promises no JS (`ui/mod.rs`). A proxy in front of
//! the listener may set `script-src 'self'` without `'unsafe-inline'` or `'unsafe-hashes'`;
//! under it an inline `<script>`, an `on*` handler or a `javascript:` link silently does
//! nothing. No lock read these pages before (re-check tv-bn M9e, B-M1): a `script { … }` in
//! `layout` would have shipped on every `/ui/*` page.
//!
//! What this file pins: every route of `build_router` -- read from `src/router.rs`, not
//! named here -- is requested through the router, against a live colony and against one
//! whose inbox is gone, and every answer that is `text/html` is read by the parser every
//! page lock shares (`meclaw_testing::page_scripts`). Those answers must come from exactly
//! the `/ui/*` routes this file knows, and between them carry the four forms a UI page
//! takes: 200, 400, 500 and 503. A new HTML route is red until it is in the list.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tower::ServiceExt;

#[allow(unused_imports)]
use crate::common; // harness_fold: mod common;

/// The routes that answer with HTML today, as the router lists them.
const UI_PAGES: &[&str] = &[
    "/ui/",
    "/ui/dead_letters",
    "/ui/graph",
    "/ui/message",
    "/ui/messages",
    "/ui/registry",
    "/ui/templates",
    "/ui/trace",
];

/// Queries a route gets besides its bare request: a valid id, so the handler reaches its
/// colony read (and, with the inbox gone, its 500 page), and an invalid one for its 400.
const QUERIES: &[(&str, &str)] = &[
    ("/ui/message", "id=01900000-0000-7000-8000-000000000000"),
    ("/ui/message", "id=not-a-uuid"),
    ("/ui/trace", "trace_id=01900000-0000-7000-8000-000000000000"),
    ("/ui/trace", "trace_id=not-a-uuid"),
];

/// Every `.route("<path>", <handlers>)` of `src/router.rs`, with whether it has a GET.
fn router_routes() -> Vec<(String, bool)> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/router.rs");
    let src = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let mut out = Vec::new();
    let mut rest = src.as_str();
    while let Some(at) = rest.find(".route(") {
        let call = &rest[at + ".route(".len()..];
        // The call up to its closing paren.
        let mut depth = 1usize;
        let mut end = call.len();
        for (i, c) in call.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let args = &call[..end];
        let path = args
            .trim_start()
            .strip_prefix('"')
            .and_then(|a| a.split('"').next())
            .unwrap_or_else(|| panic!("a route whose path this reader cannot read: {args}"));
        out.push((path.to_string(), args.contains("get(")));
        rest = &call[end..];
    }
    assert!(
        out.len() >= 20,
        "src/router.rs lists {} routes -- not the router this was written for",
        out.len()
    );
    out
}

fn app(inbox: tokio::sync::mpsc::Sender<meclaw_colony::ColonyMsg>) -> (Router, tempfile::TempDir) {
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox,
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, td) = common::test_blob_store();
    let app = meclaw_api::router::build_router(
        api_colony,
        blob_store,
        meclaw_core::MESSAGE_DEFAULT_TTL,
        Arc::new(meclaw_colony::SurfaceRegistry::new()),
    );
    (app, td)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_html_answer_of_the_api_router_carries_no_inline_code() {
    let routes = router_routes();
    for (route, _) in QUERIES {
        assert!(
            routes.iter().any(|(r, _)| r == route),
            "a query for {route}, which the router no longer has"
        );
    }

    let live = meclaw_testing::ColonyHandle::new();
    let (live_app, _live_td) = app(live.inbox_tx.clone());
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    drop(rx);
    let (dead_app, _dead_td) = app(tx);

    let mut html_routes = BTreeSet::new();
    let mut statuses: BTreeMap<u16, String> = BTreeMap::new();
    for (route, has_get) in &routes {
        if !has_get {
            continue;
        }
        let path = route.replace(['{', '}'], "");
        let mut uris = vec![path.clone()];
        uris.extend(
            QUERIES
                .iter()
                .filter(|(r, _)| r == route)
                .map(|(_, q)| format!("{path}?{q}")),
        );
        for uri in uris {
            for (colony, app) in [("live", &live_app), ("no inbox", &dead_app)] {
                let resp = tokio::time::timeout(
                    meclaw_testing::deadline::HANG,
                    app.clone()
                        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap()),
                )
                .await
                .unwrap_or_else(|_| panic!("GET {uri} ({colony}) never answered"))
                .expect("the router answers");
                let status = resp.status().as_u16();
                let html = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|ct| ct.to_ascii_lowercase().starts_with("text/html"));
                // The type first: a streaming answer that is no page is never read to its
                // end, and a page's body is read under the hang mark (re-check tv-bn M9f, H2).
                if !html {
                    continue;
                }
                let bytes = tokio::time::timeout(
                    meclaw_testing::deadline::HANG,
                    to_bytes(resp.into_body(), 16 << 20),
                )
                .await
                .unwrap_or_else(|_| panic!("HANG: GET {uri} ({colony}) never ended its page"))
                .expect("a body");
                let page = String::from_utf8(bytes.to_vec()).expect("an utf-8 page");
                let label = format!("GET {uri} ({colony}, {status})");
                meclaw_testing::page_scripts::assert_no_inline_code(&label, &page);
                html_routes.insert(route.clone());
                statuses.entry(status).or_insert(label);
            }
        }
    }

    let want: BTreeSet<String> = UI_PAGES.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        html_routes, want,
        "the routes that answer with HTML are another set than the one this lock was written \
         for; a new page is read above already -- add its route to UI_PAGES"
    );
    for status in [200, 400, 500, 503] {
        assert!(
            statuses.contains_key(&status),
            "no UI page answered {status}, so its form went unread: {statuses:#?}"
        );
    }
}
