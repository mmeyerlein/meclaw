//! W8 Task 3 (GH #380): a `web` cell owns a listener and serves its shell.
//!
//! The claim under test is the one the issue opens with: a display is not the
//! CLI's privilege any more. A `web` cell binds the port named in its own
//! `params`, and a plain GET against that port answers with the LiveView shell
//! — no `--api`, no `/surface/` prefix, no colony round trip on the request
//! path.
//!
//! # Why the factory is driven directly
//!
//! The obvious spelling would boot a colony from a template. `templates/web/`
//! is a later task, and a test that needed it could not be written before that
//! template existed — so this drives `CellFactory::spawn_cell` itself, the way
//! `boot_inactive_respawn_long_running.rs` drives the three long-running
//! factories. That is also the honest scope for this task: what is being proven
//! here is the factory and the listener, not the topology around them.
//!
//! # What "the shell" is asserted by
//!
//! `meclaw_surface::session::container_id` is the single source of the id the
//! LiveView client joins on (`lv:<id>`). The test computes it from the cell's
//! own path rather than hard-coding a string: the id is derived, and asserting a
//! guessed literal would pass for the wrong reason the day the derivation
//! changes.

use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::{CellEmission, Path, serde_json::json};
use meclaw_testing::free_port;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// GET the URL until the cell answers as a SERVED cell, or the deadline passes.
///
/// A `web` cell comes up in two steps that are not one moment: the I/O half
/// binds the socket, and the handler half publishes the first page snapshot
/// (the readiness seam, GH #395). Between the two the listener is reachable and
/// answers `503 starting` — a truthful "not published yet", not a verdict about
/// the routes. Waiting only for a connection therefore measured the bind and
/// read the gap to the publish as a broken cell (GH #578: `503` where `200` was
/// expected, 0.017 s into the test, under parallel load).
///
/// So both pre-publish states are retried — no connection yet, and `503` — and
/// what ends the wait is the positive signal that the cell is serving: any
/// answer that is not `503`. The window is the repo's 30 s failure-marker
/// convention.
///
/// **Why a real defect still fails this.** The retry consumes exactly one
/// status, the one the cell itself emits while it has nothing to serve. A cell
/// that never binds still ends at the marker; a cell that binds but never
/// publishes stays at `503` and ends at the marker too; and every wrong answer
/// a served cell can give — `404` from a broken page map, a `200` with the
/// wrong body — is handed to the caller's assertions untouched.
async fn get_with_retry(url: &str) -> reqwest::Response {
    let deadline = Instant::now() + Duration::from_secs(30);
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

/// Seed a cell directory with one page at `/` so there is something to serve.
///
/// Since Task 5 the `pages` table is the **only** route source (R-W8-3): a cell
/// with no pages correctly answers 404 everywhere, so a test about serving has
/// to say what it serves.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_web_cell_serves_its_shell_on_its_own_port() {
    let port = free_port();
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
    seed_one_page(&cell_dir, "hello");

    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);

    let spawned = Arc::new(WebCellFactory)
        .spawn_cell(
            Path::new("/web"),
            json!({ "port": port }),
            out_tx,
            cell_dir,
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("a web cell with a valid port must spawn");

    // A display must be up when the colony is: the type is eager, so spawning
    // it yields a running task rather than a mailbox waiting to be woken.
    //
    // The mailbox `sender` is bound rather than dropped, and that is load
    // bearing: dropping it closes the mailbox, which ends the handler, which
    // closes the reconfig channel, which is exactly the shutdown signal
    // `run_io` waits on — the listener would go down before the request
    // arrived. In a real colony the registry holds this end.
    let SpawnedCellKind::Active {
        join,
        sender: _sender,
        stop_tx: _stop_tx,
        ..
    } = spawned
    else {
        panic!(
            "the web cell must spawn Active — a display that waits for a first message is a blank screen"
        );
    };

    let url = format!("http://127.0.0.1:{port}/");
    let resp = get_with_retry(&url).await;
    assert_eq!(resp.status().as_u16(), 200, "a GET on the page route");

    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        ctype.starts_with("text/html"),
        "the shell is HTML, got content-type {ctype:?}"
    );

    let body = resp.text().await.expect("read the body");
    // The materialised page, embedded in the shell: the first paint IS the
    // page, not a spinner the client replaces on connect.
    assert!(body.contains("<h1>hello</h1>"), "body was:\n{body}");
    assert!(
        body.contains("<title>Home</title>"),
        "the page title travels"
    );
    let container = meclaw_surface::session::container_id("/web");
    assert!(
        body.contains(&format!("id=\"{container}\"")),
        "the shell must carry the LiveView container id {container:?}; body was:\n{body}"
    );
    assert!(
        body.contains("data-phx-main"),
        "the shell must mark its main container for the LiveView client"
    );
    // And it must say so when that client loses the socket. The states are the
    // ones the shipped bundle writes on the very container above; a page may
    // override the look, but no page has to remember to add one.
    assert!(
        body.contains("[data-phx-main].phx-error::after"),
        "the served shell must carry the default connection-state style"
    );

    join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_web_cells_serve_two_ports_independently() {
    // Acceptance bullet 1 of GH #380: the type is deliberately multiple. Two
    // instances, two ports, and neither knows the other exists.
    let (port_a, port_b) = (free_port(), free_port());
    assert_ne!(port_a, port_b);

    let td = TempDir::new().expect("tempdir");
    let mut joins = Vec::new();
    for (name, port) in [("a", port_a), ("b", port_b)] {
        let cell_dir = td.path().join(name);
        std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
        // Different content per instance, so the assertion below cannot pass
        // by accident if the two cells ever shared state.
        seed_one_page(&cell_dir, &format!("page-{name}"));
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = Arc::new(WebCellFactory)
            .spawn_cell(
                Path::new(&format!("/{name}")),
                json!({ "port": port }),
                out_tx,
                cell_dir,
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
        // Held for the length of the test — see the note in the first test.
        joins.push((name, join, sender, stop_tx));
    }

    for (name, port) in [("a", port_a), ("b", port_b)] {
        let body = get_with_retry(&format!("http://127.0.0.1:{port}/"))
            .await
            .text()
            .await
            .expect("read the body");
        let mine = meclaw_surface::session::container_id(&format!("/{name}"));
        assert!(
            body.contains(&format!("id=\"{mine}\"")),
            "the cell on {port} must serve its OWN container id {mine:?}"
        );
        assert!(
            body.contains(&format!("<h1>page-{name}</h1>")),
            "and its OWN content — two displays share nothing"
        );
    }

    for (_, join, _sender, _stop_tx) in joins {
        join.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_port_outside_the_range_is_refused_before_anything_binds() {
    // The parser is the gate: `validate_params` and `spawn_cell` share it, so a
    // bad port is a named refusal at plan time rather than a bind error at boot.
    for bad in [json!({}), json!({ "port": 0 }), json!({ "port": 70000 })] {
        assert!(
            WebCellFactory.validate_params(&bad).is_err(),
            "params {bad} must be refused"
        );
    }
    assert!(
        WebCellFactory
            .validate_params(&json!({ "port": 7800 }))
            .is_ok(),
        "a plain valid port must pass"
    );
}
