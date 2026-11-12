//! `HarnessCell` — the handler side of the cell type.
//!
//! It owns three things: the task table in `cell.db`, the identity of the one
//! task that may be running, and the translation of the child's frames into
//! emissions. It owns no pipe and no process; those live in the I/O sub-task.
//!
//! The one behaviour to keep in mind when reading this file is what happens on
//! `Exited`. In `mcp` that event is a panic — a dead child means the cell can
//! no longer answer anything. Here it is ORDINARY, and panicking would restart
//! a cell that is perfectly healthy. See `mcp/cell.rs` for the other side of
//! that decision.
//!
//! A child and a task are not the same thing (D57 V6): a task is one TURN of a
//! child, and it ends at the child's `result` frame, not at its exit. The
//! handler therefore keeps three things apart — `child` (the live process),
//! `closing` (children sent a `Shutdown` whose `Exited` is still due, oldest
//! first) and `active` (the running turn). Because the I/O sub-task runs
//! children strictly one after the other, every `Exited` belongs to the oldest
//! closing child before the live one, and every frame that arrives while
//! `closing` is not empty comes from a closing child.

use crate::harness::claude_code::{self, ChildStart, HarnessSignal};
use crate::harness::db;
use crate::harness::emit::{self, TaskOutcome};
use crate::harness::io::{HarnessEvent, HarnessIo, HarnessReconfig, StartTask};
use crate::harness::params::{
    ApprovalMode, CUSTOM_HEADERS_ENV, HarnessOverlay, HarnessParams, SessionMode,
};
use crate::harness::parse::parse_tool_call;
use crate::harness::task::{AnswerRequest, HarnessCommand, TaskRequest};
use crate::stdio_child::{ChildCommand, ChildEvent, ChildSpec, ServeConfig};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Message, OriginSink, OutputSink};
use std::collections::VecDeque;
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// The turn currently being run, as far as the handler knows.
#[derive(Debug)]
struct ActiveTask {
    task_id: String,
    /// Absolute workspace path — our record, used in every emission.
    workspace: String,
    session_id: Option<String>,
    model: Option<String>,
    started: Instant,
    /// Set by a cancel: the turn then ends at its child's exit, as cancelled.
    cancelled: bool,
    /// Context of this turn's last top-level model call (D57 V3).
    context_tokens: Option<u64>,
}

/// The live child process.
#[derive(Debug)]
struct LiveChild {
    /// The task whose start spawned it — the key of a `TaskFailed`.
    spawned_for: String,
    /// What it was started with (model, turns, budget, resume).
    start: ChildStart,
    /// The process-wide `total_cost_usd` at the last `result`: the CLI
    /// counts per process, so a turn costs the difference (OR-57-7).
    last_total_cost_usd: Option<f64>,
    /// Absolute workspace it runs in.
    workspace: String,
    /// The session its init event reported; `None` before the first.
    session_id: Option<String>,
}

/// The open vendor session of a `persistent` cell (D57 V9), mirrored from
/// `harness_sessions`.
#[derive(Debug)]
struct OpenSession {
    session_id: String,
    workspace: String,
    /// Sum of its turn costs, for the cost rotation.
    cost_usd: f64,
}

