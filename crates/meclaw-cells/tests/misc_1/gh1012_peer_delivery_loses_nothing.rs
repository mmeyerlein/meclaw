//! GH #1012: peer delivery is at-least-once with an idempotent receive.
//!
//! Before this lock a crossing was at-most-once: the sending proxy posted
//! once and a carrier failure was only a `refused` receipt, the far mount
//! answered `200` once the event sat in memory, nothing was replayed after a
//! crash, and a frame carried no id, so a repeat arrived twice. Every case is
//! measured where it counts: at the receiving end (the arrivals the mount
//! hands its handler, the frames a counting peer saw) and in the books the
//! cell keeps in its own `cell.db`.
//!
//! Review C1 (R-HV-3): the delivery id and the sender's clock travel as the
//! request headers `X-Meclaw-Frame-Id` and `X-Meclaw-Sent-Ms`, never in the
//! frame. The frame body stays byte-identical to 0.47.1, whose parser
//! (`deny_unknown_fields`) refuses any new key as `invalid_frame`.
use meclaw_cells::proxy::meclaw::{
    cell::MeclawCell,
    io::run_io,
    mount::{MeclawIo, PeerEvent, PeerReconfig},
    params::MeclawParams,
    wire::{FRAME_ID_HEADER, SENT_MS_HEADER},
};
use meclaw_colony::{ColonyMsg, DbConn, DeadLetterReason, LongRunningCell, SurfaceRegistry};
use meclaw_core::serde_json::{self, Value, json};
use meclaw_core::{
    Body, CellEmission, Headers, Message, MessageBuilder, OriginSink, OutputSink, Path, Uuid,
};
use meclaw_testing::surface_listener::{surface_listener, wait_for_mount};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

const HDR: &str = "X-Meclaw-Peer";
/// Failure-marker timeout (30 s convention).
const WAIT: Duration = Duration::from_secs(30);

fn params(egress: &str, deadline_s: Option<u64>) -> Value {
    let mut v = json!({"platform": "meclaw", "mount": "peer", "identity_header": HDR,
        "boundary": "north", "emit_to": "/sink", "external_timeout_ms": 2000,
        "egress": [egress], "lanes": {
            "accepts": [{"route": "topic", "fields": ["topic"], "because": "a subject"}],
            "emits": [{"route": "proposal", "fields": ["proposal"], "because": "one proposal"}]}});
    if let Some(d) = deadline_s {
        v["peer_retry_deadline_s"] = json!(d);
    }
    v
}

fn db_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("cell.db")
}

