//! `TimerCell` — the `LongRunningCell` implementation of the `timer` cell type
//! (double task: handler + I/O). The handler is the DB authority: it parses
//! schedule ops (`add`/`modify`/`remove`) off the mailbox, persists them into
//! `cell.db` and sends the I/O task a fresh active snapshot via `reconfig_tx`.
//! The I/O task computes `sleep_until` on the working copy and emits the firings.
//! Spec: `docs/cell-types.md` § `timer`.

use crate::timer::io::{TimerEvent, TimerReconfig, TimerReplan};
use crate::timer::schedule::ActiveSchedule;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Message, OriginSink, OutputSink, Path};
use std::future::Future;
use tokio::sync::mpsc;

/// The `timer` cell. State lives single-threaded in the handler sub-task of
/// `cell_task_long_running` (no mutex — phase-1 discipline). `initial_io` is
/// pulled out once by `split_io` and handed to the I/O task.
pub struct TimerCell {
    /// The cell's own routing path — disambiguates the `handle_event` skip logs
    /// when several `timer` cells run in one colony.
    pub(crate) own_path: Path,
    /// The initial I/O set, consumed exactly once by `split_io`.
    pub(crate) initial_io: Option<Vec<ActiveSchedule>>,
    /// β: live effective `query_timeout_ms` (the timer's only overlay field).
    /// A runtime `params` update merges over it and applies it to the `DbConn`
    /// live (path C); the next cell.db op runs under the new A timeout.
    pub(crate) query_timeout_ms: u64,
    /// GH #922: the instant this incarnation's plan was loaded against. A
    /// `catch_up` one-shot due at or before it was missed while the cell was
    /// down, and its strike is `late` ([`is_late`]).
    pub(crate) booted_at: chrono::DateTime<chrono::Utc>,
    /// GH #956: the handler's way to plan a strike again whose state step
    /// failed ([`TimerReplan`]). Opened by `split_io`; `None` before it (a
    /// cell whose I/O never ran has nothing to plan again).
    pub(crate) replan_tx: Option<mpsc::Sender<TimerReplan>>,
}

impl TimerCell {
    /// Constructor. `initial_active` comes from the factory (sync load from
    /// `cell.db`, past one-shots filtered out). `query_timeout_ms` is the
    /// effective A timeout (birth ⊕ cell.db overlay).
    pub fn new(own_path: Path, initial_active: Vec<ActiveSchedule>, query_timeout_ms: u64) -> Self {
        Self {
            own_path,
            initial_io: Some(initial_active),
            query_timeout_ms,
            booted_at: chrono::Utc::now(),
            replan_tx: None,
        }
    }

    /// GH #922: pin the boot instant to the one the factory loaded the plan
    /// with, so "missed" means the same thing for the plan and the strike.
    pub fn with_booted_at(mut self, booted_at: chrono::DateTime<chrono::Utc>) -> Self {
        self.booted_at = booted_at;
        self
    }
}

/// I/O-local state struct. Single owner (held by-value by the I/O sub-task).
/// No mutex, no Arc.
pub struct TimerIo {
    /// Working copy of the active schedules (cron + future-at + the missed
    /// `catch_up` one-shots, GH #922).
    pub active: Vec<ActiveSchedule>,
    /// Issue #7: progress mark, set after every schedule this loop actually
    /// fired. Default = disabled (reports nowhere).
    pub liveness: meclaw_colony::IoLivenessMark,
    /// GH #956: the handler's replans ([`TimerReplan`]). `None` = a loop that
    /// only plans from `active` and the reconfig channel, as before.
    pub replan_rx: Option<mpsc::Receiver<TimerReplan>>,
}

impl LongRunningCell for TimerCell {
    type Event = TimerEvent;
    type Reconfig = TimerReconfig;
    type Io = TimerIo;

    fn split_io(&mut self) -> Self::Io {
        // GH #956: the replan channel opens with the I/O task it feeds.
        let (replan_tx, replan_rx) = mpsc::channel(crate::timer::io::REPLAN_CAPACITY);
        self.replan_tx = Some(replan_tx);
        TimerIo {
            active: self.initial_io.take().unwrap_or_default(),
            // Replaced by the substrate via `attach_liveness` when the cell is
            // spawned inside a colony.
            liveness: meclaw_colony::IoLivenessMark::disabled(),
            replan_rx: Some(replan_rx),
        }
    }

    /// Issue #7: a timer's round trip is its own clock — the mark says when this
    /// loop last delivered a due tick. Read it against the cell's cadence: a
    /// daily cron is legitimately quiet for a day.
    fn attach_liveness(io: &mut Self::Io, mark: meclaw_colony::IoLivenessMark) {
        io.liveness = mark;
    }

