//! W8 Task 3 (GH #380), rewritten for `web@2.0.0` (GH #645): a `web` cell is
//! reached under its mount on the colony's one listener.
//!
//! The claim under test is the one #380 opened with, minus the port: a display
//! is not the CLI's privilege, and it is not a second listener either. A `web`
//! cell registers the name in its own `params.mount`, and a plain GET against
//! `/<mount>/` on the colony's listener answers with the LiveView shell — no
//! `--api`, no `/surface/` prefix, no colony round trip on the request path,
//! and no socket of the cell's own.
//!
//! # Why the factory is driven directly
//!
//! The obvious spelling would boot a colony from a template. This drives
//! `CellFactory::spawn_cell` itself, the way
//! `boot_inactive_respawn_long_running.rs` drives the three long-running
//! factories: what is being proven here is the factory, the registration and
//! the nesting, not the topology around them. The listener is the real one —
//! `meclaw_testing::surface_listener` runs
//! `meclaw_colony::surfaces::listener::serve` with a `404` where the CLI puts
//! the HTTP API.
//!
//! # What "the shell" is asserted by
//!
//! `meclaw_surface::session::container_id` is the single source of the id the
//! LiveView client joins on (`lv:<id>`). The test computes it from the cell's
//! own path rather than hard-coding a string: the id is derived, and asserting a
//! guessed literal would pass for the wrong reason the day the derivation
//! changes.

use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind, SurfaceRegistry};
use meclaw_core::{CellEmission, Path, serde_json::json};
use meclaw_testing::{surface_listener, wait_for_mount};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// GET the URL until the cell answers as a SERVED cell, or the deadline passes.
///
/// A `web` cell comes up in two steps that are not one moment: the mount goes
/// on the table, and the handler half publishes the first page snapshot (the
/// readiness seam, GH #395). Between the two the page is reachable and answers
/// `503 starting` — a truthful "not published yet", not a verdict about the
/// routes.
///
/// So the pre-publish state is retried, and what ends the wait is the positive
/// signal that the cell is serving: any answer that is not `503`. The window is
/// the repo's 30 s failure-marker convention.
///
/// **Why a real defect still fails this.** The retry consumes exactly one
/// status, the one the cell itself emits while it has nothing to serve. Every
/// wrong answer a served cell can give — the listener's `404` for a name
/// nothing holds, a `404` from a broken page map, a `200` with the wrong body —
/// is handed to the caller's assertions untouched.
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

/// Boot one `web` cell under `mount` over a seeded directory, on one listener.
///
/// Returns the listener's address and everything that has to stay alive: the
/// mailbox sender, the stop end and the join handle. Binding them is load
/// bearing — dropping the sender closes the mailbox, which ends the handler,
/// which closes the reconfig channel, which is exactly the shutdown signal the
/// I/O half waits on. In a real colony the registry holds that end.
async fn boot(cell_dir: &std::path::Path, path: &str, mount: &str) -> (String, Live) {
    boot_with(cell_dir, path, json!({ "mount": mount })).await
}

/// [`boot`], with the params spelled out — for the identity header.
async fn boot_with(
    cell_dir: &std::path::Path,
    path: &str,
    params: meclaw_core::JsonValue,
) -> (String, Live) {
    let mount = params["mount"].as_str().expect("a mount").to_string();
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new(path),
            params,
            out_tx,
            cell_dir.to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            64,
        )
        .expect("a web cell with a valid mount must spawn");

    // A display must be up when the colony is: the type is eager, so spawning
    // it yields a running task rather than a mailbox waiting to be woken.
    let SpawnedCellKind::Active {
        join,
        sender,
        stop_tx,
        ..
    } = spawned
    else {
        panic!(
            "the web cell must spawn Active — a display that waits for a first message is a blank screen"
        );
    };

    // The name before the request: the cell registers at the top of its I/O
    // half, and a GET that overtakes it reads the listener's `404` for a name
    // nobody holds.
    wait_for_mount(&surfaces, &mount).await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    (
        format!("http://{addr}"),
        Live {
            join,
            listener,
            sender,
            _stop_tx: stop_tx,
        },
    )
}

/// What a booted display needs kept alive for the length of a test.
struct Live {
    join: tokio::task::JoinHandle<()>,
    listener: tokio::task::JoinHandle<()>,
    /// The mailbox. Held rather than dropped: closing it is what ends the
    /// handler, and the handler ending is what ends the I/O half.
    sender: mpsc::Sender<meclaw_core::Message>,
    _stop_tx: tokio::sync::oneshot::Sender<()>,
}

impl Live {
    /// End both tasks. The listener owes nobody an answer after the test.
    fn stop(self) {
        self.join.abort();
        self.listener.abort();
    }

