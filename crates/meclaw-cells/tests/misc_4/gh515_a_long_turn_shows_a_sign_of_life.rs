//! GH #515: the telegram connector shows no sign of life while a long turn runs.
//!
//! Telegram's primitive for this is `sendChatAction` with `action=typing`: the
//! client renders "typing…" without a message ever being posted into the chat,
//! and the status decays after roughly five seconds — so it has to be repeated
//! while the turn is still running.
//!
//! What this file pins:
//!
//! 1. an incoming turn starts the keeper — at least one `sendChatAction` with
//!    `action=typing` and a NUMERIC `chat_id` reaches the API;
//! 2. it repeats while the answer is late (more than one action);
//! 3. it ENDS once the answer went out — no further chat action after the
//!    `sendMessage`;
//! 4. it ends on its own when no answer ever comes (the bounded timeout), so a
//!    dead turn cannot leave a repeater running forever.
//!
//! The cadence is production-derived (4 s, ceiling = the turn's backstop,
//! `TypingCadence::for_backstop`, GH #1099); the test drives a scaled-down one
//! through `ProxyCell::set_typing_cadence`.
//!
//! GH #1199 (K9): every claim is an event, none a window. The Bot API here is
//! the test's own ([`bot_api`]): it hands each request to the test with its
//! connection still open, so the test decides when the keeper's next tick may
//! happen, and it sees the keeper GIVE UP a request (the connection closes)
//! -- the cancellation itself, not a quiet half second after it. The keeper's
//! own end is its own log line, after which the requests are counted.

use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::db::setup_proxy_schema;
use meclaw_cells::proxy::io::ProxyEvent;
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_cells::proxy::typing::TypingCadence;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::Map;
use meclaw_core::{
    Body, CellEmission, MessageBuilder, OriginSink, OutputSink, Path, Uuid, serde_json::json,
};
use meclaw_testing::deadline;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// The chat this test talks to. A NUMBER everywhere — the recurring trap in
/// this code base is a `chat_id` that turns into a string somewhere on the way
/// to the Bot API.
const CHAT_ID: i64 = 987654321;

/// The chat of the turn that is never answered: its own number, so the
/// keeper's log line names this test's keeper and no other.
const LONELY_CHAT_ID: i64 = 987654322;

fn context_with_chat_id(chat_id: i64) -> Map<String, meclaw_core::serde_json::Value> {
    let mut m = Map::new();
    m.insert("chat_id".into(), json!(chat_id));
    m
}

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

/// One request the test's Bot API took. Its connection stays open until the
/// test answers it ([`Taken::answer`]) or the client gives it up
/// ([`Taken::given_up`]).
struct Taken {
    method: String,
    path: String,
    body: serde_json::Value,
    stream: TcpStream,
}

impl Taken {
    /// Telegram's `ok=true`, and the connection closes behind it.
    async fn answer(mut self) {
        let body = br#"{"ok":true,"result":true}"#;
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        self.stream
            .write_all(head.as_bytes())
            .await
            .expect("answer head");
        self.stream.write_all(body).await.expect("answer body");
        let _ = self.stream.shutdown().await;
    }

