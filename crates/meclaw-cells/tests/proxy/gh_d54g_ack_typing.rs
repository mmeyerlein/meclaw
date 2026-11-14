//! GH #1504: the typing status stands from the incoming turn to its answer.
//!
//! A message with `hop.interim == "1"` is not the answer: the turn goes on
//! behind it. Until this file the connector ended the typing keeper after
//! EVERY message it sent, so after an interim the chat showed nothing until
//! the answer came. Telegram clients also clear the status when a message of
//! the bot arrives, so "stands" means two things: the keeper is not ended,
//! and it shows the status again right behind the interim instead of at its
//! next regular tick.
//!
//! What this file pins:
//!
//! 1. an interim leaves the keeper standing and makes it type again at once;
//!    the first message without `interim` ends it;
//! 2. an interim does not lift the ceiling: a keeper past `max_total` ends on
//!    its own, interim or not, and sends nothing after that;
//! 3. an answer marked `hop.repeat` (GH #1505: its text already went out in
//!    this turn, so nothing is sent) ends the keeper like a sent answer: it
//!    is the answer of its turn.
//!
//! Every claim is an event, none a window (as `gh515_*`): the Bot API is the
//! test's own and hands each request over with its connection still open, so
//! the test decides when the keeper's next step may happen. The cadences make
//! the keeper's own clock irrelevant: in (1) the interval is the product wait,
//! so the only tick that can follow the first is the one the interim asks
//! for; in (2) the ceiling is shorter than the interval, so the keeper's
//! verdict after its first tick is "stop" whatever the host's speed.

use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::db::setup_proxy_schema;
use meclaw_cells::proxy::io::ProxyEvent;
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_cells::proxy::typing::TypingCadence;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{
    Body, CellEmission, Message, MessageBuilder, OriginSink, OutputSink, Path, Uuid,
    serde_json::json,
};
use meclaw_testing::deadline;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// The chat of the turn that is answered. Its own number: the keeper's log
/// lines name the chat, and the recorder is shared by the whole binary.
const ANSWERED_CHAT_ID: i64 = 150_400_001;

/// The chat of the turn that only ever gets an interim.
const CEILING_CHAT_ID: i64 = 150_400_002;

/// The chat of the turn whose answer is a repeat.
const REPEAT_CHAT_ID: i64 = 150_400_003;

fn empty_sink(tx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx,
        Path::new("/p"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    )
}

/// A finished message as it reaches the connector, interim or not.
fn outgoing(chat_id: i64, text: &str, interim: bool) -> Message {
    let mut context = Map::new();
    context.insert("chat_id".into(), json!(chat_id));
    let mut hop: Map<String, Value> = Map::new();
    hop.insert("route".into(), json!("answer"));
    if interim {
        hop.insert("interim".into(), json!("1"));
    }
    MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/sender"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [{ "origin": "assistant", "type": "text", "text": text }]
        })))
        .build()
}

/// The answer of a turn, marked as a repeat of a text this turn already sent
/// (GH #1505): no interim, `hop.repeat == "1"`.
fn repeated(chat_id: i64, text: &str) -> Message {
    let mut context = Map::new();
    context.insert("chat_id".into(), json!(chat_id));
    let mut hop: Map<String, Value> = Map::new();
    hop.insert("route".into(), json!("answer"));
    hop.insert("repeat".into(), json!("1"));
    MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/sender"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [{ "origin": "assistant", "type": "text", "text": text }]
        })))
        .build()
}

/// One request the test's Bot API took. Its connection stays open until the
/// test answers it ([`Taken::answer`]) or the client gives it up
/// ([`Taken::given_up`]).
struct Taken {
    path: String,
    body: Value,
    stream: TcpStream,
}

impl Taken {
    /// Telegram's `ok=true`, and the connection closes behind it. A client
    /// that already gave the request up is no failure here: the verdict is
    /// the event the test waits for next.
    async fn answer(mut self) {
        let body = br#"{"ok":true,"result":true}"#;
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = self.stream.write_all(head.as_bytes()).await;
        let _ = self.stream.write_all(body).await;
        let _ = self.stream.shutdown().await;
    }

