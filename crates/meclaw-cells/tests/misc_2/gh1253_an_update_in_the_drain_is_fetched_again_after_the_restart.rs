//! GH #1253: a Telegram update that arrives while the colony drains for
//! shutdown is not lost.
//!
//! The colony refuses every new source emission during the drain (GH #47). The
//! proxy used to hand every polled update to its handler, which saved the cursor
//! past it and emitted it into that refusal, while the I/O task polled on with
//! the advanced offset, which confirms the update to Telegram for good. Now the
//! I/O task reads the drain (`DrainView`): a poll that returns into the drain
//! hands nothing over and polls no further, so neither the cursor nor Telegram
//! moves past the update, and the restarted proxy fetches it again.
//!
//! Event-bound: the liveness mark of the returned poll is the marker that the
//! I/O task has the update in hand; the end of the I/O task closes the window.

use meclaw_cells::proxy::io::{ProxyEvent, ProxyReconfig, RunIoConfig, run_io_with_drain};
use meclaw_cells::proxy::telegram::TelegramClient;
use meclaw_colony::{ColonyMsg, DrainView, IoLivenessMark};
use meclaw_core::Path;
use meclaw_testing::deadline::HANG;
use meclaw_testing::mock_http::{MockResponse, start_mock_server_capturing};
use tokio::sync::mpsc;

const ONE_UPDATE: &[u8] = br#"{
  "ok": true,
  "result": [
    {"update_id": 42,
     "message": {"message_id": 7, "chat": {"id": 100}, "from": {"id": 200},
                 "text": "hello"}}
  ]
}"#;

fn cfg(base_url: &str, inbox: mpsc::Sender<ColonyMsg>) -> RunIoConfig {
    RunIoConfig {
        client: TelegramClient::new(base_url, "TOKEN").unwrap(),
        // The cursor `cell.db` holds: nothing past update 42 was ever saved.
        initial_offset: 0,
        long_poll_request_secs: 1,
        long_poll_timeout_ms: 5_000,
        liveness: IoLivenessMark::new(Path::new("/telegram"), Some(inbox)),
    }
}

async fn next_liveness(inbox: &mut mpsc::Receiver<ColonyMsg>) {
    match tokio::time::timeout(HANG, inbox.recv()).await {
        Ok(Some(ColonyMsg::IoLiveness { .. })) => {}
        Ok(_) => panic!("expected an I/O liveness mark"),
        Err(_) => panic!("HANG: no I/O liveness mark"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_update_in_the_drain_is_fetched_again_after_the_restart() {
    // 1. Incarnation one: the colony drains; the poll in flight returns update 42.
    let (addr, _server, captured) =
        start_mock_server_capturing(vec![MockResponse::ok_json(ONE_UPDATE)]).await;
    let (inbox_tx, mut inbox_rx) = mpsc::channel::<ColonyMsg>(8);
    let (drain_tx, drain) = DrainView::pair();
    drain_tx.send_replace(true);
    let (events_tx, mut events_rx) = mpsc::channel::<ProxyEvent>(8);
    let (rc_tx, rc_rx) = mpsc::channel::<ProxyReconfig>(1);
    let io = tokio::spawn(run_io_with_drain(
        cfg(&format!("http://{addr}"), inbox_tx),
        drain,
        events_tx,
        rc_rx,
    ));
    next_liveness(&mut inbox_rx).await; // announce
    next_liveness(&mut inbox_rx).await; // the poll returned with update 42
    drop(rc_tx); // the colony's shutdown closes the reconfig lane
    tokio::time::timeout(HANG, io)
        .await
        .expect("HANG: the I/O task did not end")
        .unwrap();
    assert!(
        events_rx.try_recv().is_err(),
        "an update that arrives in the drain is not handed over (its cursor would be saved \
         past it and its emission refused)"
    );
    assert!(
        !captured
            .lock()
            .await
            .iter()
            .any(|r| r.path.contains("offset=43")),
        "no poll past update 42 in the drain: Telegram still holds it"
    );

    // 2. The restart: from the unchanged cursor, Telegram hands update 42 over again.
    let (addr, _server, captured) =
        start_mock_server_capturing(vec![MockResponse::ok_json(ONE_UPDATE)]).await;
    let (inbox_tx, _inbox_rx) = mpsc::channel::<ColonyMsg>(8);
    let (events_tx, mut events_rx) = mpsc::channel::<ProxyEvent>(8);
    let (rc_tx, rc_rx) = mpsc::channel::<ProxyReconfig>(1);
    let io = tokio::spawn(run_io_with_drain(
        cfg(&format!("http://{addr}"), inbox_tx),
        DrainView::default(),
        events_tx,
        rc_rx,
    ));
    let event = tokio::time::timeout(HANG, events_rx.recv())
        .await
        .expect("HANG: the restarted proxy fetched nothing")
        .expect("the I/O task ended without an event");
    assert_eq!(event.update_id(), 42, "the update the drain held back");
    assert!(captured.lock().await[0].path.contains("offset=0"));
    drop(rc_tx);
    let _ = tokio::time::timeout(HANG, io).await;
}
