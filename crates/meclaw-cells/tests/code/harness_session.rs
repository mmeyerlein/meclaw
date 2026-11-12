//! D57 — a child is not a task: the handler's book-keeping of children and
//! turns.
//!
//! Driven against the handler alone (the rig is a copy of the one in
//! `harness_cell_lifecycle.rs`; test stems do not import each other): events
//! are played in, commands read off `reconfig_rx`. Real children run in
//! `harness_io_loop.rs` and `p8_harness_demo.rs`.

use meclaw_cells::harness::HarnessParams;
use meclaw_cells::harness::cell::HarnessCell;
use meclaw_cells::harness::db::{load_task, setup_harness_schema};
use meclaw_cells::harness::io::{HarnessEvent, HarnessReconfig, StartTask};
use meclaw_cells::stdio_child::ChildCommand;
use meclaw_cells::stdio_child::{ChildEvent, ChildExit};
use meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::serde_json::{Value as JsonValue, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, OriginSink, OutputSink, Path};
use std::time::Duration;
use tokio::sync::mpsc;

/// Everything a handler test needs, wired the way `cell_task_long_running`
/// wires it.
#[allow(dead_code)] // the persistent-mode tests use the rest of the rig
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

#[allow(dead_code)]
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

    /// Bring the cell up the way `cell_task_long_running` does: startup first,
    /// service afterwards.
    async fn start(&mut self) {
        self.cell.on_start(&self.origin, &mut self.db).await;
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

impl Rig {
    /// The next command to the I/O sub-task, if any.
    fn command(&mut self) -> Option<HarnessReconfig> {
        self.reconfig_rx.try_recv().ok()
    }

    /// Start a task and return its `Start` command.
    async fn start_task(&mut self, task_id: &str) -> StartTask {
        self.call("start_task", start_args(task_id)).await;
        assert_eq!(self.reply()["header"]["harness_event"], "accepted");
        loop {
            match self.command().expect("no start command") {
                HarnessReconfig::Start(s) => return s,
                _ => continue,
            }
        }
    }

    /// Run the turn of `task_id` up to its `result` emission.
    async fn finish_turn(&mut self, session: &str) -> JsonValue {
        self.event(frame(json!({"type": "system", "subtype": "init",
                                "session_id": session, "model": "m-1"})))
            .await;
        assert_eq!(self.emission()["header"]["phase"], "started");
        self.event(frame(json!({"type": "result", "subtype": "success",
                                "is_error": false, "num_turns": 1,
                                "total_cost_usd": 0.25, "result": "done"})))
            .await;
        self.emission()
    }
}

fn exited() -> HarnessEvent {
    HarnessEvent::Child(ChildEvent::Exited(ChildExit::Code(0)))
}

fn is_shutdown(cmd: Option<HarnessReconfig>) -> bool {
    matches!(cmd, Some(HarnessReconfig::Child(ChildCommand::Shutdown)))
}

/// D57 V7 per_task: the task ends at the `result` frame — the outcome goes out
/// there, the child is told to stop right after (its stdin stays open, it
/// would not end on its own), and its exit is silent. The prompt travels as
/// the first turn, not on the argv.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_per_task_cell_closes_its_child_after_the_result() {
    let mut r = rig(json!({}));
    let start = r.start_task("t-1").await;
    assert_eq!(
        start.first_turn,
        Some(json!({"type": "user", "message": {"role": "user", "content": "do it"}}))
    );
    assert!(
        !start.spec.args.contains(&"do it".to_string()),
        "the prompt is not on the argv"
    );

    let em = r.finish_turn("s-1").await;
    assert_eq!(em["header"]["harness_event"], "result");
    assert_eq!(em["header"]["status"], "ok");
    assert_eq!(em["header"]["cost_usd"], 0.25);
    assert_eq!(r.probe_status("t-1").as_deref(), Some("ok"));
    assert!(
        is_shutdown(r.command()),
        "the child is closed after its task"
    );

    r.event(exited()).await;
    assert!(
        r.origin_rx.try_recv().is_err(),
        "the exit after the result is silent"
    );
}