/// A session that closed and whose `session_closed` emission is still due:
/// (session, reason, task that caused it). The close may happen in `handle`,
/// which has no origin lane; it goes out with the next event (OR-d57.s1.6).
type Unannounced = (String, &'static str, String);

/// "Same start" for a live child (D57 V9 table, OR-57-26): model, turn and
/// money budget are on its command line; the resume id is not compared.
fn same_start(a: &ChildStart, b: &ChildStart) -> bool {
    a.model == b.model && a.max_turns == b.max_turns && a.max_budget_usd == b.max_budget_usd
}

/// A child that was sent `Shutdown` and whose `Exited` is still due.
#[derive(Debug)]
struct ClosingChild {
    /// The task whose start spawned it.
    spawned_for: String,
    /// The turn that ends with this exit — set when a cancel closed the
    /// child under a running turn, `None` when the turn already ended.
    turn: Option<String>,
}

/// The `harness` cell. Single-threaded state in the handler sub-task.
pub struct HarnessCell {
    params: HarnessParams,
    initial_io: Option<HarnessIo>,
    active: Option<ActiveTask>,
    child: Option<LiveChild>,
    closing: VecDeque<ClosingChild>,
    /// `persistent` only: the open session.
    session: Option<OpenSession>,
    /// Closed sessions whose emission is still due, oldest first.
    unannounced: Vec<Unannounced>,
    /// Cloned from the first `handle` call, so `handle_event` can talk to the
    /// child too (the trait hands the reconfig sender to `handle` only).
    ///
    /// Sound by construction: a child only ever exists because a `start_task`
    /// message went through `handle` first, so anything `handle_event` could
    /// need to answer arrives strictly after this is set.
    reconfig: Option<mpsc::Sender<HarnessReconfig>>,
}

impl HarnessCell {
    /// Build a cell from its (birth ⊕ overlay) params.
    pub fn new(params: HarnessParams) -> Self {
        Self {
            params,
            initial_io: Some(HarnessIo),
            active: None,
            child: None,
            closing: VecDeque::new(),
            session: None,
            unannounced: Vec::new(),
            reconfig: None,
        }
    }

    /// Resolve a task's workspace against the configured root.
    ///
    /// Canonicalising both sides is what makes the containment real: `..`,
    /// symlinks and absolute paths all collapse before the prefix check, so
    /// there is no spelling of "outside" that survives it.
    fn resolve_workspace(&self, relative: &str) -> Result<std::path::PathBuf, String> {
        let joined = self.params.workspace_root.join(relative);
        let canonical = std::fs::canonicalize(&joined)
            .map_err(|e| format!("workspace {relative:?} unusable: {e}"))?;
        if !canonical.starts_with(&self.params.workspace_root) {
            return Err(format!(
                "workspace {relative:?} resolves outside the configured root"
            ));
        }
        if !canonical.is_dir() {
            return Err(format!("workspace {relative:?} is not a directory"));
        }
        Ok(canonical)
    }

    /// Build the child spec for one child: the adapter's argv plus the whole
    /// containment configuration.
    fn child_spec(&self, start: &ChildStart, cwd: std::path::PathBuf) -> ChildSpec {
        let mut env = self.params.env.clone();
        for key in &self.params.env_passthrough {
            if let Ok(value) = std::env::var(key) {
                env.push((key.clone(), value));
            }
        }
        // AGENTS rule 15: the child's LLM requests carry the app id. Parse
        // refuses an explicit env entry next to it, so there is one source.
        if let Some(headers) = self.params.custom_headers() {
            env.push((CUSTOM_HEADERS_ENV.to_string(), headers));
        }
        ChildSpec {
            program: self.params.command.clone(),
            args: claude_code::build_argv(&self.params, start),
            env,
            cwd: Some(cwd),
            kill_grace_ms: self.params.kill_grace_ms,
            // A harness spawns process trees of its own; without a group its
            // build and search tools outlive the cell.
            process_group: true,
            // Containment: the child sees the passthrough list and nothing
            // else of this process's environment.
            env_clear: true,
            // GH #85: the same profile `code` and `bash` carry, on the cell
            // whose child brings the largest surface of all three.
            sandbox: self.params.sandbox.clone().map(Box::new),
        }
    }

    /// Send a command to the I/O sub-task, if the channel is known.
    async fn to_io(&self, cmd: HarnessReconfig) -> bool {
        match &self.reconfig {
            Some(tx) => tx.send(cmd).await.is_ok(),
            None => false,
        }
    }
}

impl LongRunningCell for HarnessCell {
    type Event = HarnessEvent;
    type Reconfig = HarnessReconfig;
    type Io = HarnessIo;

    fn split_io(&mut self) -> Self::Io {
        self.initial_io.take().expect("split_io called twice")
    }

    /// I/O sub-task — delegates to `crate::harness::io::run_io`.
    /// `+ Send` is load-bearing (the `tokio::spawn` in
    /// `cell_task_long_running` needs it).
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        crate::harness::io::run_io(io, events_tx, reconfig_rx)
    }

    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
        reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            // Remember the channel for the event side (see the field's docs).
            if self.reconfig.is_none() {
                self.reconfig = Some(reconfig_tx.clone());
            }

            // β params-update slot (`config.md` § access), handled FIRST. What
            // may change is tuning; containment is immutable and a message
            // touching it gets a loud reject rather than a silent no-op.
            if let meclaw_core::Body::Inline(ref v) = msg.body
                && v.get("params").is_some()
            {
                self.apply_params_update(&msg, sink, db).await;
                return;
            }

            let parsed = match parse_tool_call(&msg) {
                Ok(p) => p,
                Err(reason) => {
                    emit::error(sink, &msg, "<unknown>", "invalid_input", &reason).await;
                    return;
                }
            };
            let command = match HarnessCommand::parse(&parsed.name, &parsed.arguments) {
                Ok(c) => c,
                Err(reason) => {
                    emit::error(sink, &msg, &parsed.call_id, "invalid_input", &reason).await;
                    return;
                }
            };

            match command {
                HarnessCommand::Start(task) => {
                    self.start_task(task, &msg, sink, db, &parsed.call_id).await
                }
                HarnessCommand::Answer(answer) => {
                    self.answer(answer, &msg, sink, &parsed.call_id).await
                }
                HarnessCommand::Cancel { task_id, reason } => {
                    self.cancel(&task_id, &reason, &msg, sink, db, &parsed.call_id)
                        .await
                }
                HarnessCommand::Status { task_id } => {
                    self.status(&task_id, &msg, sink, db, &parsed.call_id).await
                }
            }
        }
    }

    /// Restart recovery, before this cell serves anything.
    ///
    /// It used to hang off `HarnessEvent::Booted`, which raced the mailbox:
    /// under load the first `start_task` was handled first, and the late
    /// recovery then swept its brand-new `running` row into `unknown` — a false
    /// "interrupted by a cell restart" outcome for a task whose child was very
    /// much alive, followed by a second outcome under the same id when it
    /// really ended. Here the ordering is structural, not a race won.
    #[allow(clippy::manual_async_fn)]
    fn on_start<'a>(
        &'a mut self,
        sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move { self.recover_after_restart(sink, db).await }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            // A session closed while handling a message is announced here,
            // before anything the new turn says (D57 V10, OR-d57.s1.6).
            self.announce_closed_sessions(sink).await;
            match event {
                // Liveness only. Recovery happened in `on_start`, before any
                // message was served; there is nothing left to do here.
                HarnessEvent::Booted => tracing::debug!("harness: io sub-task up"),
                HarnessEvent::TaskFailed {
                    task_id,
                    error_code,
                    detail,
                } => {
                    self.on_task_failed(&task_id, error_code, detail, sink, db)
                        .await
                }
                HarnessEvent::Child(ChildEvent::Frame(v)) => self.on_frame(v, sink, db).await,
                HarnessEvent::Child(ChildEvent::Malformed { raw }) => {
                    // Harnesses print banners. Not an error, not worth an
                    // emission — the operator can read the child's stderr.
                    tracing::debug!(line = %raw, "harness: non-json line from child");
                }
                // NOT a panic: unlike `mcp`, this cell survives its child by
                // design — it exists to run the next task.
                HarnessEvent::Child(ChildEvent::Exited(exit)) => {
                    self.on_exit(exit.detail(), sink, db).await
                }
                HarnessEvent::IdleExpired => self.on_idle().await,
                HarnessEvent::WriteFailed { detail } => {
                    self.on_write_failed(detail, sink, db).await
                }
            }
        }
    }
}

