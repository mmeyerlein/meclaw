//! GH #1019 -- run a schedule now, by name, from outside the colony.
//!
//! Replays and tests need the nightly consolidation between two recorded days.
//! The memory hive mints its clock's schedule id at instantiation
//! (`${uuid7:nightly-dream}`, `templates/memory-hive/clock/config.json`), the id
//! is not readable over the API, and before this issue the hive had no lane to
//! its clock at all: a POST at the clock's own path ended as `hive_boundary`.
//!
//! Two halves, two receivers:
//!
//! - the `timer` cell: `trigger` takes a `schedule_name` in place of the id.
//!   One trigger is one strike (the GH #17 path by id after the lookup), the
//!   cron and the plan stay, an unknown name and an ambiguous name are refused;
//! - the shipped memory hive, booted alone: `clock_op` on the hive path reaches
//!   `./clock` through the sealed rim, and the night it fires writes its own
//!   `consolidation_log` row, `status: done` -- measured in the real `cell.db`.
//!
//! No model is called: every `llm` cell of the hive points at a dead port, and
//! an empty store is a night without a model call.

#[path = "support/memory_hive_subject.rs"]
mod support;

use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::{ColonyHandle, NEVER_CRON, topologies::phase_3a::CaptureCell};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::{DEADLINE, HIVE, boot, build, db, shipped};
use tempfile::TempDir;
use tokio::sync::mpsc;

// ------------------------------------------------------------- the timer cell

/// `/sink` first, then the timer via its factory, then the edge -- the
/// topology of `fitness_timer.rs`, with the timer's `cell.db` under `td/timer`.
async fn timer() -> (ColonyHandle, mpsc::Receiver<Message>, TempDir) {
    let h = ColonyHandle::new();
    let (tx, rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(tx.clone()))
        .await;
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("timer");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let spawned = Arc::new(TimerCellFactory)
        .spawn_cell(
            Path::new("/timer"),
            json!({}),
            h.runtime().outputs_tx,
            cell_dir,
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
    (h, rx, td)
}

/// A timer op as a dispatcher delivers it: one `tool_call` turn.
fn op_call(args: Value, call_id: &str) -> Message {
    MessageBuilder::new(Path::new("/timer"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant", "type": "tool_call",
            "text": args.to_string(), "id": call_id
        }]})))
        .build()
}

async fn receipt(rx: &mut mpsc::Receiver<Message>) -> Message {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no receipt at /sink within 30s")
        .expect("sink channel closed")
}

/// `add` a repeating schedule that never comes due on its own, acked.
async fn add_quiet(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, id: Uuid, name: &str) {
    h.send(op_call(
        json!({"op": "add", "schedule_id": id.to_string(), "schedule_name": name,
               "cron": NEVER_CRON, "emit_to": "/sink",
               "emit_body": {"messages": []}, "emit_headers": {"msg_type": "night"}}),
        &format!("add-{id}"),
    ))
    .await;
    let ack = receipt(rx).await;
    assert_eq!(
        ack.headers.hop["msg_type"], "timer_op_ack",
        "{:?}",
        ack.headers.hop
    );
}

