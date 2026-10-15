//! GH #81: the timer becomes a tool lane like every other tool cell.
//!
//! Every other tool cell reads structured JSON args out of a `tool_call` turn
//! and answers with a `tool_result` carrying the same `id`. That uniformity is
//! what makes a tool loop possible at all: the dispatcher unwraps
//! `{name, arguments}` into a `tool_call` turn, and the collector fans the
//! `tool_result` back in on the `id`. The timer sat outside the convention on
//! both legs -- it read the op off the body's top level, and it acked nothing --
//! so `remind` needed a bespoke bridge that translated the shape AND fabricated
//! the `tool_result` the loop waits for, because nothing downstream would ever
//! produce one.
//!
//! Three things are pinned here:
//!
//!   1. INBOUND. The op parses out of a `tool_call` turn, and the legacy raw
//!      body (config-born ops, the #17 HTTP form) keeps working untouched.
//!   2. THE ANSWER. When an inbound `tool_call` id was supplied, success AND
//!      failure answer with a `tool_result` carrying it. Without an id nothing
//!      changes -- the legacy path stays as silent as it was.
//!   3. THE PARSE ERROR. "No op arrived" and "the op is missing its id" are
//!      different sentences. The reported case said `schedule_id: required`
//!      about a body whose `schedule_id` was present one level down, which
//!      reads as "you forgot the id" when the message is "the op does not go in
//!      a turn".

use meclaw_cells::timer::cell::TimerCell;
use meclaw_cells::timer::db::{load_schedule, setup_timer_schema};
use meclaw_cells::timer::io::TimerReconfig;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, validate_ubf_body};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

fn build_msg(body: serde_json::Value) -> meclaw_core::Message {
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

/// The op the way every other tool is driven: JSON args inside a `tool_call`.
fn tool_call_body(call_id: &str, op: serde_json::Value) -> serde_json::Value {
    json!({
        "messages": [{
            "id": call_id,
            "origin": "assistant",
            "type": "tool_call",
            "text": op.to_string()
        }]
    })
}

fn add_op(schedule_id: Uuid) -> serde_json::Value {
    json!({
        "op": "add",
        "schedule_id": schedule_id.to_string(),
        "schedule_name": "assistant-reminder",
        "emit_to": "/notify",
        "emit_body": {"messages": [{"origin": "user", "type": "text", "text": "stretch your legs"}]},
        "at": "2099-01-01T09:00:00Z"
    })
}

struct Rig {
    db: DbConn,
    cell: TimerCell,
    out_rx: mpsc::Receiver<CellEmission>,
    out_tx: mpsc::Sender<CellEmission>,
    rc_tx: mpsc::Sender<TimerReconfig>,
    rc_rx: mpsc::Receiver<TimerReconfig>,
}

fn rig() -> Rig {
    let conn = rusqlite::Connection::open_in_memory().expect("sqlite");
    setup_timer_schema(&conn).expect("schema");
    let (out_tx, out_rx) = mpsc::channel::<CellEmission>(8);
    let (rc_tx, rc_rx) = mpsc::channel::<TimerReconfig>(8);
    Rig {
        db: DbConn::wrap(conn, None),
        cell: TimerCell::new(Path::new("/t"), vec![], 5000),
        out_rx,
        out_tx,
        rc_tx,
        rc_rx,
    }
}

/// No emission within a generous window. Used where the contract is silence.
async fn expect_silence(rx: &mut mpsc::Receiver<CellEmission>) {
    if let Ok(Some(em)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
        panic!("expected no emission, got: {}", em.content);
    }
}

async fn expect_emission(rx: &mut mpsc::Receiver<CellEmission>) -> CellEmission {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no emission within 30s")
        .expect("channel closed")
}

// ---- 1. inbound ------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_op_inside_a_tool_call_turn_is_accepted() {
    let mut r = rig();
    let id = Uuid::now_v7();
    let msg = build_msg(tool_call_body("sched-019ff7dc", add_op(id)));
    let sink = sink_from(&msg, r.out_tx.clone());

    r.cell.handle(msg, &sink, &mut r.db, &r.rc_tx).await;

    let row =
        r.db.call(move |c| load_schedule(c, id))
            .await
            .expect("db")
            .expect("the op must have created a row");
    assert_eq!(row.status, "active");
    assert_eq!(row.schedule_name, "assistant-reminder");
    let rc = tokio::time::timeout(Duration::from_secs(30), r.rc_rx.recv())
        .await
        .expect("no SetActive within 30s")
        .expect("channel closed");
    assert!(matches!(rc, TimerReconfig::SetActive(_)));
}