/// D57 V6 / OR-57-31 M4: the exit — and a late `result` — of a replaced child
/// belong to that child and never close the next task's turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replaced_childs_exit_never_closes_the_next_turn() {
    let mut r = rig(json!({}));
    r.start_task("t-1").await;
    r.finish_turn("s-1").await;
    assert!(is_shutdown(r.command()));

    // The next task starts while the first child is still closing.
    r.start_task("t-2").await;

    // A late frame of the old child, then its exit.
    r.event(frame(
        json!({"type": "result", "is_error": true, "result": "late"}),
    ))
    .await;
    r.event(exited()).await;
    assert!(
        r.origin_rx.try_recv().is_err(),
        "neither the late result nor the old exit may speak for t-2"
    );
    assert_eq!(r.probe_status("t-2").as_deref(), Some("running"));

    // t-2's own turn closes normally.
    let em = r.finish_turn("s-2").await;
    assert_eq!(em["header"]["task_id"], "t-2");
    assert_eq!(em["header"]["status"], "ok");
}

/// D57 V6/V7 / OR-57-24: a cancel of a task whose child waits behind a
/// closing one is bound to the NEW child: the old exit closes nothing, the
/// new exit ends the turn as cancelled, and the cell is free afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_while_the_previous_child_closes_is_bound_to_the_new_child() {
    let mut r = rig(json!({}));
    r.start_task("t-1").await;
    r.finish_turn("s-1").await;
    assert!(is_shutdown(r.command()));
    r.start_task("t-2").await;

    r.call("cancel", json!({"task_id": "t-2", "reason": "obsolete"}))
        .await;
    assert_eq!(r.reply()["header"]["harness_event"], "accepted");
    assert!(is_shutdown(r.command()), "the cancel stops t-2's child");
    assert_eq!(r.probe_status("t-2").as_deref(), Some("cancelled"));

    // The old child's exit: t-2 is still in flight, the cell still busy.
    r.event(exited()).await;
    assert!(r.origin_rx.try_recv().is_err());
    r.call("start_task", start_args("t-3")).await;
    assert_eq!(r.reply()["header"]["error_code"], "harness_busy");

    // t-2's child's exit ends the turn as cancelled, and frees the cell.
    r.event(exited()).await;
    let em = r.emission();
    assert_eq!(em["header"]["task_id"], "t-2");
    assert_eq!(em["header"]["status"], "cancelled");
    r.call("start_task", start_args("t-3")).await;
    assert_eq!(r.reply()["header"]["harness_event"], "accepted");
}

/// B6 / AGENTS rule 15: the app id reaches the child as
/// `ANTHROPIC_CUSTOM_HEADERS`, one line per set key, and nothing is sent
/// when neither is set (OR-57-8, no default).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_app_id_reaches_the_child_as_custom_headers() {
    fn headers(s: &StartTask) -> Option<String> {
        s.spec
            .env
            .iter()
            .find(|(k, _)| k == "ANTHROPIC_CUSTOM_HEADERS")
            .map(|(_, v)| v.clone())
    }

    let mut r = rig(json!({"x_title": "meclaw-lab harness", "http_referer": "https://meclaw.ai"}));
    let s = r.start_task("t-1").await;
    assert_eq!(
        headers(&s).as_deref(),
        Some("X-Title: meclaw-lab harness\nHTTP-Referer: https://meclaw.ai")
    );

    let mut r = rig(json!({"x_title": "meclaw-lab harness"}));
    let s = r.start_task("t-1").await;
    assert_eq!(headers(&s).as_deref(), Some("X-Title: meclaw-lab harness"));

    let mut r = rig(json!({}));
    let s = r.start_task("t-1").await;
    assert_eq!(headers(&s), None);
}

// ---------------------------------------------------------------------------
// `persistent` mode (D57 V6–V12): one child carries a session over many tasks.
// ---------------------------------------------------------------------------

use meclaw_cells::harness::claude_code::build_user_turn;
use meclaw_cells::harness::db::{SessionRow, load_session};

fn persistent(extra: JsonValue) -> Rig {
    let mut cfg = json!({"session": "persistent"});
    for (k, v) in extra.as_object().expect("object") {
        cfg[k] = v.clone();
    }
    rig(cfg)
}

/// What a command to the I/O sub-task is, in one word (plus the resume id
/// of a start).
fn kind(cmd: &HarnessReconfig) -> String {
    match cmd {
        HarnessReconfig::Start(s) => match resume_of(s) {
            Some(r) => format!("start --resume {r}"),
            None => "start".to_string(),
        },
        HarnessReconfig::Child(ChildCommand::Send { .. }) => "send".to_string(),
        HarnessReconfig::Child(ChildCommand::Shutdown) => "shutdown".to_string(),
        HarnessReconfig::ArmIdle(_) => "arm".to_string(),
    }
}

