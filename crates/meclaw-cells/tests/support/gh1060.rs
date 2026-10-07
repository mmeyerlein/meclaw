//! GH #1060 — shared harness of the `gh1060_*` locks: a counting HTTP stub and
//! a cell driven through its real factory and dispatcher.
//!
//! The stub runs on std threads (one per connection) so it needs nothing from
//! the runtime the cell runs on, and it counts what the locks are about: the
//! `Authorization` header of every request and how many requests were in
//! flight at once. A `std::sync::Mutex` in a TEST stub; the no-lock rule is
//! about cell state.

#![allow(dead_code)]

use meclaw_colony::{CellFactory, ContractView, SpawnedCellKind};
use meclaw_core::serde_json::{self, Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path, Uuid};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// What the stub saw.
#[derive(Default)]
pub struct Seen {
    /// The `Authorization` header of each request, in arrival order (`None` =
    /// no header at all).
    pub auth: Vec<Option<String>>,
    in_flight: usize,
    /// The most requests that were inside the stub at the same time.
    pub max_in_flight: usize,
}

/// A loopback HTTP stub that answers every request with `body` after `delay`.
pub struct Stub {
    pub addr: SocketAddr,
    seen: Arc<Mutex<Seen>>,
}

impl Stub {
    pub fn start(delay: Duration, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let addr = listener.local_addr().expect("stub address");
        let seen = Arc::new(Mutex::new(Seen::default()));
        let shared = Arc::clone(&seen);
        let body = Arc::new(body);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let seen = Arc::clone(&shared);
                let body = Arc::clone(&body);
                std::thread::spawn(move || serve(conn, &seen, delay, &body));
            }
        });
        Self { addr, seen }
    }

    pub fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    pub fn auth(&self) -> Vec<Option<String>> {
        self.seen.lock().unwrap().auth.clone()
    }

    pub fn requests(&self) -> usize {
        self.seen.lock().unwrap().auth.len()
    }

    pub fn max_in_flight(&self) -> usize {
        self.seen.lock().unwrap().max_in_flight
    }
}

fn serve(mut conn: TcpStream, seen: &Mutex<Seen>, delay: Duration, body: &str) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut auth = None;
    let mut length = 0usize;
    for line in head.lines().skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            if k == "authorization" {
                auth = Some(v.trim().to_string());
            } else if k == "content-length" {
                length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    while buf.len() < head_end + length {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    {
        let mut s = seen.lock().unwrap();
        s.auth.push(auth);
        s.in_flight += 1;
        s.max_in_flight = s.max_in_flight.max(s.in_flight);
    }
    std::thread::sleep(delay);
    seen.lock().unwrap().in_flight -= 1;
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = conn.write_all(reply.as_bytes());
}

/// A cell spawned through its factory: the mailbox the dispatcher reads and
/// the emissions it writes.
pub struct Running {
    pub tx: mpsc::Sender<Message>,
    out: mpsc::Receiver<CellEmission>,
    _spawned: SpawnedCellKind,
    _dir: tempfile::TempDir,
}

pub fn spawn(factory: Arc<dyn CellFactory>, params: Value) -> Running {
    spawn_with_message_timeout(factory, params, None)
}

/// [`spawn`] with the cell's `message_timeout` backstop set, the way the colony
/// passes `cell.message_timeout` (GH #1061: the ticket wait counts against it).
pub fn spawn_with_message_timeout(
    factory: Arc<dyn CellFactory>,
    params: Value,
    message_timeout: Option<Duration>,
) -> Running {
    let (out_tx, out) = mpsc::channel(1024);
    let (inbox_tx, mut inbox_rx) = mpsc::channel(1024);
    // The colony's side of the inbox: drained, so no worker ever waits on it.
    tokio::spawn(async move { while inbox_rx.recv().await.is_some() {} });
    let dir = tempfile::TempDir::new().expect("tempdir");
    let spawned = factory
        .spawn_cell(
            Path::new("/c"),
            params,
            out_tx,
            dir.path().to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            0,
            message_timeout,
            None,
            1024,
        )
        .expect("the cell spawns");
    let tx = match &spawned {
        SpawnedCellKind::Active { sender, .. } => sender.clone(),
        _ => panic!("a stateless cell spawns active"),
    };
    Running {
        tx,
        out,
        _spawned: spawned,
        _dir: dir,
    }
}

impl Running {
    pub async fn send(&self, body: Value) {
        let msg = MessageBuilder::new(Path::new("/c"))
            .reply_to(Path::new("/sink"))
            .trace_id(Uuid::now_v7())
            .body(Body::Inline(body))
            .build();
        self.tx.send(msg).await.expect("the mailbox is open");
    }

    /// The next emission's content, skipping the dispatcher's consumption
    /// marks. Panics after 30 s (failure marker, not a timing claim).
    pub async fn next(&mut self) -> Value {
        loop {
            let e = tokio::time::timeout(Duration::from_secs(30), self.out.recv())
                .await
                .expect("an emission within 30 s")
                .expect("the emission channel is open");
            if e.target.as_str() != meclaw_core::CONSUMED_MARK_TARGET {
                return e.content;
            }
        }
    }

    /// Everything emitted within `window` (the quiet period after which a lock
    /// says "and nothing else came").
    pub async fn drain(&mut self, window: Duration) -> Vec<Value> {
        let mut all = Vec::new();
        while let Ok(Some(e)) = tokio::time::timeout(window, self.out.recv()).await {
            if e.target.as_str() != meclaw_core::CONSUMED_MARK_TARGET {
                all.push(e.content);
            }
        }
        all
    }
}

pub fn is_credential_request(content: &Value) -> bool {
    content["header"]["route"] == "credential_request"
}

/// The recipient key a `credential_request` emission carries.
pub fn recipient_of(request: &Value) -> String {
    let text = request["messages"][0]["text"]
        .as_str()
        .expect("the request carries its args as text");
    let args: Value = serde_json::from_str(text).expect("args are JSON");
    args["payload"]["recipient_key"]
        .as_str()
        .expect("payload.recipient_key")
        .to_string()
}

/// The box a vault would deliver: `secret` sealed to `recipient`.
pub fn sealed_box(recipient: &str, secret: &str) -> Value {
    let sealed = meclaw_cells::sealed::seal_to(recipient, secret.as_bytes()).expect("seal");
    json!({"sealed": sealed.to_json()})
}

/// A tool call the way a model's tool loop sends it.
pub fn tool_call(args: Value, id: &str) -> Value {
    json!({"messages": [{
        "origin": "assistant", "type": "tool_call",
        "text": args.to_string(), "id": id
    }]})
}

pub fn error_code(content: &Value) -> Option<&str> {
    content["header"]["error_code"].as_str()
}