    /// Resolves once the client gave the unanswered request up: its end of
    /// the connection closed (EOF or reset). A client still waiting for the
    /// answer never gets here.
    async fn given_up(mut self) {
        let mut b = [0u8; 64];
        loop {
            match self.stream.read(&mut b).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    }

    /// The request is a `typing` chat action for `chat_id`, with the
    /// `chat_id` a NUMBER on the wire.
    fn assert_typing(&self, chat_id: i64) {
        assert_eq!(self.method, "POST");
        assert!(
            self.path.contains("/botT/sendChatAction"),
            "path: {}",
            self.path
        );
        assert_eq!(self.body["action"], "typing");
        assert!(
            self.body["chat_id"].is_number(),
            "chat_id must be a NUMBER, got {}",
            self.body["chat_id"]
        );
        assert_eq!(self.body["chat_id"].as_i64(), Some(chat_id));
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
    let mut first = head.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
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
    let body = serde_json::from_slice(&buf[end..end + len]).unwrap_or(serde_json::Value::Null);
    Some(Taken {
        method,
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
/// wait (TV-Z9): a request the test holds is held, never cut short.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_incoming_turn_types_until_the_answer_goes_out() {
    let (addr, mut api) = bot_api().await;
    let mut cell = cell_against(addr);
    // The ceiling is far past the test: only the answer ends this keeper.
    cell.set_typing_cadence(TypingCadence {
        interval: Duration::from_millis(50),
        max_total: Duration::from_millis(deadline::product_wait_ms()),
    });

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(origin_tx, Path::new("/p"), 64);

    // 1. The incoming turn.
    cell.handle_event(
        ProxyEvent::UserMessage {
            update_id: 42,
            chat_id: CHAT_ID,
            user_id: Some(200),
            message_id: Some(7),
            text: "how long does this take".into(),
        },
        &origin,
        &mut db,
    )
    .await;
    let _ = origin_rx.recv().await.expect("user turn emitted");

    // 2. The answer is late. The keeper must keep the chat alive meanwhile:
    // a first action, and -- the status decays after ~5 s -- a repeat.
    let first = next(
        &mut api,
        "GH #515: an incoming turn sends at least one sendChatAction",
    )
    .await;
    first.assert_typing(CHAT_ID);
    first.answer().await;
    let second = next(
        &mut api,
        "GH #515: the status decays after ~5s, so it REPEATS while the answer is late",
    )
    .await;
    second.assert_typing(CHAT_ID);
    second.answer().await;
    // The keeper's next tick is held unanswered: the keeper waits on it, so
    // it stands exactly there when the answer goes out.
    let parked = next(&mut api, "the keeper's third tick").await;
    parked.assert_typing(CHAT_ID);

    // 3. The answer goes out. That ends the keeper.
    let msg = MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/sender"))
        .context(context_with_chat_id(CHAT_ID))
        .body(Body::Inline(json!({
            "messages": [
                { "origin": "assistant", "type": "text", "text": "took a while" }
            ]
        })))
        .build();
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = empty_sink(out_tx);
    let (rc_tx, _rc_rx) = mpsc::channel(8);
    tokio::join!(cell.handle(msg, &sink, &mut db, &rc_tx), async {
        // The sendMessage landed, after the typing actions above.
        let send = next(&mut api, "the answer's sendMessage reaches the API").await;
        assert!(
            send.path.contains("/sendMessage"),
            "the request after the typing actions is the answer, got {}",
            send.path
        );
        assert_eq!(send.body["chat_id"].as_i64(), Some(CHAT_ID));
        send.answer().await;
    });

    // The keeper is cancelled: it gives up the tick it was waiting on. A
    // keeper still running would wait for that answer for the whole product
    // wait instead.
    deadline::hang(
        "GH #515: the keeper is cancelled once the answer went out (it gives its held tick up)",
        parked.given_up(),
    )
    .await;
    // Marker on the same path, zero wait: the keeper was parked on that tick
    // and is gone, so nothing followed the answer.
    if let Ok(extra) = api.try_recv() {
        panic!(
            "GH #515: no chat action may follow the answer - the keeper must be cancelled \
             (saw {} after the sendMessage)",
            extra.path
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_never_answers_stops_typing_by_itself() {
    let log = meclaw_testing::log_recorder::global();
    let (addr, mut api) = bot_api().await;
    let mut cell = cell_against(addr);
    // The ceiling leaves the first round trip a whole hang floor, scaled,
    // before the keeper may stop, so a repeat is owed on any host; the
    // keeper's own clock decides when it ends, the test only waits for it to
    // say so. A first round trip past that floor is a hang and is named as one
    // below (re-check RC2 M1, as OR-HB.1 for gh1037/gh146) -- only when the
    // hang mark below has run out too, so a keeper that stops early on its
    // own is judged on its content (re-check RC-T4 I1).
    let interval = Duration::from_millis(500);
    let max_total = interval + deadline::scaled(deadline::FLOOR);
    cell.set_typing_cadence(TypingCadence {
        interval,
        max_total,
    });

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(origin_tx, Path::new("/p"), 64);

    // The Bot API answers every request, and books it BEFORE answering: the
    // keeper's next step waits on that answer, so the book is complete the
    // moment the keeper stops.
    let (seen_tx, mut seen) = mpsc::unbounded_channel::<(String, Option<i64>, String)>();
    tokio::spawn(async move {
        while let Some(t) = api.recv().await {
            let _ = seen_tx.send((
                t.path.clone(),
                t.body["chat_id"].as_i64(),
                t.body["action"].as_str().unwrap_or_default().to_string(),
            ));
            t.answer().await;
        }
    });

    // The hang mark, never a verdict: the wall clock from the turn on, read
    // only to name a lone tick a hang once a whole floor has passed.
    let hang_mark = tokio::time::Instant::now();
    cell.handle_event(
        ProxyEvent::UserMessage {
            update_id: 1,
            chat_id: LONELY_CHAT_ID,
            user_id: None,
            message_id: None,
            text: "no answer will ever come".into(),
        },
        &origin,
        &mut db,
    )
    .await;
    let _ = origin_rx.recv().await.expect("user turn emitted");

    // The keeper ends on its own: its own line, written after its last
    // request came back and right before it returns.
    let chat = format!("chat_id={LONELY_CHAT_ID}");
    let stop = meclaw_testing::wait::until_some(
        "GH #515: with no answer at all the keeper ends on its own timeout",
        || {
            let lines = log.lines(
                tracing::Level::DEBUG,
                &[&chat, "no answer within max_total"],
            );
            async move { lines.into_iter().next() }
        },
    )
    .await;
    let ticks: usize = stop
        .split_whitespace()
        .find_map(|f| f.strip_prefix("ticks="))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("the keeper's stop line names its ticks: {stop}"));
    // One tick only AND the scaled floor has passed since the turn: the
    // first round trip ate the whole floor, so no repeat was owed -- a hang of
    // the host, never a verdict on the keeper. Two signals, as OR-HB.1: a
    // keeper that stops after one tick inside the floor is a keeper defect and
    // falls through to the content verdict right below.
    let floor = deadline::scaled(deadline::FLOOR);
    if ticks <= 1 && hang_mark.elapsed() >= floor {
        panic!(
            "HANG: the first round trip did not fit the ceiling, the hang floor scaled \
             ({} ms), so no repeat was owed: {stop}",
            floor.as_millis()
        );
    }
    assert!(
        ticks > 1,
        "GH #515: the keeper stopped after {ticks} tick(s) although its first round trip \
         fit the ceiling ({} ms of floor left a repeat owed) - it must keep the sign of \
         life up until max_total: {stop}",
        floor.as_millis()
    );

    let mut actions = Vec::new();
    while let Ok(r) = seen.try_recv() {
        actions.push(r);
    }
    for (path, chat_id, action) in &actions {
        assert!(path.contains("/botT/sendChatAction"), "path: {path}");
        assert_eq!(*chat_id, Some(LONELY_CHAT_ID));
        assert_eq!(action, "typing");
    }
    let after_timeout = actions.len();
    assert!(
        after_timeout > 1,
        "the keeper must have run at all (saw {after_timeout})"
    );
    // Every tick sleeps one interval before the next, and the keeper stops
    // once one more would pass max_total: at most max_total/interval + 1.
    let bound = (max_total.as_millis() / interval.as_millis()) as usize + 1;
    assert!(
        after_timeout <= bound,
        "GH #515: the keeper is bounded by max_total - {}ms/{}ms is at most {bound} \
         actions, saw {after_timeout}",
        max_total.as_millis(),
        interval.as_millis()
    );
    assert_eq!(
        after_timeout, ticks,
        "GH #515: with no answer at all the keeper must end on its own timeout - every \
         tick reached the API and none followed its last"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_chat_action_posts_a_numeric_chat_id() {
    let (addr, _j, cap) =
        start_mock_server_capturing(vec![MockResponse::ok_json(br#"{"ok":true,"result":true}"#)])
            .await;
    let c = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    c.send_chat_action(
        CHAT_ID,
        "typing",
        Duration::from_millis(deadline::product_wait_ms()),
    )
    .await
    .unwrap();

    let cap = cap.lock().await;
    assert_eq!(cap[0].method, "POST");
    assert!(cap[0].path.contains("/botT/sendChatAction"));
    let body: serde_json::Value = serde_json::from_slice(&cap[0].body).unwrap();
    assert!(body["chat_id"].is_number(), "chat_id must be a NUMBER");
    assert_eq!(body["chat_id"], CHAT_ID);
    assert_eq!(body["action"], "typing");
}

/// Drift lock (`docs/development-rules.md` § 2d): the template README states the
/// typing behaviour in numbers, so the numbers are DERIVED from the code here
/// rather than written down a second time. A cadence change that leaves the
/// prose behind is red.
#[test]
fn the_readme_names_the_cadence_the_code_actually_keeps() {
    let readme = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates/telegram-connector/README.md"),
    )
    .expect("telegram-connector README");

    let cadence = TypingCadence::default();
    let interval_s = cadence.interval.as_secs();
    // GH #1099: the default ceiling is the colony's default backstop.
    assert_eq!(
        cadence.max_total,
        Duration::from_millis(meclaw_colony::DEFAULT_MESSAGE_TIMEOUT_MS)
    );
    let max_total_s = cadence.max_total.as_secs();

    // The sentence, and the mechanism behind each half of it.
    assert!(
        readme.contains("`sendChatAction` with\n`action=typing`")
            || readme.contains("`sendChatAction` with `action=typing`"),
        "the README must name the primitive it relies on"
    );
    assert!(
        readme.contains(&format!("every {interval_s} seconds")),
        "README does not name the interval the code keeps ({interval_s}s)"
    );
    assert!(
        readme.contains(&format!("{max_total_s} seconds by default")),
        "README does not name the ceiling the code keeps ({max_total_s}s)"
    );
    assert!(
        readme.contains("`typing_max_ms`"),
        "README does not name the key that sets the ceiling"
    );
    // The margin the prose claims under Telegram's ~5s decay.
    assert!(
        cadence.interval < Duration::from_secs(5),
        "the interval must sit under Telegram's ~5s decay"
    );
    assert!(
        cadence.max_total > cadence.interval,
        "the ceiling must allow at least one refresh"
    );
    // GH #1099 (review fix1): the shipped connector names the ceiling of the
    // turn that answers it -- `typing_max_ms`, the only typing key it has; the
    // number itself is held to the answering brain's backstop in
    // `proxy::typing::tests::the_ceiling_is_the_backstop_of_the_answering_turn`.
    let config = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates/telegram-connector/config.json"),
    )
    .expect("telegram-connector config.json");
    assert_eq!(
        config.matches("typing").count(),
        1,
        "the shipped connector carries exactly one typing key, `typing_max_ms`"
    );
    assert!(config.contains("\"typing_max_ms\""));
}