fn resume_of(s: &StartTask) -> Option<String> {
    let i = s.spec.args.iter().position(|a| a == "--resume")?;
    s.spec.args.get(i + 1).cloned()
}

fn kinds(cmds: &[HarnessReconfig]) -> Vec<String> {
    cmds.iter().map(kind).collect()
}

/// `harness_event` plus phase/status of each emission, in order.
fn shape(ems: &[JsonValue]) -> Vec<String> {
    ems.iter()
        .map(|e| {
            let h = &e["header"];
            match h["harness_event"].as_str() {
                Some("progress") => format!("progress {}", h["phase"].as_str().unwrap_or("")),
                Some("result") => format!("result {}", h["status"].as_str().unwrap_or("")),
                other => other.unwrap_or("").to_string(),
            }
        })
        .collect()
}

impl Rig {
    /// Every command sent to the I/O sub-task so far.
    fn commands(&mut self) -> Vec<HarnessReconfig> {
        std::iter::from_fn(|| self.reconfig_rx.try_recv().ok()).collect()
    }

    /// Every origin emission so far.
    fn emissions(&mut self) -> Vec<JsonValue> {
        std::iter::from_fn(|| self.origin_rx.try_recv().ok().map(|e| e.content)).collect()
    }

    /// Start a task with these arguments; it must be accepted.
    async fn submit(&mut self, args: JsonValue) -> Vec<HarnessReconfig> {
        self.call("start_task", args).await;
        assert_eq!(self.reply()["header"]["harness_event"], "accepted");
        self.commands()
    }

    /// One turn as the CLI streams it: init, one model call with `context`
    /// tokens, result with the process-wide `total` cost. Returns what was
    /// emitted.
    async fn turn(&mut self, session: &str, context: u64, total: f64) -> Vec<JsonValue> {
        self.event(frame(json!({"type": "system", "subtype": "init",
                                "session_id": session, "model": "m-1"})))
            .await;
        self.event(frame(
            json!({"type": "assistant", "parent_tool_use_id": null,
                                "message": {"content": [{"type": "text", "text": "working"}],
                                            "usage": {"input_tokens": context}}}),
        ))
        .await;
        self.event(frame(json!({"type": "result", "subtype": "success",
                                "is_error": false, "num_turns": 1,
                                "total_cost_usd": total, "result": "done"})))
            .await;
        self.emissions()
    }

    fn session_row(&self, id: &str) -> Option<SessionRow> {
        let c = rusqlite::Connection::open(&self.db_path).expect("probe");
        load_session(&c, id).expect("load")
    }

    fn session_of_task(&self, task_id: &str) -> Option<String> {
        let c = rusqlite::Connection::open(&self.db_path).expect("probe");
        load_task(&c, task_id)
            .expect("load")
            .and_then(|r| r.session_id)
    }

    /// A cell restart: a new handler over the same `cell.db`, brought up the
    /// way the supervisor does (recovery first).
    async fn restart(&mut self) {
        let (conn, _s) = open_or_create_cell_db_with_status(&self.db_path).expect("open");
        setup_harness_schema(&conn).expect("schema");
        self.db = DbConn::wrap(conn, Some(Duration::from_secs(2)));
        self.cell = HarnessCell::new(self.params.clone());
        self.start().await;
    }
}

/// ADR pin. D57: a `persistent` cell runs two tasks as two turns of ONE
/// child — the second task is a `Send` to it, no spawn, no shutdown — and
/// both turns are booked on one session; between turns the idle deadline is
/// armed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persistent_cell_runs_two_tasks_in_one_child() {
    let mut r = persistent(json!({}));
    assert_eq!(kinds(&r.submit(start_args("t-1")).await), vec!["start"]);
    let ems = r.turn("s-1", 1_000, 0.25).await;
    assert_eq!(
        shape(&ems),
        vec!["progress started", "progress text", "result ok"]
    );
    assert_eq!(kinds(&r.commands()), vec!["arm"], "the child stays up");

    let cmds = r
        .submit(json!({"task_id": "t-2", "prompt": "and then this", "workspace": "ws"}))
        .await;
    assert_eq!(kinds(&cmds), vec!["send"], "no second child");
    let HarnessReconfig::Child(ChildCommand::Send { line, .. }) = &cmds[0] else {
        unreachable!()
    };
    assert_eq!(line, &build_user_turn("and then this"));

    let ems = r.turn("s-1", 2_000, 0.75).await;
    assert_eq!(ems.last().expect("result")["header"]["task_id"], "t-2");
    assert_eq!(ems.last().expect("result")["header"]["session_id"], "s-1");
    assert_eq!(kinds(&r.commands()), vec!["arm"]);

    let row = r.session_row("s-1").expect("session row");
    assert_eq!((row.turns, row.closed_at), (2, None));
}

