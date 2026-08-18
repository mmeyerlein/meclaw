//! Track T (#104) — fitness battery for the `timer` cell on the TOOL LANE.
//!
//! The scheduling internals have their own suites (`timer_*`,
//! `phase_10b_timer_demo.rs`); this battery pins the contract a coding agent
//! drives through a dispatcher (`docs/cell-types.md` § timer, GH #81):
//!
//! - `add` as a `tool_call` is acked with a `tool_result` on the SAME id
//!   (`msg_type: "timer_op_ack"`) — the fan-in of a tool round closes;
//! - `trigger` fires an existing schedule NOW, indistinguishably from cron;
//! - a once-schedule whose `at` lies in the past is REFUSED on the same lane
//!   (`at_in_past`), proven against a positive control that does fire (GH #231);
//! - op errors keep the tool lane closed too: same turn, same id, plus
//!   `finish_reason: "error"` and the typed code.

use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid, serde_json::json};
use meclaw_testing::{ColonyHandle, topologies::phase_3a::CaptureCell};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// `/sink` first (anti-cascade), then the timer via its factory, then the edge.
async fn topology() -> (ColonyHandle, mpsc::Receiver<Message>, TempDir) {
    let h = ColonyHandle::new();
    let (recv_tx, recv_rx) = mpsc::channel::<Message>(32);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(recv_tx.clone())
    })
    .await;

    let td = TempDir::new().unwrap();
    let cell_dir = td.path().join("timer");
    std::fs::create_dir_all(&cell_dir).unwrap();
    let factory = Arc::new(TimerCellFactory);
    let spawned = factory
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
    (h, recv_rx, td)
}

/// A timer op as the dispatcher delivers it: one `tool_call` turn whose text
/// is the op object.
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
        .expect("no receipt at /sink within 30s")
        .expect("sink channel closed")
}

