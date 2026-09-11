//! GH #644: `GET /colony/surfaces` publishes the mount table, and `?format=traefik`
//! renders it as the document a proxy polls.
//!
//! The endpoint is served over a real socket rather than driven with `oneshot`,
//! because half of what it answers is derived from the request's own `Host`
//! header: a poller reaches this colony on the address it can reach, and that
//! address is the one the Traefik service has to name.

use meclaw_api::router::build_router;
use meclaw_colony::{SurfaceEntry, SurfaceRegistry};
use meclaw_core::Path;
use serde_json::Value;
use std::sync::Arc;

mod common;

/// Bind a throwaway port, serve `router` on it, and return where it listens.
async fn serve(router: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a throwaway port");
    let addr = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    addr
}

/// A colony handle whose inbox nobody drains: this endpoint reads the registry,
/// so no message is ever sent. The receiver travels back so the caller keeps it
/// alive — a dropped one would close the inbox, which is a different test.
fn colony_stub() -> (
    Arc<meclaw_api::ColonyHandle>,
    tokio::sync::mpsc::Receiver<meclaw_colony::ColonyMsg>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    (
        Arc::new(meclaw_api::ColonyHandle {
            inbox: tx,
            templates_root: std::path::PathBuf::new(),
        }),
        rx,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mount_table_and_its_traefik_document() {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let _held = surfaces
        .register(
            "voice",
            SurfaceEntry {
                kind: "voice",
                cell_path: Path::new("/m/voice"),
                links: None,
            },
        )
        .await
        .expect("the mount is free");
    surfaces.set_listener("0.0.0.0:7777".parse().expect("a listener address"));

    let (blob_store, _blob_td) = common::test_blob_store();
    let (colony, _inbox) = colony_stub();
    let router = build_router(
        colony,
        blob_store,
        meclaw_core::MESSAGE_DEFAULT_TTL,
        Arc::clone(&surfaces),
    );
    let addr = serve(router).await;

    let plain: Value = reqwest::get(format!("http://{addr}/colony/surfaces"))
        .await
        .expect("the mount table answers")
        .json()
        .await
        .expect("it answers JSON");
    assert_eq!(plain["listener"], "0.0.0.0:7777");
    assert_eq!(plain["surfaces"][0]["mount"], "voice");
    assert_eq!(plain["surfaces"][0]["kind"], "voice");
    assert!(
        plain["surfaces"][0].get("own_addr").is_none(),
        "no surface cell owns a port any more, so a row names no second address"
    );
    assert!(
        plain["surfaces"][0].get("cell_path").is_none(),
        "the tree path never leaves the colony"
    );

    let t: Value = reqwest::get(format!("http://{addr}/colony/surfaces?format=traefik"))
        .await
        .expect("the traefik document answers")
        .json()
        .await
        .expect("it answers JSON");
    assert_eq!(
        t["http"]["routers"]["meclaw-voice"]["rule"],
        "PathPrefix(`/voice`)"
    );
    assert_eq!(t["http"]["routers"]["meclaw-voice"]["service"], "meclaw");
    assert_eq!(
        t["http"]["services"]["meclaw"]["loadBalancer"]["servers"][0]["url"],
        format!("http://{addr}"),
        "the Host header is the reachable address"
    );

    let bad = reqwest::get(format!("http://{addr}/colony/surfaces?format=nginx"))
        .await
        .expect("an unknown format answers");
    assert_eq!(bad.status(), 400);
    let refusal: Value = bad.json().await.expect("the refusal is JSON");
    assert_eq!(refusal["error"], "bad_query");
    assert_eq!(refusal["detail"], "format must be traefik");
}