fn rows(path: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    let c = rusqlite::Connection::open(path).expect("open");
    let mut st = c.prepare(sql).expect("prepare");
    let n = st.column_count();
    st.query_map([], |r| {
        (0..n)
            .map(|i| {
                r.get::<_, rusqlite::types::Value>(i).map(|v| match v {
                    rusqlite::types::Value::Null => "NULL".to_string(),
                    rusqlite::types::Value::Integer(i) => i.to_string(),
                    rusqlite::types::Value::Real(f) => f.to_string(),
                    rusqlite::types::Value::Text(t) => t,
                    rusqlite::types::Value::Blob(_) => "BLOB".to_string(),
                })
            })
            .collect::<Result<Vec<_>, _>>()
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .expect("rows")
}

// ---------------------------------------------------------------- receiving

/// A mount with its inbox in `path`: listener address, the handler's event
/// channel, and the task holding the I/O half (abort = the process dies).
async fn mount_at(
    path: &std::path::Path,
) -> (
    SocketAddr,
    mpsc::Receiver<PeerEvent>,
    tokio::task::JoinHandle<()>,
    mpsc::Sender<PeerReconfig>,
) {
    mount_with(path, 16).await
}

/// [`mount_at`] with an event channel of `cap` places.
async fn mount_with(
    path: &std::path::Path,
    cap: usize,
) -> (
    SocketAddr,
    mpsc::Receiver<PeerEvent>,
    tokio::task::JoinHandle<()>,
    mpsc::Sender<PeerReconfig>,
) {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let p = MeclawParams::parse(&params("http://127.0.0.1:1", None)).expect("params");
    let (events_tx, events_rx) = mpsc::channel(cap);
    let (reconfig_tx, reconfig_rx) = mpsc::channel(1);
    let io = MeclawIo::new(&p, "/friend", Arc::clone(&surfaces)).with_cell_db(path.to_path_buf());
    let task = tokio::spawn(run_io(io, events_tx, reconfig_rx));
    wait_for_mount(&surfaces, "peer").await;
    let (addr, _join) = surface_listener(Arc::clone(&surfaces)).await;
    (addr, events_rx, task, reconfig_tx)
}

/// A frame as a sender posts it: the body (0.47.1 form) and the delivery
/// headers that travel next to it.
struct Framed {
    body: Value,
    headers: Vec<(&'static str, String)>,
}

fn frame(id: Option<Uuid>, sent_ms: Option<u64>) -> Framed {
    let body = json!({"v": 1, "type": "message", "lane": "topic",
        "trace_id": Uuid::now_v7().to_string(), "ttl": 5, "context": {},
        "body": {"messages": [], "topic": "gardening"}});
    let mut headers = Vec::new();
    if let Some(id) = id {
        headers.push((FRAME_ID_HEADER, id.to_string()));
    }
    if let Some(ms) = sent_ms {
        headers.push((SENT_MS_HEADER, ms.to_string()));
    }
    Framed { body, headers }
}

async fn post(addr: SocketAddr, f: &Framed) -> (u16, Value) {
    post_as(addr, "south", f, None)
        .await
        .expect("the mount answers")
}

/// One POST signed as `sender`; `Err` when `timeout` ran out first (the
/// client gives up and drops the connection, as a sender's `peer_timeout`).
async fn post_as(
    addr: SocketAddr,
    sender: &str,
    f: &Framed,
    timeout: Option<Duration>,
) -> Result<(u16, Value), reqwest::Error> {
    let mut req = reqwest::Client::new()
        .post(format!("http://{addr}/peer/"))
        .header(HDR, sender)
        .json(&f.body);
    for (k, v) in &f.headers {
        req = req.header(*k, v);
    }
    if let Some(t) = timeout {
        req = req.timeout(t);
    }
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    Ok((status, resp.json().await.unwrap_or(Value::Null)))
}

/// The frame id an arrival carries, `None` for one booked without.
fn arrival_id(ev: &PeerEvent) -> Option<Uuid> {
    match ev {
        PeerEvent::Arrived { delivery, .. } => delivery.frame_id,
        other => panic!("expected an arrival, got {other:?}"),
    }
}

/// Part b, review C1 lock 2: the same frame id twice, carried in the
/// `X-Meclaw-Frame-Id` header, is one arrival; the repeat reads `crossed`
/// with `duplicate: true` and raises nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repeated_frame_is_one_arrival() {
    let dir = tempfile::tempdir().expect("dir");
    let (addr, mut rx, _task, _keep) = mount_at(&db_path(&dir)).await;
    let f = frame(Some(Uuid::now_v7()), Some(1_000));
    let (s1, r1) = post(addr, &f).await;
    let (s2, r2) = post(addr, &f).await;
    assert_eq!((s1, s2), (200, 200));
    assert_eq!(r1["result"], json!("crossed"), "{r1}");
    assert_eq!(r1.get("duplicate"), None, "the first answer is plain: {r1}");
    assert_eq!(r2["result"], json!("crossed"), "{r2}");
    assert_eq!(r2["duplicate"], json!(true), "{r2}");
    let id = f.headers[0].1.clone();
    let ev = rx.try_recv().expect("the first arrival");
    assert_eq!(arrival_id(&ev).map(|u| u.to_string()), Some(id));
    assert!(
        rx.try_recv().is_err(),
        "the repeat raised a second arrival at the receiver"
    );
    let n = rows(&db_path(&dir), "SELECT count(*) FROM peer_inbox");
    assert_eq!(n[0][0], "1", "one inbox row per id");
}

/// Part a, receiving half, review C1 lock 3: a POST without the delivery
/// headers (a sender before #1012) takes exactly the old way: accepted, the
/// plain 0.47.1 receipt, no id on the arrival, and no deduplication (an old
/// sender cannot be told apart from a new message).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_without_id_is_accepted_and_not_deduplicated() {
    let dir = tempfile::tempdir().expect("dir");
    let (addr, mut rx, _task, _keep) = mount_at(&db_path(&dir)).await;
    let f = frame(None, None);
    for _ in 0..2 {
        let (s, r) = post(addr, &f).await;
        assert_eq!((s, r["result"].clone()), (200, json!("crossed")), "{r}");
        assert_eq!(
            r,
            meclaw_cells::proxy::meclaw::wire::crossed_receipt(
                "topic",
                "north",
                &["topic".to_string()]
            ),
            "the old receipt, key for key"
        );
    }
    for _ in 0..2 {
        let ev = rx.try_recv().expect("an arrival per POST");
        assert_eq!(arrival_id(&ev), None, "no id without the header");
    }
}

/// Review C1: a delivery header that is present but unreadable is an
/// `invalid_frame`, like any other part of the frame that does not parse;
/// nothing is booked and nothing arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bad_delivery_header_is_an_invalid_frame() {
    let dir = tempfile::tempdir().expect("dir");
    let (addr, mut rx, _task, _keep) = mount_at(&db_path(&dir)).await;
    for (k, v) in [(FRAME_ID_HEADER, "not-a-uuid"), (SENT_MS_HEADER, "soon")] {
        let mut f = frame(None, None);
        f.headers.push((k, v.to_string()));
        let (s, r) = post(addr, &f).await;
        assert_eq!(s, 200, "a refusal is a receipt: {r}");
        assert_eq!(r["result"], json!("refused"), "{k}: {r}");
        assert_eq!(r["error_code"], json!("invalid_frame"), "{k}: {r}");
        assert!(matches!(rx.try_recv(), Ok(PeerEvent::Refused { .. })));
    }
    assert!(rx.try_recv().is_err(), "nothing arrived");
    let n = rows(&db_path(&dir), "SELECT count(*) FROM peer_inbox");
    assert_eq!(n[0][0], "0", "a refused frame is not booked");
}