/// D57 V8/V11: every turn has its own tombstone, and its `cost_usd` is the
/// turn's share of the process-wide counter (OR-57-7). The session sums them
/// and keeps the last call's context.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_turn_has_its_own_tombstone_and_turn_cost() {
    let mut r = persistent(json!({}));
    r.submit(start_args("t-1")).await;
    let first = r.turn("s-1", 1_000, 0.25).await;
    r.commands();
    r.submit(start_args("t-2")).await;
    let second = r.turn("s-1", 2_000, 0.75).await;

    assert_eq!(first.last().expect("result")["header"]["cost_usd"], 0.25);
    assert_eq!(second.last().expect("result")["header"]["cost_usd"], 0.5);
    for t in ["t-1", "t-2"] {
        assert_eq!(r.probe_status(t).as_deref(), Some("ok"), "{t}");
        assert_eq!(r.session_of_task(t).as_deref(), Some("s-1"), "{t}");
    }
    let row = r.session_row("s-1").expect("session row");
    assert_eq!(row.cost_usd, 0.75);
    assert_eq!(row.last_context_tokens, Some(2_000));
}

/// D57 V7 + table row 4: a child that dies mid-turn crashes that turn only;
/// the session stays open and the next task resumes it in a new child.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crash_mid_turn_marks_the_turn_and_the_next_task_resumes_the_session() {
    let mut r = persistent(json!({}));
    r.submit(start_args("t-1")).await;
    r.event(frame(
        json!({"type": "system", "subtype": "init", "session_id": "s-1"}),
    ))
    .await;
    r.event(exited()).await;
    let ems = r.emissions();
    assert_eq!(shape(&ems), vec!["progress started", "result crashed"]);
    assert_eq!(r.probe_status("t-1").as_deref(), Some("crashed"));
    assert_eq!(r.session_row("s-1").expect("row").closed_at, None);

    assert_eq!(
        kinds(&r.submit(start_args("t-2")).await),
        vec!["start --resume s-1"]
    );
}

/// D57 V12: a restart turns the running turn into `unknown` (its tombstone),
/// but the open session survives and the next task resumes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_marks_the_running_turn_unknown_and_resumes_the_open_session() {
    let mut r = persistent(json!({}));
    r.submit(start_args("t-1")).await;
    r.turn("s-1", 1_000, 0.25).await;
    r.commands();
    r.submit(start_args("t-2")).await;
    r.event(frame(
        json!({"type": "system", "subtype": "init", "session_id": "s-1"}),
    ))
    .await;
    r.emissions();

    r.restart().await;
    let ems = r.emissions();
    assert_eq!(ems.len(), 1);
    assert_eq!(ems[0]["header"]["task_id"], "t-2");
    assert_eq!(ems[0]["header"]["status"], "unknown");
    assert_eq!(r.probe_status("t-2").as_deref(), Some("unknown"));

    assert_eq!(
        kinds(&r.submit(start_args("t-3")).await),
        vec!["start --resume s-1"]
    );
}

