//! GH #922 — a one-shot marked `catch_up` that a stop made the timer miss
//! fires once after the next boot, marked `late`; an unmarked one does not.
//!
//! The acceptance sentence of the issue, measured at the receiver (the sink
//! a colony delivers to) and at the seam (the row in the timer's `cell.db`):
//!
//! 1. Colony 1 takes two `at` rows due a few seconds ahead — one with
//!    `catch_up: true` (armed through `add … rearm: true` on a fresh id, which
//!    is an insert: the stale-strike guard of GH #904 never sees
//!    `at != scheduled_at` here — a catch-up is planned as `At(at)` and struck
//!    for exactly that `at`, so the guard cannot drop it), one without — and
//!    refuses `catch_up` on a `cron` row with `invalid_params` /
//!    `catch_up_cron_unsupported`. It is stopped before either row is due.
//! 2. With the colony down both moments pass; both rows are still `active`
//!    (nothing fired while it was down).
//! 3. Colony 2 boots on the same `cell.db`: exactly one strike arrives — the
//!    marked row, with `late: true` and its own `scheduled_at` — and the row
//!    is `completed`; the unmarked row stays silent (and `active`, as before).
//! 4. Colony 3 boots once more: nothing arrives.

use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::{ColonyHandle, topologies::phase_3a::CaptureCell};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// One colony on the given timer directory: `/sink` first (anti-cascade),
/// then the timer through its factory, then the edge. Every boot uses the
/// same `cell_dir`, so the second and third boot resume the first one's
/// `cell.db` — the restart the issue is about.
async fn boot(cell_dir: &std::path::Path) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new();
    let (recv_tx, recv_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;
    let spawned = Arc::new(TimerCellFactory)
        .spawn_cell(
            Path::new("/timer"),
            json!({}),
            h.runtime().outputs_tx,
            cell_dir.to_path_buf(),
            meclaw_colony::ContractView::default(),
            h.inbox_tx.clone(),
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("spawn timer");
    h.register_spawned(Path::new("/timer"), spawned).await;
    h.add_edge(Uuid::now_v7(), Path::new("/timer"), Path::new("/sink"))
        .await;
    (h, recv_rx)
}

/// A timer op as a dispatcher delivers it: one `tool_call` turn, so every
/// op is answered (ack or error) and the test knows it was stored.
fn op_call(args: meclaw_core::JsonValue, call_id: &str) -> Message {
    MessageBuilder::new(Path::new("/timer"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({"messages":[{
            "origin": "assistant", "type": "tool_call",
            "text": args.to_string(), "id": call_id
        }]})))
        .build()
}

async fn receipt(rx: &mut mpsc::Receiver<Message>) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no message at /sink within 30s")
        .expect("sink channel closed")
}

/// Asserts that nothing reaches `/sink` for `secs`.
async fn silence(rx: &mut mpsc::Receiver<Message>, secs: u64, why: &str) {
    let extra = tokio::time::timeout(Duration::from_secs(secs), rx.recv()).await;
    assert!(
        !matches!(extra, Ok(Some(_))),
        "{why}, got {:?}",
        extra.map(|m| m.map(|m| m.headers.hop.clone()))
    );
}