/// Review M5: the inbox key is the sender AND the id. Two senders that pick
/// the same id do not swallow each other's frame; each arrives once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_senders_with_one_frame_id_are_two_arrivals() {
    let dir = tempfile::tempdir().expect("dir");
    let (addr, mut rx, _task, _keep) = mount_at(&db_path(&dir)).await;
    let id = Uuid::now_v7();
    let f = frame(Some(id), None);
    for sender in ["south", "west", "south"] {
        let (s, r) = post_as(addr, sender, &f, None).await.expect("answer");
        assert_eq!((s, r["result"].clone()), (200, json!("crossed")), "{r}");
    }
    let mut peers = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        match ev {
            PeerEvent::Arrived { peer, .. } => peers.push(peer),
            other => panic!("expected an arrival, got {other:?}"),
        }
    }
    assert_eq!(peers, vec!["south".to_string(), "west".to_string()]);
    let keys = rows(&db_path(&dir), "SELECT id FROM peer_inbox ORDER BY rowid");
    assert_eq!(
        keys,
        vec![vec![format!("south:{id}")], vec![format!("west:{id}")]],
        "one row per (sender, id)"
    );
}

/// Review C2: a request the sender abandons after the commit (its own
/// `peer_timeout` while this side's handler is busy) must not hold the
/// arrival back. Before the fix the commit happened in the request future
/// and the hand-on after it: the dropped future left a committed `pending`
/// row nobody handed on until the next start, and every retry read
/// `duplicate: true`. Now the arrival comes exactly once, without a
/// restart, in commit order, and the retry is a duplicate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abandoned_request_still_hands_its_arrival_on() {
    let dir = tempfile::tempdir().expect("dir");
    let path = db_path(&dir);
    // One place: the first arrival fills the handler's channel.
    let (addr, mut rx, _task, _keep) = mount_with(&path, 1).await;
    let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (s, _) = post(addr, &frame(Some(a), None)).await;
    assert_eq!(s, 200);
    // B: committed, then the hand-on waits for the full channel; the client
    // gives up after 500 ms and drops the connection.
    let gave_up = post_as(
        addr,
        "south",
        &frame(Some(b), None),
        Some(Duration::from_millis(500)),
    )
    .await
    .is_err();
    assert!(gave_up, "the request should have been abandoned");
    let n = rows(&path, "SELECT count(*) FROM peer_inbox");
    assert_eq!(n[0][0], "2", "B was committed before the client gave up");
    let first = rx.recv().await.expect("A");
    assert_eq!(arrival_id(&first), Some(a));
    let second = tokio::time::timeout(WAIT, rx.recv())
        .await
        .expect("B arrives without a restart")
        .expect("B");
    assert_eq!(arrival_id(&second), Some(b));
    // The sender retries B: a duplicate, no second arrival.
    let (s, r) = post(addr, &frame(Some(b), None)).await;
    assert_eq!((s, r["duplicate"].clone()), (200, json!(true)), "{r}");
    // C comes after B: the order per sender holds.
    let (s, _) = post(addr, &frame(Some(c), None)).await;
    assert_eq!(s, 200);
    let third = tokio::time::timeout(WAIT, rx.recv())
        .await
        .expect("C")
        .expect("C");
    assert_eq!(arrival_id(&third), Some(c), "B arrived once, then C");
    assert!(rx.try_recv().is_err(), "nothing else arrived");
}