/// D57 V9 decision table, rows 4, 1, 5 and 2 in that order: another model in
/// the same tree resumes the session in a new child; `new_session` ends it;
/// another tree ends it; an explicit resume id replaces the open session.
/// Every ended session is announced as `session_closed` and closed in the
/// table with its reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_model_and_new_session_follow_the_decision_table() {
    let mut r = persistent(json!({}));
    std::fs::create_dir_all(r.root.join("ws2")).expect("second workspace");
    r.submit(start_args("t-1")).await;
    r.turn("s-1", 1_000, 0.25).await;
    r.commands();

    // Row 4: another model, same tree.
    let cmds = r
        .submit(json!({"task_id": "t-2", "prompt": "p", "workspace": "ws", "model": "m-2"}))
        .await;
    assert_eq!(kinds(&cmds), vec!["shutdown", "start --resume s-1"]);
    r.event(exited()).await;
    r.turn("s-1", 1_000, 0.25).await;
    r.commands();

    // Row 1: a new session on request.
    let cmds = r
        .submit(json!({"task_id": "t-3", "prompt": "p", "workspace": "ws",
                       "model": "m-2", "new_session": true}))
        .await;
    assert_eq!(kinds(&cmds), vec!["shutdown", "start"]);
    r.event(exited()).await;
    let ems = r.turn("s-2", 1_000, 0.25).await;
    assert_eq!(shape(&ems)[0], "progress session_closed");
    assert_eq!(ems[0]["header"]["session_id"], "s-1");
    assert_eq!(ems[0]["header"]["task_id"], "t-3");
    assert_eq!(ems[0]["messages"][0]["text"], "new_session");
    assert_eq!(
        r.session_row("s-1").expect("row").close_reason.as_deref(),
        Some("new_session")
    );
    r.commands();

    // Row 5: another tree.
    let cmds = r
        .submit(json!({"task_id": "t-4", "prompt": "p", "workspace": "ws2", "model": "m-2"}))
        .await;
    assert_eq!(kinds(&cmds), vec!["shutdown", "start"]);
    r.event(exited()).await;
    let ems = r.turn("s-3", 1_000, 0.25).await;
    assert_eq!(ems[0]["messages"][0]["text"], "workspace");
    assert_eq!(
        r.session_row("s-2").expect("row").close_reason.as_deref(),
        Some("workspace")
    );
    r.commands();

    // Row 2: an explicit resume of another session.
    let cmds = r
        .submit(json!({"task_id": "t-5", "prompt": "p", "workspace": "ws2",
                       "model": "m-2", "resume_session_id": "s-1"}))
        .await;
    assert_eq!(kinds(&cmds), vec!["shutdown", "start --resume s-1"]);
    r.event(exited()).await;
    let ems = r.turn("s-1", 1_000, 0.25).await;
    assert_eq!(ems[0]["messages"][0]["text"], "replaced");
    assert_eq!(
        r.session_row("s-3").expect("row").close_reason.as_deref(),
        Some("replaced")
    );
    let s1 = r.session_row("s-1").expect("row");
    assert_eq!(s1.closed_at, None, "reopened");
    // Review d57 s1 Minor 2: the re-opened row follows its new tree, or a
    // restart would load the old one and close the session as `workspace`.
    assert!(
        s1.workspace.ends_with("ws2"),
        "the row keeps the old tree: {}",
        s1.workspace
    );
}

/// Review d57 s1 Minor 2: re-opening a session writes the tree and the time of
/// THIS opening (`SessionRow::opened_at`: "first (or last re-)opening"); the
/// booked turns and cost stay.
#[test]
fn reopening_a_session_carries_its_new_tree_and_opening_time() {
    use meclaw_cells::harness::db::{close_session, open_session, record_turn};
    let conn = rusqlite::Connection::open_in_memory().expect("db");
    setup_harness_schema(&conn).expect("schema");
    open_session(&conn, "s-1", "/w/ws", "2026-11-11T10:00:00Z").expect("open");
    record_turn(&conn, "s-1", 0.25, Some(1_000)).expect("turn");
    close_session(&conn, "s-1", "replaced", "2026-11-11T10:05:00Z").expect("close");
    let cost = open_session(&conn, "s-1", "/w/ws2", "2026-11-11T10:10:00Z").expect("reopen");
    assert_eq!(cost, 0.25);
    let row = load_session(&conn, "s-1").expect("load").expect("row");
    assert_eq!(row.workspace, "/w/ws2");
    assert_eq!(row.opened_at, "2026-11-11T10:10:00Z");
    assert_eq!(
        (row.closed_at, row.close_reason, row.turns),
        (None, None, 1)
    );
}