    /// Close the mailbox and wait for the cell to wind itself down.
    ///
    /// The ORDINARY end, not an abort: the handler reads a closed mailbox as
    /// "nobody will send here again", ends, and the closed reconfig channel is
    /// the I/O half's shutdown signal. What a test measures after this is what
    /// a colony's own stop leaves behind.
    async fn hang_up(self) -> tokio::task::JoinHandle<()> {
        let Self {
            join,
            listener,
            sender,
            _stop_tx,
        } = self;
        drop(sender);
        drop(_stop_tx);
        tokio::time::timeout(Duration::from_secs(30), join)
            .await
            .expect("the cell ends when its mailbox closes")
            .expect("and it ends without panicking");
        listener
    }
}

/// One GET with a header set, so a proxy's own headers can be spelled out.
async fn get_with_header(url: &str, name: &str, value: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(url)
        .header(name, value)
        .send()
        .await
        .expect("the listener answers")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_web_cell_serves_its_shell_under_its_mount() {
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
    seed_one_page(&cell_dir, "hello");

    let (base, live) = boot(&cell_dir, "/web", "screen").await;

    let resp = get_with_retry(&format!("{base}/screen/")).await;
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
    // And it must say so when that client loses the socket.
    assert!(
        body.contains("[data-phx-main].phx-error::after"),
        "the served shell must carry the default connection-state style"
    );

    // Every link the shell writes starts at the mount. A shell that still said
    // `/live` would join whatever else the colony's listener has there.
    assert!(
        body.contains("\"/screen/live\""),
        "the socket URL is the mount's; body was:\n{body}"
    );
    assert!(
        body.contains("src=\"/screen/@client/phoenix.min.js\""),
        "and so are the bundles; body was:\n{body}"
    );
    assert!(
        body.contains("<base href=\"/screen/\">"),
        "and a `<base>` so a materialised page can link its own files relatively"
    );

    // The name is the door: nothing answers beside it, and the listener says so
    // rather than hanging.
    let elsewhere = reqwest::get(format!("{base}/")).await.expect("an answer");
    assert_eq!(
        elsewhere.status().as_u16(),
        404,
        "the origin root is not this cell's"
    );

    live.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_web_cells_serve_two_mounts_on_one_listener() {
    // Acceptance bullet 1 of GH #380, in its `web@2.0.0` form: the type is
    // deliberately multiple, and multiple no longer means a port each. Two
    // instances, two names, ONE listener, and neither knows the other exists.
    let td = TempDir::new().expect("tempdir");
    let surfaces = Arc::new(SurfaceRegistry::new());
    let mut alive = Vec::new();
    for name in ["a", "b"] {
        let cell_dir = td.path().join(name);
        std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
        // Different content per instance, so the assertion below cannot pass
        // by accident if the two cells ever shared state.
        seed_one_page(&cell_dir, &format!("page-{name}"));
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
            .spawn_cell(
                Path::new(&format!("/{name}")),
                json!({ "mount": format!("screen-{name}") }),
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
        // Held for the length of the test — see the note in `boot_with`.
        alive.push((join, sender, stop_tx));
        wait_for_mount(&surfaces, &format!("screen-{name}")).await;
    }
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;

    for name in ["a", "b"] {
        let body = get_with_retry(&format!("http://{addr}/screen-{name}/"))
            .await
            .text()
            .await
            .expect("read the body");
        let mine = meclaw_surface::session::container_id(&format!("/{name}"));
        assert!(
            body.contains(&format!("id=\"{mine}\"")),
            "the cell mounted as screen-{name} must serve its OWN container id {mine:?}"
        );
        assert!(
            body.contains(&format!("<h1>page-{name}</h1>")),
            "and its OWN content — two displays share nothing"
        );
        assert!(
            body.contains(&format!("\"/screen-{name}/live\"")),
            "and its own socket URL, or the two pages would join one socket"
        );
    }

    for (join, _sender, _stop_tx) in alive {
        join.abort();
    }
    listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forwarded_prefix_moves_every_link_of_the_shell() {
    // The proxy shape of the ruling: a display may live on a domain path, and
    // the proxy says which one. Nothing about the cell changes — the same
    // mount, the same page — but every URL the shell writes has to move, or
    // the browser asks the domain root for a socket nobody serves there.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
    seed_one_page(&cell_dir, "hello");
    let (base, live) = boot(&cell_dir, "/web", "screen").await;
    // The page is there before the prefixed request, so the assertions below
    // are about the prefix and not about a cell that had not published.
    let _ = get_with_retry(&format!("{base}/screen/")).await;

    let body = get_with_header(&format!("{base}/screen/"), "X-Forwarded-Prefix", "/egon")
        .await
        .text()
        .await
        .expect("read the body");
    assert!(
        body.contains("\"/egon/screen/live\""),
        "the socket URL carries the prefix; body was:\n{body}"
    );
    assert!(
        body.contains("src=\"/egon/screen/@client/phoenix.min.js\"")
            && body.contains("src=\"/egon/screen/@client/phoenix_live_view.min.js\""),
        "and so do the bundles; body was:\n{body}"
    );
    assert!(
        body.contains("<base href=\"/egon/screen/\">"),
        "and the base every relative link in the page resolves against"
    );
    assert!(
        !body.contains("\"/screen/live\""),
        "nothing may be left pointing at the unproxied path; body was:\n{body}"
    );

    live.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_malformed_forwarded_prefix_is_ignored() {
    // O-P-3: the header comes off the wire, and whoever reaches the listener
    // can set it. A value outside the grammar is ignored — never repaired into
    // something plausible, and never written into a link.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
    seed_one_page(&cell_dir, "hello");
    let (base, live) = boot(&cell_dir, "/web", "screen").await;
    let _ = get_with_retry(&format!("{base}/screen/")).await;

    for bad in [
        "egon",
        "/egon/",
        "/egon?x=1",
        "/e\"><script>",
        // Two shapes made of nothing but path characters, and neither is a
        // path: a protocol-relative host, and a walk out of the mount.
        "//evil.com",
        "/egon/../..",
    ] {
        let body = get_with_header(&format!("{base}/screen/"), "X-Forwarded-Prefix", bad)
            .await
            .text()
            .await
            .expect("read the body");
        assert!(
            body.contains("<base href=\"/screen/\">") && body.contains("\"/screen/live\""),
            "{bad:?} must be ignored; body was:\n{body}"
        );
        assert!(
            !body.contains("egon") && !body.contains("evil.com"),
            "and nothing of it may reach the page — least of all a host \
             somebody else named; body was:\n{body}"
        );
    }

    live.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parse_refuses_port_and_bind_since_2_0_0() {
    // The parser is the gate: `validate_params` and `spawn_cell` share it, so a
    // tree written for `web@1.1.0` is a named refusal at plan time rather than
    // a display that comes up somewhere nobody expects.
    let factory = WebCellFactory::default();
    for old in [
        json!({ "port": 7800 }),
        json!({ "port": 7800, "bind": "127.0.0.1" }),
        json!({ "mount": "screen", "port": 7800 }),
    ] {
        let err = factory
            .validate_params(&old)
            .expect_err("a document carrying a port must be refused");
        assert_eq!(
            err,
            "port: removed in web 2.0.0 — the cell is reached at /<mount>/ on the colony's \
             listener; drop the key and name a mount",
            "the refusal names the migration; params were {old}"
        );
    }
    let err = factory
        .validate_params(&json!({ "mount": "screen", "bind": "0.0.0.0" }))
        .expect_err("and so must a bind");
    assert!(err.starts_with("bind: removed in web 2.0.0"), "got {err}");

    for bad in [
        json!({}),
        json!({ "mount": "Screen" }),
        json!({ "mount": "live" }),
    ] {
        assert!(
            factory.validate_params(&bad).is_err(),
            "params {bad} must be refused"
        );
    }
    assert!(
        factory
            .validate_params(&json!({ "mount": "screen" }))
            .is_ok(),
        "a plain valid mount must pass"
    );
}

/// A socket a browser is holding ends when the cell does.
///
/// Every handed connection is served on a task of its own, and until those
/// tasks were put in a `JoinSet` they outlived the cell that spawned them: a
/// wall screen kept its LiveView socket open against a half that had returned,
/// drew its last snapshot indefinitely, and never saw the close that makes the
/// client reconnect — so the next life of the cell served nobody until
/// somebody reloaded the tab by hand. `web@1.1.0` got this for free by
/// dropping the `axum::serve` future with the round.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_socket_ends_when_the_handler_goes_away() {
    use futures_util::{SinkExt, StreamExt};

    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
    seed_one_page(&cell_dir, "hello");
    let (base, live) = boot(&cell_dir, "/web", "screen").await;
    let _ = get_with_retry(&format!("{base}/screen/")).await;

    // A real client on a real socket, joined the way a page joins: an upgraded
    // WebSocket is no longer the HTTP connection the cell was handed, so this
    // is the case a `JoinSet` around the handed streams cannot reach on its
    // own.
    let page = reqwest::get(format!("{base}/screen/"))
        .await
        .expect("the page")
        .text()
        .await
        .expect("its body");
    let marker = "data-phx-session=\"";
    let start = page.find(marker).expect("a session token") + marker.len();
    let end = start + page[start..].find('"').expect("the token is quoted");
    let token = page[start..end].to_string();

    let ws_url = format!("{}/screen/live/websocket", base.replace("http://", "ws://"));
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .expect("the display accepts a websocket under its mount");
    let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        json!(["1", "1", topic, "phx_join", {"session": token, "url": "/screen/"}])
            .to_string()
            .into(),
    ))
    .await
    .expect("join");
    let reply = tokio::time::timeout(Duration::from_secs(30), ws.next())
        .await
        .expect("the join is answered")
        .expect("a frame")
        .expect("no error");
    assert!(
        reply.to_text().unwrap_or_default().contains("\"ok\""),
        "the viewer is joined before the cell is stopped: {reply:?}"
    );

    let listener = live.hang_up().await;

    // Not "the cell stopped answering" — the socket that was already open has
    // to END, and within a window a person would not notice.
    let ended = tokio::time::timeout(Duration::from_secs(30), ws.next())
        .await
        .expect("a socket the cell was serving must close when the cell goes");
    assert!(
        match ended {
            None => true,
            Some(Ok(msg)) => msg.is_close(),
            Some(Err(_)) => true,
        },
        "the stream must end (a close frame or a broken pipe), not hang"
    );

    listener.abort();
}

/// O-P-2, end to end: a renamed display comes back under the new name.
///
/// The promise is made in three places — `docs/cell-types*`,
/// `templates/web/README.md` and the overlay's own doc comment — and it has
/// two halves that only a respawn can show together: the running life keeps
/// the name it registered (no live remount, or a proxy rule would be pointing
/// at nothing), and the NEXT life reads the update off the `cell.db` overlay
/// and registers the new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mount_update_is_read_by_the_next_life() {
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
    seed_one_page(&cell_dir, "hello");

    // Spawned by hand rather than through `boot`: this test needs the respawn
    // the factory hands back, which is the only way to reach a "next life"
    // without a colony.
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
        .spawn_cell(
            Path::new("/web"),
            json!({ "mount": "screen" }),
            out_tx,
            cell_dir.clone(),
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
        respawn,
        ..
    } = spawned
    else {
        panic!("web cells spawn Active");
    };
    wait_for_mount(&surfaces, "screen").await;
    let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
    let _ = get_with_retry(&format!("http://{addr}/screen/")).await;

    sender
        .send(
            meclaw_core::MessageBuilder::new(Path::new("/web"))
                .body(meclaw_core::Body::Inline(
                    json!({"params": {"mount": "screen-two"}}),
                ))
                .reply_to(Path::new("/somebody"))
                .build(),
        )
        .await
        .expect("the mailbox takes a params update");

    // The update is silent on success, so what is waited for is the write it
    // makes: the overlay row a respawn replays. Reading the cell's own
    // database is what `web_cell_schema_and_seed` does for the same reason —
    // there is no message that reports it.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).expect("open cell.db");
        let stored: Option<String> = conn
            .query_row("SELECT value FROM params WHERE key = 'mount'", [], |r| {
                r.get(0)
            })
            .ok();
        drop(conn);
        if stored.as_deref() == Some("\"screen-two\"") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the params update never reached the overlay; the row was {stored:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // The RUNNING life still answers where it was registered, and under
    // nothing else: a rename that took effect at once would move a display out
    // from under the proxy rule pointed at it.
    assert_eq!(
        reqwest::get(format!("http://{addr}/screen/"))
            .await
            .expect("an answer")
            .status()
            .as_u16(),
        200,
        "the life that is running keeps the name it registered"
    );
    assert_eq!(
        reqwest::get(format!("http://{addr}/screen-two/"))
            .await
            .expect("an answer")
            .status()
            .as_u16(),
        404,
        "and the new name reaches nothing yet"
    );

    // The next life. Ending this one first is what a respawn does too — the
    // colony's restart barrier runs the same closure over the same `cell.db`.
    drop(sender);
    drop(stop_tx);
    tokio::time::timeout(Duration::from_secs(30), join)
        .await
        .expect("the first life ends when its mailbox closes")
        .expect("without panicking");
    let (sender_two, join_two, _peace, _backstop) = respawn();

    wait_for_mount(&surfaces, "screen-two").await;
    let page = get_with_retry(&format!("http://{addr}/screen-two/")).await;
    assert_eq!(
        page.status().as_u16(),
        200,
        "the next life answers under the name the update gave it"
    );
    let body = page.text().await.expect("the body");
    assert!(
        body.contains("<h1>hello</h1>") && body.contains("<base href=\"/screen-two/\">"),
        "with the same `cell.db` behind it and every link on the new name; \
         body was:\n{body}"
    );

    drop(sender_two);
    join_two.abort();
    listener.abort();
}
