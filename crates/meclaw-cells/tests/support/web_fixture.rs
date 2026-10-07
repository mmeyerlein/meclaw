//! The shared lab of the `web` cell (GH #1001 and the strands beside it).
//!
//! One page shaped like the deployed page that measured the cell's gaps: a root with
//! `figures` flat root children (`fig-<i>`) and `chunks` container root children
//! (`chunk-<c>`) of `kids` children each (`c<c>-o<k>`). The defaults are the
//! measured page's: 1 000 figures, 30 chunks of 128.
//!
//! Two ways in, over the same rows:
//!
//! - [`seed_db`] fills an in-memory database for the pure render functions;
//! - [`Lab::start`] writes the same rows as seed files, spawns a real `web`
//!   cell behind a real listener, and lets a test send tool-call bundles,
//!   read their answers (`duration_ms`) and open real LiveView sockets that
//!   count frames and bytes.
//!
//! Other strands add to this file; nothing here is changed in place.

#![allow(dead_code)]

use futures_util::{SinkExt, StreamExt};
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind, SurfaceRegistry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, Path};
use meclaw_testing::{surface_listener, wait_for_mount};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The name the lab's display answers to.
pub const MOUNT: &str = "lab";

/// The cell's path.
pub const CELL: &str = "/web";

/// The page's root object.
pub const ROOT: &str = "stage";

/// How big the page is.
#[derive(Debug, Clone, Copy)]
pub struct Shape {
    /// Flat root children (`fig-<i>`).
    pub figures: usize,
    /// Container root children (`chunk-<c>`).
    pub chunks: usize,
    /// Children per container (`c<c>-o<k>`).
    pub kids: usize,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            figures: 1_000,
            chunks: 30,
            kids: 128,
        }
    }
}

impl Shape {
    /// A pool of `figures` figures, everything else default.
    pub fn with_figures(figures: usize) -> Self {
        Self {
            figures,
            ..Self::default()
        }
    }

    /// Every object of the page, the root included.
    pub fn objects(&self) -> usize {
        1 + self.figures + self.chunks * (1 + self.kids)
    }
}

/// The six components: `(name, template, prop_schema, editable)`.
///
/// Shaped like the measured page's markup: data attributes for position, a binding
/// with a value, a conditional, a container, and one raw-html prop.
pub const COMPONENTS: &[(&str, &str, &str, &str)] = &[
    (
        "stage",
        r#"<main class="stage" data-w="{{w}}">{{children}}</main>"#,
        r#"{"w":"int"}"#,
        "[]",
    ),
    (
        "fig",
        r#"<div class="fig fig--{{kind}}" data-id="{{name}}" data-x="{{x}}" data-y="{{y}}" data-dir="{{dir}}" phx-click="pick" phx-value-for="{{name}}"><span class="fig__name">{{name}}</span></div>"#,
        r#"{"kind":"text","name":"text","x":"int","y":"int","dir":"text"}"#,
        r#"["x","y"]"#,
    ),
    (
        "chunk",
        r#"<section class="chunk" data-c="{{c}}" data-lod="{{lod}}">{{children}}</section>"#,
        r#"{"c":"int","lod":"text"}"#,
        "[]",
    ),
    (
        "house",
        r#"<div class="house" data-x="{{x}}" data-y="{{y}}" data-roof="{{roof}}">{{#if lit}}<i class="lamp"></i>{{/if}}</div>"#,
        r#"{"x":"int","y":"int","roof":"text","lit":"bool"}"#,
        "[]",
    ),
    (
        "tree",
        r#"<div class="tree" data-x="{{x}}" data-y="{{y}}"></div>"#,
        r#"{"x":"int","y":"int"}"#,
        "[]",
    ),
    (
        "note",
        r#"<aside class="note">{{&body}}</aside>"#,
        r#"{"body":"html"}"#,
        "[]",
    ),
];

/// Every object row: `(id, parent, component, ord, props)`.
pub fn objects(shape: Shape) -> Vec<(String, Option<String>, &'static str, i64, Value)> {
    let mut rows = Vec::with_capacity(shape.objects());
    rows.push((ROOT.to_string(), None, "stage", 0, json!({"w": 4096})));
    for i in 0..shape.figures {
        rows.push((
            format!("fig-{i}"),
            Some(ROOT.to_string()),
            "fig",
            i as i64,
            json!({
                "kind": if i % 3 == 0 { "walker" } else { "sitter" },
                "name": format!("fig-{i}"),
                "x": (i * 37) % 4096,
                "y": (i * 53) % 4096,
                "dir": "s",
            }),
        ));
    }
    for c in 0..shape.chunks {
        let chunk = format!("chunk-{c}");
        rows.push((
            chunk.clone(),
            Some(ROOT.to_string()),
            "chunk",
            (shape.figures + c) as i64,
            json!({"c": c, "lod": "near"}),
        ));
        for k in 0..shape.kids {
            let (component, props) = if k % 2 == 0 {
                (
                    "house",
                    json!({"x": k * 16, "y": c * 16, "roof": "red", "lit": k % 4 == 0}),
                )
            } else {
                ("tree", json!({"x": k * 16, "y": c * 16}))
            };
            rows.push((kid(c, k), Some(chunk.clone()), component, k as i64, props));
        }
    }
    rows
}

/// The id of child `k` of chunk `c`.
pub fn kid(c: usize, k: usize) -> String {
    format!("c{c}-o{k}")
}

/// Fill a database that already has the `web` schema.
pub fn seed_db(conn: &rusqlite::Connection, shape: Shape) {
    for (name, template, schema, editable) in COMPONENTS {
        conn.execute(
            "INSERT INTO components (name, template, prop_schema, editable, layer) \
             VALUES (?1, ?2, ?3, ?4, 'content')",
            rusqlite::params![name, template, schema, editable],
        )
        .expect("insert component");
    }
    let tx = conn.unchecked_transaction().expect("tx");
    for (id, parent, component, ord, props) in objects(shape) {
        tx.execute(
            "INSERT INTO objects (id, parent, component, ord, props) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, parent, component, ord, props.to_string()],
        )
        .expect("insert object");
    }
    tx.commit().expect("commit");
    conn.execute(
        "INSERT INTO pages (route, root, title) VALUES ('/', ?1, 'Lab')",
        [ROOT],
    )
    .expect("insert page");
}

/// An in-memory database with the schema and the page of `shape`.
pub fn memory_db(shape: Shape) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open");
    meclaw_cells::web::db::setup_web_schema(&conn).expect("schema");
    seed_db(&conn, shape);
    conn
}

/// Write the page of `shape` as seed files under `cell_dir/seed`.
pub fn seed_files(cell_dir: &std::path::Path, shape: Shape) {
    let seed = cell_dir.join("seed");
    std::fs::create_dir_all(&seed).expect("seed dir");
    let mut components = String::from(
        r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
    );
    components.push('\n');
    for (name, template, schema, editable) in COMPONENTS {
        components.push_str(
            &json!({
                "name": name,
                "template": template,
                "prop_schema": schema,
                "editable": editable,
                "layer": "content",
            })
            .to_string(),
        );
        components.push('\n');
    }
    std::fs::write(seed.join("components.jsonl"), components).expect("components");

    let mut objects_jsonl = String::from(
        r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
    );
    objects_jsonl.push('\n');
    for (id, parent, component, ord, props) in objects(shape) {
        objects_jsonl.push_str(
            &json!({
                "id": id,
                "parent": parent,
                "component": component,
                "ord": ord,
                "props": props.to_string(),
            })
            .to_string(),
        );
        objects_jsonl.push('\n');
    }
    std::fs::write(seed.join("objects.jsonl"), objects_jsonl).expect("objects");

    std::fs::write(
        seed.join("pages.jsonl"),
        format!(
            "{}\n{}\n",
            r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
            json!({"route": "/", "root": ROOT, "title": "Lab"})
        ),
    )
    .expect("pages");
}

