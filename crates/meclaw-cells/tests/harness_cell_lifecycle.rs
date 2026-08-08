//! P8 block 5 — the handler: accepting tasks, following them, closing them out.
//!
//! Driven against the cell directly (the `mcp` handler-test pattern), so the
//! assertions are about the handler's own decisions rather than about routing.
//! The frames are the ones the fixture really emits.

use meclaw_cells::harness::HarnessParams;
use meclaw_cells::harness::cell::HarnessCell;
use meclaw_cells::harness::db::{load_task, setup_harness_schema};
use meclaw_cells::harness::io::{HarnessEvent, HarnessReconfig};
use meclaw_cells::stdio_child::{ChildEvent, ChildExit};
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Value as JsonValue, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, OriginSink, OutputSink, Path};
use std::time::Duration;
use tokio::sync::mpsc;

/// Everything a handler test needs, wired the way `cell_task_long_running`
/// wires it.
struct Rig {
    cell: HarnessCell,
    db: DbConn,
    db_path: std::path::PathBuf,
    outputs: mpsc::Receiver<CellEmission>,
    origin_rx: mpsc::Receiver<CellEmission>,
    origin: OriginSink,
    reconfig_tx: mpsc::Sender<HarnessReconfig>,
    reconfig_rx: mpsc::Receiver<HarnessReconfig>,
    out_tx: mpsc::Sender<CellEmission>,
    params: HarnessParams,
    _root: tempfile::TempDir,
    _db_dir: tempfile::TempDir,
    root: std::path::PathBuf,
}

fn rig(extra: JsonValue) -> Rig {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(root.path().join("ws")).expect("workspace");
    let db_dir = tempfile::tempdir().expect("db dir");
    let db_path = db_dir.path().join("cell.db");
    let (conn, _s) = open_or_create_cell_db_with_status(&db_path).expect("open");
    setup_harness_schema(&conn).expect("schema");
    let db = DbConn::wrap(conn, Some(Duration::from_secs(2)));

    let mut cfg = json!({
        "adapter": "claude-code",
        "emit_to": "/main/coord",
        "workspace_root": root.path().display().to_string(),
        "command": env!("CARGO_BIN_EXE_stream_json_harness_fixture"),
    });
    for (k, v) in extra.as_object().expect("object") {
        cfg[k] = v.clone();
    }
    let params = HarnessParams::parse(&cfg).expect("params");
    let root_path = params.workspace_root.clone();

    let (out_tx, outputs) = mpsc::channel::<CellEmission>(16);
    let (origin_tx, origin_rx) = mpsc::channel::<CellEmission>(16);
    let (reconfig_tx, reconfig_rx) = mpsc::channel::<HarnessReconfig>(8);
    Rig {
        cell: HarnessCell::new(params.clone()),
        params,
        db,
        db_path,
        outputs,
        origin_rx,
        origin: OriginSink::new(origin_tx, Path::new("/harness"), 16),
        reconfig_tx,
        reconfig_rx,
        out_tx,
        root: root_path,
        _root: root,
        _db_dir: db_dir,
    }
}

impl Rig {
    /// Feed one tool call into `handle`, as the substrate would.
    async fn call(&mut self, name: &str, args: JsonValue) -> Message {
        let msg = tool_call(name, args);
        let sink = OutputSink::new(
            self.out_tx.clone(),
            Path::new("/harness"),
            msg.id,
            msg.trace_id,
            msg.ttl,
            msg.headers.clone(),
            None,
        );
        self.cell
            .handle(msg.clone(), &sink, &mut self.db, &self.reconfig_tx)
            .await;
        msg
    }

    async fn event(&mut self, ev: HarnessEvent) {
        self.cell.handle_event(ev, &self.origin, &mut self.db).await;
    }

    /// Feed a runtime params update, the β slot every stateful cell type has.
    async fn params_update(&mut self, params: JsonValue) {
        let msg = MessageBuilder::new(Path::new("/harness"))
            .reply_to(Path::new("/sink"))
            .body(Body::Inline(json!({"messages": [], "params": params})))
            .build();
        let sink = OutputSink::new(
            self.out_tx.clone(),
            Path::new("/harness"),
            msg.id,
            msg.trace_id,
            msg.ttl,
            msg.headers.clone(),
            None,
        );
        self.cell
            .handle(msg, &sink, &mut self.db, &self.reconfig_tx)
            .await;
    }

    /// The next reply on the requester's lane.
    fn reply(&mut self) -> JsonValue {
        self.outputs.try_recv().expect("no reply emitted").content
    }

    /// The next emission on the origin lane.
    fn emission(&mut self) -> JsonValue {
        self.origin_rx
            .try_recv()
            .expect("no origin emission")
            .content
    }

