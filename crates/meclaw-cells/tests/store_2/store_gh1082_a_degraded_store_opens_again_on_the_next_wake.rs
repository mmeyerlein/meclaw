//! GH #1082 — a `store` that woke degraded opens its database again on the
//! next wake.
//!
//! A store whose `cell.db` cannot be opened at wake starts degraded
//! (`wake_degraded`, issue #57) and answers every message with a named error.
//! Before #1082 that degraded cell never went back to sleep: it was a stateless
//! dispatcher without an idle arm, so the entry stayed `Awake` for the life of
//! the process and the `WakeFn` that would have tried the open again was never
//! called. In the lab colony that filed the issue the cause was fd exhaustion
//! (226 `cell.db` × 3 fds plus ~216 pipes against a soft limit of 1024); once
//! the pressure was gone, the store still refused everything until a restart.
//!
//! Now the degraded cell answers one message and goes to sleep through the
//! colony's ordinary sleep path (`ColonyMsg::Sleep`), so the next message wakes
//! it again and the wake tries the open again. The test runs that through a
//! real colony: degraded first answer, cause removed, healthy second answer,
//! and the second insert is really in the database.
//!
//! Topology class (four workers): events only — every wait is a reply at the
//! sink under a hang mark, nothing is timed.

use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::deadline::hang;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use tokio::sync::mpsc;

const STORE: &str = "/notes";
const SINK: &str = "/sink";

/// One `tool_call` against the store, answered to the sink.
fn call(id: &str, args: JsonValue) -> Message {
    MessageBuilder::new(Path::new(STORE))
        .reply_to(Path::new(SINK))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant",
            "type": "tool_call",
            "text": args.to_string(),
            "id": id,
        }]})))
        .build()
}

fn body(reply: &Message) -> JsonValue {
    match &reply.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("inline body expected"),
    }
}