/// `(cron_expr, status, iteration_n)` of one row, read where the timer keeps it.
fn row_of(db: &std::path::Path, id: Uuid) -> (String, String, i64) {
    let c = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open the timer's cell.db");
    c.query_row(
        "SELECT cron_expr, status, iteration_n FROM schedules WHERE schedule_id = ?1",
        [id.to_string()],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .expect("the row")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1019_a_trigger_by_name_is_one_strike_and_keeps_the_plan() {
    let (h, mut rx, td) = timer().await;
    let id = Uuid::now_v7();
    add_quiet(&h, &mut rx, id, "nightly-dream").await;

    h.send(op_call(
        json!({"op": "trigger", "schedule_name": "nightly-dream"}),
        "call-fire",
    ))
    .await;
    let (a, b) = (receipt(&mut rx).await, receipt(&mut rx).await);
    let (ack, fire) = if a.headers.hop.get("msg_type") == Some(&json!("timer_op_ack")) {
        (a, b)
    } else {
        (b, a)
    };
    assert_eq!(ack.headers.hop["op"], "trigger");
    assert_eq!(
        ack.headers.hop["schedule_id"],
        id.to_string(),
        "the ack names the row the name resolved to"
    );
    assert_eq!(fire.headers.hop["msg_type"], "night");
    assert_eq!(fire.headers.hop["schedule_name"], "nightly-dream");
    assert_eq!(fire.headers.hop["schedule_id"], id.to_string());

    // One trigger, one strike: nothing else arrives.
    let extra = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(
        extra.is_err(),
        "a second emission: {:?}",
        extra.map(|m| m.map(|m| m.headers.hop))
    );
    assert_eq!(
        row_of(&td.path().join("timer/cell.db"), id),
        (NEVER_CRON.to_string(), "active".to_string(), 1),
        "the cron and the plan stay; the strike counts like a cron strike"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1019_an_unknown_or_ambiguous_name_is_refused_and_fires_nothing() {
    let (h, mut rx, _td) = timer().await;

    h.send(op_call(
        json!({"op": "trigger", "schedule_name": "no-such-night"}),
        "call-ghost",
    ))
    .await;
    let err = receipt(&mut rx).await;
    assert_eq!(err.headers.hop["error_code"], "schedule_not_found");
    assert_eq!(err.headers.hop["finish_reason"], "error");

    add_quiet(&h, &mut rx, Uuid::now_v7(), "twice").await;
    add_quiet(&h, &mut rx, Uuid::now_v7(), "twice").await;
    h.send(op_call(
        json!({"op": "trigger", "schedule_name": "twice"}),
        "call-twice",
    ))
    .await;
    let err = receipt(&mut rx).await;
    assert_eq!(
        err.headers.hop["error_code"], "invalid_params",
        "two active rows under one name: {:?}",
        err.headers.hop
    );
    let extra = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(
        extra.is_err(),
        "a refused trigger fired: {:?}",
        extra.map(|m| m.map(|m| m.headers.hop))
    );
    h.shutdown().await;
}

// ------------------------------------------- a clock that takes only `trigger`

/// A timer born with `accept_ops` and one quiet seeded row, `/sink` as the
/// answer path -- the shape of the memory hive's clock, without the hive.
async fn trigger_only_timer(id: Uuid) -> (ColonyHandle, mpsc::Receiver<Message>, TempDir) {
    accepting_timer(id, json!(["trigger"])).await
}

/// A timer born with one seeded schedule and `params.accept_ops = accept`.
async fn accepting_timer(
    id: Uuid,
    accept: Value,
) -> (ColonyHandle, mpsc::Receiver<Message>, TempDir) {
    let h = ColonyHandle::new();
    let (tx, rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(tx.clone()))
        .await;
    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("timer");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let params = json!({
        "accept_ops": accept,
        "schedules": [{
            "schedule_id": id.to_string(), "schedule_name": "quiet",
            "cron": NEVER_CRON, "emit_to": "/sink",
            "emit_body": {"messages": []}, "emit_headers": {"msg_type": "night"}
        }]
    });
    let spawned = Arc::new(TimerCellFactory)
        .spawn_cell(
            Path::new("/timer"),
            params,
            h.runtime().outputs_tx,
            cell_dir,
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
    (h, rx, td)
}

/// Every row the timer keeps: `(schedule_id, cron_expr, status, iteration_n)`.
fn all_rows(db: &std::path::Path) -> Vec<(String, String, String, i64)> {
    let c = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open the timer's cell.db");
    let mut st = c
        .prepare(
            "SELECT schedule_id, cron_expr, status, iteration_n FROM schedules \
             ORDER BY schedule_id",
        )
        .unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1019_a_trigger_only_clock_refuses_every_other_op_and_still_fires() {
    let id = Uuid::now_v7();
    let (h, mut rx, td) = trigger_only_timer(id).await;
    let db = td.path().join("timer/cell.db");
    let seeded = all_rows(&db);
    assert_eq!(seeded.len(), 1, "the seed row: {seeded:?}");

    let refused = [
        json!({"op": "add", "schedule_id": Uuid::now_v7().to_string(),
               "schedule_name": "planted", "cron": NEVER_CRON, "emit_to": "/sink",
               "emit_body": {"messages": []}}),
        json!({"op": "modify", "schedule_id": id.to_string(), "cron": "0 0 4 * * *"}),
        json!({"op": "remove", "schedule_id": id.to_string()}),
    ];
    for (i, op) in refused.iter().enumerate() {
        h.send(op_call(op.clone(), &format!("call-{i}"))).await;
        let err = receipt(&mut rx).await;
        assert_eq!(
            err.headers.hop["error_code"], "op_not_accepted",
            "{op}: {:?}",
            err.headers.hop
        );
    }
    // The params slot is the other door into a timer; it stays shut as well.
    h.send(
        MessageBuilder::new(Path::new("/timer"))
            .reply_to(Path::new("/sink"))
            .body(Body::Inline(json!({"params": {"query_timeout_ms": 1}})))
            .build(),
    )
    .await;
    let err = receipt(&mut rx).await;
    assert_eq!(err.headers.hop["error_code"], "op_not_accepted");
    assert_eq!(all_rows(&db), seeded, "nothing planted, changed or removed");

    h.send(op_call(
        json!({"op": "trigger", "schedule_name": "quiet"}),
        "call-fire",
    ))
    .await;
    let (a, b) = (receipt(&mut rx).await, receipt(&mut rx).await);
    let fired = [a, b]
        .into_iter()
        .any(|m| m.headers.hop.get("msg_type") == Some(&json!("night")));
    assert!(fired, "the trigger still passes");
    h.shutdown().await;
}

/// KT review K4: the gate reads `op` with the parser's own default. A body
/// without `op` but with a `schedule_id` IS an `add` to `TimerOp::parse`
/// (`timer/op.rs`), so a timer that accepts `add` takes it -- and one that
/// accepts only `trigger` still refuses it, as `op_not_accepted`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1019_an_op_less_body_is_gated_as_the_add_it_parses_to() {
    let opless = |id: Uuid| {
        json!({"schedule_id": id.to_string(), "schedule_name": "opless",
               "cron": NEVER_CRON, "emit_to": "/sink", "emit_body": {"messages": []}})
    };
    let (h, mut rx, td) = accepting_timer(Uuid::now_v7(), json!(["add", "trigger"])).await;
    let planted = Uuid::now_v7();
    h.send(op_call(opless(planted), "call-opless")).await;
    let ack = receipt(&mut rx).await;
    assert_eq!(
        ack.headers.hop["msg_type"], "timer_op_ack",
        "an op-less add passes a gate that accepts add: {:?}",
        ack.headers.hop
    );
    assert_eq!(
        row_of(&td.path().join("timer/cell.db"), planted).1,
        "active"
    );
    h.shutdown().await;

    let (h, mut rx, td) = trigger_only_timer(Uuid::now_v7()).await;
    let db = td.path().join("timer/cell.db");
    let seeded = all_rows(&db);
    h.send(op_call(opless(Uuid::now_v7()), "call-opless")).await;
    let err = receipt(&mut rx).await;
    assert_eq!(err.headers.hop["error_code"], "op_not_accepted");
    assert_eq!(all_rows(&db), seeded, "nothing planted");
    h.shutdown().await;
}

/// KT review K3: the clock's own description says what its mailbox takes --
/// `trigger` only (`accept_ops` in the same file), never a runtime
/// add/modify/remove a caller would then see refused.
#[test]
fn gh1019_the_clock_describes_only_the_trigger_it_accepts() {
    if !shipped() {
        eprintln!("gh1019: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/memory-hive/clock/config.json");
    let cfg: Value = meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
        .expect("clock config.json");
    assert_eq!(cfg["params"]["accept_ops"], json!(["trigger"]));
    for key in ["use_when", "consumes_meaning"] {
        let text = cfg["description"][key].as_str().unwrap_or_default();
        assert!(text.contains("trigger"), "{key}: {text}");
        for refused in ["add/", "modify", "remove"] {
            assert!(
                !text.contains(refused),
                "{key} promises an op the clock refuses ({refused}): {text}"
            );
        }
    }
}

// ------------------------------------------------- the shipped memory hive

/// `clock_op` as an outside caller sends it: at the hive path, nothing else.
fn clock_op(body: Value) -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!("clock_op"));
    MessageBuilder::new(Path::new(HIVE))
        .hop(hop)
        .context(Map::new())
        .body(Body::Inline(body))
        .build()
}

fn read_only(p: &std::path::Path) -> Option<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// The night's schedule as the clock keeps it: `(cron_expr, status, iteration_n)`.
fn night_row(td: &TempDir) -> Option<(String, String, i64)> {
    read_only(&td.path().join("main/memory-hive/clock/cell.db"))?
        .query_row(
            "SELECT cron_expr, status, iteration_n FROM schedules \
             WHERE schedule_name = 'nightly-dream'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
}

/// `(run_id, status)` of every night the store has a receipt for.
fn nights(td: &TempDir) -> Vec<(String, String)> {
    let Some(c) = read_only(&db(td)) else {
        return Vec::new();
    };
    let Ok(mut st) = c.prepare("SELECT run_id, status FROM consolidation_log ORDER BY run_id")
    else {
        return Vec::new();
    };
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Waits for `f` to hold, reading the receiver's files; `None` at the deadline.
async fn until<T>(f: impl Fn() -> Option<T>) -> Option<T> {
    let end = Instant::now() + DEADLINE;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() > end {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1019_the_night_runs_now_from_outside_the_sealed_hive() {
    if !shipped() {
        eprintln!("gh1019: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let td = TempDir::new().expect("tempdir");
    build(&td, &[]);
    let (h, _rx) = boot(&td).await;
    let before = until(|| night_row(&td))
        .await
        .expect("the clock seeds its nightly schedule");
    assert!(nights(&td).is_empty(), "no night before the trigger");

    h.send(clock_op(
        json!({"messages": [], "op": "trigger", "schedule_name": "nightly-dream"}),
    ))
    .await;
    let done = until(|| {
        let n = nights(&td);
        n.iter().any(|(_, s)| s == "done").then_some(n)
    })
    .await;
    assert!(
        done.is_some(),
        "the trigger crossed the rim and the night booked itself: {:?}",
        nights(&td)
    );
    assert_eq!(done.unwrap().len(), 1, "one trigger, one night");
    let after = night_row(&td).expect("the row");
    assert_eq!(
        (after.0, after.1, after.2),
        (before.0, before.1, before.2 + 1),
        "the cron and the plan stay; the strike counts once"
    );
    h.shutdown().await;
}

/// Every row of the hive clock: `(schedule_name, cron_expr, status, iteration_n)`.
fn clock_rows(td: &TempDir) -> Vec<(String, String, String, i64)> {
    let Some(c) = read_only(&td.path().join("main/memory-hive/clock/cell.db")) else {
        return Vec::new();
    };
    let mut st = c
        .prepare(
            "SELECT schedule_name, cron_expr, status, iteration_n FROM schedules \
             ORDER BY schedule_id",
        )
        .unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1019_the_rim_carries_the_trigger_and_nothing_else() {
    if !shipped() {
        eprintln!("gh1019: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let td = TempDir::new().expect("tempdir");
    build(&td, &[]);
    let (h, _rx) = boot(&td).await;
    until(|| night_row(&td))
        .await
        .expect("the clock seeds its nightly schedule");
    let before = clock_rows(&td);
    assert_eq!(before.len(), 1, "one seeded night: {before:?}");
    let night_id: String = read_only(&td.path().join("main/memory-hive/clock/cell.db"))
        .expect("clock cell.db")
        .query_row(
            "SELECT schedule_id FROM schedules WHERE schedule_name = 'nightly-dream'",
            [],
            |r| r.get(0),
        )
        .expect("the night's id");

    // Review B1: an `add` from outside would plant a strike the clock emits
    // from INSIDE the hive -- at the store, past the rim; a `modify`/`remove`
    // would move or switch off the night. All of them are refused.
    let soon = (chrono::Utc::now() + chrono::Duration::seconds(2))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    for body in [
        json!({"messages": [], "op": "add", "schedule_id": Uuid::now_v7().to_string(),
               "schedule_name": "planted", "at": soon,
               "emit_to": format!("{HIVE}/store"), "emit_body": {"messages": []}}),
        json!({"messages": [], "op": "modify", "schedule_id": night_id,
               "cron": "0 0 4 * * *"}),
        json!({"messages": [], "op": "remove", "schedule_id": night_id}),
    ] {
        h.send(clock_op(body)).await;
    }
    // The trigger still crosses; the clock's mailbox is in order, so once the
    // night it fires is done, the three ops before it were decided.
    h.send(clock_op(
        json!({"messages": [], "op": "trigger", "schedule_name": "nightly-dream"}),
    ))
    .await;
    let done = until(|| {
        let n = nights(&td);
        n.iter().any(|(_, s)| s == "done").then_some(n)
    })
    .await;
    assert!(
        done.is_some(),
        "the trigger still crosses the rim: {:?} / clock {:?}",
        nights(&td),
        clock_rows(&td)
    );
    let mut want = before.clone();
    want[0].3 += 1;
    assert_eq!(
        clock_rows(&td),
        want,
        "nothing planted, moved or removed; the trigger counted once"
    );
    h.shutdown().await;
}