/// One leg of a tool-call turn.
pub fn leg(i: usize, op: Value) -> Value {
    json!({
        "origin": "assistant",
        "type": "tool_call",
        "id": format!("c{i}"),
        "text": op.to_string(),
    })
}

/// An `object.update` op.
pub fn update(id: &str, props: Value) -> Value {
    json!({"op": "object.update", "id": id, "props": props})
}

/// A live `web` cell over the lab page.
pub struct Lab {
    /// The port of the listener in front of the cell.
    pub port: u16,
    /// The shape the page was seeded with.
    pub shape: Shape,
    sender: mpsc::Sender<meclaw_core::Message>,
    out_rx: mpsc::Receiver<CellEmission>,
    _listener: tokio::task::JoinHandle<()>,
    _stop: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<()>,
    _dir: TempDir,
}

impl Drop for Lab {
    fn drop(&mut self) {
        self.join.abort();
    }
}

impl Lab {
    /// Seed, spawn, mount, and wait until the page is served.
    pub async fn start(shape: Shape) -> Lab {
        let dir = TempDir::new().expect("tempdir");
        let cell_dir = dir.path().join("web");
        std::fs::create_dir_all(&cell_dir).expect("cell dir");
        seed_files(&cell_dir, shape);

        let surfaces = Arc::new(SurfaceRegistry::new());
        let (out_tx, out_rx) = mpsc::channel::<CellEmission>(256);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
            .spawn_cell(
                Path::new(CELL),
                // The lab measures what a WRITE costs, and its viewer reads the
                // join as one reply. The default 4 000 figures are above the
                // join cut of GH #1002 (`CUT_ABOVE_BYTES`), so at the default
                // piece limit the first frames after the reply are join pieces,
                // not the write's diff (seen: a 92 882-byte "child update").
                // At the largest piece the lab's join fits one frame, as before.
                json!({ "mount": MOUNT, "join_chunk_bytes": 4 * 1024 * 1024 }),
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
            ..
        } = spawned
        else {
            panic!("web cells spawn Active");
        };

        wait_for_mount(&surfaces, MOUNT).await;
        let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
        let port = addr.port();

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(r) = reqwest::get(format!("http://127.0.0.1:{port}/{MOUNT}/")).await
                && r.status().is_success()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the lab cell never served its page"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        Lab {
            port,
            shape,
            sender,
            out_rx,
            _listener: listener,
            _stop: stop_tx,
            join,
            _dir: dir,
        }
    }

    /// Send one message of tool-call turns and wait for its answer.
    pub async fn call(&mut self, ops: Vec<Value>) -> Value {
        let msg = bundle_message(ops);
        let id = msg.id;
        self.sender
            .send(msg)
            .await
            .expect("the cell takes messages");
        // GH #1020: the answer is matched to its call, not taken as whatever
        // the output channel carries next — a cell task may put its own
        // bookkeeping on the same channel after a delivery.
        loop {
            let emission = tokio::time::timeout(Duration::from_secs(30), self.out_rx.recv())
                .await
                .expect("the cell answers within the failure-marker window")
                .expect("the cell is alive");
            if is_reply_to(&emission, id) {
                return emission.content;
            }
        }
    }

    /// The cell's own database file (`<cell dir>/cell.db`), for a test that
    /// reads what a write left in it through a connection of its own.
    pub fn cell_db(&self) -> std::path::PathBuf {
        self._dir.path().join("web").join("cell.db")
    }

    /// The page as served on a GET (shell included).
    pub async fn get_page(&self) -> String {
        self.get_page_at("/").await
    }

    /// The page of `route` (`/`, `/two`) as served on a GET.
    pub async fn get_page_at(&self, route: &str) -> String {
        reqwest::get(self.url_of(route))
            .await
            .expect("get")
            .text()
            .await
            .expect("text")
    }

    /// The URL a browser opens for `route`.
    fn url_of(&self, route: &str) -> String {
        let tail = route.trim_start_matches('/');
        format!("http://127.0.0.1:{}/{MOUNT}/{tail}", self.port)
    }

    /// Open a socket and join the page like the client does.
    pub async fn viewer(&self) -> Viewer {
        self.viewer_at("/").await
    }

    /// Open a socket and join the page of `route`.
    pub async fn viewer_at(&self, route: &str) -> Viewer {
        viewer_on(self.port, route).await
    }
}

