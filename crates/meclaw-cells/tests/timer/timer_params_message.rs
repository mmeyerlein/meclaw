//! Phase-16 β3: `timer` runtime params-overlay.
//!
//! The timer's only overlay field is `query_timeout_ms` (path C, immediately live).
//! `schedules` are ops-managed, NOT overlay-managed (a params-update touching
//! `schedules` is an Unknown reject). Immutable set is empty.
//!
//! Live-receipt: a params-update lowers `query_timeout_ms` and the live `DbConn`
//! reflects it immediately (no respawn). The enforcement mechanism — the live
//! value interrupting a slow query — is proven once, at the shared layer the
//! rewired timer ops now use: `db_conn::set_query_timeout_takes_effect_on_next_call`.
//! Timer's own ops are sub-ms metadata writes, so a per-type deterministic
//! interrupt test would be flaky; this slice proves the live propagation +
//! reject semantics and relies on the db_conn behavioral proof for enforcement.

use meclaw_cells::params_overlay;
use meclaw_cells::timer::cell::TimerCell;
use meclaw_cells::timer::db::setup_timer_schema;
use meclaw_cells::timer::io::TimerReconfig;
use meclaw_cells::timer::params::TimerOverlay;
use meclaw_colony::persist::open_or_create_cell_db;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path};
use serde_json::json;
use tokio::sync::mpsc;

fn params_msg(body: serde_json::Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/t"))
        .reply_to(Path::new("/reply"))
        .body(Body::Inline(body))
        .build()
}

fn sink_from(msg: &meclaw_core::Message, tx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        tx,
        Path::new("/t"),
        msg.id,
        msg.trace_id,
        32,
        meclaw_core::Headers::new(),
        None,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn params_update_query_timeout_persisted_and_live() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    setup_timer_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, Some(std::time::Duration::from_millis(5000)));
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, _rc_rx) = mpsc::channel::<TimerReconfig>(8);
    let msg = params_msg(json!({ "params": { "query_timeout_ms": 1234 } }));
    let sink = sink_from(&msg, out_tx);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;

    // Path C, immediately live: the running DbConn carries the new timeout immediately.
    assert_eq!(
        db.query_timeout(),
        Some(std::time::Duration::from_millis(1234)),
        "DbConn must carry the new query_timeout live"
    );
    let _ = &cell; // cell retains the new timeout (field is pub(crate))
    // Persisted to cell.db params table.
    let persisted: String = db
        .call(|c| {
            c.query_row(
                "SELECT value FROM params WHERE key='query_timeout_ms'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        })
        .await;
    assert_eq!(persisted, "1234");
    // params-only message is silent (no emission).
    drop(sink);
    assert!(out_rx.recv().await.is_none(), "params-only must not emit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn params_update_unknown_key_rejected_no_partial() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    setup_timer_schema(&conn).unwrap();
    let mut db = DbConn::wrap(conn, Some(std::time::Duration::from_millis(5000)));
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, _rc_rx) = mpsc::channel::<TimerReconfig>(8);
    // `schedules` is NOT an overlay key (ops-managed) → Unknown reject.
    let msg = params_msg(json!({ "params": { "schedules": [] } }));
    let sink = sink_from(&msg, out_tx);

    cell.handle(msg, &sink, &mut db, &rc_tx).await;
    drop(sink);

    let em = out_rx.recv().await.expect("unknown-key reject must emit");
    assert_eq!(em.content["header"]["error_code"], "invalid_input");
    // No partial apply: the live query_timeout is unchanged.
    assert_eq!(
        db.query_timeout(),
        Some(std::time::Duration::from_millis(5000))
    );
    let _ = &cell;
}

#[test]
fn restore_replays_query_timeout_overlay_over_birth() {
    // β restore (wake/respawn): a persisted query_timeout_ms overlay survives a
    // rebuild from birth-params; schedules in birth are ignored by TimerOverlay.
    let td = tempfile::TempDir::new().unwrap();
    let conn = open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    // Seed the cell.db params-overlay table directly (pub(crate) persist helper
    // is not reachable from an integration test; the params DDL is shared).
    conn.execute(
        "INSERT INTO params (key, value, updated_at) VALUES ('query_timeout_ms', '777', 100)",
        [],
    )
    .unwrap();
    let birth = json!({ "query_timeout_ms": 5000, "schedules": [] });
    let effective = params_overlay::restore::<TimerOverlay>(&conn, &birth).unwrap();
    assert_eq!(effective.query_timeout_ms, 777);
}
