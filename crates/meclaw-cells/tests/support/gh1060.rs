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
use std::sync::{Arc, Condvar, Mutex};
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
    /// The gate of [`Stub::start_gated`]: open, a request is answered at once;
    /// closed, it waits inside the stub.
    closed: bool,
    /// More requests than the gate's bound were inside at once: the gate
    /// opened itself, the run goes on and ends red on `max_in_flight`.
    broken: bool,
}

/// A loopback HTTP stub that answers every request with `body`.
pub struct Stub {
    pub addr: SocketAddr,
    seen: Arc<Mutex<Seen>>,
    met: Arc<Condvar>,
    hold: Hold,
}

/// How the stub holds a request before it answers.
#[derive(Clone, Copy)]
enum Hold {
    /// Not at all.
    Never,
    /// Until `k` requests have been inside the stub at the same time, at most
    /// the scaled hang mark (tv-br M10o): overlap shown by the event itself,
    /// never by a delay window that a host under a CPU quota outran.
    UntilOverlap(usize),
    /// While the gate is closed, until the test opens it -- or until more than
    /// `bound` requests are inside at once (re-check R K7, see
    /// [`Stub::start_gated`]).
    Gate(usize),
}

impl Stub {
    /// A stub that answers every request at once.
    pub fn start(body: String) -> Self {
        Self::start_holding(Hold::Never, body)
    }

    /// A stub that holds every request until `k` of them were in flight at
    /// once (or the scaled `HANG` mark ran out, so a cell that never lets `k`
    /// calls overlap ends red on `max_in_flight() >= k` or on the cell's own
    /// timeout, not as a runner kill). Proof of overlap by event: the old
    /// 400/500 ms windows failed a correct cell under an 80 % CPU quota
    /// (tv-br M10o, `gh1060_parallel_calls_stay_parallel_code`).
    pub fn start_until_overlap(k: usize, body: String) -> Self {
        Self::start_holding(Hold::UntilOverlap(k), body)
    }

    /// The bound check of a cell with `bound` run tickets, held by event in
    /// waves (re-check R K7, tv-br M10q-b). The gate starts open; the test
    /// closes it ([`Stub::close`]) before the calls it judges, and
    /// [`Stub::hold_wave`] opens it once the cell itself shows that the calls
    /// beyond the bound wait: `bound` requests inside the stub and the slot
    /// task's INFO line for the expected number of queued calls. A cell
    /// without the bound never writes that line and sends its extra calls on
    /// to the stub instead; the first request over `bound` opens the gate,
    /// and `max_in_flight() == bound` fails. WHY not a fixed hold (M10p
    /// `hold_back`, before that `thread::sleep`): a window shows overlap only
    /// if the calls arrive inside it; under a CPU quota the python starts of
    /// an unbounded cell spread past 300 ms / 1 s, and the bound check went
    /// green without a witness (re-check R, store_1 M1 / cells-rest M2).
    pub fn start_gated(bound: usize, body: String) -> Self {
        // Installed before the cell logs: the slot task's line goes to the
        // process-wide recorder only once it is the global default.
        let _ = meclaw_testing::log_recorder::global();
        Self::start_holding(Hold::Gate(bound), body)
    }

    fn start_holding(hold: Hold, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let addr = listener.local_addr().expect("stub address");
        let seen = Arc::new(Mutex::new(Seen::default()));
        let met = Arc::new(Condvar::new());
        let shared = Arc::clone(&seen);
        let notify = Arc::clone(&met);
        let body = Arc::new(body);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let seen = Arc::clone(&shared);
                let met = Arc::clone(&notify);
                let body = Arc::clone(&body);
                std::thread::spawn(move || serve(conn, &seen, &met, hold, &body));
            }
        });
        Self {
            addr,
            seen,
            met,
            hold,
        }
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

    /// Closes the gate of [`Stub::start_gated`]: requests from now on wait
    /// inside the stub.
    pub fn close(&self) {
        self.seen.lock().unwrap().closed = true;
    }

    fn open(&self) {
        self.seen.lock().unwrap().closed = false;
        self.met.notify_all();
    }

    /// One wave of the bound check: waits until `bound` requests are inside
    /// the closed gate and the cell's slot task has logged `queued` calls
    /// waiting for a run ticket of `grant` -- the cell's own sign that the
    /// calls beyond the bound wait, not an elapsed window -- or until the
    /// gate broke; then opens the gate. A hang is the `HANG:` marker of
    /// `wait::until` with the stub's state.
    pub async fn hold_wave(&self, grant: &str, queued: usize) {
        let Hold::Gate(bound) = self.hold else {
            panic!("hold_wave needs a stub from Stub::start_gated");
        };
        let needles = [
            "a call waits for a free run ticket".to_string(),
            format!(" grant={grant}"),
            format!(" waiting={queued}"),
        ];
        meclaw_testing::wait::until_with(
            &format!("{bound} requests held and {queued} calls of {grant} queued for a run ticket"),
            || {
                let (inside, broken) = {
                    let s = self.seen.lock().unwrap();
                    (s.in_flight, s.broken)
                };
                let needles: Vec<&str> = needles.iter().map(String::as_str).collect();
                let logged = !meclaw_testing::log_recorder::global()
                    .lines(tracing::Level::INFO, &needles)
                    .is_empty();
                async move { broken || (inside == bound && logged) }
            },
            || {
                let s = self.seen.lock().unwrap();
                format!(
                    "in flight {}, max {}, requests {}, broken {}",
                    s.in_flight,
                    s.max_in_flight,
                    s.auth.len(),
                    s.broken
                )
            },
        )
        .await;
        self.open();
    }
}