/// A moment `secs` from now, rendered to the millisecond.
///
/// GH #231: this used to render at second precision, which truncated the
/// requested lead to a value uniformly distributed in (0, 1] — a fixture that
/// asked for a second of lead and sometimes granted a millisecond, and then
/// blamed the timer for the miss. The lead a test asks for is now the lead it
/// gets.
fn rfc3339_in(secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(secs))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_add_tool_call_is_acked_on_the_same_id_and_the_schedule_fires() {
    let (h, mut rx, _td) = topology().await;
    let sid = Uuid::now_v7();

    h.send(op_call(
        json!({"op": "add", "schedule_id": sid.to_string(),
               "schedule_name": "workshop-step", "at": rfc3339_in(1),
               "emit_to": "/sink",
               "emit_body": {"messages": [{"origin": "user", "type": "text", "text": "tick"}]},
               "emit_headers": {"msg_type": "workshop_tick"}}),
        "call-add",
    ))
    .await;

    // The ack closes the tool round: tool_result on the INBOUND id.
    let ack = receipt(&mut rx).await;
    assert_eq!(ack.headers.hop["msg_type"], "timer_op_ack");
    assert_eq!(ack.headers.hop["op"], "add");
    assert_eq!(ack.headers.hop["schedule_id"], sid.to_string());
    let Body::Inline(body) = &ack.body else {
        panic!("inline ack expected")
    };
    assert_eq!(body["messages"][0]["type"], "tool_result");
    assert_eq!(body["messages"][0]["id"], "call-add");

    // The fire is a SEPARATE emission with the auto header set and the body
    // the schedule carried.
    let fire = receipt(&mut rx).await;
    assert_eq!(fire.headers.hop["msg_type"], "workshop_tick");
    assert_eq!(fire.headers.hop["schedule_id"], sid.to_string());
    assert_eq!(fire.headers.hop["schedule_name"], "workshop-step");
    assert!(fire.headers.hop.get("fired_at").is_some());
    assert!(fire.headers.hop.get("event_id").is_some());
    let Body::Inline(fb) = &fire.body else {
        panic!("inline fire expected")
    };
    assert_eq!(fb["messages"][0]["text"], "tick");

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trigger_fires_an_existing_schedule_now() {
    let (h, mut rx, _td) = topology().await;
    let sid = Uuid::now_v7();

    // A far-future once-schedule: it will never fire on its own clock.
    h.send(op_call(
        json!({"op": "add", "schedule_id": sid.to_string(),
               "schedule_name": "deferred-step", "at": "2099-01-01T00:00:00Z",
               "emit_to": "/sink", "emit_body": {"messages": []},
               "emit_headers": {"msg_type": "deferred"}}),
        "call-add",
    ))
    .await;
    let ack = receipt(&mut rx).await;
    assert_eq!(ack.headers.hop["msg_type"], "timer_op_ack");

    let started = std::time::Instant::now();
    h.send(op_call(
        json!({"op": "trigger", "schedule_id": sid.to_string()}),
        "call-trig",
    ))
    .await;

    // Ack and fire both arrive; order between the trigger-ack and the fire is
    // scheduling-dependent, so collect both.
    let (a, b) = (receipt(&mut rx).await, receipt(&mut rx).await);
    let (ack2, fire) = if a.headers.hop.get("msg_type") == Some(&json!("timer_op_ack")) {
        (a, b)
    } else {
        (b, a)
    };
    assert_eq!(ack2.headers.hop["op"], "trigger");
    let Body::Inline(ab) = &ack2.body else {
        panic!("inline")
    };
    assert_eq!(ab["messages"][0]["id"], "call-trig");
    assert_eq!(fire.headers.hop["msg_type"], "deferred");
    assert_eq!(fire.headers.hop["schedule_id"], sid.to_string());
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "the fire came NOW, not at the 2099 date"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_past_at_is_refused_on_the_lane_proven_against_a_control() {
    // POC contract: the timer only plans the next occurrence AFTER now, so a
    // once-schedule already in the past never fires. GH #231 settled what the
    // caller hears about it: the op is REFUSED (`at_in_past`, no row), not
    // accepted into a silence. The control (a future once) proves the lane
    // itself is live — the absence of the past fire is then meaningful.
    let (h, mut rx, _td) = topology().await;
    let past = Uuid::now_v7();
    let control = Uuid::now_v7();

    h.send(op_call(
        json!({"op": "add", "schedule_id": past.to_string(),
               "schedule_name": "missed", "at": rfc3339_in(-3600),
               "emit_to": "/sink", "emit_body": {"messages": []},
               "emit_headers": {"msg_type": "missed_fire"}}),
        "call-past",
    ))
    .await;
    let refusal = receipt(&mut rx).await;
    assert_eq!(
        refusal.headers.hop["msg_type"], "timer_op_error",
        "the past add is REFUSED, not quietly accepted (GH #231)"
    );
    assert_eq!(
        refusal.headers.hop["error_code"], "at_in_past",
        "got: {:?}",
        refusal.headers.hop
    );
    assert_eq!(
        refusal.headers.hop["finish_reason"], "error",
        "the tool round closes on the refusal instead of waiting"
    );

    h.send(op_call(
        json!({"op": "add", "schedule_id": control.to_string(),
               "schedule_name": "control", "at": rfc3339_in(1),
               "emit_to": "/sink", "emit_body": {"messages": []},
               "emit_headers": {"msg_type": "control_fire"}}),
        "call-ctl",
    ))
    .await;
    let ack = receipt(&mut rx).await;
    assert_eq!(ack.headers.hop["msg_type"], "timer_op_ack");

    // The control fires…
    let fire = receipt(&mut rx).await;
    assert_eq!(fire.headers.hop["msg_type"], "control_fire");
    assert_eq!(fire.headers.hop["schedule_id"], control.to_string());

    // …and the refused one does not, on top of never having been stored.
    // Semantic discriminator, deliberately tight: the past fire would have been
    // due 1h ago, so 2s of silence after the LATER control fire is conclusive.
    let extra = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(
        extra.is_err(),
        "the past schedule must lapse, got {:?}",
        extra.map(|m| m.map(|m| m.headers.hop.clone()))
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn op_errors_keep_the_tool_lane_closed() {
    let (h, mut rx, _td) = topology().await;
    let ghost = Uuid::now_v7();

    // trigger on an unknown schedule: the error travels as a tool_result on
    // the SAME id plus finish_reason error — a tool loop closes over the
    // failure instead of waiting forever.
    h.send(op_call(
        json!({"op": "trigger", "schedule_id": ghost.to_string()}),
        "call-ghost",
    ))
    .await;
    let err = receipt(&mut rx).await;
    assert_eq!(err.headers.hop["error_code"], "schedule_not_found");
    assert_eq!(err.headers.hop["finish_reason"], "error");
    let Body::Inline(body) = &err.body else {
        panic!("inline")
    };
    assert_eq!(body["messages"][0]["type"], "tool_result");
    assert_eq!(body["messages"][0]["id"], "call-ghost");

    // add with an invalid cron: rejected loudly, no silent never-firing row.
    let sid = Uuid::now_v7();
    h.send(op_call(
        json!({"op": "add", "schedule_id": sid.to_string(),
               "schedule_name": "broken", "cron": "not a cron",
               "emit_to": "/sink", "emit_body": {"messages": []}}),
        "call-cron",
    ))
    .await;
    let err = receipt(&mut rx).await;
    assert_eq!(err.headers.hop["error_code"], "invalid_cron");
    let Body::Inline(body) = &err.body else {
        panic!("inline")
    };
    assert_eq!(body["messages"][0]["id"], "call-cron");

    h.shutdown().await;
}