    /// I/O sub-task — delegates to `crate::timer::io::run_io` (T8 correction B).
    ///
    /// `+ Send` is load-bearing (AFIT does not bind Send; `tokio::spawn` in
    /// `cell_task_long_running` needs it). `clippy::manual_async_fn` is a
    /// stable-1.95 false positive — see the pattern in
    /// `crates/meclaw-colony/src/long_running_cell.rs:96-110`.
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        crate::timer::io::run_io(io, events_tx, reconfig_rx)
    }

    /// Handler for mailbox messages. Parses the op (T12) and dispatches into
    /// `add`/`modify`/`remove`. On parse/op errors: an error reply via
    /// `OutputSink` to `msg.reply_to` (fallback `msg.target` (W2d: its own path,
    /// not the READ endpoint)). On success: a fresh active snapshot to the I/O
    /// task via `reconfig_tx` (T13: `add`; T14: `modify`/`remove`).
    ///
    /// Correction A: parse errors with the prefix `"cron:"` map to
    /// `invalid_cron`; everything else to a generic `parse_error`.
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
        reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let body_val = match msg.body {
                meclaw_core::Body::Inline(ref v) => v.clone(),
                _ => {
                    crate::timer::emit::emit_op_error(
                        sink,
                        &msg,
                        "invalid_body",
                        "expected inline json",
                        None,
                    )
                    .await;
                    return;
                }
            };
            // β: params-update slot (config.md § Access l.20). The timer's only
            // overlay field is `query_timeout_ms` (path C, immediately live);
            // schedules change via the ops below, not here. A params update is
            // standalone:
            // apply + persist + live-set, then return silently (no op follows).
            if let Some(params_val) = body_val.get("params") {
                let update_obj = match params_val.as_object() {
                    Some(o) => o.clone(),
                    None => {
                        crate::timer::emit::emit_op_error(
                            sink,
                            &msg,
                            "invalid_input",
                            "params slot: not a JSON object",
                            None,
                        )
                        .await;
                        return;
                    }
                };
                let current = crate::timer::params::TimerOverlay {
                    query_timeout_ms: self.query_timeout_ms,
                };
                match crate::params_overlay::apply_update(&current, &update_obj) {
                    Ok((new_ov, overlay)) => {
                        let now = crate::params_overlay::now_unix_seconds();
                        let persist = db
                            .call_with_timeout(move |c| {
                                crate::params_overlay::persist_params_overlay(c, &overlay, now)
                            })
                            .await;
                        match persist {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                crate::timer::emit::emit_op_error(
                                    sink,
                                    &msg,
                                    "invalid_input",
                                    &format!("cell.db params write failed: {e}"),
                                    None,
                                )
                                .await;
                                return;
                            }
                            Err(meclaw_colony::QueryTimeout::Interrupted) => {
                                crate::timer::emit::emit_op_error(
                                    sink,
                                    &msg,
                                    "query_timeout",
                                    "params write exceeded query_timeout_ms",
                                    None,
                                )
                                .await;
                                return;
                            }
                        }
                        // Live apply (path C, immediately live).
                        self.query_timeout_ms = new_ov.query_timeout_ms;
                        db.set_query_timeout(Some(std::time::Duration::from_millis(
                            self.query_timeout_ms,
                        )));
                    }
                    Err(e) => {
                        crate::timer::emit::emit_op_error(
                            sink,
                            &msg,
                            "invalid_input",
                            &e.detail(),
                            None,
                        )
                        .await;
                    }
                }
                // Standalone params-update → done (no schedule op in this message).
                return;
            }

            // GH #81: an op may arrive the way every other tool cell is driven
            // — structured JSON args in a `tool_call` turn — or at the body's
            // own top level (config-born ops, the #17 HTTP form). The turn also
            // carries the id every answer below has to echo; on the raw-body
            // path there is none, and that `None` is what keeps that path
            // exactly as silent as it was.
            let (op_val, tool_call_id) = match crate::timer::op::resolve_op_source(&msg, &body_val)
            {
                Ok(src) => (src.value, src.tool_call_id),
                Err(e) => {
                    crate::timer::emit::emit_op_error(sink, &msg, "parse_error", &e, None).await;
                    return;
                }
            };

            // GH #231: ONE clock read decides this whole op. The guard below
            // and the snapshot at the end of every successful branch share it,
            // so a one-shot the guard accepted as "still ahead" cannot be
            // dropped as "already past" by the re-read a few microseconds
            // later. That gap between the accepting check and the planning
            // read is the window this issue was reported from.
            let op_now = chrono::Utc::now();

            // GH #904 (PP-7, OR-KX.K.23): every error answer to an op that named a
            // `schedule_id` names it back, as the caller sent it -- a refused
            // order (`at_in_past`, `schedule_id_exists`, ...) is then the
            // caller's to match, not a bare code. Taken before the parse, so a
            // parse error of an op with an id carries it too.
            let op_schedule_id = op_val
                .get("schedule_id")
                .and_then(|x| x.as_str())
                .map(str::to_string);
            let op = match crate::timer::op::TimerOp::parse(&op_val) {
                Ok(o) => o,
                Err(e) => {
                    let code = if e.starts_with("cron:") {
                        "invalid_cron"
                    } else if e.starts_with(crate::timer::op::CATCH_UP_CRON_UNSUPPORTED) {
                        // GH #922: a flag the row cannot carry, not a parse error.
                        "invalid_params"
                    } else {
                        "parse_error"
                    };
                    crate::timer::emit::emit_op_error_for(
                        sink,
                        &msg,
                        code,
                        &e,
                        tool_call_id.as_deref(),
                        op_schedule_id.as_deref(),
                    )
                    .await;
                    return;
                }
            };
            match op {
                crate::timer::op::TimerOp::Add { row, rearm } => {
                    // GH #231: a one-shot whose `at` has passed by the time the
                    // op is processed is refused here, before the INSERT. It is
                    // never planned (`load_active_filter_past` and
                    // `compute_next_occurrence` both drop it), so accepting it
                    // would store exactly the thing the spec's `add` validation
                    // exists to prevent — a schedule that sits there and never
                    // fires. The op that ran out of lead time in flight now
                    // says so instead of being swallowed.
                    if let crate::timer::schedule::ScheduleKind::At(at) = &row.kind
                        && let Some(detail) = past_at_detail("add", *at, op_now)
                    {
                        crate::timer::emit::emit_op_error_for(
                            sink,
                            &msg,
                            "at_in_past",
                            &detail,
                            tool_call_id.as_deref(),
                            op_schedule_id.as_deref(),
                        )
                        .await;
                        return;
                    }
                    // GH #690: a repeated order is one order. An `add` that
                    // names an active row with the same moment changes
                    // nothing and is acknowledged; one that names a removed
                    // row revives it; only a different order under the same
                    // id is `schedule_id_exists`. GH #904: with `rearm` the
                    // standing row is replaced in any status (`Rearmed`,
                    // answered like a revival: a snapshot, no new emission).
                    let row_for_call = row.clone();
                    let added = db
                        .call_with_timeout(move |c| {
                            crate::timer::db::add_schedule(c, &row_for_call, rearm)
                                .map_err(|e| format!("{e}"))
                        })
                        .await;
                    let schedule_id = row.schedule_id;
                    let added = match added {
                        Ok(Ok(crate::timer::db::AddOutcome::Exists)) => Ok(Err(format!(
                            "schedule_id {schedule_id} already names another order"
                        ))),
                        other => other,
                    };
                    match added {
                        Ok(Ok(outcome)) => {
                            if outcome != crate::timer::db::AddOutcome::Same {
                                send_setactive_snapshot(db, reconfig_tx, op_now).await;
                            }
                            // GH #81: the answer a tool loop waits for. Only when
                            // the op arrived as a `tool_call` -- the raw-body path
                            // stays unacked, as it always was.
                            if let Some(tcid) = tool_call_id.as_deref() {
                                crate::timer::emit::emit_op_ack(
                                    sink,
                                    &msg,
                                    "add",
                                    schedule_id,
                                    tcid,
                                )
                                .await;
                            }
                        }
                        Ok(Err(e)) => {
                            crate::timer::emit::emit_op_error_for(
                                sink,
                                &msg,
                                "schedule_id_exists",
                                &format!("add: {e}"),
                                tool_call_id.as_deref(),
                                op_schedule_id.as_deref(),
                            )
                            .await
                        }
                        Err(meclaw_colony::QueryTimeout::Interrupted) => {
                            crate::timer::emit::emit_op_error_for(
                                sink,
                                &msg,
                                "query_timeout",
                                "add: query exceeded query_timeout_ms",
                                tool_call_id.as_deref(),
                                op_schedule_id.as_deref(),
                            )
                            .await
                        }
                    }
                }
                crate::timer::op::TimerOp::Modify {
                    schedule_id,
                    new_name,
                    new_cron,
                    new_at,
                    new_emit_to,
                    new_catch_up,
                } => {
                    // Type-mismatch guard: a cron update on an at row or an at
                    // update on a cron row is rejected (spec: modify does not
                    // switch the type).
                    let current = match db
                        .call_with_timeout(move |c| crate::timer::db::load_schedule(c, schedule_id))
                        .await
                    {
                        Ok(r) => r.unwrap_or(None),
                        Err(meclaw_colony::QueryTimeout::Interrupted) => {
                            crate::timer::emit::emit_op_error_for(
                                sink,
                                &msg,
                                "query_timeout",
                                "modify: load exceeded query_timeout_ms",
                                tool_call_id.as_deref(),
                                op_schedule_id.as_deref(),
                            )
                            .await;
                            return;
                        }
                    };
                    let Some(cur) = current else {
                        crate::timer::emit::emit_op_error_for(
                            sink,
                            &msg,
                            "schedule_not_found",
                            &format!("modify: id {schedule_id} unknown"),
                            tool_call_id.as_deref(),
                            op_schedule_id.as_deref(),
                        )
                        .await;
                        return;
                    };
                    let mismatch =
                        (matches!(cur.kind, crate::timer::schedule::ScheduleKind::Cron(_))
                            && new_at.is_some())
                            || (matches!(cur.kind, crate::timer::schedule::ScheduleKind::At(_))
                                && new_cron.is_some());
                    if mismatch {
                        crate::timer::emit::emit_op_error_for(
                            sink,
                            &msg,
                            "kind_mismatch",
                            "modify: cannot switch cron<->at (use remove+add)",
                            tool_call_id.as_deref(),
                            op_schedule_id.as_deref(),
                        )
                        .await;
                        return;
                    }
                    // GH #922: `catch_up` is for one-shots; a cron row refuses
                    // it the way `add` does.
                    if new_catch_up == Some(true)
                        && matches!(cur.kind, crate::timer::schedule::ScheduleKind::Cron(_))
                    {
                        crate::timer::emit::emit_op_error_for(
                            sink,
                            &msg,
                            "invalid_params",
                            &format!(
                                "{}: modify: `catch_up` applies to one-shot `at` rows only",
                                crate::timer::op::CATCH_UP_CRON_UNSUPPORTED
                            ),
                            tool_call_id.as_deref(),
                            op_schedule_id.as_deref(),
                        )
                        .await;
                        return;
                    }
                    // GH #231: the same refusal on the modify lane — moving a
                    // one-shot to a time that has already passed would leave an
                    // active row nothing will ever plan.
                    if let Some(at) = new_at
                        && let Some(detail) = past_at_detail("modify", at, op_now)
                    {
                        crate::timer::emit::emit_op_error_for(
                            sink,
                            &msg,
                            "at_in_past",
                            &detail,
                            tool_call_id.as_deref(),
                            op_schedule_id.as_deref(),
                        )
                        .await;
                        return;
                    }
                    let n = match db
                        .call_with_timeout(move |c| -> rusqlite::Result<usize> {
                            let n = crate::timer::db::modify_schedule_fields(
                                c,
                                schedule_id,
                                new_cron.as_deref(),
                                new_name.as_deref(),
                                new_emit_to.as_deref(),
                                new_at,
                            )?;
                            // GH #922: the flag rides in the same call, on the
                            // row the UPDATE above just found.
                            if n == 1
                                && let Some(flag) = new_catch_up
                            {
                                crate::timer::db::set_catch_up(c, schedule_id, flag)?;
                            }
                            Ok(n)
                        })
                        .await
                    {
                        Ok(r) => r.unwrap_or(0),
                        Err(meclaw_colony::QueryTimeout::Interrupted) => {
                            crate::timer::emit::emit_op_error_for(
                                sink,
                                &msg,
                                "query_timeout",
                                "modify: update exceeded query_timeout_ms",
                                tool_call_id.as_deref(),
                                op_schedule_id.as_deref(),
                            )
                            .await;
                            return;
                        }
                    };
                    if n == 0 {
                        crate::timer::emit::emit_op_error_for(
                            sink,
                            &msg,
                            "schedule_not_found",
                            "modify: 0 rows updated",
                            tool_call_id.as_deref(),
                            op_schedule_id.as_deref(),
                        )
                        .await;
                    } else {
                        send_setactive_snapshot(db, reconfig_tx, op_now).await;
                        // GH #81: the answer a tool loop waits for. Only when
                        // the op arrived as a `tool_call` -- the raw-body path
                        // stays unacked, as it always was.
                        if let Some(tcid) = tool_call_id.as_deref() {
                            crate::timer::emit::emit_op_ack(
                                sink,
                                &msg,
                                "modify",
                                schedule_id,
                                tcid,
                            )
                            .await;
                        }
                    }
                }
                crate::timer::op::TimerOp::Trigger { schedule_id } => {
                    // GH #17: fire an EXISTING schedule once, now. The handler
                    // does the two checks it owns -- the row exists, the row is
                    // active -- and then hands the firing over. It emits nothing
                    // itself and writes nothing: `handle_event` does the
                    // state-before-emit and the OriginSink emit, which is what
                    // makes the run indistinguishable from a cron-fired one.
                    let row = match db
                        .call_with_timeout(move |c| crate::timer::db::load_schedule(c, schedule_id))
                        .await
                    {
                        Ok(r) => r.unwrap_or(None),
                        Err(meclaw_colony::QueryTimeout::Interrupted) => {
                            crate::timer::emit::emit_op_error_for(
                                sink,
                                &msg,
                                "query_timeout",
                                "trigger: load exceeded query_timeout_ms",
                                tool_call_id.as_deref(),
                                op_schedule_id.as_deref(),
                            )
                            .await;
                            return;
                        }
                    };
                    let Some(row) = row else {
                        crate::timer::emit::emit_op_error_for(
                            sink,
                            &msg,
                            "schedule_not_found",
                            &format!("trigger: id {schedule_id} unknown"),
                            tool_call_id.as_deref(),
                            op_schedule_id.as_deref(),
                        )
                        .await;
                        return;
                    };
                    // A removed or completed schedule is not a firing target.
                    // Refused here rather than left to `handle_event`'s race
                    // check: that check skips silently, and a trigger that
                    // silently does nothing is the failure mode #17 was about.
                    if row.status != "active" {
                        crate::timer::emit::emit_op_error_for(
                            sink,
                            &msg,
                            "schedule_not_found",
                            &format!("trigger: id {schedule_id} is {}, not active", row.status),
                            tool_call_id.as_deref(),
                            op_schedule_id.as_deref(),
                        )
                        .await;
                        return;
                    }
                    let _ = reconfig_tx
                        .send(crate::timer::io::TimerReconfig::FireNow { schedule_id })
                        .await;
                    // GH #81: the answer a tool loop waits for. Only when
                    // the op arrived as a `tool_call` -- the raw-body path
                    // stays unacked, as it always was.
                    if let Some(tcid) = tool_call_id.as_deref() {
                        crate::timer::emit::emit_op_ack(sink, &msg, "trigger", schedule_id, tcid)
                            .await;
                    }
                }
                crate::timer::op::TimerOp::Remove { schedule_id } => {
                    let n = match db
                        .call_with_timeout(move |c| crate::timer::db::mark_removed(c, schedule_id))
                        .await
                    {
                        Ok(r) => r.unwrap_or(0),
                        Err(meclaw_colony::QueryTimeout::Interrupted) => {
                            crate::timer::emit::emit_op_error_for(
                                sink,
                                &msg,
                                "query_timeout",
                                "remove: query exceeded query_timeout_ms",
                                tool_call_id.as_deref(),
                                op_schedule_id.as_deref(),
                            )
                            .await;
                            return;
                        }
                    };
                    if n == 0 {
                        crate::timer::emit::emit_op_error_for(
                            sink,
                            &msg,
                            "schedule_not_found",
                            "remove: 0 rows updated",
                            tool_call_id.as_deref(),
                            op_schedule_id.as_deref(),
                        )
                        .await;
                    } else {
                        send_setactive_snapshot(db, reconfig_tx, op_now).await;
                        // GH #81: the answer a tool loop waits for. Only when
                        // the op arrived as a `tool_call` -- the raw-body path
                        // stays unacked, as it always was.
                        if let Some(tcid) = tool_call_id.as_deref() {
                            crate::timer::emit::emit_op_ack(
                                sink,
                                &msg,
                                "remove",
                                schedule_id,
                                tcid,
                            )
                            .await;
                        }
                    }
                }
            }
        }
    }

    /// Handler for I/O events — T11: race check + state-before-emit (phase-5
    /// canon). Persist BEFORE emitting. The emit follows in T15.
    ///
    /// Sequence:
    /// 1. `load_schedule` — fetch the row. None or `status != "active"`: skip
    ///    (race: a remove/complete op happened between the I/O fire push and
    ///    handle_event).
    ///
    ///    1b. `strike_is_stale` — a strike that is not a moment of the current
    ///    order is skipped: no state step, no emission (GH #904, GH #913). An
    ///    operator trigger fires the order as it stands.
    /// 2. Persist: repeating → `bump_iteration`; one-shot → `mark_completed`.
    /// 3. Emit (T15).
    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let TimerEvent::Fire {
                schedule_id,
                scheduled_at,
                forced,
            } = event;

            // 1. SELECT — race check. Under query_timeout (path C): on Interrupt,
            // skip this fire (no emit) — a timed-out load means the cell.db is
            // overloaded; better to drop the tick than block.
            let row = match db
                .call_with_timeout(move |c| crate::timer::db::load_schedule(c, schedule_id))
                .await
            {
                Ok(r) => r.expect("load_schedule"),
                Err(meclaw_colony::QueryTimeout::Interrupted) => {
                    tracing::debug!(
                        path = self.own_path.as_str(),
                        ?schedule_id,
                        "fire: load_schedule timed out (query_timeout_ms), skip"
                    );
                    // GH #956: the I/O task dropped a one-shot from its
                    // working copy when it pushed this strike; skipped here,
                    // the row stayed `active` and planned nowhere. The I/O
                    // task ignores the frame for a cron row (its next tick is
                    // planned; this one is dropped, as before).
                    if !forced {
                        self.replan(schedule_id, scheduled_at);
                    }
                    return;
                }
            };
            let Some(row) = row else {
                tracing::debug!(
                    path = self.own_path.as_str(),
                    ?schedule_id,
                    "fire: row not found, skip"
                );
                return;
            };
            if row.status != "active" {
                tracing::debug!(
                    path = self.own_path.as_str(),
                    ?schedule_id,
                    status = %row.status,
                    "fire: not active, skip"
                );
                return;
            }
            // GH #904 fix round 1 (review I-1): a sleep strike belongs to the
            // moment it slept for. `rearm` (and `modify`) replace a one-shot in
            // place, still `active`, so a strike for the OLD `at` that was
            // already queued in the event channel would otherwise complete the
            // NEW order and emit its body -- the curator re-arms its one clock
            // id on every brain call, so a call landing as the cache goes cold
            // rebuilt at once and the order for the new moment never fired
            // (test `a_stale_strike_does_not_fire_the_order_a_rearm_put_in_its_place`).
            // The new order's own strike follows from the SetActive the op sent.
            // An operator trigger (GH #17) fires the schedule as it stands.
            // GH #913: the same holds for a cron row -- `modify` and
            // `add … rearm` change its expression in place, and the strike
            // queued for the old expression must not fire the new order at a
            // moment the new expression does not have ([`strike_is_stale`]).
            if strike_is_stale(&row.kind, scheduled_at, forced) {
                match &row.kind {
                    crate::timer::schedule::ScheduleKind::At(at) => tracing::debug!(
                        path = self.own_path.as_str(),
                        ?schedule_id,
                        %at,
                        %scheduled_at,
                        "fire: one-shot re-armed for another moment, stale strike skipped"
                    ),
                    crate::timer::schedule::ScheduleKind::Cron(expr) => tracing::debug!(
                        path = self.own_path.as_str(),
                        ?schedule_id,
                        cron = expr.as_str(),
                        %scheduled_at,
                        "fire: cron order changed to an expression without this moment, stale strike skipped"
                    ),
                }
                return;
            }

            // 2. State before emit (phase-5 canon).
            let is_once = matches!(row.kind, crate::timer::schedule::ScheduleKind::At(_));
            if is_once {
                let marked = db
                    .call_with_timeout(move |c| crate::timer::db::mark_completed(c, schedule_id))
                    .await;
                // GH #922 fix round 1 (review M-A): a `catch_up` row whose mark
                // did not land stays `active` and is planned again -- no emit.
                let marked = match marked {
                    Ok(Ok(n)) => Some(n),
                    _ => None,
                };
                if !one_shot_strike_may_emit(row.catch_up, marked) {
                    tracing::debug!(
                        path = self.own_path.as_str(),
                        ?schedule_id,
                        ?marked,
                        "fire: catch_up one-shot not marked completed, strike withheld"
                    );
                    // GH #956: "planned again" was a claim with nothing behind
                    // it until the next op or boot -- the I/O task had dropped
                    // the one-shot already. A mark that did not land (`None`)
                    // plans the strike again; `Some(0)` is a row another op
                    // completed or removed meanwhile, and stays done.
                    if marked.is_none() && !forced {
                        self.replan(schedule_id, scheduled_at);
                    }
                    return;
                }
            } else {
                let _ = db
                    .call_with_timeout(move |c| crate::timer::db::bump_iteration(c, schedule_id))
                    .await;
            }

            // 3. UBF body + auto-set headers (T15). Auto-set headers strictly
            //    override colliding `emit_headers` (cell-types.md l.441-451).
            //    RFC-3339-Z via `to_rfc3339_opts(SecondsFormat::Secs, true)`.
            //    Emit via OriginSink → parent_message_id=None, fresh trace_id
            //    (overview l.852).
            // GH #922: a missed `catch_up` one-shot says so on its strike.
            let late = is_late(&row, scheduled_at, forced, self.booted_at);
            let content = build_fire_content(&row, scheduled_at, is_once, late);
            let _ = sink
                .emit(meclaw_core::CellOutput {
                    target: row.emit_to.clone(),
                    content,
                })
                .await;
        }
    }
}

