//! GH #159 — what a browser can reach over HTTP, and what it must not.
//!
//! Driven through the real router with `tower::ServiceExt::oneshot`, so the route
//! table, the parser and the locator are all in the path. No colony is needed for
//! any case here, which is itself the receipt for the page: a dead render costs no
//! cell call and touches no database.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use meclaw_api::router::{SurfaceState, build_router};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

/// A colony tree with one declared surface (with assets), its private store, and
/// an unrelated store holding "real" data.
fn tree() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().unwrap();
    write_cell(
        td.path(),
        "org/acme/canvy/render",
        r#"{
          "cell": {
            "type": "code",
            "surface": {
              "title": "Acme topology",
              "assets": "client",
              "boot_hint": "reading the colony"
            }
          },
          "params": { "runner": "python3", "script_inline": "pass" }
        }"#,
    );
    let client = td.path().join("main/org/acme/canvy/render/client");
    fs::create_dir_all(&client).unwrap();
    fs::write(client.join("surface.js"), b"window.SurfaceHooks = {};\n").unwrap();
    fs::write(client.join("surface.css"), b":root { --bg: #fff; }\n").unwrap();
    fs::write(client.join("notes.txt"), b"not an asset type\n").unwrap();

    write_cell(
        td.path(),
        "org/acme/canvy/store",
        r#"{ "cell": { "type": "store" },
             "params": { "schema": { "canvas": { "kind": "text" } } } }"#,
    );
    write_cell(
        td.path(),
        "org/acme/vault",
        r#"{ "cell": { "type": "store" },
             "params": { "schema": { "secrets": { "id": "text" } } } }"#,
    );
    write_cell(
        td.path(),
        "org/acme/broken/render",
        r#"{ "cell": { "type": "code", "surface": { "assets": "../etc" } },
             "params": { "runner": "python3", "script_inline": "pass" } }"#,
    );
    td
}

fn write_cell(root: &Path, rel: &str, config: &str) {
    let dir = root.join("main").join(rel);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.json"), config).unwrap();
}

/// A router whose surface state points at `root` and whose colony is dead.
///
/// The dead colony is deliberate: everything asserted below must work without one.
fn app(root: &Path) -> axum::Router {
    let (colony_tx, colony_rx) = tokio::sync::mpsc::channel(1);
    let (_egress_tx, egress_rx) = tokio::sync::mpsc::channel(1);
    let (dispatcher, _join) = meclaw_api::surface::render::Dispatcher::new(
        colony_tx,
        egress_rx,
        meclaw_core::MESSAGE_DEFAULT_TTL,
    );
    drop(colony_rx);
    let api_colony = Arc::new(meclaw_api::ColonyHandle {
        inbox: tokio::sync::mpsc::channel(1).0,
        templates_root: root.join("templates"),
    });
    let (blob_store, blob_td) = common::test_blob_store();
    // The blob TempDir must outlive the router; leaking it in a test is cheaper
    // than threading it through every call site.
    std::mem::forget(blob_td);
    build_router(
        api_colony,
        blob_store,
        meclaw_core::MESSAGE_DEFAULT_TTL,
        SurfaceState {
            colony_root: Arc::new(root.to_path_buf()),
            dispatcher,
        },
    )
}