fn status_of(cell_dir: &std::path::Path, id: Uuid) -> String {
    let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
    conn.query_row(
        "SELECT status FROM schedules WHERE schedule_id = ?1",
        [id.to_string()],
        |r| r.get(0),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh922_a_missed_strike_fires_late_once() {
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("timer");
    std::fs::create_dir_all(&cell_dir).unwrap();

    // Both rows are due at the same moment, a lead ahead that the three ops
    // and the stop fit into comfortably.
    let lead = chrono::Duration::seconds(3);
    let at = chrono::Utc::now() + lead;
    let at_s = at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let marked = Uuid::now_v7();
    let unmarked = Uuid::now_v7();

    // --- 1. colony 1: two orders and one refusal, then stopped in time.
    let (h, mut rx) = boot(&cell_dir).await;
    h.send(op_call(
        json!({"op": "add", "schedule_id": marked.to_string(),
               "schedule_name": "probe-marked", "at": at_s, "catch_up": true,
               "rearm": true, "emit_to": "/sink", "emit_body": {"messages": []},
               "emit_headers": {"msg_type": "probe_marked"}}),
        "call-marked",
    ))
    .await;
    let ack = receipt(&mut rx).await;
    assert_eq!(
        ack.headers.hop["msg_type"], "timer_op_ack",
        "{:?}",
        ack.headers.hop
    );

    h.send(op_call(
        json!({"op": "add", "schedule_id": unmarked.to_string(),
               "schedule_name": "probe-unmarked", "at": at_s,
               "emit_to": "/sink", "emit_body": {"messages": []},
               "emit_headers": {"msg_type": "probe_unmarked"}}),
        "call-unmarked",
    ))
    .await;
    let ack = receipt(&mut rx).await;
    assert_eq!(
        ack.headers.hop["msg_type"], "timer_op_ack",
        "{:?}",
        ack.headers.hop
    );

    h.send(op_call(
        json!({"op": "add", "schedule_id": Uuid::now_v7().to_string(),
               "schedule_name": "probe-cron", "cron": "*/1 * * * * *", "catch_up": true,
               "emit_to": "/sink", "emit_body": {"messages": []}}),
        "call-cron",
    ))
    .await;
    let refusal = receipt(&mut rx).await;
    assert_eq!(refusal.headers.hop["msg_type"], "timer_op_error");
    assert_eq!(
        refusal.headers.hop["error_code"], "invalid_params",
        "catch_up on cron is refused: {:?}",
        refusal.headers.hop
    );
    let Body::Inline(rb) = &refusal.body else {
        panic!("inline refusal expected")
    };
    assert!(
        rb["meta"]["detail"]
            .as_str()
            .is_some_and(|d| d.starts_with("catch_up_cron_unsupported")),
        "the refusal names its reason: {rb}"
    );

    h.shutdown().await;
    assert!(
        chrono::Utc::now() < at,
        "setup outran the {lead} lead; the stop has to come before the moment"
    );

    // --- 2. down: both moments pass, nothing fires.
    let wait = (at - chrono::Utc::now()).to_std().unwrap_or_default() + Duration::from_millis(1500);
    tokio::time::sleep(wait).await;
    assert_eq!(
        status_of(&cell_dir, marked),
        "active",
        "nothing fired while down"
    );
    assert_eq!(
        status_of(&cell_dir, unmarked),
        "active",
        "nothing fired while down"
    );

    // --- 3. colony 2: exactly the marked row, once, late.
    let (h, mut rx) = boot(&cell_dir).await;
    let fire = receipt(&mut rx).await;
    assert_eq!(
        fire.headers.hop["msg_type"], "probe_marked",
        "{:?}",
        fire.headers.hop
    );
    assert_eq!(fire.headers.hop["schedule_id"], marked.to_string());
    assert_eq!(fire.headers.hop["late"], true, "{:?}", fire.headers.hop);
    assert_eq!(
        fire.headers.hop["scheduled_at"],
        at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "the strike names the moment it was due, not the boot"
    );
    silence(
        &mut rx,
        2,
        "the unmarked row and a second strike stay silent",
    )
    .await;
    h.shutdown().await;
    assert_eq!(
        status_of(&cell_dir, marked),
        "completed",
        "fired = completed"
    );
    assert_eq!(
        status_of(&cell_dir, unmarked),
        "active",
        "an unmarked past row behaves as before: kept, never planned"
    );

    // --- 4. colony 3: nothing is caught up twice.
    let (h, mut rx) = boot(&cell_dir).await;
    silence(&mut rx, 2, "a second restart catches nothing up").await;
    h.shutdown().await;
}