impl TimerCell {
    /// GH #956: hand a strike whose state step failed back to the I/O task
    /// ([`TimerReplan`]). `try_send`: the handler never waits on the I/O task,
    /// which may itself be waiting to push the next event here. A full or
    /// closed channel leaves the row `active` for the next op snapshot or
    /// boot, the state before this fix, and says so.
    fn replan(&self, schedule_id: meclaw_core::Uuid, scheduled_at: chrono::DateTime<chrono::Utc>) {
        let Some(tx) = &self.replan_tx else {
            return;
        };
        if tx
            .try_send(TimerReplan {
                schedule_id,
                scheduled_at,
            })
            .is_err()
        {
            tracing::warn!(
                path = self.own_path.as_str(),
                ?schedule_id,
                %scheduled_at,
                "fire: replan channel full or closed, strike left for the next plan"
            );
        }
    }
}

/// Builds the UBF content for a fire emission. `row.emit_body` carries the cell
/// payload; `row.emit_headers` is merged with the auto-set headers — auto-set
/// headers strictly **override** colliding keys (spec cell-types.md l.441-451).
/// `iteration_n` is set ONLY for repeating (cron) schedules; a one-shot omits the
/// field (spec l.427/449).
///
/// `iteration_n` is the PRE-bump value: T11 already did the +1 in the DB, but
/// `row` was loaded before that — so the first fire of a freshly INSERTed cron
/// carries iteration_n=0 (spec: "from 0").
///
/// GH #922: a `late` strike (a missed `catch_up` one-shot) carries the auto
/// header `late: true` next to its `scheduled_at` -- the moment it was due,
/// not the boot. A strike that is not late carries no `late` key at all, so
/// every existing emission is unchanged.
fn build_fire_content(
    row: &crate::timer::schedule::ScheduleRow,
    scheduled_at: chrono::DateTime<chrono::Utc>,
    is_once: bool,
    late: bool,
) -> meclaw_core::JsonValue {
    use chrono::SecondsFormat;
    use serde_json::json;
    let fired_at = chrono::Utc::now();
    let mut headers = row.emit_headers.clone();
    headers.insert(
        "event_id".into(),
        json!(meclaw_core::Uuid::now_v7().to_string()),
    );
    headers.insert("schedule_id".into(), json!(row.schedule_id.to_string()));
    headers.insert("schedule_name".into(), json!(row.schedule_name.clone()));
    headers.insert(
        "scheduled_at".into(),
        json!(scheduled_at.to_rfc3339_opts(SecondsFormat::Secs, true)),
    );
    headers.insert(
        "fired_at".into(),
        json!(fired_at.to_rfc3339_opts(SecondsFormat::Secs, true)),
    );
    if !is_once {
        headers.insert("iteration_n".into(), json!(row.iteration_n));
    }
    if late {
        headers.insert("late".into(), json!(true));
    }

    let mut content = row.emit_body.clone();
    if !content.is_object() {
        content = json!({});
    }
    content
        .as_object_mut()
        .expect("content normalized to object above")
        .insert("header".into(), meclaw_core::JsonValue::Object(headers));
    content
}