fn serve(mut conn: TcpStream, seen: &Mutex<Seen>, met: &Condvar, hold: Hold, body: &str) {
    // tv-br M10o (re-check B-M2): the stub's read wait is the scaled hang
    // mark like its gh1061 siblings, never a literal that stays fixed while
    // every mark grows with the time factor.
    let _ = conn.set_read_timeout(Some(meclaw_testing::deadline::scaled(
        meclaw_testing::deadline::HANG,
    )));
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
        met.notify_all();
    }
    match hold {
        Hold::Never => {}
        Hold::UntilOverlap(k) => {
            let cap = meclaw_testing::deadline::scaled(meclaw_testing::deadline::HANG);
            let s = seen.lock().unwrap();
            drop(
                met.wait_timeout_while(s, cap, |s| s.max_in_flight < k)
                    .unwrap(),
            );
        }
        Hold::Gate(bound) => {
            // The cap is the scaled hang mark only: `hold_wave` names a hang
            // with `HANG:` first; this keeps a forgotten gate from holding a
            // connection for ever.
            let cap = meclaw_testing::deadline::scaled(meclaw_testing::deadline::HANG);
            let mut s = seen.lock().unwrap();
            if s.closed && s.in_flight > bound {
                s.broken = true;
                s.closed = false;
                met.notify_all();
            }
            drop(met.wait_timeout_while(s, cap, |s| s.closed).unwrap());
        }
    }
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
    /// Deliveries put into the mailbox by [`Running::send`]; [`Running::settle`]
    /// waits for exactly this many consume marks.
    sent: usize,
    taken: Taken,
    _spawned: SpawnedCellKind,
    _dir: tempfile::TempDir,
}

/// What [`next_emission`] has handed out so far -- the last observed state a
/// hang of the next one names (TV-Z3).
#[derive(Debug, Default)]
pub struct Taken {
    pub count: usize,
    pub last: Option<String>,
    /// Consume marks seen (GH #1015): one per finished delivery.
    pub marks: usize,
}

/// The next emission's content on `out`, skipping the dispatcher's
/// consumption marks (re-check tv-br M6 M5, TV-Z3). A failure marker, not a
/// timing claim: after the scaled `deadline::HANG` it panics `HANG:` with the
/// emissions taken so far and the last of them. Lock (c) of
/// `gh1061_a_granted_embedder_keeps_its_budget_under_load` lays a stuck run
/// ticket on this mark.
pub async fn next_emission(out: &mut mpsc::Receiver<CellEmission>, taken: &mut Taken) -> Value {
    loop {
        let e = meclaw_testing::deadline::hang_with(
            "the next emission of the cell",
            out.recv(),
            || format!("{taken:?}"),
        )
        .await
        .expect("the emission channel is open");
        if e.target.as_str() != meclaw_core::CONSUMED_MARK_TARGET {
            taken.count += 1;
            taken.last = Some(e.content.to_string().chars().take(240).collect());
            return e.content;
        }
        taken.marks += 1;
    }
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
        sent: 0,
        taken: Taken::default(),
        _spawned: spawned,
        _dir: dir,
    }
}

impl Running {
    pub async fn send(&mut self, body: Value) {
        let msg = MessageBuilder::new(Path::new("/c"))
            .reply_to(Path::new("/sink"))
            .trace_id(Uuid::now_v7())
            .body(Body::Inline(body))
            .build();
        self.tx.send(msg).await.expect("the mailbox is open");
        self.sent += 1;
    }

    /// The next emission's content ([`next_emission`]: a hang is a `HANG:`
    /// marker with the emissions taken so far).
    pub async fn next(&mut self) -> Value {
        next_emission(&mut self.out, &mut self.taken).await
    }

    /// Every emission still to come from the deliveries sent so far, once
    /// all of them are finished (dev-rules § 11: absence by quiescence, never
    /// by a quiet window -- re-check tv-br M10g, M10h).
    ///
    /// WHY this is exact: the cell task puts a delivery's consume mark on the
    /// output channel after every emission its handler made (GH #1015), and the
    /// channel is FIFO. Once the marks of all [`Running::send`]s are in, nothing
    /// more can come from them -- however late a stray emission would have
    /// been. The old `drain(300 ms)` read "nothing in 300 ms" as "nothing", so
    /// an emission later than the window passed unseen. A hang is the
    /// `HANG:` marker of [`next_emission`] with the marks still missing.
    pub async fn settle(&mut self) -> Vec<Value> {
        let mut rest = Vec::new();
        while self.taken.marks < self.sent {
            let (sent, marks) = (self.sent, self.taken.marks);
            let e = meclaw_testing::deadline::hang_with(
                "every delivery sent is finished (its consume mark is in)",
                self.out.recv(),
                || format!("{marks}/{sent} marks, {rest:?}"),
            )
            .await
            .expect("the emission channel is open");
            if e.target.as_str() == meclaw_core::CONSUMED_MARK_TARGET {
                self.taken.marks += 1;
            } else {
                rest.push(e.content);
            }
        }
        rest
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