impl HarnessCell {
    /// Apply a runtime params update.
    ///
    /// Only the tunables move, and only for the NEXT task: changing a budget
    /// under a running harness would be a promise this cell cannot keep, since
    /// the limits live in the child's own command line. An accepted update is
    /// silent (the W4b convention); a rejected one names the offending key.
    async fn apply_params_update(&mut self, msg: &Message, sink: &OutputSink, db: &mut DbConn) {
        let meclaw_core::Body::Inline(body) = &msg.body else {
            return;
        };
        let Some(update) = body.get("params").and_then(|p| p.as_object()).cloned() else {
            emit::error(
                sink,
                msg,
                "__params__",
                "invalid_input",
                "params slot: not a JSON object",
            )
            .await;
            return;
        };

        let current = HarnessOverlay::of(&self.params);
        let (new_overlay, persisted) = match crate::params_overlay::apply_update(&current, &update)
        {
            Ok(pair) => pair,
            Err(e) => {
                emit::error(sink, msg, "__params__", "invalid_input", &e.detail()).await;
                return;
            }
        };

        let now = crate::params_overlay::now_unix_seconds();
        let write = db
            .call_with_timeout(move |c| {
                crate::params_overlay::persist_params_overlay(c, &persisted, now)
            })
            .await;
        match write {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                emit::error(
                    sink,
                    msg,
                    "__params__",
                    "invalid_input",
                    &format!("cell.db params write failed: {e}"),
                )
                .await;
                return;
            }
            Err(meclaw_colony::QueryTimeout::Interrupted) => {
                emit::error(
                    sink,
                    msg,
                    "__params__",
                    "query_timeout",
                    "params write exceeded query_timeout_ms",
                )
                .await;
                return;
            }
        }