/// Open a socket on the lab listener at `port` and join the page of `route`
/// like the client does (the body of [`Lab::viewer_at`], shared with
/// [`held::HeldLab`]).
pub async fn viewer_on(port: u16, route: &str) -> Viewer {
    let url_of = |route: &str| {
        let tail = route.trim_start_matches('/');
        format!("http://127.0.0.1:{port}/{MOUNT}/{tail}")
    };
    let body = reqwest::get(url_of(route))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    let marker = "data-phx-session=\"";
    let start = body
        .find(marker)
        .expect("the shell carries a session token")
        + marker.len();
    let end = start + body[start..].find('"').expect("token is quoted");
    let token = body[start..end].to_string();
    let container = meclaw_surface::session::container_id(CELL);
    let topic = format!("lv:{container}");
    let (ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/{MOUNT}/live/websocket"))
            .await
            .expect("the cell accepts a websocket");
    let mut v = Viewer {
        ws,
        topic: topic.clone(),
        join_bytes: 0,
        rendered: Value::Null,
        frames: 0,
        bytes: 0,
    };
    v.send(json!(["1", "1", topic, "phx_join", {
        "session": token,
        "url": url_of(route)
    }]))
    .await;
    let (n, reply) = v.recv_raw().await;
    assert_eq!(reply[4]["status"], json!("ok"), "join reply: {reply}");
    v.join_bytes = n;
    v.rendered = reply[4]["response"]["rendered"].clone();
    v
}

/// The `duration_ms` an answer reports.
pub fn duration_ms(answer: &Value) -> i64 {
    answer["header"]["duration_ms"]
        .as_i64()
        .unwrap_or_else(|| panic!("the answer carries duration_ms: {answer}"))
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// One joined socket, counting what reaches it.
pub struct Viewer {
    ws: Ws,
    /// The joined topic.
    pub topic: String,
    /// Bytes of the join reply frame.
    pub join_bytes: usize,
    /// The rendered tree the join answered with.
    pub rendered: Value,
    /// Push frames received after the join.
    pub frames: usize,
    /// Bytes of those frames.
    pub bytes: usize,
}

impl Viewer {
    async fn send(&mut self, frame: Value) {
        self.ws
            .send(WsMessage::Text(frame.to_string().into()))
            .await
            .expect("send");
    }

    async fn recv_raw(&mut self) -> (usize, Value) {
        let msg = tokio::time::timeout(Duration::from_secs(30), self.ws.next())
            .await
            .expect("a frame within the failure-marker window")
            .expect("stream open")
            .expect("frame");
        let WsMessage::Text(t) = msg else {
            panic!("expected a text frame, got {msg:?}")
        };
        let v = meclaw_core::serde_json::from_str(&t).expect("a frame is JSON");
        (t.len(), v)
    }

    /// The next `diff` push: its frame size in bytes and its payload.
    pub async fn next_diff(&mut self) -> (usize, Value) {
        loop {
            let (n, frame) = self.recv_raw().await;
            if frame[3] == json!("diff") {
                self.frames += 1;
                self.bytes += n;
                return (n, frame[4].clone());
            }
        }
    }
}

/// GH #1006: the backlog lab — a `web` cell that reports `viewer:backlog`,
/// real sockets whose reader the test throttles, and an app that writes
/// tranches at a fixed cadence and may react to what it hears.
///
/// Its own page (400 flat figures under one root) and its own spawn, because
/// the test needs every emission that is not a reply: the backlog reports, and
/// for the opt-out lock the log-growth witness. Added here, not changed in
/// place (the rule of this file).
pub mod backlog_lab {
    use futures_util::{SinkExt, StreamExt};
    use meclaw_cells::web::WebCellFactory;
    use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
    use meclaw_core::serde_json::{Value, json};
    use meclaw_core::{Body, CellEmission, MessageBuilder, Path};
    use meclaw_testing::{EmissionsExt, surface_listener, wait_for_mount};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    use tempfile::TempDir;
    use tokio::sync::{Mutex, mpsc};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    /// The cell's mount.
    pub const MOUNT: &str = "screen";
    /// Figures on the page, and the full tranche the app rewrites per pass.
    pub const FIGURES: usize = 400;
    /// The app's pass interval: one tranche every 125 ms, the cadence of the
    /// measured 2-D world app this signal is for.
    pub const PASS: Duration = Duration::from_millis(125);
    /// The throttled reader's rate: a slow phone.
    pub const SLOW: u64 = 50 * 1024;
    /// The cell's default thresholds of `high`, which the lab runs with.
    pub const HIGH_BYTES: u64 = 256 * 1024;
    /// See [`HIGH_BYTES`].
    pub const HIGH_MS: u64 = 250;

    fn seed(cell_dir: &std::path::Path, routes: &[&str]) {
        let seed = cell_dir.join("seed");
        std::fs::create_dir_all(&seed).expect("seed dir");
        std::fs::write(
            seed.join("components.jsonl"),
            concat!(
                r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
                "\n",
                r#"{"name":"stack","template":"<main>{{children}}</main>","prop_schema":"{}","editable":"[]","layer":"content"}"#,
                "\n",
                r#"{"name":"fig","template":"<i data-x=\"{{x}}\">{{label}}</i>","prop_schema":"{\"x\":\"text\",\"label\":\"text\"}","editable":"[]","layer":"content"}"#,
                "\n"
            ),
        )
        .expect("components");
        let mut objects = String::from(
            r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
        );
        objects.push('\n');
        objects.push_str(r#"{"id":"root","parent":null,"component":"stack","ord":0,"props":"{}"}"#);
        objects.push('\n');
        for i in 0..FIGURES {
            let props = json!({"x": "0", "label": format!("figure {i:04}")}).to_string();
            objects.push_str(
                &json!({"id": format!("fig-{i}"), "parent": "root", "component": "fig", "ord": i, "props": props})
                    .to_string(),
            );
            objects.push('\n');
        }
        std::fs::write(seed.join("objects.jsonl"), objects).expect("objects");
        // GH #1003: one page per route, all of the same tree — the screen
        // classes of one member (display-hive § 6.1).
        let mut pages = String::from(r#"{"schema":{"route":"text","root":"text","title":"text"}}"#);
        pages.push('\n');
        for route in routes {
            pages.push_str(&json!({"route": route, "root": "root", "title": "Home"}).to_string());
            pages.push('\n');
        }
        std::fs::write(seed.join("pages.jsonl"), pages).expect("pages");
    }

    /// One `viewer:backlog` as the app hears it, with when it heard it.
    #[derive(Debug, Clone)]
    pub struct Heard {
        /// When the test's emission reader got it.
        pub at: Instant,
        /// The viewer's session.
        pub session: String,
        /// `high` (true) or `clear`.
        pub high: bool,
        /// The numbers in the report.
        pub bytes: u64,
        /// Age of the oldest outstanding frame, by the cell's clock.
        pub oldest_ms: u64,
    }

    /// A running backlog cell and the app's end of it.
    pub struct Live {
        _td: TempDir,
        /// The listener's port.
        pub port: u16,
        _listener: tokio::task::JoinHandle<()>,
        mailbox: mpsc::Sender<meclaw_core::Message>,
        replies: mpsc::Receiver<Value>,
        heard: Arc<Mutex<Vec<Heard>>>,
        /// Every `viewer:screen` value, in order (GH #1003).
        screens: Arc<Mutex<Vec<Value>>>,
        /// Every emission that is not a reply: the log-growth witness.
        pub emitted: Arc<AtomicU64>,
        _stop: tokio::sync::oneshot::Sender<()>,
        /// The cell task.
        pub join: tokio::task::JoinHandle<()>,
    }

    /// Spawn the cell with `params` and wait until it serves its page.
    pub async fn start(params: Value) -> Live {
        start_routes(params, &["/"]).await
    }

    /// [`start`] with one page per route, all of the same tree (GH #1003).
    pub async fn start_routes(params: Value, routes: &[&str]) -> Live {
        let td = TempDir::new().expect("td");
        let cell_dir = td.path().join("web");
        std::fs::create_dir_all(&cell_dir).expect("dir");
        seed(&cell_dir, routes);
        let surfaces = Arc::new(meclaw_colony::SurfaceRegistry::new());
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(256);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = Arc::new(WebCellFactory::new(Arc::clone(&surfaces)))
            .spawn_cell(
                Path::new("/web"),
                params,
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
            ..
        } = spawned
        else {
            panic!("Active");
        };
        wait_for_mount(&surfaces, MOUNT).await;
        let (addr, listener) = surface_listener(Arc::clone(&surfaces)).await;
        let port = addr.port();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(r) = reqwest::get(format!("http://127.0.0.1:{port}/{MOUNT}/")).await
                && r.status().is_success()
            {
                break;
            }
            assert!(Instant::now() < deadline, "the cell never served its page");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // Sort what the cell emits: replies to the app's calls, and events.
        let heard = Arc::new(Mutex::new(Vec::new()));
        let screens = Arc::new(Mutex::new(Vec::new()));
        let emitted = Arc::new(AtomicU64::new(0));
        let (reply_tx, replies) = mpsc::channel(64);
        {
            let heard = Arc::clone(&heard);
            let screens = Arc::clone(&screens);
            let emitted = Arc::clone(&emitted);
            tokio::spawn(async move {
                while let Some(e) = out_rx.recv_answer().await {
                    let header = &e.content["header"];
                    if header["route"] == json!("event") {
                        emitted.fetch_add(1, Ordering::Relaxed);
                        if header["event_name"] == json!("viewer:backlog") {
                            let v = &e.content["event"]["value"];
                            heard.lock().await.push(Heard {
                                at: Instant::now(),
                                session: v["session_id"].as_str().unwrap_or("").to_string(),
                                high: v["level"] == json!("high"),
                                bytes: v["bytes"].as_u64().unwrap_or(0),
                                oldest_ms: v["oldest_ms"].as_u64().unwrap_or(0),
                            });
                        } else if header["event_name"] == json!("viewer:screen") {
                            screens
                                .lock()
                                .await
                                .push(e.content["event"]["value"].clone());
                        }
                    } else if e.target.as_str() != super::REPLY_TO {
                        // GH #1020: bookkeeping on the output channel, not a reply.
                        continue;
                    } else if reply_tx.send(e.content).await.is_err() {
                        return;
                    }
                }
            });
        }
        Live {
            _td: td,
            port,
            _listener: listener,
            mailbox: sender,
            replies,
            heard,
            screens,
            emitted,
            _stop: stop_tx,
            join,
        }
    }

    /// The params of a cell that opted in to backlog reports.
    pub fn opted_in() -> Value {
        json!({"mount": MOUNT, "viewer_events": ["backlog"]})
    }

    impl Live {
        /// One message of tool calls, answered.
        pub async fn call(&mut self, calls: Vec<Value>) -> Value {
            let messages: Vec<Value> = calls
                .iter()
                .enumerate()
                .map(|(i, a)| json!({"origin":"tool","type":"tool_call","text": a.to_string(),"id": format!("c{i}")}))
                .collect();
            let msg = MessageBuilder::new(Path::new("/web"))
                .body(Body::Inline(json!({ "messages": messages })))
                .reply_to(Path::new("/caller"))
                .build();
            self.mailbox.send(msg).await.expect("mailbox");
            tokio::time::timeout(Duration::from_secs(30), self.replies.recv())
                .await
                .expect("the cell must answer")
                .expect("reply")
        }

        /// One app pass: `k` figures rewritten in one bundle, one frame out.
        pub async fn pass(&mut self, round: u64, k: usize) {
            let calls = (0..k.max(2))
                .map(|i| {
                    json!({"op": "object.update", "id": format!("fig-{i}"),
                           "props": {"x": format!("{round}-{i}"), "label": format!("figure {i:04} round {round:06}")}})
                })
                .collect();
            let _ = self.call(calls).await;
        }

        /// The `viewers` op's rows.
        pub async fn viewers(&mut self) -> Vec<Value> {
            let r = self.call(vec![json!({"op": "viewers"})]).await;
            let rows = find_rows(&r).unwrap_or_else(|| panic!("viewers rows in {r}"));
            rows.as_array().cloned().unwrap_or_default()
        }

        /// Every backlog report heard so far.
        pub async fn heard(&self) -> Vec<Heard> {
            self.heard.lock().await.clone()
        }

        /// Every `viewer:screen` value heard so far (GH #1003).
        pub async fn screens(&self) -> Vec<Value> {
            self.screens.lock().await.clone()
        }
    }

    /// The `viewers` array wherever the reply shape put it.
    fn find_rows(v: &Value) -> Option<Value> {
        match v {
            Value::Object(m) => {
                if let Some(rows) = m.get("viewers")
                    && rows.is_array()
                {
                    return Some(rows.clone());
                }
                m.values().find_map(find_rows)
            }
            Value::Array(a) => a.iter().find_map(find_rows),
            Value::String(s) if s.contains("\"viewers\"") => {
                meclaw_core::serde_json::from_str::<Value>(s)
                    .ok()
                    .and_then(|p| find_rows(&p))
            }
            _ => None,
        }
    }

    /// The row of `session` in a `viewers` answer.
    pub fn row_of<'a>(rows: &'a [Value], session: &str) -> Option<&'a Value> {
        rows.iter().find(|r| r["session_id"] == json!(session))
    }

    /// A field of a row as a number, 0 when absent.
    pub fn num(row: &Value, key: &str) -> u64 {
        row[key].as_u64().unwrap_or(0)
    }

    /// A joined viewer whose reader the test controls.
    pub struct Reader {
        /// Its session id, as the reports and rows name it.
        pub session: String,
        /// Frames it has read.
        pub frames: Arc<AtomicU64>,
        /// Bytes it has read (GH #1067: the rate the app really wrote).
        pub bytes: Arc<AtomicU64>,
        /// Bytes per second the reader takes; 0 is unthrottled.
        pub rate: Arc<AtomicU64>,
        /// The reading task.
        pub task: tokio::task::JoinHandle<()>,
    }

    /// The page's LiveView token, as a client reads it from the shell.
    pub async fn token(port: u16) -> String {
        let body = reqwest::get(format!("http://127.0.0.1:{port}/{MOUNT}/"))
            .await
            .expect("get")
            .text()
            .await
            .expect("text");
        let marker = "data-phx-session=\"";
        let start = body.find(marker).expect("token") + marker.len();
        let end = start + body[start..].find('"').expect("quote");
        body[start..end].to_string()
    }

    /// Join like the client, then read at `rate` bytes/s (0 = as fast as it comes).
    pub async fn join(port: u16, rate: u64) -> Reader {
        let token = token(port).await;
        let socket = tokio::net::TcpSocket::new_v4().expect("socket");
        // A small receive window, so the backlog shows in the cell's queue and
        // not in a kernel buffer the cell cannot see.
        socket.set_recv_buffer_size(8 * 1024).expect("rcvbuf");
        // And a small segment size, advertised in the SYN, so the CELL's side
        // of the connection does not grow a send buffer of megabytes either:
        // on loopback the MSS is ~64 KB and Linux sizes the send buffer by it
        // (`tcp_sndbuf_expand`), which swallowed the whole backlog of a 4 s
        // run in the first measurement (0 bytes queued at 130 KB/s in, 50 KB/s
        // out). A real phone sits behind a 1.4 KB MSS.
        {
            use std::os::fd::AsRawFd;
            let mss: libc::c_int = 1200;
            // SAFETY: a valid fd we own, a c_int option of the size we pass.
            let rc = unsafe {
                libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::IPPROTO_TCP,
                    libc::TCP_MAXSEG,
                    (&mss as *const libc::c_int).cast(),
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                )
            };
            assert_eq!(rc, 0, "TCP_MAXSEG");
        }
        let stream = socket
            .connect(([127, 0, 0, 1], port).into())
            .await
            .expect("connect");
        let (mut ws, _) = tokio_tungstenite::client_async(
            format!("ws://127.0.0.1:{port}/{MOUNT}/live/websocket"),
            stream,
        )
        .await
        .expect("handshake");
        let topic = format!("lv:{}", meclaw_surface::session::container_id("/web"));
        ws.send(WsMessage::Text(
            json!(["1", "1", topic, "phx_join", {"session": token, "url": "/"}])
                .to_string()
                .into(),
        ))
        .await
        .expect("join");
        let _ = ws.next().await.expect("open").expect("join reply");
        let frames = Arc::new(AtomicU64::new(0));
        let bytes = Arc::new(AtomicU64::new(0));
        let rate = Arc::new(AtomicU64::new(rate));
        let task = {
            let (frames, bytes, rate) =
                (Arc::clone(&frames), Arc::clone(&bytes), Arc::clone(&rate));
            tokio::spawn(async move {
                let mut taken: u64 = 0;
                let mut since = Instant::now();
                let mut paced_at = rate.load(Ordering::Relaxed);
                while let Some(Ok(m)) = ws.next().await {
                    frames.fetch_add(1, Ordering::Relaxed);
                    bytes.fetch_add(m.len() as u64, Ordering::Relaxed);
                    let r = rate.load(Ordering::Relaxed);
                    // GH #1067: a new rate starts its own budget. Kept, the
                    // bytes taken at the old rate would be re-priced at the
                    // new one and the reader would stall for the difference.
                    if r != paced_at {
                        paced_at = r;
                        taken = 0;
                        since = Instant::now();
                    }
                    if r == 0 {
                        taken = 0;
                        since = Instant::now();
                        continue;
                    }
                    taken += m.len() as u64;
                    let due = since + Duration::from_secs_f64(taken as f64 / r as f64);
                    tokio::time::sleep_until(due.into()).await;
                }
            })
        };
        Reader {
            session: token.split('.').next().expect("nonce").to_string(),
            frames,
            bytes,
            rate,
            task,
        }
    }

    /// GH #1067: hold a slow reader to the app's measured pace.
    ///
    /// Under full-suite load a strand gate's app made one pass per 415 ms
    /// instead of [`PASS`]: a full tranche (~23 KB) then came at ~55 KB/s,
    /// a fixed 50 KB/s reader kept up and no `high` came in 30 s. Scaled by
    /// the measured mean pass, the reader (joined at `base`) stays ~3.6x
    /// slower than the full tranche at any load, as on an idle host. Only a
    /// change of more than a tenth moves it: every move restarts the
    /// reader's budget.
    fn pace(rate: &AtomicU64, base: u64, started: Instant, passes: u64) {
        let mean = started.elapsed().as_secs_f64() / passes.max(1) as f64;
        let paced = (base as f64 * PASS.as_secs_f64() / mean.max(PASS.as_secs_f64())) as u64;
        let now = rate.load(Ordering::Relaxed);
        if now.abs_diff(paced) > now / 10 {
            rate.store(paced, Ordering::Relaxed);
        }
    }

    /// Push full tranches at the app's cadence for `d`, sampling `viewers`
    /// every pass. Returns the samples `(at, rows)`.
    pub async fn drive(
        live: &mut Live,
        d: Duration,
        round: &mut u64,
    ) -> Vec<(Instant, Vec<Value>)> {
        drive_paced(live, d, round, PASS).await
    }

    /// [`drive`] with the app's pass interval set to `pass`.
    pub async fn drive_paced(
        live: &mut Live,
        d: Duration,
        round: &mut u64,
        pass: Duration,
    ) -> Vec<(Instant, Vec<Value>)> {
        let mut samples = Vec::new();
        let end = Instant::now() + d;
        let mut tick = tokio::time::interval(pass);
        while Instant::now() < end {
            tick.tick().await;
            *round += 1;
            live.pass(*round, FIGURES).await;
            samples.push((Instant::now(), live.viewers().await));
        }
        samples
    }

    /// Drive full tranches until `slow` has been reported at `level`
    /// (`high` = true), then `after` more. Panics after 30 s: a failure
    /// marker, not a measurement.
    pub async fn drive_until(
        live: &mut Live,
        slow: &Reader,
        high: bool,
        after: Duration,
        round: &mut u64,
    ) -> Vec<(Instant, Vec<Value>)> {
        drive_until_paced(live, slow, high, after, round, PASS)
            .await
            .0
    }

    /// [`drive_until`] with the app's pass interval set to `pass` -- a
    /// starved app (GH #1067). While it waits for `high`, and for `after`
    /// past it (GH #1080), the slow reader runs at the app's measured pace ([`pace`]), as in [`lab_paced`]:
    /// T1, T2 and T5 drive full tranches against a 50 KB/s reader, the
    /// arithmetic that left T7 without a `high` in a gate. Returns the samples and what the run up to
    /// the level measured (`passes`, `secs`, `pass_ms`, `wrote_bps`,
    /// `reader_bps`; the other fields stay 0).
    pub async fn drive_until_paced(
        live: &mut Live,
        slow: &Reader,
        high: bool,
        after: Duration,
        round: &mut u64,
        pass: Duration,
    ) -> (Vec<(Instant, Vec<Value>)>, Lab) {
        let started = Instant::now();
        let base = slow.rate.load(Ordering::Relaxed);
        let read_before = slow.bytes.load(Ordering::Relaxed);
        let mut lab = Lab::default();
        let mut samples = Vec::new();
        let mut queued = 0;
        // What the app wrote to the slow viewer is what it read plus what
        // still waits on its queue (the kernel's few KB aside): a starved app
        // shows as a write rate the reader keeps up with.
        let settle = |lab: &mut Lab, queued: u64| {
            lab.secs = started.elapsed().as_secs_f64();
            let wrote = slow.bytes.load(Ordering::Relaxed) - read_before + queued;
            lab.wrote_bps = (wrote as f64 / lab.secs) as u64;
            lab.pass_ms = (lab.secs * 1000.0 / lab.passes.max(1) as f64) as u64;
            lab.reader_bps = slow.rate.load(Ordering::Relaxed);
        };
        let mut tick = tokio::time::interval(pass);
        loop {
            let heard = live.heard().await;
            if heard
                .iter()
                .any(|h| h.session == slow.session && h.high == high)
            {
                break;
            }
            if started.elapsed() >= Duration::from_secs(30) {
                settle(&mut lab, queued);
                panic!(
                    "no {} for {} in 30 s: {}: {} passes in {:.1} s; heard {heard:?}",
                    if high { "high" } else { "clear" },
                    slow.session,
                    lab.why_no_high(),
                    lab.passes,
                    lab.secs
                );
            }
            tick.tick().await;
            *round += 1;
            lab.passes += 1;
            live.pass(*round, FIGURES).await;
            if high && base > 0 {
                pace(&slow.rate, base, started, lab.passes);
            }
            let rows = live.viewers().await;
            queued = row_of(&rows, &slow.session).map_or(0, |r| num(r, "bytes"));
            samples.push((Instant::now(), rows));
        }
        settle(&mut lab, queued);
        // GH #1080: the reader stays paced for `after` too. Released to the
        // rate it had at the level while the app starved further, it drained
        // the queue and T5's last sample showed 0 bytes for the slow viewer.
        let end = Instant::now() + after;
        let mut passes = lab.passes;
        while Instant::now() < end {
            tick.tick().await;
            *round += 1;
            passes += 1;
            live.pass(*round, FIGURES).await;
            if high && base > 0 {
                pace(&slow.rate, base, started, passes);
            }
            samples.push((Instant::now(), live.viewers().await));
        }
        (samples, lab)
    }

    /// How the app sizes its next tranche.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum App {
        /// Always the full tranche; hears nothing.
        Deaf,
        /// The plan's app (W6 § 4, A9), word for word: starts with the full
        /// tranche, halves it on every `high` it hears, doubles it on every
        /// `clear` (never past the full tranche, never below one figure).
        Plan,
        /// The first builder's app, kept as a second, named
        /// line of numbers only: starts at an eighth, halves every pass while
        /// the last report was `high`, doubles once a second while `clear`
        /// until the first `high` and grows by a quarter a second after it.
        Stateful,
    }

    /// When a lab run ends.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Until {
        /// After this long.
        After(Duration),
        /// As soon as the slow viewer was reported `high` (30 s marker).
        High,
        /// As soon as a `clear` followed a `high` (60 s marker).
        HighThenClear,
    }

    /// What one lab run measured.
    #[derive(Debug, Default)]
    pub struct Lab {
        /// App passes made.
        pub passes: u64,
        /// Wall time of the run, seconds.
        pub secs: f64,
        /// The slow viewer's `resyncs_total` at the end.
        pub resyncs: u64,
        /// The slow viewer's oldest outstanding frame, sampled every pass:
        /// 95th percentile and maximum, ms.
        pub p95_ms: u64,
        /// See [`Lab::p95_ms`].
        pub max_ms: u64,
        /// Frames per second each fast viewer read.
        pub fast_fps: f64,
        /// The most bytes a fast viewer ever had outstanding.
        pub fast_max_bytes: u64,
        /// The mean size of a frame on the slow viewer's queue.
        pub frame_bytes: u64,
        /// Backlog reports for the slow viewer: `high`, `clear`.
        pub highs: usize,
        /// See [`Lab::highs`].
        pub clears: usize,
        /// Reports for the fast viewers (should be none).
        pub fast_reports: usize,
        /// Whether a `clear` came after the first `high`.
        pub cleared_after_high: bool,
        /// From the cell's first `high`: how long after the crossing it was
        /// reported, by the cell's own clock (`oldest_ms − backlog_high_ms`
        /// when age crossed first; 0 when bytes crossed first, because the
        /// enqueue that crossed is the check that reported).
        pub first_high_lag_ms: Option<u64>,
        /// Seconds into the run of the cell's first `high` for the slow
        /// viewer: where the backlog window of T3 begins (W6 review N1).
        pub first_high_secs: Option<f64>,
        /// The tranche after each reaction of the app: (heard `high`, new k).
        pub reactions: Vec<(bool, usize)>,
        /// GH #1067: the mean time between two app passes, ms (nominal
        /// [`PASS`]; a starved app is slower).
        pub pass_ms: u64,
        /// GH #1067: bytes per second the app wrote to each viewer, as the
        /// fast readers took them.
        pub wrote_bps: u64,
        /// GH #1067: the slow reader's rate at the end, bytes per second.
        pub reader_bps: u64,
    }

    impl Lab {
        /// GH #1067: why a run that waited for `high` never heard one.
        ///
        /// A strand gate failed with "no high in 30 s" after 72 passes in
        /// 30 s (415 ms per pass, nominal 125): a full tranche (~23 KB) every
        /// 415 ms is ~55 KB/s against a 50 KB/s reader, so the slow viewer
        /// was never behind. Before this the message showed `secs: 0.0`,
        /// `fast_fps: 0.0` and `frame_bytes: 0` because they were computed
        /// after the loop, which read as "no frame reached any viewer".
        pub fn why_no_high(&self) -> String {
            if self.wrote_bps == 0 {
                "no viewer read a single frame".to_string()
            } else if self.wrote_bps < 2 * self.reader_bps {
                format!(
                    "the app never outran the slow reader: it wrote {} B/s per viewer \
                     at {} ms per pass (nominal {} ms) against a reader of {} B/s -- \
                     a starved app, not a deaf cell",
                    self.wrote_bps,
                    self.pass_ms,
                    PASS.as_millis(),
                    self.reader_bps
                )
            } else {
                format!(
                    "the app wrote {} B/s per viewer against a reader of {} B/s and \
                     the cell reported nothing",
                    self.wrote_bps, self.reader_bps
                )
            }
        }
    }

    /// One lab run: two fast viewers, optionally a slow one, and `app`.
    pub async fn lab(slow: bool, app: App, until: Until) -> Lab {
        lab_paced(slow, app, until, PASS).await
    }

    /// [`lab`] with the app's pass interval set to `pass` -- a starved app
    /// (GH #1067: 415 ms per pass in a strand gate under full-suite load).
    pub async fn lab_paced(slow: bool, app: App, until: Until, pass: Duration) -> Lab {
        let mut live = start(opted_in()).await;
        let a = join(live.port, 0).await;
        let b = join(live.port, 0).await;
        let s = if slow {
            Some(join(live.port, SLOW).await)
        } else {
            None
        };
        let mut k = if app == App::Stateful {
            FIGURES / 8
        } else {
            FIGURES
        };
        let mut lab = Lab::default();
        let mut level_high = false;
        let mut was_high = false;
        let mut seen = 0;
        let mut grown = Instant::now();
        let mut ages = Vec::new();
        let mut slow_frames = 0u64;
        let mut slow_bytes = 0u64;
        let fast_before = a.frames.load(Ordering::Relaxed) + b.frames.load(Ordering::Relaxed);
        let fast_bytes_before = a.bytes.load(Ordering::Relaxed) + b.bytes.load(Ordering::Relaxed);
        let started = Instant::now();
        let mut tick = tokio::time::interval(pass);
        let settle = |lab: &mut Lab, ages: &mut Vec<u64>, slow_frames: u64, slow_bytes: u64| {
            lab.secs = started.elapsed().as_secs_f64();
            let fast =
                a.frames.load(Ordering::Relaxed) + b.frames.load(Ordering::Relaxed) - fast_before;
            lab.fast_fps = fast as f64 / 2.0 / lab.secs;
            let wrote = a.bytes.load(Ordering::Relaxed) + b.bytes.load(Ordering::Relaxed)
                - fast_bytes_before;
            lab.wrote_bps = (wrote as f64 / 2.0 / lab.secs) as u64;
            lab.pass_ms = (lab.secs * 1000.0 / lab.passes.max(1) as f64) as u64;
            lab.reader_bps = s.as_ref().map_or(0, |s| s.rate.load(Ordering::Relaxed));
            lab.frame_bytes = slow_bytes.checked_div(slow_frames).unwrap_or(0);
            ages.sort_unstable();
            lab.p95_ms = ages
                .get((ages.len() * 95 / 100).min(ages.len().saturating_sub(1)))
                .copied()
                .unwrap_or(0);
            lab.max_ms = ages.last().copied().unwrap_or(0);
        };
        loop {
            let marker = match until {
                Until::After(d) if started.elapsed() >= d => break,
                Until::High if lab.highs > 0 => break,
                Until::HighThenClear if lab.cleared_after_high => break,
                Until::High => Some(("no high in 30 s", Duration::from_secs(30))),
                Until::HighThenClear => {
                    Some(("no high then clear in 60 s", Duration::from_secs(60)))
                }
                Until::After(_) => None,
            };
            if let Some((what, limit)) = marker
                && started.elapsed() >= limit
            {
                settle(&mut lab, &mut ages, slow_frames, slow_bytes);
                panic!("{what}: {}: {lab:?}", lab.why_no_high());
            }
            tick.tick().await;
            let heard = live.heard().await;
            for h in &heard[seen..] {
                if s.as_ref().is_some_and(|s| s.session == h.session) {
                    if h.high {
                        if lab.highs == 0 {
                            lab.first_high_lag_ms = Some(h.oldest_ms.saturating_sub(HIGH_MS));
                            lab.first_high_secs = Some(started.elapsed().as_secs_f64());
                        }
                        lab.highs += 1;
                    } else {
                        lab.clears += 1;
                        lab.cleared_after_high |= lab.highs > 0;
                    }
                    if app == App::Plan {
                        k = if h.high {
                            (k / 2).max(1)
                        } else {
                            (k * 2).min(FIGURES)
                        };
                        lab.reactions.push((h.high, k));
                    }
                    level_high = h.high;
                    was_high |= h.high;
                } else {
                    lab.fast_reports += 1;
                }
            }
            seen = heard.len();
            if app == App::Stateful {
                if level_high {
                    k = (k / 2).max(8);
                } else if grown.elapsed() >= Duration::from_secs(1) {
                    k = if was_high { k + k / 4 } else { k * 2 }.min(FIGURES);
                    grown = Instant::now();
                }
            }
            lab.passes += 1;
            live.pass(lab.passes, k).await;
            // GH #1067: a run that waits for `high` holds the slow reader to
            // the app's real pace (see [`pace`]). A measuring run
            // (`Until::After`) keeps the fixed rate: its numbers are a
            // phone's, not a ratio.
            if !matches!(until, Until::After(_))
                && let Some(s) = s.as_ref()
            {
                pace(&s.rate, SLOW, started, lab.passes);
            }
            let rows = live.viewers().await;
            for r in &rows {
                if s.as_ref()
                    .is_some_and(|s| r["session_id"] == json!(s.session))
                {
                    ages.push(num(r, "oldest_ms"));
                    lab.resyncs = num(r, "resyncs_total");
                    slow_frames += num(r, "frames");
                    slow_bytes += num(r, "bytes");
                } else {
                    lab.fast_max_bytes = lab.fast_max_bytes.max(num(r, "bytes"));
                }
            }
        }
        settle(&mut lab, &mut ages, slow_frames, slow_bytes);
        for r in [Some(a), Some(b), s].into_iter().flatten() {
            r.task.abort();
        }
        live.join.abort();
        lab
    }
}