/// Part b: `200` only after the inbox row is committed. A test connection
/// holds the write lock (a writer that is slow to commit); the POST must not
/// be answered while it holds, and is answered once it lets go.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_answer_waits_for_the_commit() {
    let dir = tempfile::tempdir().expect("dir");
    let path = db_path(&dir);
    let (addr, _rx, _task, _keep) = mount_at(&path).await;
    let blocker = rusqlite::Connection::open(&path).expect("open");
    blocker
        .execute_batch("BEGIN IMMEDIATE")
        .expect("hold the write lock");
    let f = frame(Some(Uuid::now_v7()), None);
    let pending = tokio::spawn(async move { post(addr, &f).await });
    // Semantic discriminator: 400 ms is far longer than an in-memory answer
    // takes (the pre-#1012 mount answered in single milliseconds).
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !pending.is_finished(),
        "the mount answered while its inbox write could not have committed"
    );
    blocker.execute_batch("COMMIT").expect("let go");
    let (s, r) = tokio::time::timeout(WAIT, pending)
        .await
        .expect("an answer after the commit")
        .expect("join");
    assert_eq!((s, r["result"].clone()), (200, json!("crossed")), "{r}");
    let n = rows(&path, "SELECT count(*) FROM peer_inbox");
    assert_eq!(n[0][0], "1");
}

/// Part c: the process dies between the answer and the handler's work. After
/// the start the arrival comes again, exactly once; once handled it is `done`,
/// and a third life raises the pending row once more and the colony drops it
/// by its key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answered_frame_survives_a_crash_and_arrives_once() {
    let dir = tempfile::tempdir().expect("dir");
    let path = db_path(&dir);
    let id = Uuid::now_v7();
    {
        let (addr, _rx_never_read, task, _keep) = mount_at(&path).await;
        let (s, r) = post(addr, &frame(Some(id), Some(7))).await;
        assert_eq!((s, r["result"].clone()), (200, json!("crossed")), "{r}");
        task.abort(); // the handler never saw it
    }
    let (_addr, mut rx, task, _keep) = mount_at(&path).await;
    let ev = rx
        .try_recv()
        .expect("the answered frame is replayed at the start");
    assert!(
        rx.try_recv().is_err(),
        "exactly one replayed arrival per answered frame"
    );
    let PeerEvent::Arrived { delivery, .. } = &ev else {
        panic!("an arrival, not {ev:?}")
    };
    let key = delivery
        .inbox_key
        .clone()
        .expect("the replayed arrival names its inbox row");
    assert!(
        delivery.replayed,
        "raised from the inbox, not from the wire"
    );
    // The handler half hands it on (GH #1015: keyed, marked as a repeat).
    let p = MeclawParams::parse(&params("http://127.0.0.1:1", None)).expect("params");
    let mut cell = MeclawCell::new(&p).expect("cell");
    let (out_tx, mut out_rx) = mpsc::channel(8);
    let sink = OriginSink::new(out_tx, Path::new("/friend"), 64);
    let conn = meclaw_colony::persist::cell_db::open_or_create_cell_db(&path).expect("db");
    let mut db = DbConn::wrap(conn, Some(Duration::from_secs(5)));
    cell.handle_event(ev, &sink, &mut db).await;
    let arrival = out_rx.recv().await.expect("the arrival");
    assert_eq!(
        arrival.content["header"]["peer_frame_id"],
        json!(id.to_string())
    );
    assert_eq!(arrival.content["header"]["delivery_key"], json!(key));
    assert_eq!(arrival.content["header"]["delivery_replay"], json!(true));
    task.abort();
    let st = rows(&path, "SELECT state FROM peer_inbox");
    assert_eq!(
        st,
        vec![vec!["pending".to_string()]],
        "the row stays pending: a handed-on row is booked done only after HAND_ON_SETTLE (GH #1015)"
    );
    // GH #1015 (OR-HV-60): a row without follow-up traffic stays pending, so
    // the third life raises it once more — exactly once, same key, marked as
    // a repeat. The colony drops it by that key: the receiver-side lock is
    // `gh1015_…::a_repeated_keyed_hand_on_is_dropped_direct_and_through_a_hive_transit`.
    let (_addr, mut rx, _task, _keep) = mount_at(&path).await;
    let again = rx
        .try_recv()
        .expect("the pending row is raised again in the third life");
    assert!(rx.try_recv().is_err(), "exactly once");
    let PeerEvent::Arrived { delivery, .. } = &again else {
        panic!("an arrival, not {again:?}")
    };
    assert_eq!(
        delivery.inbox_key.as_deref(),
        Some(key.as_str()),
        "the same key"
    );
    assert!(delivery.replayed, "marked as a repeat");
}