fn hop(reply: &Message, key: &str) -> Option<String> {
    reply.headers.hop.get(key).map(|v| {
        v.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string())
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_degraded_store_opens_its_database_again_on_the_next_wake() {
    let td = tempfile::TempDir::new().unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(StoreCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("store".to_string(), factory)]);
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new(SINK), move || CaptureCell::new(sink_tx.clone()))
        .await;

    // A directory where the database file belongs: `sqlite3_open` fails with
    // SQLITE_CANTOPEN, the same class as an exhausted fd table, and it needs no
    // permission games (root-proof).
    let cell_dir = td.path().join("notes-cell");
    std::fs::create_dir_all(cell_dir.join("cell.db")).unwrap();

    // The real store factory, dormant until the first message wakes it. No idle
    // timeout: a healthy store with `cell.timeout: -1` never sleeps, and a
    // degraded one must still try the open again.
    let spawned = Arc::new(StoreCellFactory)
        .spawn_cell(
            Path::new(STORE),
            json!({"schema": {"items": {"id": "int", "name": "text"}}}),
            h.outputs_sender(),
            cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            h.runtime().inbox_tx,
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("params are well-formed");
    h.register_spawned_typed(Path::new(STORE), spawned, "store")
        .await;
    // An emission without a matching out-edge dead-letters as `no_route`
    // (ruling A1), so the store's answers get their way to the sink.
    h.add_edge(Uuid::now_v7(), Path::new(STORE), Path::new(SINK))
        .await;

    // 1. The first message wakes the store, the open fails, the cell answers
    //    degraded.
    h.send(call(
        "call_1",
        json!({"operation": "insert", "table": "items", "row": {"id": 1, "name": "a"}}),
    ))
    .await;
    let first = hang("the degraded reply to the first insert", sink_rx.recv())
        .await
        .expect("sink open");
    assert_eq!(
        hop(&first, "finish_reason").as_deref(),
        Some("error"),
        "the store woke degraded: {:?} {}",
        first.headers.hop,
        body(&first)
    );
    let text = body(&first)["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(text.contains("cell.db"), "the defect is named: {text}");

    // 2. The cause goes away.
    std::fs::remove_dir(cell_dir.join("cell.db")).unwrap();

    // 3. The next message wakes the store again, the open succeeds, the insert
    //    lands.
    h.send(call(
        "call_2",
        json!({"operation": "insert", "table": "items", "row": {"id": 2, "name": "b"}}),
    ))
    .await;
    let second = hang(
        "the reply to the insert after the cause is gone",
        sink_rx.recv(),
    )
    .await
    .expect("sink open");
    assert_eq!(
        hop(&second, "error_code"),
        None,
        "the store opened its database again on the next wake instead of staying \
         degraded: {:?} {}",
        second.headers.hop,
        body(&second)
    );
    assert_eq!(
        hop(&second, "rows_affected").as_deref(),
        Some("1"),
        "the insert landed: {:?}",
        second.headers.hop
    );

    // 4. The row is in the database, not in a substitute: read it back.
    h.send(call(
        "call_3",
        json!({"operation": "select", "table": "items", "columns": ["id", "name"]}),
    ))
    .await;
    let rows = hang("the reply to the read-back", sink_rx.recv())
        .await
        .expect("sink open");
    let b = body(&rows);
    let payload: JsonValue = meclaw_core::serde_json::from_str(
        b["messages"][0]["text"]
            .as_str()
            .expect("the select answers with text"),
    )
    .unwrap_or(JsonValue::Null);
    assert_eq!(
        payload,
        json!([{"id": 2, "name": "b"}]),
        "exactly the second insert is stored — the degraded first one wrote nothing: {b}"
    );
    assert!(
        cell_dir.join("cell.db").is_file(),
        "the database is the store's own file"
    );

    h.shutdown().await;
}

/// The `vault` carries the same degraded cell (`DegradedVaultCell`, a vault
/// without storage looks locked) and had the same defect: a resident dispatcher
/// that never slept, so the `WakeFn` that would retry the open never ran again.
///
/// Driven the way the colony drives it, one hop at a time: the wake, one
/// answer, then the EVENT the fix adds — the degraded cell hands its mailbox
/// back with `ColonyMsg::Sleep` — and the second wake on exactly that mailbox,
/// which `handle_sleep` / the next routed message would perform.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_degraded_vault_sleeps_after_its_answer_and_the_next_wake_opens_again() {
    use meclaw_colony::{ColonyMsg, SpawnedCellKind};
    const VAULT: &str = "/main/vault";

    let td = tempfile::TempDir::new().unwrap();
    let cell_dir = td.path().join("vault");
    std::fs::create_dir_all(cell_dir.join("cell.db")).unwrap();
    let (otx, mut orx) = mpsc::channel(16);
    let (itx, mut irx) = mpsc::channel::<ColonyMsg>(64);
    let spawned = Arc::new(meclaw_cells::vault::VaultCellFactory)
        .spawn_cell(
            Path::new(VAULT),
            json!({"broker": "/main/invoke"}),
            otx,
            cell_dir.clone(),
            meclaw_colony::ContractView::default(),
            itx,
            None,
            0,
            None,
            None,
            16,
        )
        .expect("params are well-formed");
    let SpawnedCellKind::Dormant {
        sender,
        receiver,
        wake,
        ..
    } = spawned
    else {
        unreachable!("the stateful vault factory returns Dormant");
    };
    let status = || {
        MessageBuilder::new(Path::new(VAULT))
            .body(Body::Inline(json!({"messages": [{
                "origin": "assistant", "type": "tool_call",
                "text": r#"{"operation":"status"}"#, "id": "call_v"
            }]})))
            .build()
    };

    let (_stop_1, _ack_1) = wake(receiver);
    sender.send(status()).await.expect("mailbox live");
    let first: meclaw_core::CellEmission = hang("the degraded vault answers", orx.recv())
        .await
        .expect("outputs open");
    let text = first.content["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        text.contains("could not be opened"),
        "woke degraded: {text}"
    );

    let receiver = hang(
        "the degraded vault hands its mailbox back to sleep",
        async {
            loop {
                match irx.recv().await {
                    Some(ColonyMsg::Sleep { receiver, .. }) => break receiver,
                    Some(_) => continue,
                    None => panic!("the colony inbox closed"),
                }
            }
        },
    )
    .await;

    std::fs::remove_dir(cell_dir.join("cell.db")).unwrap();
    let (_stop_2, _ack_2) = wake(receiver);
    sender.send(status()).await.expect("mailbox live");
    let second: meclaw_core::CellEmission =
        hang("the vault answers after the second wake", orx.recv())
            .await
            .expect("outputs open");
    let text = second.content["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !text.contains("could not be opened"),
        "the second wake opened the vault's database: {:?}",
        second.content
    );
    assert!(
        cell_dir.join("cell.db").is_file(),
        "the vault's own cell.db"
    );
}