/// GH #1004: one tool-call bundle as the message the lab's `call` sends.
pub fn bundle_message(ops: Vec<Value>) -> meclaw_core::Message {
    let turns: Vec<Value> = ops
        .into_iter()
        .enumerate()
        .map(|(i, op)| leg(i, op))
        .collect();
    MessageBuilder::new(Path::new(CELL))
        .reply_to(Path::new(REPLY_TO))
        .body(Body::Inline(json!({ "messages": turns })))
        .build()
}

/// GH #1004: whether an emission is a semantic browser event (`hop.route =
/// "event"`) rather than a bundle's answer.
pub fn is_event(emission: &CellEmission) -> bool {
    emission.content["header"]["route"] == json!("event")
}

/// GH #1020: the address every lab message names as its `reply_to`.
pub const REPLY_TO: &str = "/caller";

/// GH #1020: whether `emission` is the cell's answer to the message `id` —
/// addressed to the lab's `reply_to` and parented on that message.
pub fn is_reply_to(emission: &CellEmission, id: meclaw_core::Uuid) -> bool {
    emission.target.as_str() == REPLY_TO && emission.parent_message_id == Some(id)
}

/// GH #1020: whether `emission` is something the cell said — an answer to the
/// lab or a semantic event — rather than bookkeeping a cell task puts on the
/// same output channel.
pub fn cell_spoke(emission: &CellEmission) -> bool {
    emission.target.as_str() == REPLY_TO || is_event(emission)
}

