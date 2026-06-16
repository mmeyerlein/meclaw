//! Phase-10-B T15: `handle_event` Fire-Emission. UBF-Body + strikte
//! Auto-Set-Header (event_id/schedule_id/schedule_name/scheduled_at/
//! fired_at/iteration_n nur repeating). Auto-Set-Header **ueberschreiben**
//! kollidierende `emit_headers` (cell-types.md Z.441–451). RFC-3339-Z
//! Format via `to_rfc3339_opts(SecondsFormat::Secs, true)`. Emit via
//! `OriginSink` → parent_message_id=None, fresh trace_id.

use chrono::{Duration as ChDur, SecondsFormat, TimeZone, Utc};
use meclaw_cells::timer::cell::TimerCell;
use meclaw_cells::timer::db::{insert_schedule, setup_timer_schema};
use meclaw_cells::timer::io::TimerEvent;
use meclaw_cells::timer::schedule::{ScheduleKind, ScheduleRow};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{CellEmission, OriginSink, Path, Uuid};
use serde_json::{Map, json};
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fire_emission_for_repeating_carries_all_auto_headers_and_overrides_emit_headers() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();

    // Strikter Override-Beweis: emit_headers traegt einen kollidierenden
    // `event_id` + ein NICHT kollidierendes `msg_type`. Nach Fire muss
    // event_id ein frisches UUID v7 sein, msg_type aber durchgereicht.
    let mut emit_headers = Map::new();
    emit_headers.insert("event_id".into(), json!("should-be-overridden"));
    emit_headers.insert("msg_type".into(), json!("tick"));

    let id = Uuid::now_v7();
    insert_schedule(
        &conn,
        &ScheduleRow {
            schedule_id: id,
            schedule_name: "heartbeat".into(),
            kind: ScheduleKind::Cron("*/1 * * * * *".into()),
            emit_to: Path::new("/dst"),
            emit_body: json!({}),
            emit_headers,
            status: "active".into(),
            iteration_n: 0,
        },
    )
    .unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/t"), 32);

    let scheduled_at = Utc::now() - ChDur::seconds(1);
    cell.handle_event(
        TimerEvent::Fire {
            schedule_id: id,
            scheduled_at,
        },
        &sink,
        &mut db,
    )
    .await;

    let em = tokio::time::timeout(Duration::from_secs(1), origin_rx.recv())
        .await
        .expect("keine Fire-Emission")
        .unwrap();

    assert_eq!(em.target, Path::new("/dst"));
    assert!(em.parent_message_id.is_none(), "OriginSink → parent=None");

    let hdr = em.content["header"].as_object().expect("header object");

    // Override-Beweis: event_id ist NICHT "should-be-overridden", sondern UUID v7.
    let event_id_s = hdr["event_id"].as_str().expect("event_id present");
    assert_ne!(event_id_s, "should-be-overridden");
    Uuid::parse_str(event_id_s).expect("event_id is a valid UUID");

    // msg_type bleibt — kein Auto-Set-Key.
    assert_eq!(hdr["msg_type"], json!("tick"));

    assert_eq!(hdr["schedule_id"], json!(id.to_string()));
    assert_eq!(hdr["schedule_name"], json!("heartbeat"));

    let scheduled_s = hdr["scheduled_at"].as_str().expect("scheduled_at str");
    let fired_s = hdr["fired_at"].as_str().expect("fired_at str");
    assert!(
        scheduled_s.ends_with('Z'),
        "scheduled_at muss RFC-3339-Z sein: {scheduled_s}"
    );
    assert!(
        fired_s.ends_with('Z'),
        "fired_at muss RFC-3339-Z sein: {fired_s}"
    );
    assert_eq!(
        scheduled_s,
        scheduled_at.to_rfc3339_opts(SecondsFormat::Secs, true)
    );

    // iteration_n: PRE-bump = 0 (T11 hat in DB +1 gemacht, aber `row`
    // wurde davor geladen — der Header-Wert ist 0 fuer den ersten Fire).
    assert_eq!(hdr["iteration_n"], json!(0u64));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fire_emission_for_once_omits_iteration_n() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_timer_schema(&conn).unwrap();

    let id = Uuid::now_v7();
    insert_schedule(
        &conn,
        &ScheduleRow {
            schedule_id: id,
            schedule_name: "oneshot".into(),
            kind: ScheduleKind::At(Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap()),
            emit_to: Path::new("/dst"),
            emit_body: json!({}),
            emit_headers: Map::new(),
            status: "active".into(),
            iteration_n: 0,
        },
    )
    .unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);

    let (origin_tx, mut origin_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OriginSink::new(origin_tx, Path::new("/t"), 32);

    cell.handle_event(
        TimerEvent::Fire {
            schedule_id: id,
            scheduled_at: Utc::now(),
        },
        &sink,
        &mut db,
    )
    .await;

    let em = tokio::time::timeout(Duration::from_secs(1), origin_rx.recv())
        .await
        .expect("keine Fire-Emission")
        .unwrap();
    let hdr = em.content["header"].as_object().expect("header object");
    assert!(
        !hdr.contains_key("iteration_n"),
        "once darf KEIN iteration_n im Header tragen, header={hdr:?}"
    );
}
