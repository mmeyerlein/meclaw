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
                json!({ "mount": MOUNT }),
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
        let turns: Vec<Value> = ops
            .into_iter()
            .enumerate()
            .map(|(i, op)| leg(i, op))
            .collect();
        let msg = MessageBuilder::new(Path::new(CELL))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({ "messages": turns })))
            .build();
        self.sender
            .send(msg)
            .await
            .expect("the cell takes messages");
        let emission = tokio::time::timeout(Duration::from_secs(30), self.out_rx.recv())
            .await
            .expect("the cell answers within the failure-marker window")
            .expect("the cell is alive");
        emission.content
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
        let body = self.get_page_at(route).await;
        let marker = "data-phx-session=\"";
        let start = body
            .find(marker)
            .expect("the shell carries a session token")
            + marker.len();
        let end = start + body[start..].find('"').expect("token is quoted");
        let token = body[start..end].to_string();
        let container = meclaw_surface::session::container_id(CELL);
        let topic = format!("lv:{container}");
        let (ws, _) = tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{}/{MOUNT}/live/websocket",
            self.port
        ))
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
            "url": self.url_of(route)
        }]))
        .await;
        let (n, reply) = v.recv_raw().await;
        assert_eq!(reply[4]["status"], json!("ok"), "join reply: {reply}");
        v.join_bytes = n;
        v.rendered = reply[4]["response"]["rendered"].clone();
        v
    }
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
    use meclaw_testing::{surface_listener, wait_for_mount};
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

    fn seed(cell_dir: &std::path::Path) {
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
        std::fs::write(
            seed.join("pages.jsonl"),
            concat!(
                r#"{"schema":{"route":"text","root":"text","title":"text"}}"#,
                "\n",
                r#"{"route":"/","root":"root","title":"Home"}"#,
                "\n"
            ),
        )
        .expect("pages");
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
        /// Every emission that is not a reply: the log-growth witness.
        pub emitted: Arc<AtomicU64>,
        _stop: tokio::sync::oneshot::Sender<()>,
        /// The cell task.
        pub join: tokio::task::JoinHandle<()>,
    }

    /// Spawn the cell with `params` and wait until it serves its page.
    pub async fn start(params: Value) -> Live {
        let td = TempDir::new().expect("td");
        let cell_dir = td.path().join("web");
        std::fs::create_dir_all(&cell_dir).expect("dir");
        seed(&cell_dir);
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
        let emitted = Arc::new(AtomicU64::new(0));
        let (reply_tx, replies) = mpsc::channel(64);
        {
            let heard = Arc::clone(&heard);
            let emitted = Arc::clone(&emitted);
            tokio::spawn(async move {
                while let Some(e) = out_rx.recv().await {
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
                        }
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
        /// Bytes per second the reader takes; 0 is unthrottled.
        pub rate: Arc<AtomicU64>,
        /// The reading task.
        pub task: tokio::task::JoinHandle<()>,
    }

    async fn token(port: u16) -> String {
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
        let rate = Arc::new(AtomicU64::new(rate));
        let task = {
            let (frames, rate) = (Arc::clone(&frames), Arc::clone(&rate));
            tokio::spawn(async move {
                let mut taken: u64 = 0;
                let mut since = Instant::now();
                while let Some(Ok(m)) = ws.next().await {
                    frames.fetch_add(1, Ordering::Relaxed);
                    let r = rate.load(Ordering::Relaxed);
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
            rate,
            task,
        }
    }

    /// Push full tranches at the app's cadence for `d`, sampling `viewers`
    /// every pass. Returns the samples `(at, rows)`.
    pub async fn drive(
        live: &mut Live,
        d: Duration,
        round: &mut u64,
    ) -> Vec<(Instant, Vec<Value>)> {
        let mut samples = Vec::new();
        let end = Instant::now() + d;
        let mut tick = tokio::time::interval(PASS);
        while Instant::now() < end {
            tick.tick().await;
            *round += 1;
            live.pass(*round, FIGURES).await;
            samples.push((Instant::now(), live.viewers().await));
        }
        samples
    }

    /// Drive full tranches until `session` has been reported at `level`
    /// (`high` = true), then `after` more. Panics after 30 s: a failure
    /// marker, not a measurement.
    pub async fn drive_until(
        live: &mut Live,
        session: &str,
        high: bool,
        after: Duration,
        round: &mut u64,
    ) -> Vec<(Instant, Vec<Value>)> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut samples = Vec::new();
        loop {
            samples.extend(drive(live, Duration::from_millis(500), round).await);
            if live
                .heard()
                .await
                .iter()
                .any(|h| h.session == session && h.high == high)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "no {} for {session} in 30 s: {:?}",
                if high { "high" } else { "clear" },
                live.heard().await
            );
        }
        samples.extend(drive(live, after, round).await);
        samples
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
        /// The tranche after each reaction of the app: (heard `high`, new k).
        pub reactions: Vec<(bool, usize)>,
    }

    /// One lab run: two fast viewers, optionally a slow one, and `app`.
    pub async fn lab(slow: bool, app: App, until: Until) -> Lab {
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
        let started = Instant::now();
        let mut tick = tokio::time::interval(PASS);
        loop {
            match until {
                Until::After(d) if started.elapsed() >= d => break,
                Until::High if lab.highs > 0 => break,
                Until::HighThenClear if lab.cleared_after_high => break,
                Until::High => assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "no high in 30 s: {lab:?}"
                ),
                Until::HighThenClear => assert!(
                    started.elapsed() < Duration::from_secs(60),
                    "no high then clear in 60 s: {lab:?}"
                ),
                Until::After(_) => {}
            }
            tick.tick().await;
            let heard = live.heard().await;
            for h in &heard[seen..] {
                if s.as_ref().is_some_and(|s| s.session == h.session) {
                    if h.high {
                        if lab.highs == 0 {
                            lab.first_high_lag_ms = Some(h.oldest_ms.saturating_sub(HIGH_MS));
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
        lab.secs = started.elapsed().as_secs_f64();
        let fast =
            a.frames.load(Ordering::Relaxed) + b.frames.load(Ordering::Relaxed) - fast_before;
        lab.fast_fps = fast as f64 / 2.0 / lab.secs;
        lab.frame_bytes = slow_bytes.checked_div(slow_frames).unwrap_or(0);
        ages.sort_unstable();
        lab.p95_ms = ages
            .get((ages.len() * 95 / 100).min(ages.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0);
        lab.max_ms = ages.last().copied().unwrap_or(0);
        for r in [Some(a), Some(b), s].into_iter().flatten() {
            r.task.abort();
        }
        live.join.abort();
        lab
    }
}