/// GH #1004: queue bundles without waiting for their answers, and read the
/// cell's emissions (answers and semantic events) one by one. Added beside
/// `call`, which reads one emission per bundle and so cannot be mixed with
/// browser events on the same lab.
impl Lab {
    /// Put one bundle into the cell's mailbox and return at once.
    pub async fn enqueue(&self, ops: Vec<Value>) {
        self.sender
            .send(bundle_message(ops))
            .await
            .expect("the cell takes messages");
    }

    /// A handle on the mailbox, for a task that writes at its own cadence.
    pub fn mailbox(&self) -> mpsc::Sender<meclaw_core::Message> {
        self.sender.clone()
    }

    /// The next emission, within the failure-marker window.
    pub async fn next_emission(&mut self) -> CellEmission {
        loop {
            let e = tokio::time::timeout(Duration::from_secs(30), self.out_rx.recv())
                .await
                .expect("an emission within the failure-marker window")
                .expect("the cell is alive");
            if cell_spoke(&e) {
                return e;
            }
        }
    }

    /// Every emission already waiting, without waiting for more.
    pub fn emitted_so_far(&mut self) -> Vec<CellEmission> {
        let mut out = Vec::new();
        while let Ok(e) = self.out_rx.try_recv() {
            if cell_spoke(&e) {
                out.push(e);
            }
        }
        out
    }