/// GH #922: whether a strike is the late catch-up of a missed one-shot. True
/// exactly for a sleep strike (not an operator trigger) of a `catch_up`
/// one-shot, for its own moment (`scheduled_at == at`, the guard of GH #904
/// already dropped any other), when that moment lay at or before the boot
/// instant -- the `<=` the plan used to take the row up
/// (`db::due_for_catch_up`). A catch-up row whose moment comes after the boot
/// fires on time and is not late.
fn is_late(
    row: &crate::timer::schedule::ScheduleRow,
    scheduled_at: chrono::DateTime<chrono::Utc>,
    forced: bool,
    booted_at: chrono::DateTime<chrono::Utc>,
) -> bool {
    !forced
        && row.catch_up
        && matches!(
            row.kind,
            crate::timer::schedule::ScheduleKind::At(at) if at == scheduled_at && at <= booted_at
        )
}

/// GH #904 / GH #913: whether a sleep strike belongs to an order the row no
/// longer holds, so `handle_event` drops it before any state step (no emit,
/// `iteration_n` untouched). A strike is pushed for the moment the I/O task
/// slept for; an op that lands between that push and its handling replaces
/// the row in place, still `active`. The strike is stale exactly when the
/// row's CURRENT order does not strike at `scheduled_at`: a one-shot whose
/// `at` is another moment (#904), a cron row whose current expression does
/// not have that moment. A moment the old and the new expression share is
/// the new order's own moment -- it fires, with the new body, once (the I/O
/// task plans the next one strictly after it). An operator trigger (`forced`,
/// GH #17) is never stale.
fn strike_is_stale(
    kind: &crate::timer::schedule::ScheduleKind,
    scheduled_at: chrono::DateTime<chrono::Utc>,
    forced: bool,
) -> bool {
    use crate::timer::schedule::ScheduleKind;
    if forced {
        return false;
    }
    match kind {
        ScheduleKind::At(at) => *at != scheduled_at,
        ScheduleKind::Cron(expr) => !cron_strikes_at(expr, scheduled_at),
    }
}

