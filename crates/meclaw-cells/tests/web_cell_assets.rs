//! GH #393: a `web` cell serves the `assets` table it seeds.
//!
//! The `assets` table shipped with the type (GH #380) and nothing delivered it:
//! the router answered `/live/websocket`, `/@client/:file` and the page
//! wildcard, and the page wildcard looked only in the materialised page map. A
//! seeded `/vision.css` was therefore reachable by nothing, and a page that
//! linked a stylesheet rendered unstyled.
//!
//! # Why every test here **seeds** rather than inserting
//!
//! `crate::web::seed::json_to_sql` maps every JSON string onto SQLite `TEXT`,
//! so a seeded asset body sits in the `BLOB NOT NULL` column **as text**.
//! `FromSql for Vec<u8>` refuses that with `InvalidType`. A test that wrote its
//! fixture with a hand-rolled `INSERT … VALUES (x'…')` would therefore be green
//! against a read path that is broken for every asset that ever shipped in a
//! template. The fixture here goes through `seed/assets.jsonl` — the same file
//! the loader reads at spawn — so the case under test is the case that exists.
//!
//! # Why the factory is driven directly
//!
//! Same reason as `web_cell_serves.rs`: what is under test is the listener and
//! its lookup, not the topology around them. `templates/web/` is somebody
//! else's file.

use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::{CellEmission, Path, serde_json::json};
use meclaw_testing::free_port;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The seeded stylesheet, byte for byte.
///
/// Deliberately not plain ASCII prose: it carries a newline, braces and a
/// double quote, so a body that survived JSON escaping, the TEXT column and the
/// response writer unchanged is a real byte-fidelity claim rather than a
/// coincidence over a short word.
const CSS: &str = ":root{--r-window:18px}\n/* a \"quoted\" comment */\n";

/// The content type the row declares. Not what a file-extension guess would
/// produce, so the assertion cannot pass by a sniffer being right by accident.
const CSS_TYPE: &str = "text/css; charset=utf-8";

/// Write `seed/pages.jsonl`, `seed/objects.jsonl` and `seed/components.jsonl`
/// so the cell has one declared page at `/`.
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

/// Write `seed/assets.jsonl` with one row.
///
/// The header covers all three columns, which the loader requires — and the
/// `"blob"` type word in it is only a label: the loader checks that the column
/// is *named*, not what the seed calls it, which is precisely how the body ends
/// up in the column as TEXT.
fn seed_one_asset(cell_dir: &std::path::Path, path: &str, content_type: &str, body: &str) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("create seed dir");
    let row = json!({ "path": path, "content_type": content_type, "body": body });
    std::fs::write(
        seed.join("assets.jsonl"),
        format!(
            "{}\n{}\n",
            r#"{"schema":{"path":"text","content_type":"text","body":"blob"}}"#,
            meclaw_core::serde_json::to_string(&row).expect("serialise the seed row")
        ),
    )
    .expect("write assets");
}

/// A running `web` cell, with everything that must stay alive to keep it that
/// way.
///
/// The mailbox `sender` and the `stop_tx` are held rather than dropped, and
/// that is load bearing: dropping the sender ends the handler, which closes the
/// reconfig channel, which is exactly the shutdown signal `run_io` waits on. In
/// a real colony the registry holds these ends.
struct Booted {
    port: u16,
    join: tokio::task::JoinHandle<()>,
    _sender: mpsc::Sender<meclaw_core::Message>,
    _stop_tx: tokio::sync::oneshot::Sender<()>,
}