    /// Take the emission stream away from the lab, for a reader task. `call`
    /// and `next_emission` see nothing afterwards.
    pub fn take_emissions(&mut self) -> mpsc::Receiver<CellEmission> {
        std::mem::replace(&mut self.out_rx, mpsc::channel(1).1)
    }
}

/// GH #1004: a browser event from this socket, and every frame read raw.
impl Viewer {
    /// Push one `event` frame (`phx-click` shape) under `msg_ref`.
    pub async fn push_event(&mut self, msg_ref: &str, name: &str, value: Value) {
        let frame = json!(["1", msg_ref, self.topic.clone(), "event", {
            "type": "click",
            "event": name,
            "value": value,
        }]);
        self.send(frame).await;
    }

    /// The next frame of any kind (a `diff`, a `phx_reply`, …), counted like
    /// `next_diff` counts when it is a diff.
    pub async fn next_frame(&mut self) -> Value {
        let (n, frame) = self.recv_raw().await;
        if frame[3] == json!("diff") {
            self.frames += 1;
            self.bytes += n;
        }
        frame
    }
}

/// The colony log of a `web` cell by class (GH #1005, moved here from the W5
/// test per OR-H4-15): what a bundle's request, its answer and the cell's
/// events cost in the log, read the way an operator reads it.
pub mod log_lab {
    use meclaw_core::serde_json::json;
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// Bytes of the log, by class. A row counts what the log stores of it: ids,
    /// paths, headers and body.
    #[derive(Default, Debug, Clone, Copy)]
    pub struct LogClasses {
        pub request: u64,
        pub answer: u64,
        pub event: u64,
        pub other: u64,
        pub rows: u64,
    }