/// Part f: the arrival carries the sender's and the receiver's clocks; a
/// frame without `sent_ms` carries only the receiver's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_arrival_carries_both_clocks() {
    let dir = tempfile::tempdir().expect("dir");
    let path = db_path(&dir);
    let (addr, mut rx, _task, _keep) = mount_at(&path).await;
    let p = MeclawParams::parse(&params("http://127.0.0.1:1", None)).expect("params");
    let mut cell = MeclawCell::new(&p).expect("cell");
    let (out_tx, mut out_rx) = mpsc::channel(8);
    let sink = OriginSink::new(out_tx, Path::new("/friend"), 64);
    let conn = meclaw_colony::persist::cell_db::open_or_create_cell_db(&path).expect("db");
    let mut db = DbConn::wrap(conn, Some(Duration::from_secs(5)));
    let before = now_ms();
    for (sent, id) in [(Some(4242u64), Some(Uuid::now_v7())), (None, None)] {
        post(addr, &frame(id, sent)).await;
        let ev = rx.try_recv().expect("an arrival");
        cell.handle_event(ev, &sink, &mut db).await;
        let h = out_rx.recv().await.expect("arrival").content["header"].clone();
        match sent {
            Some(ms) => assert_eq!(h["peer_sent_ms"], json!(ms), "{h}"),
            None => assert_eq!(h.get("peer_sent_ms"), None, "{h}"),
        }
        let arrived = h["peer_arrived_ms"].as_u64().expect("peer_arrived_ms");
        assert!(arrived >= before && arrived <= now_ms(), "{h}");
        let _own_receipt = out_rx.recv().await.expect("this side's crossed receipt");
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ------------------------------------------------------------------ sending

/// How the counting peer answers: down (reads the request, then drops the
/// connection unanswered), up (`crossed`), or a final `400`.
const DOWN: u8 = 0;
const UP: u8 = 1;
const BAD: u8 = 2;

struct FakePeer {
    url: String,
    mode: Arc<AtomicU8>,
    /// Every frame posted, in arrival order, with the mode it met.
    seen: Arc<Mutex<Vec<Seen>>>,
}

/// One POST as the counting peer read it.
#[derive(Clone, Debug)]
struct Seen {
    mode: u8,
    frame: Value,
    /// `X-Meclaw-Frame-Id`, when sent.
    id: Option<String>,
    /// `X-Meclaw-Sent-Ms`, when sent.
    sent_ms: Option<String>,
}

/// The `message` frame exactly as 0.47.1 reads it: the same seven keys and
/// `deny_unknown_fields`. Review C1 lock 1: whatever a sender of this build
/// posts must parse here, or a receiver before #1012 refuses it for good.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Frame0471 {
    v: u64,
    #[serde(rename = "type")]
    frame_type: String,
    lane: String,
    trace_id: String,
    ttl: u32,
    context: serde_json::Map<String, Value>,
    body: Value,
}