impl Booted {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

/// Spawn one `web` cell over `cell_dir` on a free port.
fn boot(cell_dir: &std::path::Path) -> Booted {
    let port = free_port();
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel(8);
    let spawned = Arc::new(WebCellFactory)
        .spawn_cell(
            Path::new("/web"),
            json!({ "port": port }),
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
        .expect("a web cell with a valid port must spawn");
    let SpawnedCellKind::Active {
        join,
        sender,
        stop_tx,
        ..
    } = spawned
    else {
        panic!("web cells spawn Active — one that waited for a message is a blank screen");
    };
    Booted {
        port,
        join,
        _sender: sender,
        _stop_tx: stop_tx,
    }
}

/// GET until the server answers at all, then return that answer.
///
/// The listener comes up in a spawned task, so the first request can lose the
/// race with `TcpListener::bind`. 30 s is the repo's failure-marker convention.
async fn get_once(url: &str) -> reqwest::Response {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match reqwest::get(url).await {
            Ok(r) => return r,
            Err(e) if Instant::now() < deadline => {
                let _ = e;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(e) => panic!("the web cell never answered on {url}: {e}"),
        }
    }
}

/// GET until the server answers **200**, then return that answer.
///
/// A 404 is a legitimate transient here and only here: `run_io` and the
/// handler's `on_start` are two tasks spawned back to back, so the listener can
/// bind a moment before the first snapshot — of pages or of assets — is
/// published. Retrying on 404 would hide a route that never works if the
/// deadline were open-ended; it is not, and a snapshot that never arrives fails
/// this in bounded time with the last status named.
async fn get_ok(url: &str) -> reqwest::Response {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let resp = get_once(url).await;
        let status = resp.status();
        if status.is_success() {
            return resp;
        }
        if Instant::now() >= deadline {
            panic!("{url} never answered 200 — last status was {status}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_asset_is_served_with_its_row_content_type_and_body() {
    // The acceptance line of GH #393, and the seeded-TEXT trap in one: this
    // fixture reaches the BLOB column as TEXT, so a read path built on
    // `FromSql for Vec<u8>` fails here with `InvalidType` and serves nothing.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
    seed_one_page(&cell_dir, "hello");
    seed_one_asset(&cell_dir, "/vision.css", CSS_TYPE, CSS);

    let booted = boot(&cell_dir);
    let resp = get_ok(&booted.url("/vision.css")).await;
    assert_eq!(resp.status().as_u16(), 200, "a GET on a seeded asset path");

    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        ctype, CSS_TYPE,
        "the content type comes from the row, not from the file name"
    );

    let body = resp.bytes().await.expect("read the body");
    assert_eq!(
        body.as_ref(),
        CSS.as_bytes(),
        "the asset is served byte for byte, quotes and newlines included"
    );

    booted.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_path_that_is_neither_page_nor_asset_stays_the_same_404() {
    // A display does not enumerate what it does not serve: adding a second
    // lookup must not give a probe a way to tell "no such page" from "no such
    // file". Both are the one negative answer `miss()` has always given.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
    seed_one_page(&cell_dir, "hello");
    seed_one_asset(&cell_dir, "/vision.css", CSS_TYPE, CSS);

    let booted = boot(&cell_dir);
    let resp = get_once(&booted.url("/nothing-declares-this")).await;
    assert_eq!(resp.status().as_u16(), 404, "an undeclared path");
    let body = resp.text().await.expect("read the body");
    assert_eq!(body, "not found\n", "the one negative answer, unchanged");

    // And the same body for a path that merely *looks* like a file, so the
    // answer cannot be read as "there is no page, but there might be an asset".
    let resp = get_once(&booted.url("/nothing-declares-this.css")).await;
    assert_eq!(resp.status().as_u16(), 404);
    assert_eq!(
        resp.text().await.expect("read the body"),
        "not found\n",
        "a missing asset and a missing page answer identically"
    );

    booted.join.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_page_and_an_asset_do_not_shadow_each_other() {
    // Two surfaces on one wildcard. The router asks both for every path, so
    // neither table can make the other's rows unreachable — which is the
    // property two competing axum routes over `/*path` could not have given.
    let td = TempDir::new().expect("tempdir");
    let cell_dir = td.path().join("web");
    std::fs::create_dir_all(&cell_dir).expect("create the cell dir");
    seed_one_page(&cell_dir, "hello");
    seed_one_asset(&cell_dir, "/vision.css", CSS_TYPE, CSS);

    let booted = boot(&cell_dir);

    let page = get_ok(&booted.url("/")).await;
    let ctype = page
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        ctype.starts_with("text/html"),
        "the declared page still answers as the shell, got {ctype:?}"
    );
    let page_body = page.text().await.expect("read the body");
    assert!(
        page_body.contains("<h1>hello</h1>"),
        "and still carries its materialised body; body was:\n{page_body}"
    );

    let asset = get_ok(&booted.url("/vision.css")).await;
    assert_eq!(
        asset.bytes().await.expect("read the body").as_ref(),
        CSS.as_bytes(),
        "and the asset is reachable in the same breath"
    );

    booted.join.abort();
}