    /// Resolves once the client gave the unanswered request up: its end of
    /// the connection closed (EOF or reset).
    async fn given_up(mut self) {
        let mut b = [0u8; 64];
        loop {
            match self.stream.read(&mut b).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    }

    /// The request is a `typing` chat action for `chat_id`.
    fn assert_typing(&self, chat_id: i64, claim: &str) {
        assert!(
            self.path.contains("/botT/sendChatAction"),
            "{claim}: expected a chat action, got {}",
            self.path
        );
        assert_eq!(self.body["action"], "typing", "{claim}");
        assert_eq!(self.body["chat_id"].as_i64(), Some(chat_id), "{claim}");
    }

    /// The request is a `sendMessage` to `chat_id` with `text`.
    fn assert_message(&self, chat_id: i64, text: &str, claim: &str) {
        assert!(
            self.path.contains("/botT/sendMessage"),
            "{claim}: expected a sendMessage, got {}",
            self.path
        );
        assert_eq!(self.body["chat_id"].as_i64(), Some(chat_id), "{claim}");
        assert_eq!(self.body["text"], text, "{claim}");
    }
}

/// Reads one HTTP/1.1 request (head and `content-length` body) off `s`.
async fn take(mut s: TcpStream) -> Option<Taken> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let end = loop {
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let path = head.split_whitespace().nth(1)?.to_string();
    let len = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                v.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    while buf.len() < end + len {
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = serde_json::from_slice(&buf[end..end + len]).unwrap_or(Value::Null);
    Some(Taken {
        path,
        body,
        stream: s,
    })
}

/// The test's own Bot API: every request arrives on the channel, in the order
/// the client sent it, and waits there for the test.
async fn bot_api() -> (std::net::SocketAddr, mpsc::UnboundedReceiver<Taken>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                if let Some(t) = take(s).await {
                    let _ = tx.send(t);
                }
            });
        }
    });
    (addr, rx)
}

/// The next request the Bot API took, under the hang mark.
async fn next(api: &mut mpsc::UnboundedReceiver<Taken>, what: &str) -> Taken {
    deadline::hang(what, api.recv())
        .await
        .unwrap_or_else(|| panic!("{what}: the Bot API ended"))
}

/// Builds a cell against `addr`. Every operation timeout is the test's product
/// wait: a request the test holds is held, never cut short.
fn cell_against(addr: std::net::SocketAddr) -> ProxyCell {
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        deadline::product_wait_ms(),
        deadline::product_wait_ms(),
        "https://api.telegram.org".into(),
    )
}

