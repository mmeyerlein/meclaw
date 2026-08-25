//! GH #383 — `--api` is back to what it served before GH #159.
//!
//! The claim is a removal, so it is asserted from both sides: the paths that
//! must still be there are there, and the prefix that was added for surfaces
//! answers nothing at all any more.
//!
//! # Why the probe is a wrong-method request
//!
//! axum 0.7 exposes no route table to read, so "which routes exist" has to be
//! asked over HTTP. A wrong-method request is the one question that separates
//! the two answers without running a handler: a **known** path with an
//! unhandled method is `405 Method Not Allowed`, an **unknown** path falls
//! through to the router's 404. So `PATCH /colony/graph` proves the route is
//! mounted without sending anything into a colony — which matters here, because
//! this test builds a router over an inbox nobody is reading and a handler that
//! actually ran would wait for an ack that never comes.
//!
//! One route is additionally driven for real: `GET /health` answers 200 out of
//! the HTTP layer itself. Without it the whole table could be "mounted" by a
//! router that is wired to nothing, and every 405 above would be true and
//! worthless.
//!
//! # What "equals the pre-surface set" means here
//!
//! Every path below is named explicitly, so a route that disappears turns this
//! test red. The other half — that nothing EXTRA is mounted — is asserted where
//! it is answerable: the `/surface/*` prefix and its three shapes (page, asset,
//! socket), plus one unknown path under each surviving prefix, so a wildcard
//! quietly re-appearing under `/ui` or `/colony` cannot pass either.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// The routes `--api` served before GH #159, read off `build_router` itself
/// rather than off the plan: `/health`, the `/colony/*` reads plus the two
/// writers, `POST /messages`, the `/` redirect and the `/ui/*` operator pages.
///
/// `/colony/messages` (P1) and `/colony/ledger` (GH #267) were added after
/// #159 and are on this list because the list is the router's, not history's —
/// what #383 removes is `/surface/*` and nothing else.
const EXPECTED_ROUTES: &[&str] = &[
    "/health",
    "/colony/registry",
    "/colony/dead_letters",
    "/colony/templates",
    "/colony/templates/rescan",
    "/colony/events",
    "/colony/trace",
    "/colony/ledger",
    "/colony/messages",
    "/colony/graph",
    "/colony/mutations",
    "/messages",
    "/",
    "/ui/",
    "/ui/registry",
    "/ui/graph",
    "/ui/dead_letters",
    "/ui/messages",
    "/ui/message",
    "/ui/trace",
    "/ui/templates",
];

/// Paths that must fall through to the router's 404.
///
/// The four surface shapes first — page, own asset, vendored bundle, socket —
/// because each was a separate arm of the retired handler and a partial removal
/// would leave one of them answering. Then one unknown path under each
/// surviving prefix, so a wildcard mounted by accident is caught too.
const EXPECTED_MISSES: &[&str] = &[
    "/surface",
    "/surface/",
    "/surface/x",
    "/surface/org/acme/member/alice/canvy",
    "/surface/x/@asset/style.css",
    "/surface/@client/phoenix.min.js",
    "/surface/x/live/websocket",
    "/ui/no_such_page",
    "/colony/no_such_endpoint",
    "/no_such_root",
];

/// A router over a colony nobody is listening to. No handler below is allowed
/// to run, so nothing here needs a live colony task — see the module note.
fn router() -> (axum::Router, tempfile::TempDir) {
    let (inbox, _inbox_rx) = tokio::sync::mpsc::channel(8);
    let colony = Arc::new(meclaw_api::ColonyHandle {
        inbox,
        templates_root: std::path::PathBuf::new(),
    });
    let (blob_store, td) = common::test_blob_store();
    let app =
        meclaw_api::router::build_router(colony, blob_store, meclaw_core::MESSAGE_DEFAULT_TTL);
    (app, td)
}

async fn status(app: &axum::Router, method: &str, path: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .expect("build the request");
    app.clone()
        .oneshot(req)
        .await
        .expect("the router answers")
        .status()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_surface_prefix_answers_nothing() {
    let (app, _td) = router();
    for path in EXPECTED_MISSES {
        for method in ["GET", "PATCH"] {
            assert_eq!(
                status(&app, method, path).await,
                StatusCode::NOT_FOUND,
                "{method} {path} must fall through to the router's 404 — a 405 here \
                 would mean the path is still mounted"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_route_table_is_the_pre_surface_set() {
    let (app, _td) = router();

    // The positive receipt: one route answered for real, so the 405s below are
    // a mounted table rather than a router wired to nothing.
    assert_eq!(
        status(&app, "GET", "/health").await,
        StatusCode::OK,
        "GET /health is the HTTP layer's own answer and must be 200"
    );

    for path in EXPECTED_ROUTES {
        assert_eq!(
            status(&app, "PATCH", path).await,
            StatusCode::METHOD_NOT_ALLOWED,
            "PATCH {path} must be 405 (route mounted, method unhandled); a 404 \
             means the route is gone"
        );
    }
}