    /// A fresh connection to the task table — proves persistence, not caching.
    fn probe_status(&self, task_id: &str) -> Option<String> {
        let c = rusqlite::Connection::open(&self.db_path).expect("probe");
        load_task(&c, task_id).expect("load").map(|r| r.status)
    }
}

// ------------------------------------------------------- approval channel ----

/// With the channel off the harness must not be left hanging: the question is
/// reported for the record AND answered with a refusal in the same breath.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_question_without_an_approval_channel_is_reported_and_denied() {
    let mut r = rig(json!({}));
    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();
    let _ = r.reconfig_rx.try_recv();

    r.event(frame(json!({
        "type": "control_request", "request_id": "r-1",
        "request": {"subtype": "can_use_tool", "tool_name": "Bash",
                    "input": {"command": "rm -rf /"}}
    })))
    .await;

    let em = r.emission();
    assert_eq!(em["header"]["harness_event"], "question");
    assert_eq!(em["header"]["request_id"], "r-1");

    match r.reconfig_rx.try_recv().expect("no auto-deny was sent") {
        HarnessReconfig::Child(meclaw_cells::stdio_child::ChildCommand::Send { line, .. }) => {
            assert_eq!(line["type"], "control_response");
            assert_eq!(line["response"]["request_id"], "r-1");
            assert_eq!(
                line["response"]["response"]["behavior"], "deny",
                "without a channel the answer can only be no"
            );
        }
        other => panic!("expected an auto-deny, got {other:?}"),
    }
}

/// With the channel on, the decision belongs to the topology — the cell asks
/// and then waits, exactly as a human prompt would.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_question_with_an_approval_channel_waits_for_the_topology() {
    let mut r = rig(json!({"approval": "channel"}));
    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();
    let _ = r.reconfig_rx.try_recv();

    r.event(frame(json!({
        "type": "control_request", "request_id": "r-1",
        "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {}}
    })))
    .await;
    assert_eq!(r.emission()["header"]["harness_event"], "question");
    assert!(
        r.reconfig_rx.try_recv().is_err(),
        "with a channel the cell must not decide on its own"
    );

    // The topology answers.
    r.call(
        "answer",
        json!({"task_id": "t-1", "request_id": "r-1", "behavior": "allow"}),
    )
    .await;

    match r.reconfig_rx.try_recv().expect("no decision forwarded") {
        HarnessReconfig::Child(meclaw_cells::stdio_child::ChildCommand::Send { line, .. }) => {
            assert_eq!(line["response"]["request_id"], "r-1");
            assert_eq!(line["response"]["response"]["behavior"], "allow");
        }
        other => panic!("expected the forwarded decision, got {other:?}"),
    }
    assert_eq!(r.reply()["header"]["harness_event"], "accepted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_for_a_task_that_is_not_running_is_refused() {
    let mut r = rig(json!({"approval": "channel"}));
    r.call(
        "answer",
        json!({"task_id": "t-1", "request_id": "r-1", "behavior": "allow"}),
    )
    .await;
    assert_eq!(r.reply()["header"]["error_code"], "invalid_input");

    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();
    r.call(
        "answer",
        json!({"task_id": "t-other", "request_id": "r-1", "behavior": "allow"}),
    )
    .await;
    assert_eq!(
        r.reply()["header"]["error_code"],
        "invalid_input",
        "an answer must name the task that actually asked"
    );
}

// ---------------------------------------------------------------- cancel ----

/// The stop lever (D8 condition). The tombstone is written BEFORE the kill, so
/// whoever reads the table next sees a deliberate cancellation rather than a
/// mysterious crash.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_marks_the_task_before_it_stops_the_child() {
    let mut r = rig(json!({}));
    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();
    let _ = r.reconfig_rx.try_recv();

    r.call("cancel", json!({"task_id": "t-1", "reason": "obsolete"}))
        .await;

    assert_eq!(
        r.probe_status("t-1").as_deref(),
        Some("cancelled"),
        "the tombstone must be written before the kill is requested"
    );
    assert!(
        matches!(
            r.reconfig_rx.try_recv().expect("no shutdown sent"),
            HarnessReconfig::Child(meclaw_cells::stdio_child::ChildCommand::Shutdown)
        ),
        "cancel must ask the io task to tear the child down"
    );
    assert_eq!(r.reply()["header"]["harness_event"], "accepted");

    // The child's exit then reports a cancellation, not a crash.
    r.event(HarnessEvent::Child(ChildEvent::Exited(ChildExit::Signal)))
        .await;
    let em = r.emission();
    assert_eq!(em["header"]["status"], "cancelled");
    assert_eq!(em["header"]["error_code"], "cancelled");
    assert_eq!(
        r.probe_status("t-1").as_deref(),
        Some("cancelled"),
        "the exit must not overwrite the cancellation reason"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_for_an_unknown_task_is_refused() {
    let mut r = rig(json!({}));
    r.call("cancel", json!({"task_id": "t-nope"})).await;
    assert_eq!(r.reply()["header"]["error_code"], "invalid_input");
}

// ---------------------------------------------------------------- status ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_reports_the_last_known_state_from_the_table() {
    let mut r = rig(json!({}));
    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();

    r.call("status", json!({"task_id": "t-1"})).await;
    let reply = r.reply();
    let payload: JsonValue =
        meclaw_core::serde_json::from_str(reply["messages"][0]["text"].as_str().expect("text"))
            .expect("status payload is json");
    assert_eq!(payload["task_id"], "t-1");
    assert_eq!(payload["status"], "running");
    assert!(payload["workspace"].as_str().expect("ws").ends_with("ws"));

    r.call("status", json!({"task_id": "never-existed"})).await;
    assert_eq!(r.reply()["header"]["error_code"], "invalid_input");
}