async fn get(root: &Path, path: &str) -> (StatusCode, String) {
    let res = app(root)
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_page_is_served_under_the_cell_path() {
    let td = tree();
    let (status, body) = get(td.path(), "/surface/org/acme/canvy/render").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<title>Acme topology</title>"), "{body}");
    assert!(
        body.contains("reading the colony"),
        "the boot hint is shown"
    );
    assert!(
        body.contains("data-phx-main"),
        "LiveView needs the container"
    );
    assert!(body.contains("data-phx-session=\""), "and a session token");
    assert!(body.contains("csrf-token"), "and a csrf meta tag");
}

/// **The reason the declaration is opt-in.** An undeclared cell holds real data,
/// and 404 rather than 403: a surface nobody declared must not confirm it exists.
/// The surface's OWN store is in this list — the renderer is addressable, the data
/// behind it is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeclared_cell_is_404_and_not_403() {
    let td = tree();
    for path in [
        "/surface/org/acme/vault",
        "/surface/org/acme/canvy/store",
        "/surface/org/acme/nowhere",
    ] {
        let (status, _) = get(td.path(), path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn traversal_is_a_miss_in_every_spelling() {
    let td = tree();
    for path in [
        "/surface/../colony.json",
        "/surface/..",
        "/surface/org/../../colony.json",
        "/surface/",
        "/surface/org//acme",
        "/surface/org/./acme/canvy/render",
        "/surface/org/acme/canvy/render/",
    ] {
        let (status, _) = get(td.path(), path).await;
        assert_ne!(status, StatusCode::OK, "{path} must not be served");
    }
}

/// The operator's own typo is the one error class that is NOT hidden behind a 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_broken_declaration_names_the_mistake() {
    let td = tree();
    let (status, body) = get(td.path(), "/surface/org/acme/broken/render").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("assets"), "the mistake must be named: {body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_bundles_are_served_from_the_binary() {
    let td = tree();
    for (file, needle) in [
        ("phoenix.min.js", "Socket"),
        ("phoenix_live_view.min.js", "LiveSocket"),
    ] {
        let (status, body) = get(td.path(), &format!("/surface/@client/{file}")).await;
        assert_eq!(status, StatusCode::OK, "{file}");
        assert!(body.contains(needle), "{file}");
    }
    let (status, _) = get(td.path(), "/surface/@client/nope.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_surfaces_own_files_come_from_its_own_directory() {
    let td = tree();
    let (status, body) = get(
        td.path(),
        "/surface/org/acme/canvy/render/@asset/surface.js",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("SurfaceHooks"), "{body}");
}

/// Everything in the cell directory that is not a declared asset stays private,
/// and an extension nobody declared support for is a miss rather than a download.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_declared_assets_of_known_types_are_served() {
    let td = tree();
    for file in [
        "notes.txt",   // present, unknown extension
        "missing.js",  // absent
        "config.json", // outside the asset directory
        "cell.db",     // outside, and would be state
    ] {
        let (status, _) = get(
            td.path(),
            &format!("/surface/org/acme/canvy/render/@asset/{file}"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{file} must not be served");
    }
}

/// The realistic attack: a cell directory is writable by cells in the same colony,
/// so a symlink out of the asset directory is what a string check cannot catch.
/// Canonicalisation is what catches it.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_symlink_out_of_the_asset_directory_is_refused() {
    let td = tree();
    fs::write(td.path().join("colony.json"), b"{}").unwrap();
    let link = td
        .path()
        .join("main/org/acme/canvy/render/client/escape.json");
    std::os::unix::fs::symlink(td.path().join("colony.json"), &link).unwrap();
    let (status, _) = get(
        td.path(),
        "/surface/org/acme/canvy/render/@asset/escape.json",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a symlink out of the asset directory must be refused"
    );
}

/// The socket URL in the page must sit under the surface's own prefix. If this ever
/// regresses to a colony-global /live/websocket, prefix authorisation breaks and
/// nobody notices until an nginx rule silently stops covering the transport.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_page_points_the_socket_at_its_own_prefix() {
    let td = tree();
    let (_, body) = get(td.path(), "/surface/org/acme/canvy/render").await;
    assert!(
        body.contains("\"/surface/org/acme/canvy/render/live\""),
        "the socket must hang under this surface's prefix: {body}"
    );
    assert!(
        !body.contains("\"/live"),
        "a colony-global socket URL breaks prefix authorisation"
    );
}

/// **The receipt that the page path never touches the colony.** Fifty concurrent
/// loads through a router whose colony channel nobody drains, all 200. A wedged
/// colony still serves a page that visibly fails to connect, which is a state a
/// person can read instead of a blank screen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fifty_concurrent_page_loads_do_not_need_a_colony() {
    let td = tree();
    let root = td.path().to_path_buf();
    let mut handles = Vec::new();
    for _ in 0..50 {
        let root = root.clone();
        handles.push(tokio::spawn(async move {
            get(&root, "/surface/org/acme/canvy/render").await.0
        }));
    }
    for h in handles {
        assert_eq!(h.await.unwrap(), StatusCode::OK);
    }
}

/// The socket path over an ordinary GET: the path is right, the request is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_socket_path_without_an_upgrade_is_a_bad_request() {
    let td = tree();
    let (status, _) = get(td.path(), "/surface/org/acme/canvy/render/live/websocket").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// An undeclared cell has no transport either. The 404 rule holds on every route.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeclared_cell_has_no_transport() {
    let td = tree();
    let (status, _) = get(td.path(), "/surface/org/acme/vault/live/websocket").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A surface that declares no asset directory gets a working page with no asset
/// tags — never a tag pointing at a 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_surface_without_assets_omits_the_asset_tags() {
    let td = tree();
    write_cell(
        td.path(),
        "org/acme/bare",
        r#"{ "cell": { "type": "code", "surface": { "title": "Bare" } },
             "params": { "runner": "python3", "script_inline": "pass" } }"#,
    );
    let (status, body) = get(td.path(), "/surface/org/acme/bare").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("@asset"), "no asset tags: {body}");
    assert!(body.contains("phoenix_live_view.min.js"), "but the bundles");
}