/// #17's shape: the op at the body's top level, next to an honest `messages: []`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_legacy_raw_body_path_still_works_and_still_says_nothing() {
    let mut r = rig();
    let id = Uuid::now_v7();
    let mut body = add_op(id);
    body["messages"] = json!([]);
    let msg = build_msg(body);
    let sink = sink_from(&msg, r.out_tx.clone());

    r.cell.handle(msg, &sink, &mut r.db, &r.rc_tx).await;

    assert!(
        r.db.call(move |c| load_schedule(c, id))
            .await
            .expect("db")
            .is_some(),
        "the raw-body op must still create its row"
    );
    // No inbound id, so nothing to answer: the legacy contract is unchanged.
    expect_silence(&mut r.out_rx).await;
}

// ---- 2. the answer ---------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_successful_add_answers_with_a_tool_result_carrying_the_inbound_id() {
    let mut r = rig();
    let id = Uuid::now_v7();
    let msg = build_msg(tool_call_body("call-remind-1", add_op(id)));
    let sink = sink_from(&msg, r.out_tx.clone());

    r.cell.handle(msg, &sink, &mut r.db, &r.rc_tx).await;

    let em = expect_emission(&mut r.out_rx).await;
    validate_ubf_body(&em.content).expect("the ack must be valid UBF");
    assert_eq!(em.target, Path::new("/reply"));
    let turn = &em.content["messages"][0];
    assert_eq!(turn["type"], "tool_result");
    assert_eq!(turn["origin"], "tool");
    assert_eq!(
        turn["id"], "call-remind-1",
        "the ack must carry the inbound tool_call id, or no loop can correlate it"
    );
    assert_eq!(em.content["header"]["op"], "add");
    assert_eq!(em.content["header"]["schedule_id"], id.to_string());
    assert!(
        em.content["header"].get("error_code").is_none(),
        "a successful op is not an error"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_and_trigger_answer_on_the_tool_lane_too() {
    let mut r = rig();
    let id = Uuid::now_v7();

    // add
    let msg = build_msg(tool_call_body("call-a", add_op(id)));
    let sink = sink_from(&msg, r.out_tx.clone());
    r.cell.handle(msg, &sink, &mut r.db, &r.rc_tx).await;
    let _ = expect_emission(&mut r.out_rx).await;

    // trigger
    let msg = build_msg(tool_call_body(
        "call-b",
        json!({"op": "trigger", "schedule_id": id.to_string()}),
    ));
    let sink = sink_from(&msg, r.out_tx.clone());
    r.cell.handle(msg, &sink, &mut r.db, &r.rc_tx).await;
    let em = expect_emission(&mut r.out_rx).await;
    assert_eq!(em.content["messages"][0]["id"], "call-b");
    assert_eq!(em.content["header"]["op"], "trigger");

    // remove
    let msg = build_msg(tool_call_body(
        "call-c",
        json!({"op": "remove", "schedule_id": id.to_string()}),
    ));
    let sink = sink_from(&msg, r.out_tx.clone());
    r.cell.handle(msg, &sink, &mut r.db, &r.rc_tx).await;
    let em = expect_emission(&mut r.out_rx).await;
    assert_eq!(em.content["messages"][0]["id"], "call-c");
    assert_eq!(em.content["header"]["op"], "remove");
}

/// An error on the tool lane closes the loop too, otherwise the agent waits
/// forever for a result that the substrate deliberately never sends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_op_error_on_the_tool_lane_carries_the_id_as_a_tool_result() {
    let mut r = rig();
    let unknown = Uuid::now_v7();
    let msg = build_msg(tool_call_body(
        "call-err",
        json!({"op": "remove", "schedule_id": unknown.to_string()}),
    ));
    let sink = sink_from(&msg, r.out_tx.clone());

    r.cell.handle(msg, &sink, &mut r.db, &r.rc_tx).await;

    let em = expect_emission(&mut r.out_rx).await;
    validate_ubf_body(&em.content).expect("the error must be valid UBF");
    assert_eq!(em.content["header"]["error_code"], "schedule_not_found");
    assert_eq!(em.content["header"]["msg_type"], "timer_op_error");
    let turn = &em.content["messages"][0];
    assert_eq!(turn["type"], "tool_result");
    assert_eq!(turn["id"], "call-err");
    assert!(
        turn["text"].as_str().expect("text").contains("remove"),
        "the tool_result text carries the detail, got: {turn}"
    );
    // The legacy reader keeps its slot.
    assert!(em.content["meta"]["detail"].is_string());
}