    impl LogClasses {
        pub fn total(&self) -> u64 {
            self.request + self.answer + self.event + self.other
        }
    }

    /// The colony's own `message_log`, through its read message (`GET
    /// /colony/messages` of a running meclaw) — never the database file: a test
    /// reads the log the way an operator does (`display_colony.rs` `log`).
    pub async fn read_classes(
        inbox: &mpsc::Sender<meclaw_colony::ColonyMsg>,
        web: &str,
        caller: &str,
    ) -> LogClasses {
        use meclaw_colony::api_dto::MessageLogFilter;
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        inbox
            .send(meclaw_colony::ColonyMsg::ReadMessages {
                filter: MessageLogFilter {
                    limit: 1000,
                    scan_budget: 50_000,
                    ..Default::default()
                },
                ack: ack_tx,
            })
            .await
            .expect("inbox alive");
        let reply = tokio::time::timeout(Duration::from_secs(30), ack_rx)
            .await
            .expect("the log answers")
            .expect("ack");
        assert!(!reply.scan_truncated, "the lab outgrew the scan budget");
        assert!(
            reply.entries.len() < 1000,
            "the lab outgrew one page of the log"
        );
        let mut c = LogClasses::default();
        for e in &reply.entries {
            // What the log stores of a row: ids, paths, headers, body, two ints.
            let bytes = (e.id.len()
                + e.trace_id.len()
                + e.parent_message_id.as_deref().map_or(0, str::len)
                + e.correlation_id.as_deref().map_or(0, str::len)
                + e.from_path.len()
                + e.to_path.len()
                + e.reply_to.as_deref().map_or(0, str::len)
                + e.headers_json.len()
                + e.body_kind.len()
                + e.body_payload.as_deref().map_or(0, str::len)
                + 16) as u64;
            c.rows += 1;
            if e.to_path == web {
                c.request += bytes;
            } else if e.from_path == web && e.to_path == caller {
                c.answer += bytes;
            } else if e.from_path == web && e.headers_json.contains("\"event\"") {
                c.event += bytes;
            } else {
                c.other += bytes;
            }
        }
        c
    }

    pub fn file_bytes(dir: &std::path::Path) -> u64 {
        ["colony.db", "colony.db-wal"]
            .iter()
            .map(|f| std::fs::metadata(dir.join(f)).map(|m| m.len()).unwrap_or(0))
            .sum()
    }

