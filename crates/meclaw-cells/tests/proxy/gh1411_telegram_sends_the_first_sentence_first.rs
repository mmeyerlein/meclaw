//! GH #1411 (D53 X2): a Telegram connector with `partial_delivery:
//! "first_sentence"` sends the first `partial` of a turn at once and only the
//! rest with the final answer -- at most two messages per turn from the
//! partial path, no sentence twice. `"off"` (the default) drops every partial silently and
//! leaves every other request byte-identical.
//!
//! The partials are fixture messages of the llm lane contract (D53 X1): hop
//! `route: "partial"`, `seq`, `final: false`, `partial_of`, and a body with ONE
//! cumulative assistant text.

use meclaw_cells::proxy::cell::ProxyCell;
use meclaw_cells::proxy::io::ProxyEvent;
use meclaw_cells::proxy::partial::PartialDelivery;
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{
    Body, CellEmission, Message, MessageBuilder, OriginSink, OutputSink, Path, Uuid,
    serde_json::json,
};
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

const CHAT: i64 = 4242;

fn context() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("chat_id".into(), json!(CHAT));
    m
}

fn hop(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn assistant(text: &str) -> Body {
    Body::Inline(json!({"messages": [{"origin": "assistant", "type": "text", "text": text}]}))
}

/// One partial of the lane contract: cumulative text, `seq` n.
fn partial(text: &str, seq: u64) -> Message {
    MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/quick"))
        .context(context())
        .hop(hop(&[
            ("route", json!("partial")),
            ("seq", json!(seq)),
            ("final", json!(false)),
            ("partial_of", json!(Uuid::now_v7().to_string())),
        ]))
        .body(assistant(text))
        .build()
}

/// A finished answer as it reaches the connector (`route answer`), interim or not.
fn answer(text: &str, interim: bool) -> Message {
    let mut h = vec![("route", json!("answer"))];
    if interim {
        h.push(("interim", json!("1")));
    }
    MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/collector"))
        .context(context())
        .hop(hop(&h))
        .body(assistant(text))
        .build()
}

/// The error a model call ends with after partials (`finish_reason error`).
fn error() -> Message {
    MessageBuilder::new(Path::new("/p"))
        .reply_to(Path::new("/quick"))
        .context(context())
        .hop(hop(&[
            ("finish_reason", json!("error")),
            ("error_code", json!("provider_error")),
        ]))
        .body(Body::Inline(
            json!({"messages": [], "meta": {"detail": "stream ended"}}),
        ))
        .build()
}

struct Rig {
    cell: ProxyCell,
    cap: Arc<Mutex<Vec<CapturedRequest>>>,
    sink: OutputSink,
    out: mpsc::Receiver<CellEmission>,
    origin: OriginSink,
    _origin_rx: mpsc::Receiver<CellEmission>,
    db: DbConn,
    rc: mpsc::Sender<meclaw_cells::proxy::io::ProxyReconfig>,
    _rc_rx: mpsc::Receiver<meclaw_cells::proxy::io::ProxyReconfig>,
}

async fn rig(mode: Option<PartialDelivery>) -> Rig {
    let (addr, _j, cap) =
        start_mock_server_capturing(vec![MockResponse::ok_json(br#"{"ok":true,"result":{}}"#)])
            .await;
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    let cell = ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        5000,
        5000,
        "https://api.telegram.org".into(),
    );
    let cell = match mode {
        Some(m) => cell.with_partial_delivery(m),
        None => cell,
    };
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(16);
    let sink = OutputSink::new(
        out_tx,
        Path::new("/p"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );
    let (origin_tx, origin_rx) = mpsc::channel::<CellEmission>(16);
    let origin = OriginSink::new(origin_tx, Path::new("/p"), 8);
    let (rc, rc_rx) = mpsc::channel(8);
    let db = DbConn::wrap(rusqlite::Connection::open_in_memory().unwrap(), None);
    Rig {
        cell,
        cap,
        sink,
        out: out_rx,
        origin,
        _origin_rx: origin_rx,
        db,
        rc,
        _rc_rx: rc_rx,
    }
}

impl Rig {
    async fn feed(&mut self, msg: Message) {
        self.cell
            .handle(msg, &self.sink, &mut self.db, &self.rc)
            .await;
    }

    /// The chat's user writes again (the connector's inbound side).
    async fn user_writes(&mut self, update_id: i64, text: &str) {
        let event = ProxyEvent::UserMessage {
            update_id,
            chat_id: CHAT,
            user_id: Some(7),
            message_id: Some(update_id),
            text: text.to_string(),
        };
        self.cell
            .handle_event(event, &self.origin, &mut self.db)
            .await;
    }

    /// Every captured request as `(method path tail, body)`.
    async fn wire(&self) -> Vec<(String, String)> {
        self.cap
            .lock()
            .await
            .iter()
            .map(|r| {
                let tail = r.path.rsplit('/').next().unwrap_or_default().to_string();
                (tail, String::from_utf8_lossy(&r.body).into_owned())
            })
            .collect()
    }

    async fn sent_texts(&self) -> Vec<String> {
        self.wire()
            .await
            .into_iter()
            .filter(|(m, _)| m == "sendMessage")
            .map(|(_, b)| {
                let v: Value = meclaw_core::serde_json::from_str(&b).unwrap();
                v["text"].as_str().unwrap_or_default().to_string()
            })
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_sentence_is_sent_once_and_the_rest_once() {
    let mut r = rig(Some(PartialDelivery::FirstSentence)).await;
    r.feed(partial("Yes, sure.", 1)).await;
    r.feed(partial("Yes, sure. I will check.", 2)).await;
    r.feed(answer("Yes, sure. I will check. It is 12 degrees.", false))
        .await;

    let methods: Vec<String> = r.wire().await.into_iter().map(|(m, _)| m).collect();
    assert_eq!(
        methods,
        vec!["sendMessage", "sendChatAction", "sendMessage"],
        "first sentence, typing, rest -- in that order and nothing else"
    );
    assert_eq!(
        r.sent_texts().await,
        vec!["Yes, sure.", "I will check. It is 12 degrees."],
        "the rest is the answer minus the sent prefix"
    );
    let typing: Value = meclaw_core::serde_json::from_str(&r.wire().await[1].1).unwrap();
    assert_eq!(typing["action"], "typing");
    assert_eq!(
        r.cell.open_partial_turns(),
        0,
        "the final answer ends the turn"
    );

    // An answer that is exactly what the partial showed sends nothing at all.
    r.feed(partial("Gut.", 1)).await;
    r.feed(answer("Gut.", false)).await;
    assert_eq!(
        r.sent_texts().await.len(),
        3,
        "an empty rest is no message (one early message for the second turn)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_interim_sentence_already_shown_is_not_sent_again() {
    // quick writes its bridge sentence beside a tool call: the partial shows
    // it, then the dispatcher sends the same sentence as `interim`, then the
    // brain answers -- with partials of its own, which must not open a second
    // early message. Without partial_delivery this turn is two messages.
    let mut r = rig(Some(PartialDelivery::FirstSentence)).await;
    r.feed(partial("Hold on, I will check.", 1)).await;
    r.feed(answer("Hold on, I will check.", true)).await;
    r.feed(partial("In Berlin it is 12 degrees.", 1)).await;
    r.feed(answer("In Berlin it is 12 degrees. It stays dry.", false))
        .await;
    assert_eq!(
        r.sent_texts().await,
        vec![
            "Hold on, I will check.",
            "In Berlin it is 12 degrees. It stays dry."
        ],
        "two messages, as without partial_delivery; the interim is not repeated"
    );
    assert_eq!(r.cell.open_partial_turns(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_after_partials_drops_the_rest() {
    let mut r = rig(Some(PartialDelivery::FirstSentence)).await;
    r.feed(partial("Yes, sure.", 1)).await;
    r.feed(error()).await;
    // The error appends nothing; the early message stays recorded until a
    // final answer (OR-D53.X2.12: the error may be the other branch's), so
    // the next turn's partial opens no early message and its answer, which
    // does not begin with the shown prefix, goes whole.
    r.feed(partial("New.", 1)).await;
    r.feed(answer("New. It goes on.", false)).await;
    assert_eq!(
        r.sent_texts().await,
        vec!["Yes, sure.", "New. It goes on."],
        "nothing of the failed turn is appended, nothing twice, nothing lost"
    );
    assert_eq!(r.cell.open_partial_turns(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_of_the_other_branch_does_not_repeat_the_first_sentence() {
    // brain's first sentence is out, then quick's call fails, then brain's
    // answer arrives: it still sends only the rest.
    let mut r = rig(Some(PartialDelivery::FirstSentence)).await;
    r.feed(partial("In Berlin it is 12 degrees.", 1)).await;
    r.feed(error()).await;
    r.feed(answer("In Berlin it is 12 degrees. It stays dry.", false))
        .await;
    assert_eq!(
        r.sent_texts().await,
        vec!["In Berlin it is 12 degrees.", "It stays dry."]
    );
    assert_eq!(r.cell.open_partial_turns(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_user_message_mid_answer_does_not_repeat_the_first_sentence() {
    // The user writes again while the answer is still being written: the
    // old turn's later partial opens no second early message, and its answer
    // sends only the rest.
    let mut r = rig(Some(PartialDelivery::FirstSentence)).await;
    r.feed(partial("Yes, sure.", 1)).await;
    r.user_writes(1, "And tomorrow?").await;
    r.feed(partial("Yes, sure. I will check.", 2)).await;
    r.feed(answer("Yes, sure. I will check. It is 12 degrees.", false))
        .await;
    assert_eq!(
        r.sent_texts().await,
        vec!["Yes, sure.", "I will check. It is 12 degrees."]
    );
    assert_eq!(r.cell.open_partial_turns(), 0);

    // An answer that never comes: the next turn's answer goes whole.
    r.feed(partial("Alt.", 1)).await;
    r.user_writes(2, "Hello?").await;
    r.feed(partial("New.", 1)).await;
    r.feed(answer("New. It goes on.", false)).await;
    assert_eq!(
        r.sent_texts().await[2..].to_vec(),
        vec!["Alt.", "New. It goes on."]
    );
    assert_eq!(r.cell.open_partial_turns(), 0, "at most one entry, cleared");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn whitespace_around_the_first_sentence_does_not_repeat_it() {
    let mut r = rig(Some(PartialDelivery::FirstSentence)).await;
    r.feed(partial("\nYes, sure. ", 1)).await;
    r.feed(answer("\nYes, sure. It is 12 degrees.\n", false))
        .await;
    assert_eq!(
        r.sent_texts().await,
        vec!["Yes, sure.", "It is 12 degrees.\n"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn off_drops_a_partial_before_any_check() {
    // A partial without chat_id under "off": dropped before the checks that
    // answer each message with an error.
    for mode in [None, Some(PartialDelivery::Off)] {
        let mut r = rig(mode).await;
        let mut p = partial("Yes, sure.", 1);
        p.headers.context.remove("chat_id");
        r.feed(p).await;
        assert!(r.out.try_recv().is_err(), "mode {mode:?}: no emission");
        assert!(r.wire().await.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn off_leaves_every_channel_byte_identical() {
    // The default and an explicit "off" drop every partial silently (the
    // member wires the lane always; OR-D53.X2.3) and send everything else
    // exactly as before the lane existed: every answer as its own message, no
    // chat action, the same body bytes.
    for mode in [None, Some(PartialDelivery::Off)] {
        let mut r = rig(mode).await;
        r.feed(partial("Yes, sure.", 1)).await;
        r.feed(partial("Yes, sure. I will check.", 2)).await;
        r.feed(answer("Yes, sure. It is 12 degrees.", false)).await;
        r.feed(answer("Moment.", true)).await;
        r.feed(error()).await;
        let wire = r.wire().await;
        assert_eq!(
            wire,
            vec![
                (
                    "sendMessage".to_string(),
                    format!(r#"{{"chat_id":{CHAT},"text":"Yes, sure. It is 12 degrees."}}"#)
                ),
                (
                    "sendMessage".to_string(),
                    format!(r#"{{"chat_id":{CHAT},"text":"Moment."}}"#)
                ),
            ],
            "mode {mode:?}"
        );
        assert_eq!(r.cell.open_partial_turns(), 0);
    }
}