/// Review d57 s1 Minor 3 (a, b): a runtime params update moves the money
/// budget and the three session knobs of the NEXT turn. The budget is part of
/// the "same start" (OR-57-26): the live child does not fit any more, a new one
/// continues the session in the same tree (row 4) with the new budget on its
/// argv; the idle deadline, the cost and the context limit follow the update.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_params_update_moves_the_budget_and_the_session_knobs() {
    let mut r = persistent(json!({}));
    assert_eq!(kinds(&r.submit(start_args("t-1")).await), vec!["start"]);
    r.turn("s-1", 1_000, 0.25).await;
    let cmds = r.commands();
    assert!(
        matches!(cmds.as_slice(), [HarnessReconfig::ArmIdle(d)] if *d == Duration::from_millis(1_800_000)),
        "default idle: {:?}",
        kinds(&cmds)
    );

    r.params_update(json!({"max_budget_usd": 2.0, "session_idle_ms": 5_000,
                           "session_max_usd": 0.5, "session_max_context_tokens": 1_500}))
        .await;
    assert!(r.outputs.try_recv().is_err(), "accepted silently");

    let cmds = r.submit(start_args("t-2")).await;
    assert_eq!(kinds(&cmds), vec!["shutdown", "start --resume s-1"]);
    let HarnessReconfig::Start(s) = &cmds[1] else {
        unreachable!()
    };
    let i = s
        .spec
        .args
        .iter()
        .position(|a| a == "--max-budget-usd")
        .expect("budget on the argv");
    assert_eq!(s.spec.args[i + 1], "2");
    r.event(exited()).await;
    r.turn("s-1", 1_000, 0.1).await;
    let cmds = r.commands();
    assert!(
        matches!(cmds.as_slice(), [HarnessReconfig::ArmIdle(d)] if *d == Duration::from_millis(5_000)),
        "updated idle: {:?}",
        kinds(&cmds)
    );

    // Same start now: the live child takes the turn. 0.25 + 0.1 + 0.3 passes
    // the updated cost limit.
    assert_eq!(kinds(&r.submit(start_args("t-3")).await), vec!["send"]);
    let ems = r.turn("s-1", 1_000, 0.4).await;
    assert_eq!(ems[2]["messages"][0]["text"], "cost");
    assert_eq!(kinds(&r.commands()), vec!["shutdown"]);
    r.event(exited()).await;

    // The updated context limit ends the next session after its first turn.
    assert_eq!(kinds(&r.submit(start_args("t-4")).await), vec!["start"]);
    let ems = r.turn("s-2", 2_000, 0.01).await;
    assert_eq!(ems[2]["messages"][0]["text"], "context");
}

/// Review d57 s1 Minor 3 (b): decision table row 2, first branch — an
/// explicit resume of the session the live child carries, with the same
/// start, is a turn on that child.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resuming_the_live_childs_own_session_is_a_turn_on_it() {
    let mut r = persistent(json!({}));
    r.submit(start_args("t-1")).await;
    r.turn("s-1", 1_000, 0.25).await;
    assert_eq!(kinds(&r.commands()), vec!["arm"]);
    let cmds = r
        .submit(json!({"task_id": "t-2", "prompt": "p", "workspace": "ws",
                       "resume_session_id": "s-1"}))
        .await;
    assert_eq!(kinds(&cmds), vec!["send"]);
    let ems = r.turn("s-1", 1_000, 0.5).await;
    assert_eq!(ems.last().expect("result")["header"]["task_id"], "t-2");
    assert_eq!(r.session_row("s-1").expect("row").turns, 2);
}

/// D57 V10: a session ends after the turn that reached its context or cost
/// limit — `session_closed` goes out BEFORE that turn's `result` (OR-57-31),
/// the child closes, and the next task starts a fresh session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_context_and_cost_limits_end_the_session_after_the_turn() {
    let mut r = persistent(json!({"session_max_context_tokens": 1_500}));
    r.submit(start_args("t-1")).await;
    r.turn("s-1", 1_000, 0.25).await;
    assert_eq!(kinds(&r.commands()), vec!["arm"], "below the limit");
    r.submit(start_args("t-2")).await;
    let ems = r.turn("s-1", 2_000, 0.5).await;
    assert_eq!(
        shape(&ems),
        vec![
            "progress started",
            "progress text",
            "progress session_closed",
            "result ok"
        ]
    );
    assert_eq!(ems[2]["messages"][0]["text"], "context");
    assert_eq!(kinds(&r.commands()), vec!["shutdown"]);
    assert_eq!(
        r.session_row("s-1").expect("row").close_reason.as_deref(),
        Some("context")
    );
    r.event(exited()).await;
    assert_eq!(kinds(&r.submit(start_args("t-3")).await), vec!["start"]);

    let mut r = persistent(json!({"session_max_usd": 0.5}));
    r.submit(start_args("t-1")).await;
    r.turn("s-1", 1_000, 0.25).await;
    assert_eq!(kinds(&r.commands()), vec!["arm"], "below the limit");
    r.submit(start_args("t-2")).await;
    let ems = r.turn("s-1", 1_000, 0.75).await;
    assert_eq!(shape(&ems)[2..], ["progress session_closed", "result ok"]);
    assert_eq!(ems[2]["messages"][0]["text"], "cost");
    assert_eq!(kinds(&r.commands()), vec!["shutdown"]);
}

