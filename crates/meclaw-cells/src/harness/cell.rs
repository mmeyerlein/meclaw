//! `HarnessCell` — the handler side of the cell type.
//!
//! It owns three things: the task table in `cell.db`, the identity of the one
//! task that may be running, and the translation of the child's frames into
//! emissions. It owns no pipe and no process; those live in the I/O sub-task.
//!
//! The one behaviour to keep in mind when reading this file is what happens on
//! `Exited`. In `mcp` that event is a panic — a dead child means the cell can
//! no longer answer anything. Here it is the ORDINARY end of a task, and
//! panicking would restart a cell that is perfectly healthy while losing the
//! outcome of the run. See `mcp/cell.rs` for the other side of that decision.

use crate::harness::claude_code::{self, HarnessSignal};
use crate::harness::db;
use crate::harness::emit::{self, TaskOutcome};
use crate::harness::io::{HarnessEvent, HarnessIo, HarnessReconfig, StartTask};
use crate::harness::params::{ApprovalMode, HarnessOverlay, HarnessParams};
use crate::harness::parse::parse_tool_call;
use crate::harness::task::{AnswerRequest, HarnessCommand, TaskRequest};
use crate::stdio_child::{ChildCommand, ChildEvent, ChildSpec, ServeConfig};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Message, OriginSink, OutputSink};
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The task currently being run, as far as the handler knows.
#[derive(Debug)]
struct ActiveTask {
    task_id: String,
    /// Absolute workspace path — our record, used in every emission.
    workspace: String,
    session_id: Option<String>,
    model: Option<String>,
    started: Instant,
    /// Set once a result frame arrived: what the harness said about itself.
    reported: Option<ReportedOutcome>,
    /// Set by a cancel, so the exit is classified as deliberate.
    cancelled: bool,
}

/// What the harness reported about its own run.
#[derive(Debug, Clone)]
struct ReportedOutcome {
    ok: bool,
    num_turns: Option<u64>,
    cost_usd: Option<f64>,
    text: String,
}

/// The `harness` cell. Single-threaded state in the handler sub-task.
pub struct HarnessCell {
    params: HarnessParams,
    initial_io: Option<HarnessIo>,
    active: Option<ActiveTask>,
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