// ------------------------------------------------- runtime params update ----

/// β params update (`config.md` § access): tuning applies, containment does
/// not. A message must never be able to widen what the harness may do.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_params_update_tunes_budgets_but_cannot_widen_containment() {
    let mut r = rig(json!({}));

    r.params_update(json!({"max_turns": 3})).await;
    assert!(
        r.outputs.try_recv().is_err(),
        "an accepted params update answers silently"
    );

    r.params_update(json!({"command": "/bin/sh"})).await;
    let reply = r.reply();
    assert_eq!(reply["header"]["error_code"], "invalid_input");
    assert!(
        reply["messages"][0]["text"]
            .as_str()
            .expect("text")
            .contains("command"),
        "the reject must name the immutable key"
    );
}

fn tool_call(name: &str, args: JsonValue) -> Message {
    MessageBuilder::new(Path::new("/harness"))
        .reply_to(Path::new("/sink"))
        .body(Body::Inline(json!({
            "messages": [{
                "origin": "assistant", "type": "tool_call", "id": "call-1",
                "text": json!({"name": name, "arguments": args}).to_string()
            }]
        })))
        .build()
}

fn start_args(task_id: &str) -> JsonValue {
    json!({"task_id": task_id, "prompt": "do it", "workspace": "ws"})
}

fn frame(v: JsonValue) -> HarnessEvent {
    HarnessEvent::Child(ChildEvent::Frame(v))
}

// ---------------------------------------------------------------- start ----