fn user_turn(update_id: i64, chat_id: i64) -> ProxyEvent {
    ProxyEvent::UserMessage {
        update_id,
        chat_id,
        user_id: Some(200),
        message_id: Some(7),
        text: "look that up for me".into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_stands_across_an_interim_until_the_answer() {
    let (addr, mut api) = bot_api().await;
    let mut cell = cell_against(addr);
    // The interval is the product wait and the ceiling is far past it: after
    // its first tick the keeper sleeps for longer than the test lives, so no
    // regular tick can be mistaken for the one the interim asks for.
    let interval = Duration::from_millis(deadline::product_wait_ms());
    cell.set_typing_cadence(TypingCadence {
        interval,
        max_total: interval * 4,
    });

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(origin_tx, Path::new("/p"), 64);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = empty_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    // 1. The incoming turn starts the keeper.
    cell.handle_event(user_turn(42, ANSWERED_CHAT_ID), &origin, &mut db)
        .await;
    let _ = origin_rx.recv().await.expect("user turn emitted");
    let first = next(&mut api, "the incoming turn shows the typing status").await;
    first.assert_typing(ANSWERED_CHAT_ID, "the keeper's first tick");
    first.answer().await;

    // 2. The interim goes out. It is not the answer.
    let interim = outgoing(ANSWERED_CHAT_ID, "one moment, checking", true);
    tokio::join!(cell.handle(interim, &sink, &mut db, &rc_tx), async {
        let send = next(&mut api, "the interim's sendMessage reaches the API").await;
        send.assert_message(ANSWERED_CHAT_ID, "one moment, checking", "the interim");
        send.answer().await;
    });

    // 3. The keeper still stands, and it types again at once: the client
    // dropped the status when the interim arrived. This tick is held
    // unanswered, so the keeper waits exactly there when the answer goes out.
    let parked = next(
        &mut api,
        "GH #1504: the typing status stands across an interim - the keeper shows it again \
         right behind the interim (a keeper the interim ended sends nothing here)",
    )
    .await;
    parked.assert_typing(ANSWERED_CHAT_ID, "the tick behind the interim");

    // 4. The answer goes out. That ends the keeper.
    let answer = outgoing(ANSWERED_CHAT_ID, "here is what I found", false);
    tokio::join!(cell.handle(answer, &sink, &mut db, &rc_tx), async {
        let send = next(&mut api, "the answer's sendMessage reaches the API").await;
        send.assert_message(ANSWERED_CHAT_ID, "here is what I found", "the answer");
        send.answer().await;
    });
    deadline::hang(
        "GH #1504: the first message without interim ends the keeper (it gives its held \
         tick up)",
        parked.given_up(),
    )
    .await;
    // Marker on the same path, zero wait: the keeper was parked on that tick
    // and is gone, so nothing followed the answer.
    if let Ok(extra) = api.try_recv() {
        panic!(
            "GH #1504: no chat action may follow the answer (saw {} after the sendMessage)",
            extra.path
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_still_ends_at_its_ceiling() {
    let log = meclaw_testing::log_recorder::global();
    let (addr, mut api) = bot_api().await;
    let mut cell = cell_against(addr);
    // The ceiling is shorter than one interval: once the keeper's first tick
    // came back, one more interval would pass the ceiling, on any host. So
    // its verdict there is "stop", and no clock decides this test.
    cell.set_typing_cadence(TypingCadence {
        interval: Duration::from_millis(deadline::product_wait_ms()),
        max_total: Duration::from_millis(1),
    });

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(origin_tx, Path::new("/p"), 64);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = empty_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    // 1. The incoming turn starts the keeper; its first tick is held.
    cell.handle_event(user_turn(1, CEILING_CHAT_ID), &origin, &mut db)
        .await;
    let _ = origin_rx.recv().await.expect("user turn emitted");
    let first = next(&mut api, "the incoming turn shows the typing status").await;
    first.assert_typing(CEILING_CHAT_ID, "the keeper's first tick");

    // 2. The interim goes out while the keeper waits on that tick. No answer
    // will ever follow it.
    let interim = outgoing(CEILING_CHAT_ID, "one moment, checking", true);
    tokio::join!(cell.handle(interim, &sink, &mut db, &rc_tx), async {
        let send = next(&mut api, "the interim's sendMessage reaches the API").await;
        send.assert_message(CEILING_CHAT_ID, "one moment, checking", "the interim");
        send.answer().await;
    });

    // 3. The held tick comes back. The keeper is past its ceiling and ends on
    // its own: its own line, written right before it returns. The interim
    // neither ended it before that verdict nor lifted the ceiling.
    first.answer().await;
    let chat = format!("chat_id={CEILING_CHAT_ID}");
    let stop = meclaw_testing::wait::until_some(
        "GH #1504: behind an interim the keeper still ends at its own ceiling (a keeper the \
         interim ended never reaches that verdict)",
        || {
            let lines = log.lines(
                tracing::Level::DEBUG,
                &[&chat, "no answer within max_total"],
            );
            async move { lines.into_iter().next() }
        },
    )
    .await;
    assert!(
        stop.contains("ticks=1"),
        "the keeper stopped after the one tick its ceiling allows: {stop}"
    );
    // Marker on the same path, zero wait: the stop line is the keeper's last
    // step, so the status the interim asked for was not sent past the ceiling.
    if let Ok(extra) = api.try_recv() {
        panic!(
            "GH #1504: no chat action may follow the keeper's ceiling (saw {})",
            extra.path
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_ack_typing_ends_at_a_repeat() {
    let (addr, mut api) = bot_api().await;
    let mut cell = cell_against(addr);
    // As in the first pin: the interval is the product wait, so the only tick
    // behind the first one is the one the interim asks for.
    let interval = Duration::from_millis(deadline::product_wait_ms());
    cell.set_typing_cadence(TypingCadence {
        interval,
        max_total: interval * 4,
    });

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(origin_tx, Path::new("/p"), 64);
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = empty_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);

    // 1. The incoming turn starts the keeper.
    cell.handle_event(user_turn(43, REPEAT_CHAT_ID), &origin, &mut db)
        .await;
    let _ = origin_rx.recv().await.expect("user turn emitted");
    let first = next(&mut api, "the incoming turn shows the typing status").await;
    first.assert_typing(REPEAT_CHAT_ID, "the keeper's first tick");
    first.answer().await;

    // 2. The interim goes out; the keeper stands and types again at once.
    // That tick is held, so the keeper waits exactly there.
    let interim = outgoing(REPEAT_CHAT_ID, "here is what I found", true);
    tokio::join!(cell.handle(interim, &sink, &mut db, &rc_tx), async {
        let send = next(&mut api, "the interim's sendMessage reaches the API").await;
        send.assert_message(REPEAT_CHAT_ID, "here is what I found", "the interim");
        send.answer().await;
    });
    let parked = next(&mut api, "the keeper types again behind the interim").await;
    parked.assert_typing(REPEAT_CHAT_ID, "the tick behind the interim");

    // 3. The answer is a repeat of that text: nothing is sent, and it is
    // still the answer of the turn, so it ends the keeper.
    cell.handle(
        repeated(REPEAT_CHAT_ID, "here is what I found"),
        &sink,
        &mut db,
        &rc_tx,
    )
    .await;
    deadline::hang(
        "GH #1504 contract point 2: an answer marked `repeat` ends the keeper (it gives its \
         held tick up)",
        parked.given_up(),
    )
    .await;
    // Marker on the same path, zero wait: the keeper was parked on that tick
    // and is gone, and the repeat itself sent nothing.
    if let Ok(extra) = api.try_recv() {
        panic!(
            "GH #1504: no request may follow a repeated answer (saw {})",
            extra.path
        );
    }
}
