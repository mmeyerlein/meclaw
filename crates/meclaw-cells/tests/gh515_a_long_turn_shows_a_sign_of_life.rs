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
//! The cadence is production-fixed (4 s / 60 s, `TypingCadence::default`); the
//! test drives a scaled-down one through `ProxyCell::set_typing_cadence` so the
//! same mechanism is measured in under two seconds. That seam is a test/ops
//! seam, deliberately NOT a params surface — see its doc comment.

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
use meclaw_testing::mock_http::{CapturedRequest, MockResponse, start_mock_server_capturing};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};

/// The chat this test talks to. A NUMBER everywhere — the recurring trap in
/// this code base is a `chat_id` that turns into a string somewhere on the way
/// to the Bot API.
const CHAT_ID: i64 = 987654321;

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

/// Every request the mock saw whose path names `sendChatAction`.
fn chat_actions(cap: &[CapturedRequest]) -> Vec<&CapturedRequest> {
    cap.iter()
        .filter(|r| r.path.contains("/sendChatAction"))
        .collect()
}

async fn count_chat_actions(cap: &Arc<Mutex<Vec<CapturedRequest>>>) -> usize {
    chat_actions(&cap.lock().await).len()
}

/// Builds a cell against a mock API whose every answer is Telegram's `ok=true`.
fn cell_against(addr: std::net::SocketAddr) -> ProxyCell {
    let client = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    ProxyCell::new(
        client,
        Path::new("/dst"),
        0,
        35000,
        30,
        5000,
        5000,
        "https://api.telegram.org".into(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_incoming_turn_types_until_the_answer_goes_out() {
    let (addr, _j, cap) =
        start_mock_server_capturing(vec![MockResponse::ok_json(br#"{"ok":true,"result":true}"#)])
            .await;
    let mut cell = cell_against(addr);
    cell.set_typing_cadence(TypingCadence {
        interval: Duration::from_millis(50),
        max_total: Duration::from_secs(30),
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

    // 2. The answer is late. The keeper must keep the chat alive meanwhile.
    tokio::time::sleep(Duration::from_millis(260)).await;
    let during = {
        let cap = cap.lock().await;
        let actions = chat_actions(&cap);
        assert!(
            !actions.is_empty(),
            "GH #515: an incoming turn must send at least one sendChatAction"
        );
        for a in &actions {
            assert_eq!(a.method, "POST");
            assert!(a.path.contains("/botT/sendChatAction"), "path: {}", a.path);
            let body: serde_json::Value = serde_json::from_slice(&a.body).unwrap();
            assert_eq!(body["action"], "typing");
            assert!(
                body["chat_id"].is_number(),
                "chat_id must be a NUMBER, got {}",
                body["chat_id"]
            );
            assert_eq!(body["chat_id"].as_i64(), Some(CHAT_ID));
        }
        assert!(
            actions.len() > 1,
            "GH #515: the status decays after ~5s, so it must REPEAT while the \
             answer is late - saw only {} action(s)",
            actions.len()
        );
        actions.len()
    };

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
    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    // The sendMessage landed, and a chat action preceded it.
    {
        let cap = cap.lock().await;
        let idx_send = cap
            .iter()
            .position(|r| r.path.contains("/sendMessage"))
            .expect("sendMessage reached the mock");
        assert!(
            cap[..idx_send]
                .iter()
                .any(|r| r.path.contains("/sendChatAction")),
            "the typing action must precede the answer"
        );
        let body: serde_json::Value = serde_json::from_slice(&cap[idx_send].body).unwrap();
        assert_eq!(body["chat_id"].as_i64(), Some(CHAT_ID));
    }

    // Settle any tick that was already in flight when the answer went out, then
    // hold still for ten further intervals: the count must not move again.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let settled = count_chat_actions(&cap).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        count_chat_actions(&cap).await,
        settled,
        "GH #515: no chat action may follow the answer - the keeper must be cancelled \
         (saw {during} during the turn, {settled} at cancel)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_never_answers_stops_typing_by_itself() {
    let (addr, _j, cap) =
        start_mock_server_capturing(vec![MockResponse::ok_json(br#"{"ok":true,"result":true}"#)])
            .await;
    let mut cell = cell_against(addr);
    cell.set_typing_cadence(TypingCadence {
        interval: Duration::from_millis(50),
        max_total: Duration::from_millis(250),
    });

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let origin = OriginSink::new(origin_tx, Path::new("/p"), 64);

    cell.handle_event(
        ProxyEvent::UserMessage {
            update_id: 1,
            chat_id: CHAT_ID,
            user_id: None,
            message_id: None,
            text: "no answer will ever come".into(),
        },
        &origin,
        &mut db,
    )
    .await;
    let _ = origin_rx.recv().await.expect("user turn emitted");

    // Well past max_total.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let after_timeout = count_chat_actions(&cap).await;
    assert!(
        after_timeout > 1,
        "the keeper must have run at all (saw {after_timeout})"
    );
    assert!(
        after_timeout <= 8,
        "GH #515: the keeper is bounded by max_total - 250ms/50ms is at most a \
         handful of actions, saw {after_timeout}"
    );

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        count_chat_actions(&cap).await,
        after_timeout,
        "GH #515: with no answer at all the keeper must end on its own timeout"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_chat_action_posts_a_numeric_chat_id() {
    let (addr, _j, cap) =
        start_mock_server_capturing(vec![MockResponse::ok_json(br#"{"ok":true,"result":true}"#)])
            .await;
    let c = TelegramClient::new(&format!("http://{addr}"), "T").unwrap();
    c.send_chat_action(CHAT_ID, "typing", Duration::from_millis(5000))
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
        readme.contains(&format!("at most {max_total_s} seconds")),
        "README does not name the ceiling the code keeps ({max_total_s}s)"
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
    // "there is no params key for it" - the shipped config declares none.
    let config = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates/telegram-connector/config.json"),
    )
    .expect("telegram-connector config.json");
    assert!(
        !config.contains("typing"),
        "the README says the cadence is behaviour and not a setting - \
         a params key for it would make that sentence false"
    );
}