async fn fake_peer(mode: u8) -> FakePeer {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = l.local_addr().expect("addr");
    let mode = Arc::new(AtomicU8::new(mode));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (m, s) = (Arc::clone(&mode), Arc::clone(&seen));
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = l.accept().await {
            let (m, s) = (Arc::clone(&m), Arc::clone(&s));
            tokio::spawn(async move {
                let (head, frame) = read_request(&mut sock).await;
                let now = m.load(Ordering::SeqCst);
                let header = |name: &str| {
                    let prefix = format!("{}:", name.to_ascii_lowercase());
                    head.lines()
                        .find_map(|l| l.strip_prefix(prefix.as_str()))
                        .map(|v| v.trim().to_string())
                };
                let seen = Seen {
                    mode: now,
                    frame,
                    id: header(FRAME_ID_HEADER),
                    sent_ms: header(SENT_MS_HEADER),
                };
                if let Ok(mut held) = s.lock() {
                    held.push(seen);
                }
                let answer = match now {
                    DOWN => return, // dropped unanswered
                    UP => (
                        "200 OK",
                        meclaw_cells::proxy::meclaw::wire::crossed_receipt(
                            "proposal",
                            "south",
                            &[],
                        )
                        .to_string(),
                    ),
                    _ => ("400 Bad Request", "{}".to_string()),
                };
                let msg = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{}",
                    answer.0,
                    answer.1.len(),
                    answer.1
                );
                let _ = sock.write_all(msg.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    FakePeer {
        url: format!("http://{addr}"),
        mode,
        seen,
    }
}

/// The request head (lower-cased) and the JSON body of one POST.
async fn read_request(s: &mut tokio::net::TcpStream) -> (String, Value) {
    let mut buf = Vec::new();
    let mut b = [0u8; 4096];
    let head = loop {
        match s.read(&mut b).await {
            Ok(0) | Err(_) => return (String::new(), Value::Null),
            Ok(n) => buf.extend_from_slice(&b[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let text = String::from_utf8_lossy(&buf[..head]).to_ascii_lowercase();
    let len = text
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < head + len {
        match s.read(&mut b).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&b[..n]),
        }
    }
    let body = serde_json::from_slice(&buf[head..]).unwrap_or(Value::Null);
    (text, body)
}

/// One life of a sending cell: both halves, driven like the substrate drives
/// a long-running cell, against the `cell.db` at `path`.
struct Life {
    msg_tx: mpsc::Sender<Message>,
    out_rx: mpsc::Receiver<CellEmission>,
    dl_rx: mpsc::Receiver<ColonyMsg>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Life {
    fn end(self) {
        for t in &self.tasks {
            t.abort();
        }
    }

    /// The next receipt header, with the refusal's detail copied in.
    async fn receipt(&mut self) -> Value {
        let em = tokio::time::timeout(WAIT, self.out_rx.recv())
            .await
            .expect("a receipt within 30 s")
            .expect("a receipt");
        let mut h = em.content["header"].clone();
        h["detail"] = em.content["messages"][0]["text"].clone();
        h
    }
}

async fn life(v: &Value, path: &std::path::Path) -> Life {
    let p = MeclawParams::parse(v).expect("params");
    let surfaces = Arc::new(SurfaceRegistry::new());
    let (dl_tx, dl_rx) = mpsc::channel(16);
    let io = MeclawIo::new(&p, "/friend", surfaces).with_cell_db(path.to_path_buf());
    let mut cell = MeclawCell::new(&p)
        .expect("cell")
        .with_io(io)
        .with_colony_inbox(dl_tx);
    // As the factory does it: the handler's connection and the books first,
    // synchronously, then the I/O half (a second connection switching a
    // fresh file to WAL at the same moment reads `database is locked`).
    let conn = meclaw_colony::persist::cell_db::open_or_create_cell_db(path).expect("db");
    meclaw_cells::proxy::meclaw::book::setup_peer_book(&conn).expect("books");
    let mut db = DbConn::wrap(conn, Some(Duration::from_secs(5)));
    let io_half = cell.split_io();
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (reconfig_tx, reconfig_rx) = mpsc::channel(1);
    let io_task = tokio::spawn(MeclawCell::run_io(io_half, events_tx, reconfig_rx));
    let (out_tx, out_rx) = mpsc::channel(256);
    let origin = OriginSink::new(out_tx.clone(), Path::new("/friend"), 64);
    let (msg_tx, mut msg_rx) = mpsc::channel::<Message>(64);
    let driver = tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(m) = msg_rx.recv() => {
                    let sink = OutputSink::new(out_tx.clone(), Path::new("/friend"), m.id,
                        m.trace_id, 64, Headers::new(), None);
                    cell.handle(m, &sink, &mut db, &reconfig_tx).await;
                }
                Some(e) = events_rx.recv() => cell.handle_event(e, &origin, &mut db).await,
                else => break,
            }
        }
    });
    Life {
        msg_tx,
        out_rx,
        dl_rx,
        tasks: vec![io_task, driver],
    }
}

fn proposal(peer_url: &str, n: u32) -> Message {
    let mut hop = serde_json::Map::new();
    hop.insert("route".into(), json!("proposal"));
    hop.insert("peer".into(), json!("south"));
    hop.insert("peer_url".into(), json!(peer_url));
    MessageBuilder::new(Path::new("/friend"))
        .ttl(5)
        .hop(hop)
        .body(Body::Inline(json!({"proposal": n.to_string()})))
        .build()
}

/// Parts d, d2, a and g: the target is away; every message reads exactly one
/// `deferred`, all arrive in order once it is back, each under one id on
/// every attempt, and the outbox keeps every row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_target_that_is_away_gets_everything_in_order_when_it_returns() {
    let dir = tempfile::tempdir().expect("dir");
    let path = db_path(&dir);
    let peer = fake_peer(DOWN).await;
    let mut l = life(&params(&peer.url, None), &path).await;
    for n in 1..=3 {
        l.msg_tx.send(proposal(&peer.url, n)).await.expect("send");
    }
    let mut ids = Vec::new();
    for _ in 1..=3 {
        let h = l.receipt().await;
        assert_eq!(h["peer_event"], json!("deferred"), "{h}");
        assert_eq!(h["error_code"], json!("peer_unreachable"), "{h}");
        ids.push(
            h["id"]
                .as_str()
                .expect("the deferred receipt names the id")
                .to_string(),
        );
    }
    peer.mode.store(UP, Ordering::SeqCst);
    for id in &ids {
        let h = l.receipt().await;
        assert_eq!(h["peer_event"], json!("crossed"), "{h}");
        assert_eq!(&h["id"], &json!(id), "success receipts come in order: {h}");
        assert!(h["deferred_ms"].as_u64().is_some(), "{h}");
    }
    let seen = peer.seen.lock().expect("seen").clone();
    let delivered: Vec<Value> = seen
        .iter()
        .filter(|x| x.mode == UP)
        .map(|x| x.frame["body"]["proposal"].clone())
        .collect();
    assert_eq!(delivered, vec![json!("1"), json!("2"), json!("3")], "FIFO");
    for x in &seen {
        // Review C1 lock 1: the frame is the 0.47.1 frame, key for key; the
        // id and the clock ride in the headers.
        let old: Result<Frame0471, _> = serde_json::from_value(x.frame.clone());
        assert!(
            old.is_ok(),
            "a 0.47.1 receiver refuses this frame: {:?} ({})",
            old.err(),
            x.frame
        );
        let id = x.id.clone().unwrap_or_default();
        assert!(ids.contains(&id), "{x:?}");
        assert!(
            x.sent_ms
                .as_deref()
                .and_then(|v| v.parse::<u64>().ok())
                .is_some(),
            "{x:?}"
        );
    }
    let first: Vec<&Seen> = seen
        .iter()
        .filter(|x| x.frame["body"]["proposal"] == json!("1"))
        .collect();
    assert!(first.len() >= 2, "the first message was retried: {first:?}");
    assert!(
        first.iter().all(|x| x.id == first[0].id
            && x.sent_ms == first[0].sent_ms
            && x.frame == first[0].frame),
        "every attempt carries the same id, clock and bytes: {first:?}"
    );
    let st = rows(&path, "SELECT state FROM peer_outbox ORDER BY rowid");
    assert_eq!(st.len(), 3, "rows are never deleted");
    assert!(st.iter().all(|r| r[0] == "sent"), "{st:?}");
    l.end();
}

/// Part d: a 4xx other than 401 is final: one attempt, one `refused`.
/// OR-HV-51 (review M4): a final non-delivery is never only a receipt; it
/// is also a dead letter, `peer_refused`, carrying the message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_4xx_is_not_retried() {
    let dir = tempfile::tempdir().expect("dir");
    let path = db_path(&dir);
    let peer = fake_peer(BAD).await;
    let mut l = life(&params(&peer.url, None), &path).await;
    l.msg_tx.send(proposal(&peer.url, 1)).await.expect("send");
    let h = l.receipt().await;
    assert_eq!(h["peer_event"], json!("refused"), "{h}");
    assert_eq!(h["error_code"], json!("peer_unreachable"), "{h}");
    // The first retry would fall after 1 s.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(
        peer.seen.lock().expect("seen").len(),
        1,
        "no second attempt"
    );
    let st = rows(&path, "SELECT state FROM peer_outbox");
    assert_eq!(st, vec![vec!["refused".to_string()]]);
    let dl = tokio::time::timeout(WAIT, l.dl_rx.recv())
        .await
        .expect("a dead letter within 30 s")
        .expect("a dead letter");
    match dl {
        ColonyMsg::DeadLetterMessage { reason, message } => {
            assert_eq!(reason.as_code(), "peer_refused");
            assert!(matches!(reason, DeadLetterReason::PeerRefused));
            assert_eq!(message.body, Body::Inline(json!({"proposal": "1"})));
        }
        _ => panic!("expected a dead letter"),
    }
    assert!(l.dl_rx.try_recv().is_err(), "exactly one dead letter");
    l.end();
}