    pub fn seed_figures(cell_dir: &std::path::Path, n: usize) {
        let seed = cell_dir.join("seed");
        std::fs::create_dir_all(&seed).expect("seed dir");
        std::fs::write(
            seed.join("components.jsonl"),
            concat!(
                r#"{"schema":{"name":"text","template":"text","prop_schema":"text","editable":"text","layer":"text"}}"#,
                "\n",
                r#"{"name":"screen","template":"<main>{{children}}</main>","prop_schema":"{}","editable":"[]","layer":"content"}"#,
                "\n",
                r#"{"name":"fig","template":"<i data-x=\"{{x}}\" data-y=\"{{y}}\"></i>","prop_schema":"{\"x\":\"text\",\"y\":\"text\"}","editable":"[]","layer":"content"}"#,
                "\n"
            ),
        )
        .expect("components");
        let mut objects = String::from(
            r#"{"schema":{"id":"text","parent":"text","component":"text","ord":"int","props":"text"}}"#,
        );
        objects.push('\n');
        objects
            .push_str(r#"{"id":"root","parent":null,"component":"screen","ord":0,"props":"{}"}"#);
        objects.push('\n');
        for i in 0..n {
            objects.push_str(
                &json!({"id": format!("fig-{i}"), "parent": "root", "component": "fig", "ord": i, "props": r#"{"x":"0","y":"0"}"#})
                    .to_string(),
            );
            objects.push('\n');
        }
        std::fs::write(seed.join("objects.jsonl"), objects).expect("objects");
        std::fs::write(
            seed.join("pages.jsonl"),
            concat!(
                r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
                "\n",
                r#"{"route":"/","root":"root","title":"Lab"}"#,
                "\n"
            ),
        )
        .expect("pages");
    }
}

/// GH #1013 (C1 of the review): the lab with the fan-out held in the test's
/// hand.
///
/// A real `web` cell publishes its pages BEFORE its diffs reach the fan-out;
/// in between, a join reads a snapshot that already holds the write, and the
/// write's diff follows it. That window is a few microseconds on an idle cell
/// and as long as the push backlog on a busy one, so the lock cannot wait for
/// it: here the cell's push channel ends in the test, and a write's pushes
/// reach the I/O half only when the test hands them over ([`HeldLab::release`]).
/// Same rows as [`Lab::start`] (from [`memory_db`]), same mount and cell path,
/// so [`viewer_on`] and the GET helpers work unchanged.
pub mod held {
    use super::{CELL, MOUNT, Shape, Viewer, leg, memory_db, viewer_on};
    use meclaw_cells::web::AssetMap;
    use meclaw_cells::web::cell::{WebCell, WebReconfig};
    use meclaw_cells::web::io::{WebIo, run_io};
    use meclaw_cells::web::params::WebParams;
    use meclaw_cells::web::render::materialize_all;
    use meclaw_colony::{DbConn, LongRunningCell, SurfaceRegistry};
    use meclaw_core::serde_json::{Value, json};
    use meclaw_core::{Body, CellEmission, Headers, MessageBuilder, OutputSink, Path, Uuid};
    use meclaw_testing::{surface_listener, wait_for_mount};
    use std::sync::Arc;
    use tokio::sync::{mpsc, watch};

    /// The cell, its database, the pushes it sent, and the I/O half's way in.
    pub struct HeldLab {
        /// The port of the listener in front of the I/O half.
        pub port: u16,
        cell: WebCell,
        db: DbConn,
        /// What the cell pushed — held until [`HeldLab::release`].
        pushes: mpsc::Receiver<WebReconfig>,
        /// Into the I/O half's fan-out.
        fan_out: mpsc::Sender<WebReconfig>,
        _assets: watch::Sender<Arc<AssetMap>>,
        _ready: watch::Sender<bool>,
        _reconfig: mpsc::Sender<WebReconfig>,
        _events: mpsc::Receiver<meclaw_cells::web::cell::WebEvent>,
        _listener: tokio::task::JoinHandle<()>,
        io: tokio::task::JoinHandle<()>,
    }

    impl Drop for HeldLab {
        fn drop(&mut self) {
            self.io.abort();
        }
    }

    impl HeldLab {
        /// Seed, publish, serve. `params` are the cell's (the mount is set
        /// here); the join piece limit is the I/O half's default unless
        /// `join_chunk_bytes` names one.
        pub async fn start(shape: Shape, params: Value) -> HeldLab {
            let conn = memory_db(shape);
            let boot = materialize_all(&conn).expect("the lab renders");
            let (pages_tx, pages_rx) = watch::channel(Arc::new(boot));
            let (assets_tx, assets_rx) = watch::channel(Arc::new(AssetMap::new()));
            let (ready_tx, ready_rx) = watch::channel(true);
            let (push_tx, pushes) = mpsc::channel::<WebReconfig>(64);
            let (fan_out, fan_out_rx) = mpsc::channel::<WebReconfig>(64);
            let mut params = params;
            params["mount"] = json!(MOUNT);
            let params = WebParams::parse(&params).expect("params");
            let surfaces = Arc::new(SurfaceRegistry::new());
            let mut io = WebIo::new(
                MOUNT.to_string(),
                String::new(),
                CELL,
                pages_rx,
                assets_rx,
                ready_rx,
                fan_out_rx,
                Arc::clone(&surfaces),
            );
            io.join_chunk = params.join_chunk();
            let cell = WebCell::new(
                CELL.to_string(),
                io.clone(),
                &params,
                pages_tx,
                assets_tx.clone(),
                ready_tx.clone(),
                push_tx,
            );
            let (events_tx, events_rx) = mpsc::channel(64);
            let (reconfig_tx, reconfig_rx) = mpsc::channel(8);
            let io_task = tokio::spawn(run_io(io, events_tx, reconfig_rx));
            wait_for_mount(&surfaces, MOUNT).await;
            let (addr, listener) = surface_listener(surfaces).await;
            HeldLab {
                port: addr.port(),
                cell,
                db: DbConn::wrap(conn, None),
                pushes,
                fan_out,
                _assets: assets_tx,
                _ready: ready_tx,
                _reconfig: reconfig_tx,
                _events: events_rx,
                _listener: listener,
                io: io_task,
            }
        }

        /// Apply one bundle of tool-call ops: its pages are published, its
        /// pushes are held and returned — the window a join can fall into.
        pub async fn write(&mut self, ops: Vec<Value>) -> Vec<WebReconfig> {
            let turns: Vec<Value> = ops
                .into_iter()
                .enumerate()
                .map(|(i, op)| leg(i, op))
                .collect();
            let msg = MessageBuilder::new(Path::new(CELL))
                .reply_to(Path::new("/caller"))
                .body(Body::Inline(json!({ "messages": turns })))
                .build();
            let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(64);
            let sink = OutputSink::new(
                out_tx,
                Path::new(CELL),
                Uuid::now_v7(),
                Uuid::now_v7(),
                64,
                Headers::new(),
                None,
            );
            let (rc_tx, _rc_rx) = mpsc::channel(8);
            self.cell.handle(msg, &sink, &mut self.db, &rc_tx).await;
            let answer = out_rx.try_recv().expect("the cell answers").content;
            assert!(
                !answer.to_string().contains("\"error_code\""),
                "the write was refused: {answer}"
            );
            let mut held = Vec::new();
            while let Ok(push) = self.pushes.try_recv() {
                held.push(push);
            }
            held
        }

        /// Hand held pushes to the fan-out, in order.
        pub async fn release(&self, pushes: Vec<WebReconfig>) {
            for p in pushes {
                self.fan_out.send(p).await.expect("the fan-out is alive");
            }
        }

        /// Open a socket and join `/`.
        pub async fn viewer(&self) -> Viewer {
            viewer_on(self.port, "/").await
        }

        /// The whole body of `/` as the database holds it now, rendered
        /// afresh. A GET of a page above the join cut (GH #1002) serves only
        /// its first picture, so a join in pieces is compared with this.
        pub async fn whole_body(&mut self) -> String {
            self.db
                .call(|conn| meclaw_cells::web::render::materialize(conn, "/"))
                .await
                .expect("the lab renders")
                .rendered_body()
        }

        /// The page as served on a GET (shell included).
        pub async fn get_page(&self) -> String {
            reqwest::get(format!("http://127.0.0.1:{}/{MOUNT}/", self.port))
                .await
                .expect("get")
                .text()
                .await
                .expect("text")
        }
    }
}