        new_overlay.apply_to(&mut self.params);
        db.set_query_timeout(Some(Duration::from_millis(self.params.query_timeout_ms)));
    }

    /// Accept a task: validate, record, then ask for the child — in that order.
    async fn start_task(
        &mut self,
        task: TaskRequest,
        msg: &Message,
        sink: &OutputSink,
        db: &mut DbConn,
        call_id: &str,
    ) {
        if let Some(active) = &self.active {
            emit::error(
                sink,
                msg,
                call_id,
                "harness_busy",
                &format!("task {} is still running", active.task_id),
            )
            .await;
            return;
        }
        let cwd = match self.resolve_workspace(&task.workspace) {
            Ok(p) => p,
            Err(reason) => {
                emit::error(sink, msg, call_id, "workspace_invalid", &reason).await;
                return;
            }
        };

        // State before action (phase-5 canon): the tombstone is committed
        // BEFORE anything is spawned. A crash in between is reported as an
        // unknown outcome; the reverse order would leave an agent process
        // running with no record that it ever existed.
        let workspace = cwd.display().to_string();
        let (task_id, ws) = (task.task_id.clone(), workspace.clone());
        let now = now_rfc3339();
        let written = db
            .call_with_timeout(move |c| db::insert_running_task(c, &task_id, &ws, &now))
            .await;
        match written {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                emit::error(
                    sink,
                    msg,
                    call_id,
                    "invalid_input",
                    &format!("task {:?} cannot be started: {e}", task.task_id),
                )
                .await;
                return;
            }
            Err(meclaw_colony::QueryTimeout::Interrupted) => {
                emit::error(
                    sink,
                    msg,
                    call_id,
                    "query_timeout",
                    "recording the task exceeded query_timeout_ms",
                )
                .await;
                return;
            }
        }

        let mut start = ChildStart::for_task(&self.params, &task);
        let to_live_child = self.params.session == SessionMode::Persistent
            && self.decide(&task, &workspace, &mut start, db).await;
        if to_live_child {
            // A follow-up turn: no spawn, the prompt goes to the live child
            // (V2). Its tombstone is already written, before the send (V8).
            self.active = Some(ActiveTask {
                task_id: task.task_id.clone(),
                workspace,
                session_id: None,
                model: None,
                started: Instant::now(),
                cancelled: false,
                context_tokens: None,
            });
            let line = claude_code::build_user_turn(&task.prompt);
            if !self
                .to_io(HarnessReconfig::Child(ChildCommand::Send {
                    line,
                    correlate: None,
                    reply: None,
                }))
                .await
            {
                tracing::error!("harness: io task unreachable, turn recorded but not sent");
            }
            emit::accepted(sink, msg, &task.task_id, call_id).await;
            return;
        }
        let spec = self.child_spec(&start, cwd);
        let started = StartTask {
            task_id: task.task_id.clone(),
            spec,
            // The prompt travels as the first turn, never on the argv (V1/V2).
            first_turn: Some(claude_code::build_user_turn(&task.prompt)),
            startup_timeout: Duration::from_millis(self.params.startup_timeout_ms),
            serve: ServeConfig {
                write_timeout: Duration::from_millis(self.params.external_timeout_ms),
                kill_grace: Duration::from_millis(self.params.kill_grace_ms),
            },
        };
        self.active = Some(ActiveTask {
            task_id: task.task_id.clone(),
            workspace: workspace.clone(),
            session_id: None,
            model: None,
            started: Instant::now(),
            cancelled: false,
            context_tokens: None,
        });
        // A new child for every task (`per_task`), or for a turn the live
        // child cannot take (`persistent`, D57 V9 table). Should one still be alive,
        // it is closed first; the I/O sub-task holds this `Start` until that
        // child's exit, so the order on the channel is the order of events.
        if self.child.is_some() {
            self.close_child(None).await;
        }
        self.child = Some(LiveChild {
            spawned_for: task.task_id.clone(),
            start,
            last_total_cost_usd: None,
            workspace,
            session_id: None,
        });
        if !self.to_io(HarnessReconfig::Start(started)).await {
            // The I/O side is gone; the tombstone stays, and the supervisor
            // will restart the cell. Nothing is silently swallowed.
            tracing::error!("harness: io task unreachable, task recorded but not started");
        }
        emit::accepted(sink, msg, &task.task_id, call_id).await;
    }

    /// Forward a permission decision to the waiting harness.
    async fn answer(
        &mut self,
        answer: AnswerRequest,
        msg: &Message,
        sink: &OutputSink,
        call_id: &str,
    ) {
        let Some(active) = &self.active else {
            emit::error(
                sink,
                msg,
                call_id,
                "invalid_input",
                "no task is running, so there is no question to answer",
            )
            .await;
            return;
        };
        if active.task_id != answer.task_id {
            emit::error(
                sink,
                msg,
                call_id,
                "invalid_input",
                &format!("task {:?} is not the running one", answer.task_id),
            )
            .await;
            return;
        }
        let line =
            claude_code::build_control_response(&answer.request_id, answer.allow, &answer.message);
        let sent = self
            .to_io(HarnessReconfig::Child(ChildCommand::Send {
                line,
                correlate: None,
                reply: None,
            }))
            .await;
        if sent {
            emit::accepted(sink, msg, &answer.task_id, call_id).await;
        } else {
            emit::error(
                sink,
                msg,
                call_id,
                "invalid_input",
                "the harness is no longer reachable",
            )
            .await;
        }
    }

    /// Stop the running task and reap its process group.
    ///
    /// The status is written BEFORE the kill, for the same reason the start is
    /// written before the spawn: whoever looks at the table next must see a
    /// deliberate cancellation, not a mysterious crash.
    async fn cancel(
        &mut self,
        task_id: &str,
        reason: &str,
        msg: &Message,
        sink: &OutputSink,
        db: &mut DbConn,
        call_id: &str,
    ) {
        let Some(active) = &mut self.active else {
            emit::error(
                sink,
                msg,
                call_id,
                "invalid_input",
                &format!("task {task_id:?} is not running"),
            )
            .await;
            return;
        };
        if active.task_id != task_id {
            emit::error(
                sink,
                msg,
                call_id,
                "invalid_input",
                &format!("task {task_id:?} is not the running one"),
            )
            .await;
            return;
        }
        if active.cancelled {
            // Already on its way out; a second Shutdown could only ever reach
            // a child that is not this turn's.
            emit::accepted(sink, msg, task_id, call_id).await;
            return;
        }
        active.cancelled = true;

        let (id, detail, now) = (task_id.to_string(), reason.to_string(), now_rfc3339());
        let _ = db
            .call_with_timeout(move |c| db::finish_task(c, &id, "cancelled", &detail, &now))
            .await;
        // The turn stays in `active` until exactly this child's exit — even if
        // that child has not even been spawned yet because an older one is
        // still closing (OR-57-24).
        self.close_child(Some(task_id.to_string())).await;
        emit::accepted(sink, msg, task_id, call_id).await;
    }

    /// Report the last known state of a task (V1 status: a table lookup, not
    /// live introspection).
    async fn status(
        &mut self,
        task_id: &str,
        msg: &Message,
        sink: &OutputSink,
        db: &mut DbConn,
        call_id: &str,
    ) {
        let id = task_id.to_string();
        let row = db
            .call_with_timeout(move |c| db::load_task(c, &id))
            .await
            .unwrap_or_else(|_| Ok(None));
        match row {
            Ok(Some(row)) => {
                let detail = meclaw_core::serde_json::json!({
                    "task_id": row.task_id, "status": row.status,
                    "workspace": row.workspace, "session_id": row.session_id,
                    "started_at": row.started_at, "finished_at": row.finished_at,
                    "detail": row.detail,
                })
                .to_string();
                emit::accepted_with_text(sink, msg, &row.task_id, call_id, &detail).await;
            }
            Ok(None) => {
                emit::error(
                    sink,
                    msg,
                    call_id,
                    "invalid_input",
                    &format!("no task {task_id:?} was ever started here"),
                )
                .await;
            }
            Err(e) => {
                emit::error(sink, msg, call_id, "query_timeout", &format!("{e}")).await;
            }
        }
    }

    /// Restart recovery: report every interrupted task as unknown, start none.
    async fn recover_after_restart(&mut self, sink: &OriginSink, db: &mut DbConn) {
        let now = now_rfc3339();
        let orphans = db
            .call_with_timeout(move |c| db::mark_running_as_unknown(c, &now))
            .await;
        let Ok(Ok(orphans)) = orphans else {
            tracing::error!("harness: restart recovery could not read the task table");
            return;
        };
        // V12: the open session survives the restart; the next task resumes
        // it. Only the turn that was running is a tombstone.
        if self.params.session == SessionMode::Persistent {
            match db.call_with_timeout(|c| db::load_open_session(c)).await {
                Ok(Ok(row)) => {
                    self.session = row.map(|r| OpenSession {
                        session_id: r.session_id,
                        workspace: r.workspace,
                        cost_usd: r.cost_usd,
                    })
                }
                _ => tracing::error!("harness: restart recovery could not read the session table"),
            }
        }
        for row in orphans {
            emit::result(
                sink,
                self.params.emit_to.clone(),
                &TaskOutcome::unknown_from_row(&row),
            )
            .await;
        }
    }

    /// One frame from a child.
    async fn on_frame(
        &mut self,
        frame: meclaw_core::serde_json::Value,
        sink: &OriginSink,
        db: &mut DbConn,
    ) {
        if !self.closing.is_empty() {
            // The I/O sub-task runs children one after the other, so this
            // frame is the closing child's — a late `result` of a replaced
            // child must never close the next turn (OR-57-31 M4).
            tracing::debug!("harness: frame of a closing child, not booked");
            return;
        }
        // V3: the turn remembers the context of its last top-level call.
        if let Some(tokens) = claude_code::call_context_tokens(&frame)
            && let Some(active) = &mut self.active
        {
            active.context_tokens = Some(tokens);
        }
        match claude_code::classify(&frame) {
            HarnessSignal::Started { session_id, model } => {
                if let Some(child) = &mut self.child {
                    child.session_id = Some(session_id.clone());
                }
                if self.params.session == SessionMode::Persistent {
                    self.open_session(&session_id, sink, db).await;
                }
                let Some(active) = &mut self.active else {
                    return;
                };
                active.session_id = Some(session_id.clone());
                active.model = model;
                let (id, session) = (active.task_id.clone(), session_id.clone());
                let _ = db
                    .call_with_timeout(move |c| db::set_session_id(c, &id, &session))
                    .await;
                let (task_id, session_id) = (
                    self.active
                        .as_ref()
                        .map(|a| a.task_id.clone())
                        .unwrap_or_default(),
                    Some(session_id),
                );
                emit::progress(
                    sink,
                    self.params.emit_to.clone(),
                    &task_id,
                    session_id.as_deref(),
                    "started",
                    None,
                    None,
                )
                .await;
            }
            HarnessSignal::Progress {
                phase,
                tool_name,
                text,
            } => {
                let Some(active) = &self.active else { return };
                emit::progress(
                    sink,
                    self.params.emit_to.clone(),
                    &active.task_id,
                    active.session_id.as_deref(),
                    phase,
                    tool_name.as_deref(),
                    text.as_deref(),
                )
                .await;
            }
            HarnessSignal::Question {
                request_id,
                tool_name,
                input,
            } => self.on_question(request_id, tool_name, input, sink).await,
            HarnessSignal::Finished {
                ok,
                num_turns,
                cost_usd,
                text,
            } => {
                self.on_finished(ok, num_turns, cost_usd, text, sink, db)
                    .await
            }
            HarnessSignal::Ignored => {}
        }
    }

    /// The turn reported its own end: close it here, not at the exit (V7).
    ///
    /// The child does not end on its own any more — its stdin stays open for
    /// the next turn — so `per_task` closes it right after the turn; the exit
    /// that follows is silent.
    async fn on_finished(
        &mut self,
        ok: bool,
        num_turns: Option<u64>,
        total_cost_usd: Option<f64>,
        text: Option<String>,
        sink: &OriginSink,
        db: &mut DbConn,
    ) {
        // A cancelled turn ends at its child's exit; a late result changes
        // nothing.
        if self.active.as_ref().is_none_or(|a| a.cancelled) {
            return;
        }
        let Some(active) = self.active.take() else {
            return;
        };
        let cost_usd = match self.child.as_mut() {
            Some(child) => {
                let turn = turn_cost(total_cost_usd, child.last_total_cost_usd);
                child.last_total_cost_usd = total_cost_usd.or(child.last_total_cost_usd);
                turn
            }
            None => total_cost_usd,
        };
        let (status, error_code) = if ok {
            ("ok", None)
        } else {
            ("error", Some("harness_crashed"))
        };
        let persistent = self.params.session == SessionMode::Persistent;
        let ended = if persistent {
            self.book_turn(ok, &active, cost_usd, sink, db).await
        } else {
            false
        };
        self.finish_turn(
            active,
            status,
            error_code,
            text.unwrap_or_default(),
            num_turns,
            cost_usd,
            true,
            sink,
            db,
        )
        .await;
        if !persistent || !ok || ended {
            // `per_task`: the child has done its one task. `persistent`: a
            // failed turn closes the child, the session stays open (an
            // expired login or a spent process budget fails every later turn
            // on the same process, OR-57-26); a rotated session closes it too.
            self.close_child(None).await;
        } else {
            self.to_io(HarnessReconfig::ArmIdle(Duration::from_millis(
                self.params.session_idle_ms,
            )))
            .await;
        }
    }

    /// `persistent`: book a finished turn on its session and decide whether
    /// the session ends (D57 V7/V9/V10). The `session_closed` emission goes
    /// out here — BEFORE the turn's `result`, which stays the last word of a
    /// task (OR-57-31). Returns whether the session ended.
    async fn book_turn(
        &mut self,
        ok: bool,
        active: &ActiveTask,
        cost_usd: Option<f64>,
        sink: &OriginSink,
        db: &mut DbConn,
    ) -> bool {
        // A lost session (OR-57-25): the child was to resume one, failed, and
        // never said hello — the CLI does not know the session any more.
        let lost = self
            .child
            .as_ref()
            .filter(|c| !ok && c.session_id.is_none())
            .and_then(|c| c.start.resume_session_id.clone());
        if let Some(lost) = lost {
            if self.session.as_ref().is_some_and(|s| s.session_id == lost) {
                self.session = None;
            }
            let (id, now) = (lost.clone(), now_rfc3339());
            let _ = db
                .call_with_timeout(move |c| db::close_session(c, &id, "lost", &now))
                .await;
            self.unannounced
                .push((lost, "lost", active.task_id.clone()));
            self.announce_closed_sessions(sink).await;
            return true;
        }
        let Some(session) = &mut self.session else {
            return false;
        };
        let cost = cost_usd.unwrap_or(0.0);
        session.cost_usd += cost;
        let (id, context) = (session.session_id.clone(), active.context_tokens);
        let _ = db
            .call_with_timeout(move |c| db::record_turn(c, &id, cost, context))
            .await;
        let reason = if self
            .params
            .session_max_context_tokens
            .is_some_and(|max| context.is_some_and(|t| t >= max))
        {
            Some("context")
        } else if self
            .params
            .session_max_usd
            .is_some_and(|max| session.cost_usd >= max)
        {
            Some("cost")
        } else {
            None
        };
        let Some(reason) = reason else {
            return false;
        };
        self.end_session(reason, &active.task_id, db).await;
        self.announce_closed_sessions(sink).await;
        true
    }

    /// `persistent`: which child a task's turn goes to (D57 V9 table). Returns
    /// `true` when the live child takes it; otherwise `start` is what the new
    /// child is started with, and the sessions the table ends are closed.
    async fn decide(
        &mut self,
        task: &TaskRequest,
        workspace: &str,
        start: &mut ChildStart,
        db: &mut DbConn,
    ) -> bool {
        let fits = |c: &LiveChild| c.workspace == workspace && same_start(&c.start, start);
        if task.new_session {
            start.resume_session_id = None;
            self.end_session("new_session", &task.task_id, db).await;
            return false;
        }
        if let Some(wanted) = &task.resume_session_id {
            if self
                .child
                .as_ref()
                .is_some_and(|c| c.session_id.as_ref() == Some(wanted) && fits(c))
            {
                return true;
            }
            if self
                .session
                .as_ref()
                .is_some_and(|s| &s.session_id != wanted)
            {
                self.end_session("replaced", &task.task_id, db).await;
            }
            return false;
        }
        if self.child.as_ref().is_some_and(fits) {
            return true;
        }
        match &self.session {
            // Another model or budget, same tree: a new child continues the
            // session (OR-57-16).
            Some(open) if open.workspace == workspace => {
                start.resume_session_id = Some(open.session_id.clone());
            }
            // A session never carries the context of another tree.
            Some(_) => self.end_session("workspace", &task.task_id, db).await,
            None => {}
        }
        false
    }

    /// `persistent`: a child said hello — its session is the open one now.
    /// The key is the id it reported, not the one that was asked for; any
    /// other open session is replaced (OR-57-31).
    async fn open_session(&mut self, session_id: &str, sink: &OriginSink, db: &mut DbConn) {
        let Some(workspace) = self.child.as_ref().map(|c| c.workspace.clone()) else {
            return;
        };
        if self
            .session
            .as_ref()
            .is_some_and(|s| s.session_id != session_id)
        {
            let task = self
                .active
                .as_ref()
                .map(|a| a.task_id.clone())
                .unwrap_or_default();
            if let Some(old) = self.session.take() {
                // The table row is closed by `open_session` below.
                self.unannounced.push((old.session_id, "replaced", task));
            }
            self.announce_closed_sessions(sink).await;
        }
        let (id, ws, now) = (session_id.to_string(), workspace.clone(), now_rfc3339());
        let cost = match db
            .call_with_timeout(move |c| db::open_session(c, &id, &ws, &now))
            .await
        {
            Ok(Ok(cost)) => cost,
            _ => {
                tracing::error!("harness: the session table could not be written");
                0.0
            }
        };
        self.session = Some(OpenSession {
            session_id: session_id.to_string(),
            workspace,
            cost_usd: cost,
        });
    }

    /// Close the open session, if any, for `reason`; the emission follows.
    async fn end_session(&mut self, reason: &'static str, task_id: &str, db: &mut DbConn) {
        let Some(open) = self.session.take() else {
            return;
        };
        let (id, now) = (open.session_id.clone(), now_rfc3339());
        let _ = db
            .call_with_timeout(move |c| db::close_session(c, &id, reason, &now))
            .await;
        self.unannounced
            .push((open.session_id, reason, task_id.to_string()));
    }

    /// Emit `session_closed` for every closed session not yet announced.
    async fn announce_closed_sessions(&mut self, sink: &OriginSink) {
        for (session_id, reason, task_id) in std::mem::take(&mut self.unannounced) {
            emit::progress(
                sink,
                self.params.emit_to.clone(),
                &task_id,
                Some(&session_id),
                "session_closed",
                None,
                Some(reason),
            )
            .await;
        }
    }

    /// The idle deadline passed (D57 V10): an idle live child closes; the
    /// session stays open and the next task resumes it. Silent.
    async fn on_idle(&mut self) {
        if self.active.is_some() || self.child.is_none() {
            return;
        }
        tracing::info!("harness: idle child closed, the session stays open");
        self.close_child(None).await;
    }

    /// A follow-up turn could not be written to the live child: the turn
    /// ends as `harness_crashed` and the child closes (OR-57-31 M5).
    async fn on_write_failed(&mut self, detail: String, sink: &OriginSink, db: &mut DbConn) {
        if !self.closing.is_empty() || self.child.is_none() {
            // The write belonged to a child that is already on its way out.
            return;
        }
        let Some(active) = self.active.take_if(|a| !a.cancelled) else {
            return;
        };
        self.finish_turn(
            active,
            "crashed",
            Some("harness_crashed"),
            format!("the turn could not be written to the harness: {detail}"),
            None,
            None,
            true,
            sink,
            db,
        )
        .await;
        self.close_child(None).await;
    }

    /// A permission question. With the channel off it is denied immediately —
    /// a harness left waiting for an answer nobody will give is worse than a
    /// refusal, and the refusal is reported either way.
    async fn on_question(
        &mut self,
        request_id: String,
        tool_name: String,
        input: meclaw_core::serde_json::Value,
        sink: &OriginSink,
    ) {
        let Some(active) = &self.active else { return };
        let (task_id, session_id) = (active.task_id.clone(), active.session_id.clone());
        emit::question(
            sink,
            self.params.emit_to.clone(),
            &task_id,
            session_id.as_deref(),
            &request_id,
            &tool_name,
            &input,
        )
        .await;

        if self.params.approval == ApprovalMode::Off {
            let line = claude_code::build_control_response(
                &request_id,
                false,
                "this harness cell runs without an approval channel",
            );
            self.to_io(HarnessReconfig::Child(ChildCommand::Send {
                line,
                correlate: None,
                reply: None,
            }))
            .await;
        }
    }

    /// A child is gone. It is the oldest closing child, or else the live one.
    async fn on_exit(&mut self, exit_detail: String, sink: &OriginSink, db: &mut DbConn) {
        if let Some(closed) = self.closing.pop_front() {
            // Closed on purpose. Silent — unless a cancel bound a turn to
            // exactly this exit.
            let Some(turn) = closed.turn else { return };
            let Some(active) = self.active.take_if(|a| a.task_id == turn) else {
                return;
            };
            self.finish_turn(
                active,
                "cancelled",
                Some("cancelled"),
                format!("cancelled by the topology ({exit_detail})"),
                None,
                None,
                // A cancel already wrote its tombstone; do not overwrite it.
                false,
                sink,
                db,
            )
            .await;
            return;
        }
        if self.child.take().is_none() {
            // A child died that we were not tracking, or the exit arrived twice.
            return;
        }
        // The live child died. A turn still running never got its result.
        let Some(active) = self.active.take() else {
            return;
        };
        self.finish_turn(
            active,
            "crashed",
            Some("harness_crashed"),
            format!("the harness ended without reporting a result ({exit_detail})"),
            None,
            None,
            true,
            sink,
            db,
        )
        .await;
    }

    /// A child never got going (spawn, first write, startup). It ends without
    /// an `Exited`, so it leaves the books here.
    async fn on_task_failed(
        &mut self,
        task_id: &str,
        error_code: &'static str,
        detail: String,
        sink: &OriginSink,
        db: &mut DbConn,
    ) {
        if self
            .child
            .as_ref()
            .is_some_and(|c| c.spawned_for == task_id)
        {
            self.child = None;
        } else if let Some(i) = self.closing.iter().rposition(|c| c.spawned_for == task_id) {
            self.closing.remove(i);
        }
        let Some(active) = self.active.take_if(|a| a.task_id == task_id) else {
            return;
        };
        if active.cancelled {
            // The cancel already wrote its tombstone and was promised a
            // `cancelled` outcome; the failed start does not change that.
            self.finish_turn(
                active,
                "cancelled",
                Some("cancelled"),
                format!("cancelled by the topology ({detail})"),
                None,
                None,
                false,
                sink,
                db,
            )
            .await;
            return;
        }
        self.finish_turn(
            active,
            "error",
            Some(error_code),
            detail,
            None,
            None,
            true,
            sink,
            db,
        )
        .await;
    }

    /// Send the live child a `Shutdown` and book it as closing. `turn` names
    /// the turn that ends with its exit (a cancel), if any.
    async fn close_child(&mut self, turn: Option<String>) {
        let Some(child) = self.child.take() else {
            tracing::error!("harness: no live child to close");
            return;
        };
        self.closing.push_back(ClosingChild {
            spawned_for: child.spawned_for,
            turn,
        });
        self.to_io(HarnessReconfig::Child(ChildCommand::Shutdown))
            .await;
    }

    /// Close a turn: tombstone (unless already written), then the `result`
    /// emission — always the last word of a task.
    #[allow(clippy::too_many_arguments)]
    async fn finish_turn(
        &mut self,
        active: ActiveTask,
        status: &'static str,
        error_code: Option<&'static str>,
        text: String,
        num_turns: Option<u64>,
        cost_usd: Option<f64>,
        write_tombstone: bool,
        sink: &OriginSink,
        db: &mut DbConn,
    ) {
        let duration_ms = active.started.elapsed().as_millis() as u64;
        if write_tombstone {
            let (id, detail, now) = (active.task_id.clone(), text.clone(), now_rfc3339());
            let _ = db
                .call_with_timeout(move |c| db::finish_task(c, &id, status, &detail, &now))
                .await;
        }
        emit::result(
            sink,
            self.params.emit_to.clone(),
            &TaskOutcome {
                task_id: active.task_id,
                session_id: active.session_id,
                status,
                workspace: active.workspace,
                duration_ms,
                num_turns,
                cost_usd,
                model: active.model,
                error_code,
                text,
            },
        )
        .await;
    }
}

/// What one turn cost, from the process-wide counter (OR-57-7): the
/// difference to the last turn's total; a total below it means a new counter,
/// and the total is then the turn's cost.
fn turn_cost(total: Option<f64>, last: Option<f64>) -> Option<f64> {
    match (total, last) {
        (Some(t), Some(l)) if t >= l => Some(t - l),
        (t, _) => t,
    }
}

/// Second-precision UTC timestamp, the format the other cell types persist.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::turn_cost;

    /// OR-57-7: a turn costs its share of the process-wide counter; a total
    /// below the last one is a fresh counter (a new process), and then the
    /// total itself is the turn's cost — never a negative difference.
    #[test]
    fn a_falling_total_is_a_fresh_counter() {
        assert_eq!(turn_cost(Some(0.75), Some(0.25)), Some(0.5));
        assert_eq!(turn_cost(Some(0.1), Some(0.25)), Some(0.1));
        assert_eq!(turn_cost(Some(0.1), None), Some(0.1));
        assert_eq!(turn_cost(None, Some(0.25)), None);
    }
}