/// D57 V10: after a turn the idle deadline is armed with `session_idle_ms`;
/// when it passes the idle child closes silently, the session stays open and
/// the next task resumes it. During a running turn the deadline closes
/// nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_child_closes_and_the_session_stays_open() {
    let mut r = persistent(json!({"session_idle_ms": 50}));
    r.submit(start_args("t-1")).await;
    r.event(HarnessEvent::IdleExpired).await;
    assert!(r.commands().is_empty(), "a running turn is not idle");

    r.turn("s-1", 1_000, 0.25).await;
    let cmds = r.commands();
    assert!(
        matches!(cmds.as_slice(), [HarnessReconfig::ArmIdle(d)] if *d == Duration::from_millis(50))
    );

    r.event(HarnessEvent::IdleExpired).await;
    assert_eq!(kinds(&r.commands()), vec!["shutdown"]);
    r.event(exited()).await;
    assert!(r.emissions().is_empty(), "idling out is silent");
    assert_eq!(r.session_row("s-1").expect("row").closed_at, None);

    assert_eq!(
        kinds(&r.submit(start_args("t-2")).await),
        vec!["start --resume s-1"]
    );
}

/// D57 V7 / OR-57-24: a cancel in a persistent turn ends the child (process
/// group, OR-57-6) and the turn as cancelled at its exit; the session stays
/// open and the next task resumes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancel_in_a_persistent_turn_ends_the_child_and_keeps_the_session() {
    let mut r = persistent(json!({}));
    r.submit(start_args("t-1")).await;
    r.turn("s-1", 1_000, 0.25).await;
    r.commands();
    r.submit(start_args("t-2")).await;
    r.event(frame(
        json!({"type": "system", "subtype": "init", "session_id": "s-1"}),
    ))
    .await;
    r.emissions();

    r.call("cancel", json!({"task_id": "t-2"})).await;
    assert_eq!(r.reply()["header"]["harness_event"], "accepted");
    assert_eq!(kinds(&r.commands()), vec!["shutdown"]);
    r.event(exited()).await;
    let ems = r.emissions();
    assert_eq!(shape(&ems), vec!["result cancelled"]);
    assert_eq!(r.session_row("s-1").expect("row").closed_at, None);

    assert_eq!(
        kinds(&r.submit(start_args("t-3")).await),
        vec!["start --resume s-1"]
    );
}

/// D57 V7 / OR-57-26: a failed turn (an expired login, a spent process
/// budget) closes the child — later turns on the same process would fail the
/// same way — but not the session; the next task resumes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_turn_closes_the_child_and_keeps_the_session() {
    let mut r = persistent(json!({}));
    r.submit(start_args("t-1")).await;
    r.turn("s-1", 1_000, 0.25).await;
    r.commands();
    r.submit(start_args("t-2")).await;
    r.event(frame(
        json!({"type": "system", "subtype": "init", "session_id": "s-1"}),
    ))
    .await;
    r.event(frame(
        json!({"type": "result", "subtype": "success", "is_error": true,
                         "result": "Invalid API key", "total_cost_usd": 0.25}),
    ))
    .await;
    let ems = r.emissions();
    assert_eq!(shape(&ems), vec!["progress started", "result error"]);
    assert_eq!(ems[1]["header"]["error_code"], "harness_crashed");
    assert_eq!(kinds(&r.commands()), vec!["shutdown"], "no idle arm");
    assert_eq!(r.session_row("s-1").expect("row").closed_at, None);
    r.event(exited()).await;
    assert!(r.emissions().is_empty());

    assert_eq!(
        kinds(&r.submit(start_args("t-3")).await),
        vec!["start --resume s-1"]
    );
}