/// The load-bearing order: the tombstone is written BEFORE the child is asked
/// for. A crash in between leaves a row that recovery reports as unknown; the
/// other order would leave an agent running with no record of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_started_task_is_recorded_before_the_child_is_requested() {
    let mut r = rig(json!({}));
    r.call("start_task", start_args("t-1")).await;

    assert_eq!(
        r.probe_status("t-1").as_deref(),
        Some("running"),
        "the row must be committed, not merely queued"
    );

    match r.reconfig_rx.try_recv().expect("no start command") {
        HarnessReconfig::Start(s) => {
            assert_eq!(s.task_id, "t-1");
            assert!(s.spec.process_group, "a harness spawns trees; reap them");
            assert!(s.spec.env_clear, "the child must not inherit our secrets");
            assert_eq!(
                s.spec.cwd.as_deref(),
                Some(r.root.join("ws").as_path()),
                "the child works in the assigned workspace, nowhere else"
            );
        }
        other => panic!("expected a start command, got {other:?}"),
    }

    let reply = r.reply();
    assert_eq!(reply["header"]["harness_event"], "accepted");
    assert_eq!(reply["header"]["task_id"], "t-1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_task_while_one_runs_is_refused() {
    let mut r = rig(json!({}));
    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();

    r.call("start_task", start_args("t-2")).await;
    let reply = r.reply();
    assert_eq!(reply["header"]["error_code"], "harness_busy");
    assert!(
        r.probe_status("t-2").is_none(),
        "a refused task must leave no tombstone"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workspace_outside_the_root_is_refused() {
    let mut r = rig(json!({}));
    for escape in ["../elsewhere", "/etc", "ws/../../outside"] {
        r.call(
            "start_task",
            json!({"task_id": "t-x", "prompt": "p", "workspace": escape}),
        )
        .await;
        let reply = r.reply();
        assert_eq!(
            reply["header"]["error_code"], "workspace_invalid",
            "escape {escape} was not refused"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repeated_task_id_is_refused_even_after_the_first_finished() {
    let mut r = rig(json!({}));
    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();
    let _ = r.reconfig_rx.try_recv();

    // Finish the first run.
    r.event(frame(
        json!({"type": "result", "is_error": false, "result": "done"}),
    ))
    .await;
    r.event(HarnessEvent::Child(ChildEvent::Exited(ChildExit::Code(0))))
        .await;
    let _ = r.emission();

    r.call("start_task", start_args("t-1")).await;
    let reply = r.reply();
    assert_eq!(
        reply["header"]["error_code"], "invalid_input",
        "a task id may run once, ever"
    );
}

// ---------------------------------------------------------------- frames ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_init_frame_records_the_session_and_reports_progress() {
    let mut r = rig(json!({}));
    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();

    r.event(frame(
        json!({"type": "system", "subtype": "init", "session_id": "s-9", "model": "m-1"}),
    ))
    .await;

    let em = r.emission();
    assert_eq!(em["header"]["harness_event"], "progress");
    assert_eq!(em["header"]["session_id"], "s-9");

    let c = rusqlite::Connection::open(&r.db_path).expect("probe");
    let row = load_task(&c, "t-1").expect("load").expect("row");
    assert_eq!(
        row.session_id.as_deref(),
        Some("s-9"),
        "the session id is the audit anchor and must survive a restart"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_successful_run_closes_the_tombstone_and_reports_the_outcome() {
    let mut r = rig(json!({}));
    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();

    r.event(frame(json!({"type": "system", "subtype": "init",
                         "session_id": "s-9", "model": "m-1"})))
        .await;
    let _ = r.emission();
    r.event(frame(
        json!({"type": "result", "subtype": "success", "is_error": false,
                         "num_turns": 2, "total_cost_usd": 0.5, "result": "created it"}),
    ))
    .await;
    r.event(HarnessEvent::Child(ChildEvent::Exited(ChildExit::Code(0))))
        .await;

    let em = r.emission();
    assert_eq!(em["header"]["harness_event"], "result");
    assert_eq!(em["header"]["status"], "ok");
    assert_eq!(em["header"]["num_turns"], 2);
    assert_eq!(em["header"]["cost_usd"], 0.5);
    assert_eq!(em["header"]["model"], "m-1");
    assert_eq!(em["header"]["session_id"], "s-9");
    assert_eq!(r.probe_status("t-1").as_deref(), Some("ok"));
}

/// The structural difference from `mcp`: a dead child is normal here. It must
/// close the task, not panic the cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_that_dies_without_a_result_is_a_crashed_task_not_a_panic() {
    let mut r = rig(json!({}));
    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();

    r.event(HarnessEvent::Child(ChildEvent::Exited(ChildExit::Code(1))))
        .await;

    let em = r.emission();
    assert_eq!(em["header"]["status"], "crashed");
    assert_eq!(em["header"]["error_code"], "harness_crashed");
    assert_eq!(r.probe_status("t-1").as_deref(), Some("crashed"));

    // And the cell still works: the next task is accepted.
    r.call("start_task", start_args("t-2")).await;
    assert_eq!(r.reply()["header"]["harness_event"], "accepted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_start_closes_the_task_with_its_reason() {
    let mut r = rig(json!({}));
    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();

    r.event(HarnessEvent::TaskFailed {
        task_id: "t-1".to_string(),
        error_code: "startup_timeout",
        detail: "said nothing".to_string(),
    })
    .await;

    let em = r.emission();
    assert_eq!(em["header"]["status"], "error");
    assert_eq!(em["header"]["error_code"], "startup_timeout");
    assert_eq!(r.probe_status("t-1").as_deref(), Some("error"));
}

// ------------------------------------------------------------- recovery ----

/// The most important test of this package: after a restart an interrupted
/// task is reported as unknown and NOT re-run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_reports_unknown_outcomes_and_starts_nothing() {
    let mut r = rig(json!({}));
    r.call("start_task", start_args("t-1")).await;
    let _ = r.reply();
    let _ = r.reconfig_rx.try_recv().expect("the first start command");

    // A restart: same cell.db, fresh cell (in-memory state lost, per spec).
    r.cell = HarnessCell::new(r.params.clone());
    r.event(HarnessEvent::Booted).await;

    let em = r.emission();
    assert_eq!(em["header"]["harness_event"], "result");
    assert_eq!(em["header"]["status"], "unknown");
    assert_eq!(em["header"]["error_code"], "unknown_outcome");
    assert_eq!(em["header"]["task_id"], "t-1");
    assert!(
        em["header"]["workspace"]
            .as_str()
            .expect("workspace")
            .ends_with("ws"),
        "the report must say where to look"
    );
    assert_eq!(r.probe_status("t-1").as_deref(), Some("unknown"));

    // NOTHING was started. This is the whole point.
    assert!(
        r.reconfig_rx.try_recv().is_err(),
        "a restart must never re-fire a task"
    );

    // A second restart says nothing more.
    r.event(HarnessEvent::Booted).await;
    assert!(
        r.origin_rx.try_recv().is_err(),
        "an already-reported orphan must not be reported again"
    );
}