/// Part e: past `peer_retry_deadline_s` the message is a `peer_expired` dead
/// letter AND an `expired` receipt, after more than one attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn past_the_deadline_a_message_is_a_dead_letter_and_a_receipt() {
    let dir = tempfile::tempdir().expect("dir");
    let path = db_path(&dir);
    let peer = fake_peer(DOWN).await;
    let mut l = life(&params(&peer.url, Some(2)), &path).await;
    l.msg_tx.send(proposal(&peer.url, 1)).await.expect("send");
    assert_eq!(l.receipt().await["peer_event"], json!("deferred"));
    let h = l.receipt().await;
    assert_eq!(h["peer_event"], json!("expired"), "{h}");
    assert_eq!(h["error_code"], json!("peer_expired"), "{h}");
    assert!(h["tries"].as_u64().unwrap_or(0) > 1, "{h}");
    let dl = tokio::time::timeout(WAIT, l.dl_rx.recv())
        .await
        .expect("a dead letter within 30 s")
        .expect("a dead letter");
    match dl {
        ColonyMsg::DeadLetterMessage { reason, message } => {
            assert_eq!(reason.as_code(), "peer_expired");
            assert!(matches!(reason, DeadLetterReason::PeerExpired));
            assert_eq!(message.body, Body::Inline(json!({"proposal": "1"})));
        }
        _ => panic!("expected a dead letter"),
    }
    let st = rows(&path, "SELECT state, tries > 1 FROM peer_outbox");
    assert_eq!(st, vec![vec!["expired".to_string(), "1".to_string()]]);
    l.end();
}

/// Parts c and d together: what the outbox holds when the process dies is
/// sent by the next life, without a new message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_outbox_survives_a_restart() {
    let dir = tempfile::tempdir().expect("dir");
    let path = db_path(&dir);
    let peer = fake_peer(DOWN).await;
    let v = params(&peer.url, None);
    let mut l = life(&v, &path).await;
    l.msg_tx.send(proposal(&peer.url, 1)).await.expect("send");
    let h = l.receipt().await;
    assert_eq!(h["peer_event"], json!("deferred"), "{h}");
    let id = h["id"].clone();
    l.end();
    peer.mode.store(UP, Ordering::SeqCst);
    let mut l = life(&v, &path).await;
    let h = l.receipt().await;
    assert_eq!(h["peer_event"], json!("crossed"), "{h}");
    assert_eq!(h["id"], id, "{h}");
    l.end();
}