/// D57 V7/V9 / OR-57-25: a child started to resume a session the CLI no
/// longer knows answers with one failed `result` and no init. The session
/// closes as `lost` (announced before the result), the turn's text is the
/// first error, and the next task starts fresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_session_is_closed_and_the_next_task_starts_fresh() {
    let mut r = persistent(json!({}));
    r.submit(start_args("t-1")).await;
    r.turn("s-1", 1_000, 0.25).await;
    r.commands();
    r.event(HarnessEvent::IdleExpired).await;
    r.event(exited()).await;
    r.commands();

    assert_eq!(
        kinds(&r.submit(start_args("t-2")).await),
        vec!["start --resume s-1"]
    );
    r.event(frame(
        json!({"type": "result", "subtype": "error_during_execution",
                         "is_error": true, "num_turns": 0,
                         "errors": ["No conversation found with session ID: s-1"]}),
    ))
    .await;
    let ems = r.emissions();
    assert_eq!(shape(&ems), vec!["progress session_closed", "result error"]);
    assert_eq!(ems[0]["messages"][0]["text"], "lost");
    assert_eq!(ems[0]["header"]["session_id"], "s-1");
    assert_eq!(
        ems[1]["messages"][0]["text"],
        "No conversation found with session ID: s-1"
    );
    assert_eq!(
        r.session_row("s-1").expect("row").close_reason.as_deref(),
        Some("lost")
    );
    assert_eq!(kinds(&r.commands()), vec!["shutdown"]);
    r.event(exited()).await;

    assert_eq!(kinds(&r.submit(start_args("t-3")).await), vec!["start"]);
}

/// D57 V10 / OR-57-31 M5: a follow-up turn that cannot be written to the
/// live child ends as `harness_crashed`, and the child closes; the session
/// stays open.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_follow_up_write_closes_the_turn() {
    let mut r = persistent(json!({}));
    r.submit(start_args("t-1")).await;
    r.turn("s-1", 1_000, 0.25).await;
    r.commands();
    assert_eq!(kinds(&r.submit(start_args("t-2")).await), vec!["send"]);

    r.event(HarnessEvent::WriteFailed {
        detail: "broken pipe".to_string(),
    })
    .await;
    let ems = r.emissions();
    assert_eq!(shape(&ems), vec!["result crashed"]);
    assert_eq!(ems[0]["header"]["error_code"], "harness_crashed");
    assert_eq!(r.probe_status("t-2").as_deref(), Some("crashed"));
    assert_eq!(kinds(&r.commands()), vec!["shutdown"]);
    assert_eq!(r.session_row("s-1").expect("row").closed_at, None);
}

/// D57 V13 (part C): in a `persistent` turn the question goes out on the
/// origin lane and the answer goes to the SAME child as a `Send` — no new
/// child, no shutdown — and the turn then ends on that child.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_question_in_a_persistent_turn_is_answered_on_the_same_child() {
    let mut r = persistent(json!({"approval": "channel"}));
    r.submit(start_args("t-1")).await;
    r.turn("s-1", 1_000, 0.25).await;
    r.commands();

    assert_eq!(kinds(&r.submit(start_args("t-2")).await), vec!["send"]);
    r.event(frame(
        json!({"type": "system", "subtype": "init", "session_id": "s-1"}),
    ))
    .await;
    r.event(frame(json!({
        "type": "control_request", "request_id": "r-1",
        "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {}}
    })))
    .await;
    let ems = r.emissions();
    assert_eq!(
        ems.last().expect("question")["header"]["harness_event"],
        "question"
    );
    assert!(
        r.commands().is_empty(),
        "the topology decides, not the cell"
    );

    r.call(
        "answer",
        json!({"task_id": "t-2", "request_id": "r-1", "behavior": "allow"}),
    )
    .await;
    assert_eq!(r.reply()["header"]["harness_event"], "accepted");
    let cmds = r.commands();
    assert_eq!(kinds(&cmds), vec!["send"], "the same child gets the answer");
    let HarnessReconfig::Child(ChildCommand::Send { line, .. }) = &cmds[0] else {
        unreachable!()
    };
    assert_eq!(line["response"]["request_id"], "r-1");
    assert_eq!(line["response"]["response"]["behavior"], "allow");

    r.event(frame(json!({"type": "result", "subtype": "success",
                            "is_error": false, "num_turns": 1,
                            "total_cost_usd": 0.5, "result": "done"})))
        .await;
    let ems = r.emissions();
    assert_eq!(ems.last().expect("result")["header"]["task_id"], "t-2");
    assert_eq!(shape(&ems).last().map(String::as_str), Some("result ok"));
    assert_eq!(kinds(&r.commands()), vec!["arm"], "the child stays up");
}