/// GH #913: whether `moment` is an occurrence of the cron expression `expr`,
/// read with the parser `run_io` plans with (six fields, seconds required,
/// UTC). No generation stamp and no schema change (OR-BD-14): the row's
/// expression itself says whether the moment is still its own. A strike's
/// moment is a whole second (`compute_next_occurrence` anchors on it, GH
/// #626), and croner matches each field of that second. An expression that
/// does not parse has no moment; every entrance (`add`, `modify`, the seed)
/// refuses one, so no row holds it.
fn cron_strikes_at(expr: &str, moment: chrono::DateTime<chrono::Utc>) -> bool {
    use croner::parser::{CronParser, Seconds};
    CronParser::builder()
        .seconds(Seconds::Required)
        .build()
        .parse(expr)
        .ok()
        .and_then(|c| c.is_time_matching(&moment).ok())
        .unwrap_or(false)
}

/// GH #922 fix round 1 (review M-A): whether a one-shot strike may emit
/// after its state step. A `catch_up` one-shot emits only when
/// `mark_completed` reported the row it completed (`Ok(1)`): a timed-out or
/// failed mark leaves the row `active`, `handle_event` plans the strike
/// again ([`TimerReplan`], GH #956; the next op snapshot or boot takes it up
/// through `db::due_for_catch_up` as well), and emitting now would strike
/// twice -- the timer documents "rather once too few than twice". `marked` is
/// the row count the mark returned, `None` when it timed out or failed.
fn one_shot_strike_may_emit(catch_up: bool, marked: Option<usize>) -> bool {
    !catch_up || marked == Some(1)
}

