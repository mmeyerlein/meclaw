//! GH #660: a mount another `web` cell already holds registers nothing, and the
//! half that was refused keeps running.
//!
//! The sentence under test is the one `docs/cell-types` § `web` makes about a
//! name collision: the half reports it and keeps running, because a name two
//! cells declare is an operator's mistake to read in the journal and not a
//! reason to tear a cell — and possibly a whole colony boot — down. `voice` has
//! that test inline (`voice::io`); `web` had none, and the residue pass of the
//! no-port wave is where it gets one.
//!
//! # Why the second cell is driven as its I/O half
//!
//! The first display is the real thing, spawned through `WebCellFactory` the
//! way `web_cell_serves.rs` spawns it, and it is reached over the real
//! listener. The second is `run_io` directly, because the refusal is an event
//! on the seam between the two halves of one cell: the handler logs it and
//! nothing leaves the cell. Driving the I/O half is how the sentence becomes
//! observable at all, and it is the shape `voice`'s own collision test takes.

use meclaw_cells::web::io::{WebIo, run_io};
use meclaw_cells::web::render::PageMap;
use meclaw_cells::web::{AssetMap, WebCellFactory, WebEvent, WebReconfig};
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind, SurfaceRegistry};
use meclaw_core::{CellEmission, Path, serde_json::json};
use meclaw_testing::{surface_listener, wait_for_mount};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};

/// The repo's failure-marker window: generous, because it only ever bounds a
/// wait that a correct run leaves in microseconds.
const MARKER: Duration = Duration::from_secs(30);

/// Seed a cell directory with one page at `/`, so the holder has something to
/// serve. Same shape as `web_cell_serves.rs`, which is the neighbour this test
/// was written from.
fn seed_one_page(cell_dir: &std::path::Path, body: &str) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("create seed dir");
    std::fs::write(
        seed.join("components.jsonl"),
        format!(
            "{}\n{}\n",
            r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
            r#"{"name":"page","template":"<h1>{{body}}</h1>","prop_schema":"{\"body\":\"text\"}","editable":"[]","layer":"content"}"#
        ),
    )
    .expect("write components");
    let object_row = format!(
        r#"{{"id":"root","parent":null,"component":"page","ord":0,"props":"{{\"body\":\"{body}\"}}"}}"#
    );
    std::fs::write(
        seed.join("objects.jsonl"),
        format!(
            "{}\n{}\n",
            r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
            object_row
        ),
    )
    .expect("write objects");
    std::fs::write(
        seed.join("pages.jsonl"),
        format!(
            "{}\n{}\n",
            r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
            r#"{"route":"/","root":"root","title":"Home"}"#
        ),
    )
    .expect("write pages");
}

/// GET until the cell answers as a served cell, or the marker passes.
///
/// A `web` cell comes up in two steps: the mount goes on the table and the
/// handler publishes the first page snapshot. Between them the page answers
/// `503 starting`, which is the one status this retries — every other answer
/// reaches the caller's assertions untouched.
async fn get_with_retry(url: &str) -> reqwest::Response {
    let deadline = Instant::now() + MARKER;
    loop {
        let last = match reqwest::get(url).await {
            Ok(r) if r.status() != reqwest::StatusCode::SERVICE_UNAVAILABLE => return r,
            Ok(r) => format!("{} (the cell had not published yet)", r.status()),
            Err(e) => format!("{e}"),
        };
        assert!(
            Instant::now() < deadline,
            "the web cell never served on {url}; last answer: {last}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_web_cells_one_mount_the_second_is_refused() {
    let td = TempDir::new().expect("tempdir");
    let surfaces = Arc::new(SurfaceRegistry::new());

    // The holder: a real `web` cell under `/a`, mounted as `screen`.
    let held = td.path().join("a");
    std::fs::create_dir_all(&held).expect("create the cell dir");
    seed_one_page(&held, "page-a");
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new("/a"),
            json!({ "mount": "screen" }),
            out_tx,
            held,
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("spawn");
    let SpawnedCellKind::Active {
        join,
        sender,
        stop_tx,
        ..
    } = spawned
    else {
        panic!("web cells spawn Active");
    };
    // Held for the length of the test: closing the mailbox is what would end
    // the holder, and this test is about a collision with a LIVE holder.
    let _alive = (sender, stop_tx);
    wait_for_mount(&surfaces, "screen").await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;

    // The second display, `/b`, declares the same name.
    let (_pages_tx, pages_rx) = watch::channel(Arc::new(PageMap::new()));
    let (_assets_tx, assets_rx) = watch::channel(Arc::new(AssetMap::new()));
    let (_ready_tx, ready_rx) = watch::channel(true);
    // Capacity one on purpose — see the liveness receipt below.
    let (push_tx, push_rx) = mpsc::channel::<WebReconfig>(1);
    let (events_tx, mut events_rx) = mpsc::channel::<WebEvent>(8);
    let (_reconfig_tx, reconfig_rx) = mpsc::channel::<WebReconfig>(1);
    let io = WebIo::new(
        "screen".to_string(),
        String::new(),
        "/b",
        pages_rx,
        assets_rx,
        ready_rx,
        push_rx,
        Arc::clone(&surfaces),
    );
    let refused = tokio::spawn(run_io(io, events_tx, reconfig_rx));

    // One refusal, and it names the mount and who holds it.
    match tokio::time::timeout(MARKER, events_rx.recv())
        .await
        .expect("the refusal is reported")
        .expect("the channel is open")
    {
        WebEvent::MountFailed(detail) => {
            assert!(
                detail.contains("screen"),
                "the refusal names the mount; got {detail}"
            );
            assert!(detail.contains("/a"), "and who holds it; got {detail}");
        }
        // Two variants, so the other one is spelled out rather than bound:
        // `WebEvent` carries a `oneshot::Sender` and is deliberately not
        // `Debug`.
        WebEvent::Browser { name, .. } => {
            panic!("expected MountFailed; got the browser event {name:?}")
        }
    }

    // The life goes on, and the receipt is positive rather than a clock: the
    // push channel holds ONE message, so the second send returns only once the
    // fan-out has taken the first off it. A half that had ended with its
    // refusal would leave the second send waiting for the marker.
    let push = || WebReconfig::Push {
        route: "/".to_string(),
        diff: json!({}),
    };
    push_tx.send(push()).await.expect("the half still listens");
    tokio::time::timeout(MARKER, push_tx.send(push()))
        .await
        .expect("the refused half keeps draining its push channel")
        .expect("and the channel is still open");

    // And the mount is the holder's, undisturbed: one row on the table, and the
    // listener still serves the holder's page under the name both declared.
    let rows = surfaces.table().await;
    let mounted: Vec<_> = rows.iter().filter(|r| r.mount == "screen").collect();
    assert_eq!(
        mounted.len(),
        1,
        "the name is one row, held once; table was {rows:?}"
    );
    let body = get_with_retry(&format!("http://{addr}/screen/"))
        .await
        .text()
        .await
        .expect("read the body");
    assert!(
        body.contains("<h1>page-a</h1>"),
        "the holder still serves its own page; body was:\n{body}"
    );

    refused.abort();
    join.abort();
    listener.abort();
}
