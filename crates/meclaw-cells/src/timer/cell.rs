//! `TimerCell` — the `LongRunningCell` implementation of the `timer` cell type
//! (double task: handler + I/O). The handler is the DB authority: it parses
//! schedule ops (`add`/`modify`/`remove`) off the mailbox, persists them into
//! `cell.db` and sends the I/O task a fresh active snapshot via `reconfig_tx`.
//! The I/O task computes `sleep_until` on the working copy and emits the firings.
//! Spec: `docs/cell-types.md` § `timer`.

use crate::timer::io::{TimerEvent, TimerReconfig};
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
        }
    }
}

/// I/O-local state struct. Single owner (held by-value by the I/O sub-task).
/// No mutex, no Arc.
pub struct TimerIo {
    /// Working copy of the active schedules (cron + future-at).
    pub active: Vec<ActiveSchedule>,
}

impl LongRunningCell for TimerCell {
    type Event = TimerEvent;
    type Reconfig = TimerReconfig;
    type Io = TimerIo;

    fn split_io(&mut self) -> Self::Io {
        TimerIo {
            active: self.initial_io.take().unwrap_or_default(),
        }
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
                        crate::timer::emit::emit_op_error(sink, &msg, "invalid_input", &e.detail())
                            .await;
                    }
                }
                // Standalone params-update → done (no schedule op in this message).
                return;
            }

            let op = match crate::timer::op::TimerOp::parse(&body_val) {
                Ok(o) => o,
                Err(e) => {
                    let code = if e.starts_with("cron:") {
                        "invalid_cron"
                    } else {
                        "parse_error"
                    };
                    crate::timer::emit::emit_op_error(sink, &msg, code, &e).await;
                    return;
                }
            };
            match op {
                crate::timer::op::TimerOp::Add(row) => {
                    let row_for_call = row.clone();
                    let inserted = db
                        .call_with_timeout(move |c| {
                            crate::timer::db::insert_schedule(c, &row_for_call)
                        })
                        .await;
                    match inserted {
                        Ok(Ok(())) => send_setactive_snapshot(db, reconfig_tx).await,
                        Ok(Err(e)) => {
                            crate::timer::emit::emit_op_error(
                                sink,
                                &msg,
                                "schedule_id_exists",
                                &format!("add: {e}"),
                            )
                            .await
                        }
                        Err(meclaw_colony::QueryTimeout::Interrupted) => {
                            crate::timer::emit::emit_op_error(
                                sink,
                                &msg,
                                "query_timeout",
                                "add: query exceeded query_timeout_ms",
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
                            crate::timer::emit::emit_op_error(
                                sink,
                                &msg,
                                "query_timeout",
                                "modify: load exceeded query_timeout_ms",
                            )
                            .await;
                            return;
                        }
                    };
                    let Some(cur) = current else {
                        crate::timer::emit::emit_op_error(
                            sink,
                            &msg,
                            "schedule_not_found",
                            &format!("modify: id {schedule_id} unknown"),
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
                        crate::timer::emit::emit_op_error(
                            sink,
                            &msg,
                            "kind_mismatch",
                            "modify: cannot switch cron<->at (use remove+add)",
                        )
                        .await;
                        return;
                    }
                    let n = match db
                        .call_with_timeout(move |c| {
                            crate::timer::db::modify_schedule_fields(
                                c,
                                schedule_id,
                                new_cron.as_deref(),
                                new_name.as_deref(),
                                new_emit_to.as_deref(),
                                new_at,
                            )
                        })
                        .await
                    {
                        Ok(r) => r.unwrap_or(0),
                        Err(meclaw_colony::QueryTimeout::Interrupted) => {
                            crate::timer::emit::emit_op_error(
                                sink,
                                &msg,
                                "query_timeout",
                                "modify: update exceeded query_timeout_ms",
                            )
                            .await;
                            return;
                        }
                    };
                    if n == 0 {
                        crate::timer::emit::emit_op_error(
                            sink,
                            &msg,
                            "schedule_not_found",
                            "modify: 0 rows updated",
                        )
                        .await;
                    } else {
                        send_setactive_snapshot(db, reconfig_tx).await;
                    }
                }
                crate::timer::op::TimerOp::Remove { schedule_id } => {
                    let n = match db
                        .call_with_timeout(move |c| crate::timer::db::mark_removed(c, schedule_id))
                        .await
                    {
                        Ok(r) => r.unwrap_or(0),
                        Err(meclaw_colony::QueryTimeout::Interrupted) => {
                            crate::timer::emit::emit_op_error(
                                sink,
                                &msg,
                                "query_timeout",
                                "remove: query exceeded query_timeout_ms",
                            )
                            .await;
                            return;
                        }
                    };
                    if n == 0 {
                        crate::timer::emit::emit_op_error(
                            sink,
                            &msg,
                            "schedule_not_found",
                            "remove: 0 rows updated",
                        )
                        .await;
                    } else {
                        send_setactive_snapshot(db, reconfig_tx).await;
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

            // 2. State before emit (phase-5 canon).
            let is_once = matches!(row.kind, crate::timer::schedule::ScheduleKind::At(_));
            if is_once {
                let _ = db
                    .call_with_timeout(move |c| crate::timer::db::mark_completed(c, schedule_id))
                    .await;
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
            let content = build_fire_content(&row, scheduled_at, is_once);
            let _ = sink
                .emit(meclaw_core::CellOutput {
                    target: row.emit_to.clone(),
                    content,
                })
                .await;
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
fn build_fire_content(
    row: &crate::timer::schedule::ScheduleRow,
    scheduled_at: chrono::DateTime<chrono::Utc>,
    is_once: bool,
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

/// Helper: loads the current active snapshot fresh from `cell.db` (including the
/// past-one-shot filter) and sends it as `SetActive` to the I/O task.
/// Fire-and-forget — on a full channel or a dead receiver the helper swallows
/// silently.
async fn send_setactive_snapshot(
    db: &mut DbConn,
    reconfig_tx: &mpsc::Sender<crate::timer::io::TimerReconfig>,
) {
    let snap = db
        .call_with_timeout(|c| crate::timer::db::load_active_filter_past(c, chrono::Utc::now()))
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
}