/// GH #231: the detail line for a one-shot whose `at` is not in the future, or
/// `None` when it still is.
///
/// The comparison is `at <= now`, the same boundary `load_active_filter_past`
/// draws — an op is refused exactly when the plan would have dropped it, so the
/// two can never disagree about one schedule. Both instants are rendered with
/// millisecond precision: the case this exists for is a lead time that ran out
/// in flight, and at second precision the message would read as two identical
/// timestamps.
fn past_at_detail(
    op: &str,
    at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    use chrono::SecondsFormat;
    if at > now {
        return None;
    }
    Some(format!(
        "{op}: `at` {} is not in the future (now {}) — a one-shot in the past is \
         never scheduled, so nothing was stored. If the time was still ahead when \
         the request was made, its lead time ran out while the op was in flight; \
         ask for a later one.",
        at.to_rfc3339_opts(SecondsFormat::Millis, true),
        now.to_rfc3339_opts(SecondsFormat::Millis, true),
    ))
}

/// Helper: loads the current active snapshot fresh from `cell.db` (including the
/// past-one-shot filter) and sends it as `SetActive` to the I/O task.
/// Fire-and-forget — on a full channel or a dead receiver the helper swallows
/// silently.
///
/// `now` is the op's own clock read, not a fresh one (GH #231): the filter must
/// draw its line at the same instant the op was accepted against, otherwise a
/// one-shot can be accepted and then immediately filtered out of the plan it was
/// accepted into.
async fn send_setactive_snapshot(
    db: &mut DbConn,
    reconfig_tx: &mpsc::Sender<crate::timer::io::TimerReconfig>,
    now: chrono::DateTime<chrono::Utc>,
) {
    let snap = db
        .call_with_timeout(move |c| crate::timer::db::load_active_filter_past(c, now))
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let _ = reconfig_tx
        .send(crate::timer::io::TimerReconfig::SetActive(snap))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::Path;

    #[test]
    fn split_io_moves_initial_active_out() {
        let mut cell = TimerCell::new(Path::new("/t"), vec![], 5000);
        let io = <TimerCell as meclaw_colony::LongRunningCell>::split_io(&mut cell);
        assert!(io.active.is_empty());
    }

    fn at_row(
        at: chrono::DateTime<chrono::Utc>,
        catch_up: bool,
    ) -> crate::timer::schedule::ScheduleRow {
        crate::timer::schedule::ScheduleRow {
            schedule_id: meclaw_core::Uuid::now_v7(),
            schedule_name: "probe".into(),
            kind: crate::timer::schedule::ScheduleKind::At(at),
            emit_to: Path::new("/sink"),
            emit_body: serde_json::json!({"messages": []}),
            emit_headers: serde_json::Map::new(),
            status: "active".into(),
            iteration_n: 0,
            catch_up,
        }
    }

    /// GH #922 fix round 1 (review M-A): a `catch_up` one-shot whose
    /// completion mark did not land stays `active` and would be planned again,
    /// so its strike must not emit; a one-shot without the flag keeps its old
    /// behaviour (an unmarked past row is dropped by the plan, no second strike).
    #[test]
    fn a_catch_up_one_shot_emits_only_after_its_completion_mark_landed() {
        // (label, catch_up, marked, expected)
        let table = [
            ("catch_up, marked", true, Some(1), true),
            ("catch_up, mark timed out", true, None, false),
            ("catch_up, mark found no active row", true, Some(0), false),
            ("plain, marked", false, Some(1), true),
            ("plain, mark timed out", false, None, true),
        ];
        for (label, catch_up, marked, expected) in table {
            assert_eq!(
                one_shot_strike_may_emit(catch_up, marked),
                expected,
                "{label}"
            );
        }
    }

    /// GH #922: a strike is `late` exactly when it is the catch-up of a marked
    /// one-shot whose moment lay at or before this incarnation's boot instant
    /// (the same instant the plan partitioned "missed" against). Not late: an
    /// unmarked row, a row that came due after the boot, an operator trigger,
    /// a strike for another moment, a cron row.
    #[test]
    fn is_late_marks_only_the_catch_up_of_a_marked_missed_one_shot() {
        use chrono::TimeZone;
        let boot = chrono::Utc.with_ymd_and_hms(2030, 1, 1, 12, 0, 0).unwrap();
        let s = chrono::Duration::seconds;
        let missed = boot - s(3);
        // (label, row, scheduled_at, forced, expected)
        let table = vec![
            ("marked missed", at_row(missed, true), missed, false, true),
            (
                "marked at the boot instant",
                at_row(boot, true),
                boot,
                false,
                true,
            ),
            (
                "unmarked missed",
                at_row(missed, false),
                missed,
                false,
                false,
            ),
            (
                "marked, due after boot",
                at_row(boot + s(1), true),
                boot + s(1),
                false,
                false,
            ),
            (
                "marked missed, triggered",
                at_row(missed, true),
                missed,
                true,
                false,
            ),
            (
                "marked missed, other moment",
                at_row(missed, true),
                missed - s(1),
                false,
                false,
            ),
        ];
        for (label, row, scheduled_at, forced, expected) in table {
            assert_eq!(
                is_late(&row, scheduled_at, forced, boot),
                expected,
                "{label}"
            );
        }
        let mut cron = at_row(missed, true);
        cron.kind = crate::timer::schedule::ScheduleKind::Cron("0 0 9 * * *".into());
        assert!(!is_late(&cron, missed, false, boot), "cron is never late");
    }

    /// GH #913: a strike without `forced` fires only on a moment of the order
    /// the row holds NOW. The cron rows model `modify`/`add … rearm` from
    /// `*/5` to `*/7` (seconds field) with the strike for the old expression
    /// still queued; the one-shot rows are the #904 cases, unchanged. Edges:
    /// the moment both expressions share fires (it is the new order's own),
    /// the minute turn, a minute-field expression one second either side of
    /// its moment, and an expression that does not parse (it has no moment;
    /// every entrance validates, so a row never holds one).
    #[test]
    fn a_strike_is_stale_unless_the_current_order_strikes_at_its_moment() {
        use crate::timer::schedule::ScheduleKind;
        use chrono::TimeZone;
        let t = |h: u32, m: u32, s: u32| chrono::Utc.with_ymd_and_hms(2030, 1, 1, h, m, s).unwrap();
        let cron = |e: &str| ScheduleKind::Cron(e.into());
        let every5 = "*/5 * * * * *";
        let every7 = "*/7 * * * * *";
        // (label, the row's order now, scheduled_at, forced, stale)
        let table = vec![
            (
                "row now */7, strike for */5 on :05",
                cron(every7),
                t(12, 0, 5),
                false,
                true,
            ),
            (
                "row now */7, strike on :07",
                cron(every7),
                t(12, 0, 7),
                false,
                false,
            ),
            (
                "row now */7, :35 lies on both",
                cron(every7),
                t(12, 0, 35),
                false,
                false,
            ),
            (
                "row now */7, :56 is its last of the minute",
                cron(every7),
                t(12, 0, 56),
                false,
                false,
            ),
            (
                "row now */7, :59 is not on it",
                cron(every7),
                t(12, 0, 59),
                false,
                true,
            ),
            (
                "row now */7, the minute turn :00",
                cron(every7),
                t(12, 1, 0),
                false,
                false,
            ),
            (
                "same expression after a modify",
                cron(every5),
                t(12, 0, 5),
                false,
                false,
            ),
            (
                "minute field, on the moment",
                cron("0 */5 * * * *"),
                t(12, 35, 0),
                false,
                false,
            ),
            (
                "minute field, a second before",
                cron("0 */5 * * * *"),
                t(12, 34, 59),
                false,
                true,
            ),
            (
                "minute field, a second after",
                cron("0 */5 * * * *"),
                t(12, 35, 1),
                false,
                true,
            ),
            (
                "row now */7, forced off the expression",
                cron(every7),
                t(12, 0, 5),
                true,
                false,
            ),
            (
                "an expression that does not parse",
                cron("not a cron"),
                t(12, 0, 5),
                false,
                true,
            ),
            (
                "one-shot, its own moment",
                ScheduleKind::At(t(12, 0, 0)),
                t(12, 0, 0),
                false,
                false,
            ),
            (
                "one-shot re-armed, old moment",
                ScheduleKind::At(t(12, 5, 0)),
                t(12, 0, 0),
                false,
                true,
            ),
            (
                "one-shot re-armed, forced",
                ScheduleKind::At(t(12, 5, 0)),
                t(12, 0, 0),
                true,
                false,
            ),
        ];
        for (label, kind, scheduled_at, forced, expected) in table {
            assert_eq!(
                strike_is_stale(&kind, scheduled_at, forced),
                expected,
                "{label}"
            );
        }
    }

    /// GH #922: the late strike is today's body plus `late: true` next to the
    /// `scheduled_at` it always carried; a strike that is not late is
    /// byte-for-byte what it was (no `late` key at all).
    #[test]
    fn build_fire_content_adds_late_only_to_a_late_strike() {
        use chrono::TimeZone;
        let at = chrono::Utc
            .with_ymd_and_hms(2030, 1, 1, 11, 59, 57)
            .unwrap();
        let mut row = at_row(at, true);
        row.emit_headers
            .insert("msg_type".into(), serde_json::json!("probe_tick"));
        row.emit_headers
            .insert("late".into(), serde_json::json!("forged"));

        let on_time = build_fire_content(&row, at, true, false);
        let header = on_time["header"].as_object().unwrap();
        let mut keys: Vec<&str> = header.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "event_id",
                "fired_at",
                "late",
                "msg_type",
                "schedule_id",
                "schedule_name",
                "scheduled_at"
            ],
            "the auto header set is unchanged; a caller's own header passes through"
        );
        assert_eq!(
            header["late"], "forged",
            "not late: the caller's value is not touched"
        );
        assert_eq!(on_time["messages"], serde_json::json!([]));

        let late = build_fire_content(&row, at, true, true);
        assert_eq!(
            late["header"]["late"], true,
            "auto-set, overrides the caller's key"
        );
        assert_eq!(late["header"]["scheduled_at"], "2030-01-01T11:59:57Z");
        assert_eq!(late["messages"], serde_json::json!([]));

        row.emit_headers.remove("late");
        let plain = build_fire_content(&row, at, true, false);
        assert!(
            plain["header"].get("late").is_none(),
            "no `late` on a strike in time"
        );
    }
}