/// Without an inbound id the error keeps the shape it always had.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_op_error_without_an_inbound_id_keeps_the_legacy_shape() {
    let mut r = rig();
    let unknown = Uuid::now_v7();
    let msg = build_msg(json!({
        "messages": [],
        "op": "remove",
        "schedule_id": unknown.to_string()
    }));
    let sink = sink_from(&msg, r.out_tx.clone());

    r.cell.handle(msg, &sink, &mut r.db, &r.rc_tx).await;

    let em = expect_emission(&mut r.out_rx).await;
    assert_eq!(em.content["header"]["error_code"], "schedule_not_found");
    assert_eq!(
        em.content["messages"].as_array().expect("messages").len(),
        0,
        "no inbound id, no turn to correlate"
    );
    assert!(em.content["meta"]["detail"].is_string());
}

// ---- 3. the parse error ----------------------------------------------------

/// The reported confusion: a body whose only top-level slot is `messages`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_that_carries_only_turns_says_the_op_never_arrived() {
    let mut r = rig();
    let msg = build_msg(json!({
        "messages": [{"origin": "user", "type": "text", "text": "remind me"}]
    }));
    let sink = sink_from(&msg, r.out_tx.clone());

    r.cell.handle(msg, &sink, &mut r.db, &r.rc_tx).await;

    let em = expect_emission(&mut r.out_rx).await;
    assert_eq!(em.content["header"]["error_code"], "parse_error");
    let detail = em.content["meta"]["detail"].as_str().expect("detail");
    assert!(
        detail.contains("no op object at the body top level"),
        "the error must name the SHAPE, got: {detail}"
    );
    assert!(
        detail.contains("messages"),
        "and it must name the slot the body actually carries, got: {detail}"
    );
    assert!(
        !detail.starts_with("schedule_id"),
        "naming the field first is the misdirection #81 reported, got: {detail}"
    );
}

/// An op that IS there but is missing its key still says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_op_object_without_its_id_names_the_field() {
    let mut r = rig();
    let msg = build_msg(json!({
        "messages": [],
        "op": "remove"
    }));
    let sink = sink_from(&msg, r.out_tx.clone());

    r.cell.handle(msg, &sink, &mut r.db, &r.rc_tx).await;

    let em = expect_emission(&mut r.out_rx).await;
    assert_eq!(em.content["header"]["error_code"], "parse_error");
    let detail = em.content["meta"]["detail"].as_str().expect("detail");
    assert!(
        detail.contains("schedule_id"),
        "an op object without its id names the field, got: {detail}"
    );
    assert!(
        !detail.contains("no op object"),
        "the op object IS there, got: {detail}"
    );
}