    /// Build the child spec for one task: the adapter's argv plus the whole
    /// containment configuration.
    fn child_spec(&self, task: &TaskRequest, cwd: std::path::PathBuf) -> ChildSpec {
        let mut env = self.params.env.clone();
        for key in &self.params.env_passthrough {
            if let Ok(value) = std::env::var(key) {
                env.push((key.clone(), value));
            }
        }
        ChildSpec {
            program: self.params.command.clone(),
            args: claude_code::build_argv(&self.params, task),
            env,
            cwd: Some(cwd),
            kill_grace_ms: self.params.kill_grace_ms,
            // A harness spawns process trees of its own; without a group its
            // build and search tools outlive the cell.
            process_group: true,
            // Containment: the child sees the passthrough list and nothing
            // else of this process's environment.
            env_clear: true,
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

    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            match event {
                HarnessEvent::Booted => self.recover_after_restart(sink, db).await,
                HarnessEvent::TaskFailed {
                    task_id,
                    error_code,
                    detail,
                } => {
                    self.close_task(&task_id, "error", Some(error_code), detail, sink, db)
                        .await
                }
                HarnessEvent::Child(ChildEvent::Frame(v)) => self.on_frame(v, sink, db).await,
                HarnessEvent::Child(ChildEvent::Malformed { raw }) => {
                    // Harnesses print banners. Not an error, not worth an
                    // emission — the operator can read the child's stderr.
                    tracing::debug!(line = %raw, "harness: non-json line from child");
                }
                // The ordinary end of a task. NOT a panic: unlike `mcp`, this
                // cell survives its child by design — it exists to run the
                // next task, and panicking would also lose this one's outcome.
                HarnessEvent::Child(ChildEvent::Exited(exit)) => {
                    self.on_exit(exit.detail(), sink, db).await
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

        let current = HarnessOverlay {
            model: self.params.model.clone(),
            max_turns: self.params.max_turns,
            max_budget_usd: self.params.max_budget_usd,
            startup_timeout_ms: self.params.startup_timeout_ms,
            external_timeout_ms: self.params.external_timeout_ms,
            query_timeout_ms: self.params.query_timeout_ms,
        };
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

        self.params.model = new_overlay.model;
        self.params.max_turns = new_overlay.max_turns;
        self.params.max_budget_usd = new_overlay.max_budget_usd;
        self.params.startup_timeout_ms = new_overlay.startup_timeout_ms;
        self.params.external_timeout_ms = new_overlay.external_timeout_ms;
        self.params.query_timeout_ms = new_overlay.query_timeout_ms;
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

        let spec = self.child_spec(&task, cwd);
        let started = StartTask {
            task_id: task.task_id.clone(),
            spec,
            startup_timeout: Duration::from_millis(self.params.startup_timeout_ms),
            serve: ServeConfig {
                write_timeout: Duration::from_millis(self.params.external_timeout_ms),
                kill_grace: Duration::from_millis(self.params.kill_grace_ms),
            },
        };
        self.active = Some(ActiveTask {
            task_id: task.task_id.clone(),
            workspace,
            session_id: None,
            model: None,
            started: Instant::now(),
            reported: None,
            cancelled: false,
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
        active.cancelled = true;

        let (id, detail, now) = (task_id.to_string(), reason.to_string(), now_rfc3339());
        let _ = db
            .call_with_timeout(move |c| db::finish_task(c, &id, "cancelled", &detail, &now))
            .await;
        self.to_io(HarnessReconfig::Child(ChildCommand::Shutdown))
            .await;
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
        for row in orphans {
            emit::result(
                sink,
                self.params.emit_to.clone(),
                &TaskOutcome::unknown_from_row(&row),
            )
            .await;
        }
    }

    /// One frame from the running child.
    async fn on_frame(
        &mut self,
        frame: meclaw_core::serde_json::Value,
        sink: &OriginSink,
        db: &mut DbConn,
    ) {
        match claude_code::classify(&frame) {
            HarnessSignal::Started { session_id, model } => {
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
                if let Some(active) = &mut self.active {
                    active.reported = Some(ReportedOutcome {
                        ok,
                        num_turns,
                        cost_usd,
                        text: text.unwrap_or_default(),
                    });
                }
            }
            HarnessSignal::Ignored => {}
        }
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

    /// The child is gone: decide what that means and close the task out.
    async fn on_exit(&mut self, exit_detail: String, sink: &OriginSink, db: &mut DbConn) {
        let Some(active) = self.active.take() else {
            // A child died that we were not tracking — a cancel already closed
            // the books, or the exit arrived twice.
            return;
        };
        let duration_ms = active.started.elapsed().as_millis() as u64;
        let (status, error_code, text) = match (&active.reported, active.cancelled) {
            (_, true) => (
                "cancelled",
                Some("cancelled"),
                format!("cancelled by the topology ({exit_detail})"),
            ),
            (Some(r), false) if r.ok => ("ok", None, r.text.clone()),
            (Some(r), false) => ("error", Some("harness_crashed"), r.text.clone()),
            (None, false) => (
                "crashed",
                Some("harness_crashed"),
                format!("the harness ended without reporting a result ({exit_detail})"),
            ),
        };

        // A cancel already wrote its tombstone; do not overwrite it.
        if !active.cancelled {
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
                num_turns: active.reported.as_ref().and_then(|r| r.num_turns),
                cost_usd: active.reported.as_ref().and_then(|r| r.cost_usd),
                model: active.model,
                error_code,
                text,
            },
        )
        .await;
    }

    /// Close a task that never got a child, from an I/O-side failure.
    async fn close_task(
        &mut self,
        task_id: &str,
        status: &'static str,
        error_code: Option<&'static str>,
        detail: String,
        sink: &OriginSink,
        db: &mut DbConn,
    ) {
        let active = self.active.take();
        let workspace = active
            .as_ref()
            .filter(|a| a.task_id == task_id)
            .map(|a| a.workspace.clone())
            .unwrap_or_default();
        let duration_ms = active
            .as_ref()
            .map(|a| a.started.elapsed().as_millis() as u64)
            .unwrap_or(0);

        let (id, text, now) = (task_id.to_string(), detail.clone(), now_rfc3339());
        let _ = db
            .call_with_timeout(move |c| db::finish_task(c, &id, status, &text, &now))
            .await;

        emit::result(
            sink,
            self.params.emit_to.clone(),
            &TaskOutcome {
                task_id: task_id.to_string(),
                session_id: active.and_then(|a| a.session_id),
                status,
                workspace,
                duration_ms,
                num_turns: None,
                cost_usd: None,
                model: None,
                error_code,
                text: detail,
            },
        )
        .await;
    }
}

/// Second-precision UTC timestamp, the format the other cell types persist.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
