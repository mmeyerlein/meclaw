//! Generic per-cell task: pulls messages from the cell's mailbox and
//! dispatches them to the cell's `handle()`.
//!
//! **Concept-B backstop (`cell.message_timeout`)**: `cell_task_stateful`
//! wraps each `handle()` call in `tokio::time::timeout(message_timeout, …)`
//! (Paket-3 P3-B2). On elapse it fires the `backstop` oneshot, emits a generic
//! backstop timeout error to the input's `reply_to`, and ENDS the task without
//! firing `peace_tx`, so the watcher classifies `DeathKind::Backstop` →
//! one_for_one restart (Paket-3 P3-B-restart). The `stateless_dispatcher`
//! variant (Paket-3 P3-B3) wraps each detached per-message worker's `handle()`
//! call the same way, but is SIMPLER: on elapse the worker emits the backstop
//! error and just ENDS (drops its permit) — there is NO restart, NO CellDied,
//! NO peace (the dispatcher, not the worker, is the supervised unit). `None`
//! disables the wrapper (behavior-neutral). The resolved value is plumbed from
//! `cell.message_timeout` / `colony.json message_timeout_default_ms` at the
//! spawn call-sites (Paket-3 P3-B-plumb-2). Concept-A
//! (`params.external_timeout_ms`) remains the primary I/O protection — B is a
//! pathology backstop, not an I/O timeout.

use meclaw_core::{Cell, CellEmission, Message, OutputSink, Path};
use tokio::sync::mpsc;

/// Resolve every blob reference a cell must not see, at the cell-delivery
/// boundary (spec Z.1363), so every cell gets a transparent inline body.
///
/// Three classes, in this order:
///
/// 1. **Whole-body `Body::Blob`** (Phase-13.5 A8) — the substrate's own
///    oversized-body offload, read back into `Body::Inline`.
/// 2. **In-message pointers** (GH #19 / D-025) — `messages_id` and `text_id`
///    entries inside `messages[]`, expanded recursively and depth-bounded by
///    [`crate::DiskBlobStore::max_recursion_depth`].
/// 3. **System-tree pointers** (GH #86) — `{text_id}` leaves anywhere in the
///    `system` tree, each becoming the `{"text": …}` container it names, under
///    the same limit and the same guards.
///
/// See [`crate::blob::pointers`] for the semantics and the rulings behind them.
///
/// All of them run BEFORE `enforce_consumes_for_delivery`, which inspects the
/// body's top-level slots and would otherwise validate an unexpanded body.
///
/// Classes 2 and 3 resolve against ONE working copy that is committed only if
/// both succeed. Each pass is already all-or-nothing on its own; the working
/// copy extends that across the pair, so a body whose `messages[]` resolves and
/// whose `system` tree does not is dead-lettered as it arrived, never
/// half-expanded.
///
/// Returns `true` to deliver; `false` if a reference was unresolvable and the
/// message was dead-lettered (caller must skip `handle()`). With no blob-store
/// wired (the plain test path) every reference passes through unchanged.
pub(crate) async fn resolve_blob_for_delivery(
    msg: &mut Message,
    blob_store: &Option<std::sync::Arc<crate::DiskBlobStore>>,
    colony_inbox_tx: &Option<mpsc::Sender<crate::ColonyMsg>>,
) -> bool {
    let Some(store) = blob_store else {
        return true;
    };
    // 1. Whole-body offload.
    if let meclaw_core::Body::Blob(id) = &msg.body {
        let id = *id;
        match store.read_body(id).await {
            Ok(value) => msg.body = meclaw_core::Body::Inline(value),
            Err(e) => {
                tracing::warn!(
                    blob_id = %id,
                    error = %e,
                    "blob resolution failed at cell delivery — dead-lettering (blob_unavailable)"
                );
                dead_letter(
                    msg,
                    crate::DeadLetterReason::BlobUnavailable,
                    colony_inbox_tx,
                )
                .await;
                return false;
            }
        }
    }
    // 2. + 3. Body pointers, inside the (now) inline body. The two pre-checks
    // keep an ordinary body free of any resolution work: one scan of
    // `messages[]` for the two pointer keys, one walk of the `system` tree for
    // pointer leaves, no store access either way.
    let meclaw_core::Body::Inline(body) = &msg.body else {
        return true;
    };
    let has_message_pointer = crate::blob::pointers::has_in_message_pointer(body);
    let has_system_pointer = crate::blob::pointers::has_system_pointer(body);
    if !has_message_pointer && !has_system_pointer {
        return true;
    }
    let limit = store.max_recursion_depth();
    let mut working = body.clone();
    let outcome = async {
        if has_message_pointer {
            crate::blob::pointers::resolve_in_message_pointers(&mut working, store, limit).await?;
        }
        if has_system_pointer {
            crate::blob::pointers::resolve_system_pointers(&mut working, store, limit).await?;
        }
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => {
            msg.body = meclaw_core::Body::Inline(working);
            true
        }
        Err(e) => {
            let reason = crate::blob::pointers::PointerError::dead_letter_reason(&e);
            tracing::warn!(
                msg_id = %msg.id,
                msg_target = msg.target.as_str(),
                error = %e,
                error_code = reason.as_code(),
                "body blob pointer resolution failed at cell delivery — dead-lettering"
            );
            dead_letter(msg, reason, colony_inbox_tx).await;
            false
        }
    }
}

/// Hand a message to the colony's dead-letter arm, if a colony inbox is wired.
///
/// The plain `cell_task` test path passes `None` and drops the message with the
/// warn line above as its only trace — unchanged behaviour, extracted here so
/// both blob-resolution failure classes report identically.
async fn dead_letter(
    msg: &Message,
    reason: crate::DeadLetterReason,
    colony_inbox_tx: &Option<mpsc::Sender<crate::ColonyMsg>>,
) {
    if let Some(tx) = colony_inbox_tx {
        let _ = tx
            .send(crate::ColonyMsg::DeadLetterMessage {
                message: msg.clone(),
                reason,
            })
            .await;
    }
}

/// Substrate-side `consumes` ingress check at the cell-delivery boundary
/// (config.md § consumes): runs AFTER blob resolution (body is inline).
/// Returns `true` to deliver. On violation the cell is NOT called: with a
/// `reply_to` an error message (`error_code: "consumes_violation"`) is
/// pushed via the sink; otherwise the message is dead-lettered
/// (`DeadLetterReason::ConsumesViolation`). A still-blob body (no store
/// wired, test path) passes through unchecked — mirrors blob resolution.
pub(crate) async fn enforce_consumes_for_delivery(
    msg: &Message,
    consumes: &Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
    sink: &OutputSink,
    colony_inbox_tx: &Option<mpsc::Sender<crate::ColonyMsg>>,
) -> bool {
    let Some(compiled) = consumes else {
        return true;
    };
    let meclaw_core::Body::Inline(body) = &msg.body else {
        return true;
    };
    match meclaw_core::validate_consumes(body, &msg.headers, compiled) {
        Ok(()) => true,
        Err(reason) => {
            tracing::warn!(
                msg_id = %msg.id,
                msg_target = msg.target.as_str(),
                error = %reason,
                "required consumes check failed at delivery — cell not invoked"
            );
            if let Some(target) = msg.reply_to.clone() {
                let content = meclaw_core::serde_json::json!({
                    "header": {"finish_reason": "error", "error_code": "consumes_violation"},
                    "messages": [{"origin": "assistant", "type": "text", "text": reason}]
                });
                // W2b (Ruling A1): direct error reply to the known sender (reply_to),
                // routed via route_with_log in the outputs-arm — not edge-routed.
                let _ = sink
                    .push_direct_reply(meclaw_core::CellOutput { target, content })
                    .await;
            } else if let Some(tx) = colony_inbox_tx {
                let _ = tx
                    .send(crate::ColonyMsg::DeadLetterMessage {
                        message: msg.clone(),
                        reason: crate::DeadLetterReason::ConsumesViolation,
                    })
                    .await;
            }
            false
        }
    }
}

/// Emit the generic Concept-B backstop timeout error to the input's
/// `reply_to`, best-effort. The body is a valid UBF body carrying
/// `header.finish_reason:"error"` + `header.error_code:"message_timeout"`
/// (spec § Timeouts B); Colony extracts `content.header` into the message
/// headers. When `reply_to` is `None` there is nowhere to send → silent no-op.
///
/// Best-effort: a send error (closed channel) is ignored, and if Colony's
/// outputs channel is full the `push` blocks — the documented spec behavior.
async fn emit_backstop_timeout(sink: &OutputSink, reply_to: Option<Path>) {
    let Some(target) = reply_to else {
        return;
    };
    let content = meclaw_core::serde_json::json!({
        "header": {"finish_reason": "error", "error_code": "message_timeout"},
        "messages": [
            {"origin": "assistant", "type": "text", "text": "message_timeout backstop elapsed"}
        ]
    });
    // W2b (Ruling A1): direct error reply to the known sender (reply_to), routed
    // via route_with_log in the outputs-arm — not edge-routed.
    let _ = sink
        .push_direct_reply(meclaw_core::CellOutput { target, content })
        .await;
}

/// GH #47: gives a delivery's ticket back to the colony's `DrainLedger` when the
/// handler that owns it is done.
///
/// Modelled on `TermAckGuard`: the sender lives in an `Option` so `Drop` can
/// `take()` it and report exactly once, and `None` makes the guard inert (the
/// test-only `cell_task` without a colony behind it). `try_send` because `Drop`
/// cannot await — the same constraint `MailboxGuard` works under.
///
/// The guard must be constructed on the message-taking path BEFORE any `.await`
/// of the handling itself, and it must live until after `handle()` returns. Its
/// `Drop` then covers all four ways a handler can end: a normal return, a
/// `continue` from one of the delivery gates, the B-backstop's cancellation, and
/// a panic unwinding through the `.await`.
///
pub(crate) struct WorkGuard {
    own_path: Path,
    tx: Option<mpsc::Sender<crate::ColonyMsg>>,
}

impl WorkGuard {
    /// Build a guard for `own_path`. `None` → inert (no report on drop).
    pub(crate) fn new(own_path: Path, tx: Option<mpsc::Sender<crate::ColonyMsg>>) -> Self {
        Self { own_path, tx }
    }
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            // Colony gone or inbox full: the ledger keeps the ticket and the
            // drain reports it at the deadline. Never a panic on the way out.
            let _ = tx.try_send(crate::ColonyMsg::WorkDone {
                path: self.own_path.clone(),
            });
        }
    }
}

/// Generic per-cell task loop. See module doc for the B-Backstop
/// (`cell.message_timeout`) deferral status.
///
/// `consumes`: pre-compiled required-`consumes` views, enforced at the
/// delivery boundary via `enforce_consumes_for_delivery` (Slice 2, Task 2.4).
///
/// `colony_inbox_tx`: GH #47 — where the per-delivery `WorkDone` ticket goes.
/// `None` for the unit tests that run this task without a colony behind it.
pub async fn cell_task<C: Cell + Send + 'static>(
    own_path: Path,
    mut mailbox: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<CellEmission>,
    mut cell: C,
    blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
    colony_inbox_tx: Option<mpsc::Sender<crate::ColonyMsg>>,
) {
    while let Some(mut msg) = mailbox.recv().await {
        // GH #47: the plain task has no DLQ path and no rescue, but it does have
        // to account for what the colony handed it — `meclaw-testing`'s echo
        // factory runs here behind a real colony.
        let _work = WorkGuard::new(own_path.clone(), colony_inbox_tx.clone());
        let input_id = msg.id;
        let input_trace = msg.trace_id;
        let input_ttl = msg.ttl;
        let input_headers = msg.headers.clone();
        let sink = OutputSink::new(
            outputs_tx.clone(),
            own_path.clone(),
            input_id,
            input_trace,
            input_ttl,
            input_headers,
            msg.reply_to.clone(),
        );
        // Plain cell_task is test-only (no colony_inbox_tx) → no DLQ path; a
        // failed resolution just warns and the blob passes through, and a
        // reply_to-less consumes violation is dropped without a DLQ entry.
        if resolve_blob_for_delivery(&mut msg, &blob_store, &None).await
            && enforce_consumes_for_delivery(&msg, &consumes, &sink, &None).await
        {
            cell.handle(msg, &sink).await;
        }
    }
}

/// Phase-6.5: stateful cell task variant.
///
/// Owns the `cell.db` `DbConn` in its stack frame and passes
/// `&mut DbConn` to `StatefulCell::handle` per message. The Future
/// output is `()` for `RespawnFn` `JoinHandle<()>` compatibility — the
/// `colony.rs::RespawnFn` signature stays byte-identical to phase-5-done.
///
/// Mailbox-channel drop ends the loop gracefully (`recv()` returns
/// `None`). A panic inside `cell.handle` propagates out of this function:
/// stack-unwind drops the `DbConn` (sqlite3_close), the `JoinHandle`
/// reports the panic, and the supervisor sees a crashed cell.
///
/// See module doc for the B-Backstop (`cell.message_timeout`) deferral
/// status.
///
/// Phase 13 step 13-D-1: signature carries all Phase-13 params at once —
/// `idle_timeout`, `peace_tx`, `colony_inbox_tx`, `cell_timeout`. The idle
/// arm (13-H) self-despawns on idle elapsed; the recv arm (13-M-1) additionally
/// self-despawns after each `handle()`-Return when `cell_timeout > 0`
/// (One-Shot). Behavior-neutral against Phase-12 when callers pass
/// `None, None, None, 0`.
///
/// Phase-13.5 Lifecycle-3b (F2/A2): `stop_rx` is the colony-initiated
/// **peace-stop** receiver. When the colony fires it, the biased stop-arm
/// (order `stop > recv > idle`) fires `peace_tx` (→ watcher sees Ok → NO
/// `CellDied`) and returns the mailbox `Receiver` via `ColonyMsg::Stopped`
/// (≠ `Sleep` — Disconnect, not idle). An already-running `handle()` finishes
/// first (it runs inside the recv block); after Disconnect NO new mailbox
/// message is processed (remainder → DLQ by Task 4). `None` → no stop arm
/// (behavior-neutral).
///
/// Phase-13.5 Lifecycle-3b (F2): `death_ack` is the death-ack sender, moved into
/// a `TermAckGuard` bound so it drops **after** the `cell.db` `DbConn` — the
/// colony's `death_ack_rx` only fires once `sqlite3_close` has run. `None` →
/// inert guard.
///
/// `consumes`: pre-compiled required-`consumes` views, enforced at the
/// delivery boundary via `enforce_consumes_for_delivery` (Slice 2, Task 2.4).
#[allow(clippy::too_many_arguments)]
pub async fn cell_task_stateful<C: crate::stateful_cell::StatefulCell>(
    own_path: Path,
    mailbox: mpsc::Receiver<Message>,
    outputs_tx: mpsc::Sender<CellEmission>,
    mut cell: C,
    db: crate::DbConn,
    idle_timeout: Option<std::time::Duration>,
    message_timeout: Option<std::time::Duration>,
    mut peace_tx: Option<tokio::sync::oneshot::Sender<()>>,
    mut backstop_tx: Option<tokio::sync::oneshot::Sender<()>>,
    colony_inbox_tx: Option<mpsc::Sender<crate::ColonyMsg>>,
    cell_timeout: i64,
    stop_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    death_ack: Option<tokio::sync::oneshot::Sender<()>>,
    blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
    bounds: meclaw_core::TransferBounds,
) {
    // Phase-13.5 Lifecycle-3b Task 3 (F2): death-ack guard. Declared FIRST so it
    // drops LAST (reverse-declaration drop order) — i.e. AFTER the re-bound `db`
    // below, hence after `sqlite3_close`. The colony's `death_ack_rx` therefore
    // only fires once `cell.db` is closed.
    let _term_guard = crate::TermAckGuard::new(death_ack);
    // Re-bind `db` into a body-local so it drops BEFORE `_term_guard`.
    let mut db = db;
    // GH #18: the mailbox lives in a guard from here on. Declared AFTER `db` so
    // it drops BEFORE it — the rescue reaches the colony ahead of the cell.db
    // close and ahead of the death-ack. On a panic the unwind runs this drop;
    // on the peaceful exits below `release()` disarms it.
    let mut mailbox = crate::mailbox_rescue::MailboxGuard::new(
        own_path.clone(),
        mailbox,
        colony_inbox_tx.clone(),
    );
    // Phase-13.5 Lifecycle-3b (F2/A2): colony-initiated peace-stop receiver,
    // fused so it can be polled inside the loop's `select!`. `None` → `pending`
    // (behavior-neutral: no stop arm ever fires).
    let mut stop_fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        match stop_rx {
            Some(rx) => Box::pin(async move {
                // Only an explicit `stop_tx.send(())` triggers the stop arm. A
                // dropped sender (Err) must NOT fire it — park forever so the
                // task keeps running (the stop_tx may be held elsewhere or
                // legitimately dropped without a stop request).
                if rx.await.is_err() {
                    std::future::pending::<()>().await
                }
            }),
            None => Box::pin(std::future::pending::<()>()),
        };
    loop {
        let idle_fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            match idle_timeout {
                Some(d) => Box::pin(tokio::time::sleep(d)),
                None => Box::pin(std::future::pending::<()>()),
            };
        tokio::select! {
            // A2: biased order stop > recv > idle. An already-running `handle()`
            // finishes naturally (it runs inside the recv block; `select!`
            // re-evaluates only after `handle()` returns); then this stop-arm
            // wins over a new recv → after Disconnect NO new mailbox message is
            // processed (remainder → DLQ cell_inactive via Task 4).
            biased;
            _ = &mut stop_fut => {
                // Peace before task-end → watcher sees Ok → NO CellDied →
                // handle_cell_died never fires.
                if let Some(p) = peace_tx.take() {
                    let _ = p.send(());
                }
                // Return the mailbox Receiver to the colony so the remainder
                // can be drained to the DLQ (Task 4). `Stopped` ≠ `Sleep`:
                // Disconnect, not idle. GH #18: `release()` disarms the guard —
                // this path routes the remainder itself.
                if let (Some(tx), Some(receiver)) = (&colony_inbox_tx, mailbox.release()) {
                    let _ = tx
                        .send(crate::ColonyMsg::Stopped {
                            path: own_path.clone(),
                            receiver,
                        })
                        .await;
                }
                return;
            }
            maybe_msg = mailbox.recv() => {
                let Some(mut msg) = maybe_msg else { return; };
                // GH #47: the ticket the colony took for this delivery comes
                // back when this arm ends — normal return, delivery-gate
                // `continue`, backstop return or panic unwind, all of them.
                let _work = WorkGuard::new(own_path.clone(), colony_inbox_tx.clone());
                let input_id = msg.id;
                let input_trace = msg.trace_id;
                let input_ttl = msg.ttl;
                let input_headers = msg.headers.clone();
                let input_reply_to = msg.reply_to.clone();
                let sink = OutputSink::new(
                    outputs_tx.clone(),
                    own_path.clone(),
                    input_id,
                    input_trace,
                    input_ttl,
                    input_headers,
                    msg.reply_to.clone(),
                );
                if !resolve_blob_for_delivery(&mut msg, &blob_store, &colony_inbox_tx).await {
                    continue;
                }
                // GH #253: the `transfer` body slot is answered by the SUBSTRATE,
                // above every cell type, against this cell's OWN `DbConn` — the
                // only connection that carries the extensions the cell installed
                // (a `store`'s `meclaw_stem_v1` tokenizer, without which an FTS
                // trigger cannot even open its index). A message that carries the
                // slot never reaches `handle()`.
                //
                // BEFORE the consumes gate, and deliberately: `consumes` declares
                // what the cell's `handle()` needs, and a transfer never reaches
                // it. Enforcing a body contract against a payload the cell will
                // not see would dead-letter every transfer sent to a cell that
                // requires `messages` — which is every `store` in the library.
                // AFTER the blob resolution, because a slot is only readable once
                // the body it sits in is whole.
                if crate::db_transfer::handle_transfer_slot(&msg, &sink, &mut db, bounds)
                .await
                {
                    continue;
                }
                if !enforce_consumes_for_delivery(&msg, &consumes, &sink, &colony_inbox_tx).await {
                    continue;
                }
                // Concept-B backstop: wrap the WHOLE `handle()` call in
                // `tokio::time::timeout`. `timeout` does NOT catch panics — a
                // panicking `handle()` unwinds through the `.await`, panics this
                // task, and the watcher classifies `DeathKind::Panic`
                // (AUDIT-PRE14-001). `None` → no wrapper (behavior-neutral).
                let completed = match message_timeout {
                    Some(t) => tokio::time::timeout(t, cell.handle(msg, &sink, &mut db))
                        .await
                        .is_ok(),
                    None => {
                        cell.handle(msg, &sink, &mut db).await;
                        true
                    }
                };
                if !completed {
                    // B-backstop elapsed: emit a generic timeout error to the
                    // input's `reply_to`, then END the task. We do NOT take/send
                    // `peace_tx` — the still-`Some` local drops, so the watcher's
                    // `peace_rx.await` returns `Err` (death path). But we DO fire
                    // `backstop_tx` BEFORE the clean `return` (Paket-3
                    // P3-B-restart): the watcher then classifies this death as
                    // `DeathKind::Backstop` → `handle_cell_died` → one_for_one
                    // restart (spec § Timeouts B: "Task terminiert, Supervisor
                    // restartet"). A clean `return` without the backstop signal
                    // would be `DeathKind::Normal` → removal (the old bug).
                    if let Some(b) = backstop_tx.take() {
                        let _ = b.send(());
                    }
                    emit_backstop_timeout(&sink, input_reply_to).await;
                    return;
                }
                if cell_timeout > 0 {
                    // Phase-13 13-M-1: One-Shot — despawn after this message.
                    // peace + sleep send analogous to the idle arm.
                    if let Some(p) = peace_tx.take() {
                        let _ = p.send(());
                    }
                    // GH #18: peaceful park — `release()` disarms the guard.
                    if let (Some(tx), Some(receiver)) = (&colony_inbox_tx, mailbox.release()) {
                        let _ = tx
                            .send(crate::ColonyMsg::Sleep {
                                path: own_path.clone(),
                                receiver,
                            })
                            .await;
                    }
                    return;
                }
            }
            _ = idle_fut => {
                if mailbox.is_empty() {
                    if let Some(p) = peace_tx.take() {
                        let _ = p.send(());
                    }
                    // GH #18: peaceful park — `release()` disarms the guard.
                    if let (Some(tx), Some(receiver)) = (&colony_inbox_tx, mailbox.release()) {
                        let _ = tx
                            .send(crate::ColonyMsg::Sleep {
                                path: own_path.clone(),
                                receiver,
                            })
                            .await;
                    }
                    return;
                }
                // race: new msg arrived between sleep elapsed and is_empty check.
                // loop continues — recv-arm fires next.
            }
        }
    }
}

/// Phase-10 outer task for long-running cells (dual-task pattern).
///
/// Carries the single `JoinHandle<()>` observed by the supervisor — the
/// `RespawnFn` signature stays byte-identical to `phase-9-done`. Internally
/// spawns two sub-tasks (I/O via `L::run_io`, handler via `handler_loop`),
/// `tokio::select!`s over their `JoinHandle`s, aborts the surviving
/// sibling on first completion, and re-raises any panic via
/// `std::panic::resume_unwind` so the supervisor sees `was_panic=true`
/// (one_for_one). See `docs/meclaw-overview.md` § "Long-Running-Cells:
/// Doppel-Task". B-backstop (`cell.message_timeout`) remains deferred per
/// Phase-7.5/9 limitations.
///
/// Phase-13.5 Lifecycle-3b (F2/A4): `peace_tx` + `stop_rx` + `colony_inbox_tx`
/// wire the colony-initiated **peace-stop**. `stop_rx` is moved into
/// `handler_loop` (it owns the mailbox); on stop the handler returns the
/// mailbox `Receiver` via `ColonyMsg::Stopped`, breaks gracefully and yields
/// `true`. The outer task then aborts ONLY `io_join` (abort → `Cancelled`,
/// never `is_panic`, so no `resume_unwind`) and fires `peace_tx` → watcher
/// sees Ok → NO `CellDied` → `handle_cell_died` never runs. On every other
/// exit (mailbox-close, io-death, panic) `peace_tx` is dropped without sending,
/// preserving the pre-3b death path (watcher Err → `CellDied`). `None` for the
/// params → behavior-neutral against pre-3b callers. `death_ack` is moved into
/// `handler_loop` (which owns `cell.db`) and fired by its `TermAckGuard` after
/// the connection closes.
///
/// `consumes`: pre-compiled required-`consumes` views, moved into
/// `handler_loop` (which owns the mailbox) and enforced at the delivery
/// boundary via `enforce_consumes_for_delivery` (Slice 2, Task 2.4).
#[allow(clippy::too_many_arguments)]
pub async fn cell_task_long_running<L: crate::long_running_cell::LongRunningCell + 'static>(
    own_path: meclaw_core::Path,
    mailbox: mpsc::Receiver<meclaw_core::Message>,
    outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
    default_origin_ttl: u32,
    mut cell: L,
    db: crate::DbConn,
    mut peace_tx: Option<tokio::sync::oneshot::Sender<()>>,
    colony_inbox_tx: Option<mpsc::Sender<crate::ColonyMsg>>,
    stop_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    death_ack: Option<tokio::sync::oneshot::Sender<()>>,
    blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
    bounds: meclaw_core::TransferBounds,
) {
    let (events_tx, events_rx) = mpsc::channel::<L::Event>(64);
    let (reconfig_tx, reconfig_rx) = mpsc::channel::<L::Reconfig>(8);

    let mut io_state = cell.split_io();
    // Issue #7: the I/O sub-task gets a liveness mark before it starts, so a
    // stall on the external side becomes visible instead of looking like idle.
    // Cell types that report nothing use the trait's default no-op.
    L::attach_liveness(
        &mut io_state,
        crate::io_liveness::IoLivenessMark::new(own_path.clone(), colony_inbox_tx.clone()),
    );
    let origin_sink =
        meclaw_core::OriginSink::new(outputs_tx.clone(), own_path.clone(), default_origin_ttl);

    let mut io_join = tokio::spawn(<L as crate::long_running_cell::LongRunningCell>::run_io(
        io_state,
        events_tx,
        reconfig_rx,
    ));
    let mut handler_join = tokio::spawn(handler_loop::<L>(
        own_path,
        mailbox,
        outputs_tx,
        origin_sink,
        events_rx,
        reconfig_tx,
        cell,
        db,
        stop_rx,
        colony_inbox_tx,
        death_ack,
        blob_store,
        consumes,
        bounds,
    ));

    // AUDIT-PRE14-001: a panic on EITHER side must reach the supervisor and must
    // NOT depend on which `select!` arm wins. When a handler panics, the unwinding
    // handler frame drops `reconfig_tx`, which closes `run_io` → `io_join` becomes
    // ready `Ok(())` at the same time as the panicking `handler_join`. Symmetrically
    // a `run_io` panic drops `events_tx`, closing the handler loop. So whichever arm
    // wins, we ALWAYS resolve BOTH joins (abort the loser, then await its result)
    // and derive `(panicked, stopped)` from both results — never discarding a peer
    // panic. The `biased; handler_join` order is pure hardening; correctness comes
    // from reading both results, not from the order.
    let (r_io, r_handler): (
        Result<(), tokio::task::JoinError>,
        Result<bool, tokio::task::JoinError>,
    ) = tokio::select! {
        biased;
        r = &mut handler_join => {
            io_join.abort();
            ((&mut io_join).await, r)
        }
        r = &mut io_join => {
            handler_join.abort();
            (r, (&mut handler_join).await)
        }
    };

    // `stopped` is the handler's own peace-stop signal (read before the results are
    // consumed below). Only `Ok(true)` is a deliberate peace-stop.
    let stopped = matches!(&r_handler, Ok(true));

    // Panic precedence, order-independent:
    //  - a handler panic propagates (whether this arm or the io arm won);
    //  - otherwise, a concurrent io panic propagates EXCEPT on the deliberate
    //    peace-stop path (`stopped`), where aborting `io_join` is intentional and
    //    its result is ignored by design (documented exception, pre-fix behaviour
    //    for the stop arm is preserved).
    let panicked = match r_handler {
        Err(e) if e.is_panic() => Some(e.into_panic()),
        Ok(true) => None, // deliberate peace-stop: io abort is intentional, io result ignored
        _ => match r_io {
            Err(e) if e.is_panic() => Some(e.into_panic()),
            _ => None,
        },
    };

    if let Some(p) = panicked {
        std::panic::resume_unwind(p);
    }

    // Peace-stop path: handler returned `true` → fire peace AFTER io_join was
    // aborted so the watcher sees Ok and emits NO `CellDied`.
    if stopped && let Some(p) = peace_tx.take() {
        let _ = p.send(());
    }
}

/// Phase-10 internal: handler loop for `cell_task_long_running`.
///
/// `tokio::select!` over external mailbox (`Message` from topology) and
/// internal events channel (`Event` from I/O sub-task). Cell state lives
/// here single-threaded — no `Mutex`. `reconfig_tx` is passed into
/// `handle()` so the cell can dispatch hints to the I/O sub-task.
///
/// Loop ends when either channel is closed (`recv()` → `None`).
///
/// Returns `true` iff the loop ended via the Phase-13.5 colony-initiated
/// **peace-stop** (`stop_rx` fired). On a stop the loop returns the mailbox
/// `Receiver` to the colony via `ColonyMsg::Stopped` (so the remainder can be
/// drained to the DLQ by Task 4) before breaking. Returns `false` for every
/// other exit (mailbox-close, events-close) — the outer task then takes its
/// pre-3b death path. The stop arm is **biased** (order `stop > mailbox >
/// events`): an in-flight `handle()`/`handle_event()` finishes first (the
/// `select!` only re-evaluates after the await returns); after stop no new
/// mailbox message or event is processed.
#[allow(clippy::too_many_arguments)]
async fn handler_loop<L: crate::long_running_cell::LongRunningCell>(
    own_path: meclaw_core::Path,
    mailbox: mpsc::Receiver<meclaw_core::Message>,
    outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
    origin_sink: meclaw_core::OriginSink,
    mut events_rx: mpsc::Receiver<L::Event>,
    reconfig_tx: mpsc::Sender<L::Reconfig>,
    mut cell: L,
    db: crate::DbConn,
    stop_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    colony_inbox_tx: Option<mpsc::Sender<crate::ColonyMsg>>,
    death_ack: Option<tokio::sync::oneshot::Sender<()>>,
    blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
    bounds: meclaw_core::TransferBounds,
) -> bool {
    // Phase-13.5 Lifecycle-3b (F2): death-ack guard. Declared FIRST → drops
    // LAST, i.e. AFTER the re-bound `db` below (reverse-declaration drop order),
    // hence after `sqlite3_close`. The colony's `death_ack_rx` only fires once
    // `cell.db` is closed.
    let _term_guard = crate::TermAckGuard::new(death_ack);
    // Re-bind `db` into a body-local so it drops BEFORE `_term_guard`.
    let mut db = db;
    // GH #18: the mailbox lives in a guard from here on. This is the frame the
    // outer `select!` ABORTS when the I/O sub-task ends first — dropping the
    // aborted future runs this guard's `Drop`, so the buffered remainder
    // reaches the colony instead of vanishing with the task.
    let mut mailbox = crate::mailbox_rescue::MailboxGuard::new(
        own_path.clone(),
        mailbox,
        colony_inbox_tx.clone(),
    );
    // Fuse the stop receiver so it can be polled in the `select!`. `None` →
    // `pending` (no stop arm ever fires).
    let mut stop_fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        match stop_rx {
            Some(rx) => Box::pin(async move {
                // Only an explicit `stop_tx.send(())` triggers the stop arm. A
                // dropped sender (Err) must NOT fire it — park forever so the
                // task keeps running (the stop_tx may be held elsewhere or
                // legitimately dropped without a stop request).
                if rx.await.is_err() {
                    std::future::pending::<()>().await
                }
            }),
            None => Box::pin(std::future::pending::<()>()),
        };
    // Startup before service: `on_start` runs to completion before the first
    // mailbox message and before the first event. Messages queue in the
    // mailbox meanwhile — that is the point. A cell whose startup depends on
    // its own previous life (crash recovery) cannot get that from the
    // `select!` below, which orders mailbox and events not at all. It takes no
    // I/O signal, so a `run_io` that never speaks cannot wedge the handler.
    cell.on_start(&origin_sink, &mut db).await;
    loop {
        tokio::select! {
            // A4: biased stop > mailbox > events. End peace-free, return the
            // mailbox Receiver via ColonyMsg::Stopped, break with `true`.
            biased;
            _ = &mut stop_fut => {
                // GH #18: peaceful stop — `release()` disarms the guard, this
                // path routes the remainder itself.
                if let (Some(tx), Some(receiver)) = (&colony_inbox_tx, mailbox.release()) {
                    let _ = tx
                        .send(crate::ColonyMsg::Stopped {
                            path: own_path.clone(),
                            receiver,
                        })
                        .await;
                }
                return true;
            }
            mb = mailbox.recv() => match mb {
                Some(mut msg) => {
                    // GH #47: same ticket discipline as the stateful task.
                    let _work = WorkGuard::new(own_path.clone(), colony_inbox_tx.clone());
                    let sink = meclaw_core::OutputSink::new(
                        outputs_tx.clone(),
                        own_path.clone(),
                        msg.id,
                        msg.trace_id,
                        msg.ttl,
                        msg.headers.clone(),
                        msg.reply_to.clone(),
                    );
                    // GH #253: same substrate seam as the stateful task, in the
                    // same position — after the blob resolution, before the
                    // consumes gate, never reaching `handle()`.
                    if resolve_blob_for_delivery(&mut msg, &blob_store, &colony_inbox_tx).await
                        && !crate::db_transfer::handle_transfer_slot(&msg, &sink, &mut db, bounds)
                            .await
                        && enforce_consumes_for_delivery(
                            &msg,
                            &consumes,
                            &sink,
                            &colony_inbox_tx,
                        )
                        .await
                    {
                        cell.handle(msg, &sink, &mut db, &reconfig_tx).await;
                    }
                }
                None => break,
            },
            ev = events_rx.recv() => match ev {
                Some(event) => cell.handle_event(event, &origin_sink, &mut db).await,
                None => break,
            },
        }
    }
    false
}

/// Phase-7: stateless cell dispatcher.
///
/// Long-lived task that pulls messages from the cell's mailbox and spawns a
/// short-lived worker task per message. Concurrency is capped by a
/// `tokio::sync::Semaphore(max_concurrency)`. Workers build their own
/// `OutputSink` from the consumed message and call `cell.handle(msg, &sink)`.
///
/// The dispatcher itself is the registry-visible actor (RespawnFn re-spawns
/// the dispatcher, not workers). Workers are ephemeral — their panics are NOT
/// observed by the supervisor (no `JoinHandle` kept). Disciplinary
/// requirement: `StatelessCell::handle` must be panic-free; all I/O is
/// converted to regular error messages via the cell's emit path.
///
/// `mailbox.recv()` returns `None` when all senders are dropped → dispatcher
/// ends gracefully. Pending workers continue (they own clones of `outputs_tx`
/// and `cell: Arc<F>`).
///
/// `message_timeout` is the Concept-B backstop (Paket-3 P3-B3): each worker
/// wraps its `handle()` call in `tokio::time::timeout(message_timeout, …)`. On
/// elapse the worker emits the generic backstop error to the input's `reply_to`
/// and ENDS (drops its permit) — no restart, no `CellDied`, no peace. The freed
/// permit prevents the dispatcher from wedging at `acquire_owned()` when all
/// workers hang. `None` → no wrapper (behavior-neutral). See module doc.
///
/// Phase-13.5 Lifecycle-3b (F2/A4): `stop_rx` is the colony-initiated peace-stop
/// receiver. When fired (biased over a new mailbox recv), the dispatcher fires
/// `peace_tx` BEFORE the task ends, then returns its mailbox `Receiver` to the
/// colony via `ColonyMsg::Stopped` and ends. Peace-parity with
/// `cell_task_stateful` / `cell_task_long_running`: peace fired → the watcher's
/// `peace_rx.await` resolves `Ok` → NO `CellDied` → `handle_cell_died` never
/// runs → NO `registry.remove` (the entry is preserved, No-Delete). EVERY OTHER
/// exit (mailbox-close → `recv()` yields `None`) leaves `peace_tx` `Some` → it
/// drops unsent → the watcher sees `Err` → `DeathKind::Normal` → `remove`
/// (genuine normal exit, unchanged). In-flight workers are detached and run to
/// completion — there is no drain. Stateless cells have **no `cell.db`**, so the
/// `death_ack` guard (held in this frame) fires immediately on task-end — no
/// close to wait for. `None` for the params → behavior-neutral pre-3b
/// dispatcher.
///
/// `consumes`: pre-compiled required-`consumes` views, Arc-cloned into every
/// worker and enforced at the delivery boundary via
/// `enforce_consumes_for_delivery` (Slice 2, Task 2.4).
#[allow(clippy::too_many_arguments)]
pub async fn stateless_dispatcher<F: crate::stateless_cell::StatelessCell + 'static>(
    own_path: meclaw_core::Path,
    mut mailbox: mpsc::Receiver<meclaw_core::Message>,
    outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
    cell: std::sync::Arc<F>,
    max_concurrency: usize,
    message_timeout: Option<std::time::Duration>,
    mut peace_tx: Option<tokio::sync::oneshot::Sender<()>>,
    stop_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    colony_inbox_tx: Option<mpsc::Sender<crate::ColonyMsg>>,
    death_ack: Option<tokio::sync::oneshot::Sender<()>>,
    blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
    consumes: Option<std::sync::Arc<meclaw_core::CompiledConsumes>>,
) {
    // No cell.db → the death-ack guard simply fires when this frame is dropped
    // (on any dispatcher exit). Bound here so it lives for the whole loop.
    let _term_guard = crate::TermAckGuard::new(death_ack);
    let mut stop_fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        match stop_rx {
            Some(rx) => Box::pin(async move {
                // Only an explicit `stop_tx.send(())` triggers the stop arm. A
                // dropped sender (Err) must NOT fire it — park forever so the
                // task keeps running (the stop_tx may be held elsewhere or
                // legitimately dropped without a stop request).
                if rx.await.is_err() {
                    std::future::pending::<()>().await
                }
            }),
            None => Box::pin(std::future::pending::<()>()),
        };
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrency));
    loop {
        let msg = tokio::select! {
            // A4: biased stop > mailbox recv.
            biased;
            _ = &mut stop_fut => {
                // Peace BEFORE task-end → watcher sees Ok → NO CellDied →
                // handle_cell_died never runs → NO registry.remove (No-Delete,
                // spec Z.1490). Mirror cell_task_stateful (Z.206) / _long_running
                // (Z.424). Every OTHER exit (mailbox-close → recv()==None) leaves
                // `peace_tx` Some → it drops without sending → watcher sees Err →
                // DeathKind::Normal → remove (genuine normal exit, F5, unchanged).
                if let Some(p) = peace_tx.take() {
                    let _ = p.send(());
                }
                // Return the mailbox so the remainder can be drained to the DLQ
                // by Task 4. death_ack fires on return (guard drop).
                if let Some(tx) = &colony_inbox_tx {
                    let _ = tx
                        .send(crate::ColonyMsg::Stopped {
                            path: own_path.clone(),
                            receiver: mailbox,
                        })
                        .await;
                }
                return;
            }
            maybe = mailbox.recv() => match maybe {
                Some(m) => m,
                None => return,
            },
        };
        let permit = sem
            .clone()
            .acquire_owned()
            .await
            .expect("dispatcher semaphore closed");
        // GH #47: built here, in the dispatcher, and moved into the worker.
        // Building it inside the spawned task would leave a window between the
        // `recv()` above and the worker's first line in which the message is
        // neither in the mailbox nor accounted for — and the drain samples
        // asynchronously, so that window is a real race, not a theoretical one.
        let work = WorkGuard::new(own_path.clone(), colony_inbox_tx.clone());
        let outputs_tx = outputs_tx.clone();
        let cell = cell.clone();
        let own_path = own_path.clone();
        let worker_blob_store = blob_store.clone();
        let worker_inbox_tx = colony_inbox_tx.clone();
        let worker_consumes = consumes.clone();
        tokio::spawn(async move {
            let _work = work;
            let mut msg = msg;
            let input_id = msg.id;
            let input_trace = msg.trace_id;
            let input_ttl = msg.ttl;
            let input_headers = msg.headers.clone();
            let input_reply_to = msg.reply_to.clone();
            let sink = meclaw_core::OutputSink::new(
                outputs_tx,
                own_path,
                input_id,
                input_trace,
                input_ttl,
                input_headers,
                msg.reply_to.clone(),
            );
            if resolve_blob_for_delivery(&mut msg, &worker_blob_store, &worker_inbox_tx).await
                && enforce_consumes_for_delivery(&msg, &worker_consumes, &sink, &worker_inbox_tx)
                    .await
            {
                // Concept-B backstop (stateless variant): wrap the WHOLE
                // `handle()` call in `tokio::time::timeout`. Unlike the stateful
                // path there is NO restart, NO CellDied, NO peace — on elapse the
                // worker emits the generic backstop timeout error to the input's
                // `reply_to` and simply ENDS (drops `permit`). The dispatcher
                // keeps running and the freed permit prevents it from wedging at
                // `acquire_owned()` when all workers hang. `None` → no wrapper
                // (behavior-neutral). `timeout` does NOT catch panics — a
                // panicking `handle()` unwinds through the `.await`, but worker
                // panics are ephemeral and not observed by the supervisor (only
                // the dispatcher is supervised).
                match message_timeout {
                    Some(t) => {
                        if tokio::time::timeout(t, cell.handle(msg, &sink))
                            .await
                            .is_err()
                        {
                            emit_backstop_timeout(&sink, input_reply_to).await;
                        }
                    }
                    None => {
                        cell.handle(msg, &sink).await;
                    }
                }
            }
            drop(permit);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::{MessageBuilder, Path};
    use meclaw_testing::mocks::EchoMockCell;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_dispatches_to_echo() {
        let (in_tx, in_rx) = mpsc::channel(8);
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let cell = EchoMockCell::new(Path::new("/a")).emitted_target(Path::new("/b"));
        let _join = tokio::spawn(cell_task(
            Path::new("/a"),
            in_rx,
            out_tx,
            cell,
            None,
            None,
            None,
        ));

        in_tx
            .send(MessageBuilder::new(Path::new("/a")).build())
            .await
            .unwrap();

        // With echo forward disabled (task 12), just verify no panic occurs.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    /// GH #47: the plain task reports too. `meclaw-testing`'s echo factory runs
    /// on it behind a real colony, so a silent `cell_task` would make every one
    /// of those colonies drain to its deadline.
    #[tokio::test]
    async fn cell_task_returns_its_ticket_after_handle() {
        use std::future::Future;

        struct EchoCellForTest;
        impl Cell for EchoCellForTest {
            #[allow(clippy::manual_async_fn)]
            fn handle(
                &mut self,
                _msg: Message,
                sink: &OutputSink,
            ) -> impl Future<Output = ()> + Send {
                async move {
                    let _ = sink
                        .push(meclaw_core::CellOutput {
                            target: Path::new("/out"),
                            content: meclaw_core::JsonValue::Bool(true),
                        })
                        .await;
                }
            }
        }

        let (in_tx, in_rx) = mpsc::channel::<Message>(4);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(4);
        let (colony_tx, mut colony_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let _join = tokio::spawn(cell_task(
            Path::new("/a"),
            in_rx,
            out_tx,
            EchoCellForTest,
            None,
            None,
            Some(colony_tx),
        ));
        in_tx
            .send(MessageBuilder::new(Path::new("/a")).build())
            .await
            .unwrap();
        let _em = out_rx.recv().await.expect("emission");
        match colony_rx.recv().await.expect("ticket") {
            crate::ColonyMsg::WorkDone { path } => assert_eq!(path.as_str(), "/a"),
            _ => panic!("expected WorkDone"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_constructs_sink_with_parent_context_per_message() {
        use meclaw_core::{
            Cell, CellEmission, CellOutput, JsonValue, MessageBuilder, OutputSink, Uuid,
        };
        use std::future::Future;

        struct PushOnce;
        impl Cell for PushOnce {
            #[allow(clippy::manual_async_fn)]
            fn handle(
                &mut self,
                _msg: meclaw_core::Message,
                sink: &OutputSink,
            ) -> impl Future<Output = ()> + Send {
                async move {
                    let _ = sink
                        .push(CellOutput {
                            target: Path::new("/out"),
                            content: JsonValue::Bool(true),
                        })
                        .await;
                }
            }
        }

        let (in_tx, in_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let _join = tokio::spawn(cell_task(
            Path::new("/me"),
            in_rx,
            out_tx,
            PushOnce,
            None,
            None,
            None,
        ));

        let msg = MessageBuilder::new(Path::new("/me"))
            .trace_id(Uuid::now_v7())
            .ttl(42)
            .build();
        let input_id = msg.id;
        let input_trace = msg.trace_id;
        in_tx.send(msg).await.unwrap();

        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.sender_path.as_str(), "/me");
        assert_eq!(em.parent_message_id, Some(input_id));
        assert_eq!(em.trace_id, input_trace);
        assert_eq!(
            em.input_ttl, 42,
            "ttl from consumed message carries to emission"
        );
        assert_eq!(em.target.as_str(), "/out");
        assert_eq!(em.content, JsonValue::Bool(true));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cell_task_stateful_passes_dbconn_to_handle() {
        use crate::stateful_cell::StatefulCell;

        struct Counting {
            n: std::sync::atomic::AtomicI64,
        }

        impl StatefulCell for Counting {
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a mut self,
                _msg: meclaw_core::Message,
                _sink: &'a meclaw_core::OutputSink,
                db: &'a mut crate::DbConn,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {
                    db.call(|c| {
                        c.execute("CREATE TABLE IF NOT EXISTS t (n INTEGER)", [])
                            .unwrap();
                        c.execute("INSERT INTO t VALUES (1)", []).unwrap();
                    })
                    .await;
                    self.n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }

        let (tx, rx) = mpsc::channel::<meclaw_core::Message>(8);
        let (otx, _orx) = mpsc::channel(8);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = crate::DbConn::wrap(conn, None);
        let cell = Counting { n: 0.into() };
        let join = tokio::spawn(cell_task_stateful(
            meclaw_core::Path::new("/x"),
            rx,
            otx,
            cell,
            db,
            None, // _idle_timeout
            None, // _message_timeout
            None, // _peace_tx
            None, // backstop_tx
            None, // _colony_inbox_tx
            0,    // _cell_timeout
            None, // _stop_rx
            None, // _death_ack
            None, // _blob_store
            None,
            Default::default(),
        ));
        let msg = meclaw_core::MessageBuilder::new(meclaw_core::Path::new("/x")).build();
        tx.send(msg).await.unwrap();
        drop(tx);
        join.await.unwrap();
    }

    /// GH #47: a stateful cell returns its ticket after `handle()`, and not
    /// before. The receipt is ordering: the emission is already in the outputs
    /// channel when the `WorkDone` arrives, which is what makes the drain's
    /// "outputs empty AND ledger zero" conjunction sound.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_stateful_returns_its_ticket_after_handle() {
        use crate::stateful_cell::StatefulCell;

        struct EchoStateful;
        impl StatefulCell for EchoStateful {
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a mut self,
                _msg: meclaw_core::Message,
                sink: &'a meclaw_core::OutputSink,
                _db: &'a mut crate::DbConn,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {
                    let _ = sink
                        .push(meclaw_core::CellOutput {
                            target: Path::new("/out"),
                            content: meclaw_core::JsonValue::Bool(true),
                        })
                        .await;
                }
            }
        }

        let (in_tx, in_rx) = mpsc::channel::<Message>(4);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(4);
        let (colony_tx, mut colony_rx) = mpsc::channel::<crate::ColonyMsg>(16);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = crate::DbConn::wrap(conn, None);

        let join = tokio::spawn(cell_task_stateful(
            Path::new("/s"),
            in_rx,
            out_tx,
            EchoStateful,
            db,
            None, // idle_timeout
            None, // message_timeout
            None, // peace_tx
            None, // backstop_tx
            Some(colony_tx),
            0,    // cell_timeout
            None, // stop_rx
            None, // death_ack
            None, // blob_store
            None, // consumes
            Default::default(),
        ));

        in_tx
            .send(MessageBuilder::new(Path::new("/s")).build())
            .await
            .unwrap();

        let em = tokio::time::timeout(std::time::Duration::from_secs(30), out_rx.recv())
            .await
            .expect("the cell must emit")
            .expect("emission");
        assert_eq!(em.sender_path.as_str(), "/s");

        // The emission is already out; only THEN may the ticket come back.
        let done = tokio::time::timeout(std::time::Duration::from_secs(30), colony_rx.recv())
            .await
            .expect("the ticket must come back")
            .expect("colony message");
        match done {
            crate::ColonyMsg::WorkDone { path } => assert_eq!(path.as_str(), "/s"),
            _ => panic!("expected WorkDone"),
        }
        join.abort();
    }

    /// Paket-3 P3-B2 / P3-B-restart: the Concept-B backstop fires when `handle()`
    /// hangs longer than `message_timeout`. The task must END (join completes),
    /// `peace_rx` must resolve to `Err` (NOT the silent peace path), and
    /// `backstop_rx` MUST receive `()` — i.e. the watcher classifies this death
    /// as `DeathKind::Backstop` → restart, not `Normal` → removal. The backstop
    /// error lands on the outputs channel carrying `error_code:"message_timeout"`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_stateful_backstop_fires_ends_task_without_peace() {
        use crate::stateful_cell::StatefulCell;
        use meclaw_core::Path;

        // Cell whose handle() hangs forever — only the backstop can end it.
        struct Hanger;
        impl StatefulCell for Hanger {
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a mut self,
                _msg: meclaw_core::Message,
                _sink: &'a meclaw_core::OutputSink,
                _db: &'a mut crate::DbConn,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {
                    std::future::pending::<()>().await;
                }
            }
        }

        let (mb_tx, mb_rx) = mpsc::channel::<meclaw_core::Message>(8);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let (peace_tx, mut peace_rx) = tokio::sync::oneshot::channel::<()>();
        let (backstop_tx, mut backstop_rx) = tokio::sync::oneshot::channel::<()>();
        let (inbox_tx, _inbox_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = crate::DbConn::wrap(conn, None);

        let join = tokio::spawn(cell_task_stateful(
            Path::new("/probe"),
            mb_rx,
            out_tx,
            Hanger,
            db,
            None,                                       // idle_timeout
            Some(std::time::Duration::from_millis(50)), // message_timeout
            Some(peace_tx),
            Some(backstop_tx),
            Some(inbox_tx),
            0,    // cell_timeout
            None, // stop_rx
            None, // death_ack
            None, // blob_store
            None,
            Default::default(),
        ));

        // Message with an explicit reply_to so the backstop error has somewhere
        // to go.
        let msg = meclaw_core::MessageBuilder::new(Path::new("/probe"))
            .reply_to(Path::new("/back"))
            .build();
        mb_tx.send(msg).await.unwrap();

        // The task ends (join completes) — only the backstop can end a Hanger.
        tokio::time::timeout(std::time::Duration::from_secs(5), join)
            .await
            .expect("task must END within 5s (backstop)")
            .expect("task returned cleanly (no panic)");

        // peace was NOT sent: peace_tx dropped on task-end → Err. THIS is the
        // CellDied death path, not the silent peace path.
        assert!(
            peace_rx.try_recv().is_err(),
            "peace must NOT be sent on backstop — watcher must see Err → CellDied → restart"
        );

        // backstop WAS sent: the watcher will classify this death as
        // `DeathKind::Backstop` → `handle_cell_died` → restart (P3-B-restart). A
        // clean `return` WITHOUT this signal would be `Normal` → removal.
        assert!(
            backstop_rx.try_recv().is_ok(),
            "backstop_tx must be fired on a message_timeout elapse → DeathKind::Backstop → restart"
        );

        // The backstop error landed on the outputs channel.
        let em = out_rx
            .try_recv()
            .expect("backstop timeout error must be emitted");
        assert_eq!(em.target.as_str(), "/back");
        assert_eq!(em.content["header"]["finish_reason"], "error");
        assert_eq!(em.content["header"]["error_code"], "message_timeout");
        // The emitted body validates as UBF (origin/type/text discipline).
        meclaw_core::validate_ubf_body(&em.content)
            .expect("backstop error body must be valid UBF (else Colony would DLQ it)");
    }

    /// Paket-3 P3-B2 (AUDIT-PRE14-001): `tokio::time::timeout` does NOT catch
    /// panics. A `handle()` that panics WITH a `message_timeout` set must still
    /// propagate the panic → the task's `JoinHandle` is `is_panic()` →
    /// `CellDied{was_panic:true}`. No `catch_unwind` anywhere.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_stateful_panic_propagates_with_message_timeout_set() {
        use crate::stateful_cell::StatefulCell;
        use meclaw_core::Path;

        struct Panicker;
        impl StatefulCell for Panicker {
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a mut self,
                _msg: meclaw_core::Message,
                _sink: &'a meclaw_core::OutputSink,
                _db: &'a mut crate::DbConn,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {
                    panic!("handle() panic with backstop set");
                }
            }
        }

        let (mb_tx, mb_rx) = mpsc::channel::<meclaw_core::Message>(8);
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = crate::DbConn::wrap(conn, None);

        let join = tokio::spawn(cell_task_stateful(
            Path::new("/probe"),
            mb_rx,
            out_tx,
            Panicker,
            db,
            None,                                       // idle_timeout
            Some(std::time::Duration::from_secs(3600)), // message_timeout (large)
            None,                                       // peace_tx
            None,                                       // backstop_tx
            None,                                       // colony_inbox_tx
            0,                                          // cell_timeout
            None,                                       // stop_rx
            None,                                       // death_ack
            None,                                       // blob_store
            None,
            Default::default(),
        ));

        mb_tx
            .send(meclaw_core::MessageBuilder::new(Path::new("/probe")).build())
            .await
            .unwrap();

        let r = tokio::time::timeout(std::time::Duration::from_secs(5), join)
            .await
            .expect("task must end within 5s");
        let err = r.expect_err("task must have panicked, not returned cleanly");
        assert!(
            err.is_panic(),
            "panic in handle() must propagate as task panic (was_panic:true) even with message_timeout set"
        );
    }

    /// Paket-3 P3-B2: `message_timeout = None` is behavior-neutral — a cell whose
    /// handle() takes ~100ms completes normally and the loop continues; no early
    /// death, no backstop emission.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_stateful_none_message_timeout_does_not_wrap() {
        use crate::stateful_cell::StatefulCell;
        use meclaw_core::{CellOutput, JsonValue, MessageBuilder, OutputSink, Path};

        struct SlowEmit;
        impl StatefulCell for SlowEmit {
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a mut self,
                _msg: meclaw_core::Message,
                sink: &'a OutputSink,
                _db: &'a mut crate::DbConn,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let _ = sink
                        .push(CellOutput {
                            target: Path::new("/out"),
                            content: JsonValue::Bool(true),
                        })
                        .await;
                }
            }
        }

        let (mb_tx, mb_rx) = mpsc::channel::<meclaw_core::Message>(8);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = crate::DbConn::wrap(conn, None);

        let join = tokio::spawn(cell_task_stateful(
            Path::new("/probe"),
            mb_rx,
            out_tx,
            SlowEmit,
            db,
            None, // idle_timeout
            None, // message_timeout — no wrapper
            None, // peace_tx
            None, // backstop_tx
            None, // colony_inbox_tx
            0,    // cell_timeout
            None, // stop_rx
            None, // death_ack
            None, // blob_store
            None,
            Default::default(),
        ));

        mb_tx
            .send(MessageBuilder::new(Path::new("/probe")).build())
            .await
            .unwrap();

        // The cell's own emission (not a backstop error) arrives normally.
        let em = tokio::time::timeout(std::time::Duration::from_secs(5), out_rx.recv())
            .await
            .expect("emission within 5s")
            .expect("emission present");
        assert_eq!(em.target.as_str(), "/out");
        assert_eq!(em.content, JsonValue::Bool(true));

        // Mailbox-close ends the loop cleanly — no early death.
        drop(mb_tx);
        tokio::time::timeout(std::time::Duration::from_secs(5), join)
            .await
            .expect("task ends on mailbox-close")
            .expect("task returned cleanly");
    }

    /// Phase-13 step 13-H-2: idle-arm fires after `idle_timeout` elapsed on an
    /// empty mailbox — peace oneshot fires, `ColonyMsg::Sleep` lands in colony
    /// inbox, task returns. Verifies the self-despawn sequence on a real
    /// stateful cell with an in-memory DbConn.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_stateful_idle_fires_peace_and_sleep() {
        use crate::stateful_cell::StatefulCell;
        use meclaw_core::Path;

        struct NoopStateful;
        impl StatefulCell for NoopStateful {
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a mut self,
                _msg: meclaw_core::Message,
                _sink: &'a meclaw_core::OutputSink,
                _db: &'a mut crate::DbConn,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {}
            }
        }

        let (_mb_tx, mb_rx) = mpsc::channel::<meclaw_core::Message>(8);
        let (out_tx, _out_rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
        let (peace_tx, mut peace_rx) = tokio::sync::oneshot::channel::<()>();
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = crate::DbConn::wrap(conn, None);
        let join = tokio::spawn(cell_task_stateful(
            Path::new("/probe"),
            mb_rx,
            out_tx,
            NoopStateful,
            db,
            Some(std::time::Duration::from_millis(80)),
            None, // message_timeout
            Some(peace_tx),
            None, // backstop_tx
            Some(inbox_tx),
            0,
            None, // stop_rx
            None, // death_ack
            None, // blob_store
            None,
            Default::default(),
        ));

        let peace =
            tokio::time::timeout(std::time::Duration::from_millis(300), &mut peace_rx).await;
        assert!(
            peace.is_ok() && peace.unwrap().is_ok(),
            "peace must fire on idle elapsed with empty mailbox"
        );
        let sleep_msg =
            tokio::time::timeout(std::time::Duration::from_millis(100), inbox_rx.recv()).await;
        assert!(matches!(
            sleep_msg,
            Ok(Some(crate::ColonyMsg::Sleep { .. }))
        ));
        join.await.unwrap();
    }

    /// Phase-13 step 13-D-1: regression test that `cell_task_stateful` with
    /// `None`-defaults behaves identical to the Phase-12 loop (one message in →
    /// one emission out → mailbox-close ends the loop). Proves the four new
    /// `_`-prefixed Phase-13 params (`_idle_timeout`, `_peace_tx`,
    /// `_colony_inbox_tx`, `_cell_timeout`) are behavior-neutral defaults.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_stateful_none_defaults_runs_until_mailbox_close() {
        use crate::stateful_cell::StatefulCell;
        use meclaw_core::{CellEmission, CellOutput, JsonValue, MessageBuilder, OutputSink, Path};

        struct EmitOnce;
        impl StatefulCell for EmitOnce {
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a mut self,
                _msg: meclaw_core::Message,
                sink: &'a OutputSink,
                _db: &'a mut crate::DbConn,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {
                    let _ = sink
                        .push(CellOutput {
                            target: Path::new("/out"),
                            content: JsonValue::Bool(true),
                        })
                        .await;
                }
            }
        }

        let (mb_tx, mb_rx) = mpsc::channel::<meclaw_core::Message>(8);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = crate::DbConn::wrap(conn, None);
        let join = tokio::spawn(cell_task_stateful(
            Path::new("/probe"),
            mb_rx,
            out_tx,
            EmitOnce,
            db,
            None, // _idle_timeout
            None, // _message_timeout
            None, // _peace_tx
            None, // backstop_tx
            None, // _colony_inbox_tx
            0,    // _cell_timeout
            None, // _stop_rx
            None, // _death_ack
            None, // _blob_store
            None,
            Default::default(),
        ));
        mb_tx
            .send(MessageBuilder::new(Path::new("/probe")).build())
            .await
            .unwrap();
        let _ = out_rx.recv().await.expect("emission");
        drop(mb_tx);
        join.await.unwrap();
    }

    /// Phase-13.5 Lifecycle-3b step-3.3 (A2): colony-initiated peace-stop on a
    /// stateful cell. The colony fires `stop_tx.send(())`; the task finishes its
    /// in-flight `handle()`, then the biased stop-arm wins over a new recv → it
    /// fires `peace_tx` and returns the mailbox `Receiver` via
    /// `ColonyMsg::Stopped` (NOT `Sleep`). After Disconnect NO new mailbox
    /// message is processed — the remainder lands unread in `Stopped`'s
    /// receiver. Exactly one peace, no `CellDied` (watcher would see peace=Ok).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_stateful_stop_fires_peace_and_returns_receiver_with_remainder() {
        use crate::stateful_cell::StatefulCell;
        use meclaw_core::Path;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Cell whose handle() blocks on `gate` until the test releases it, then
        // bumps `handled`. Lets the test queue messages and fire stop while a
        // handle() is in flight, deterministically.
        struct Gated {
            handled: Arc<AtomicUsize>,
            gate: Arc<tokio::sync::Semaphore>,
            entered: Arc<tokio::sync::Semaphore>,
        }
        impl StatefulCell for Gated {
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a mut self,
                _msg: meclaw_core::Message,
                _sink: &'a meclaw_core::OutputSink,
                _db: &'a mut crate::DbConn,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {
                    self.entered.add_permits(1);
                    let _p = self.gate.acquire().await.unwrap();
                    self.handled.fetch_add(1, Ordering::SeqCst);
                }
            }
        }

        let (mb_tx, mb_rx) = mpsc::channel::<meclaw_core::Message>(8);
        let (out_tx, _out_rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
        let (peace_tx, mut peace_rx) = tokio::sync::oneshot::channel::<()>();
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let handled = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let entered = Arc::new(tokio::sync::Semaphore::new(0));
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = crate::DbConn::wrap(conn, None);

        let join = tokio::spawn(cell_task_stateful(
            Path::new("/probe"),
            mb_rx,
            out_tx,
            Gated {
                handled: handled.clone(),
                gate: gate.clone(),
                entered: entered.clone(),
            },
            db,
            None, // idle_timeout
            None, // message_timeout
            Some(peace_tx),
            None, // backstop_tx
            Some(inbox_tx),
            0, // cell_timeout
            Some(stop_rx),
            None, // death_ack
            None, // blob_store
            None,
            Default::default(),
        ));

        // Queue 2 messages. The first is picked up and blocks in handle().
        mb_tx
            .send(meclaw_core::MessageBuilder::new(Path::new("/probe")).build())
            .await
            .unwrap();
        mb_tx
            .send(meclaw_core::MessageBuilder::new(Path::new("/probe")).build())
            .await
            .unwrap();

        // Wait until handle() has actually entered (in-flight).
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), entered.acquire())
            .await
            .expect("handle() must enter within 5s")
            .unwrap();

        // Fire stop while handle() is in flight, then release the gate so the
        // in-flight handle() completes. Biased stop-arm then wins over a new
        // recv → no second handle().
        stop_tx.send(()).unwrap();
        gate.add_permits(8);

        // Exactly one peace signal.
        let peace = tokio::time::timeout(std::time::Duration::from_secs(5), &mut peace_rx).await;
        assert!(
            peace.is_ok() && peace.unwrap().is_ok(),
            "peace must fire on stop"
        );

        // Colony inbox gets `Stopped` (NOT `Sleep`), carrying the receiver with
        // the unprocessed remainder. GH #47: the in-flight delivery returns its
        // `WorkDone` ticket first — skip those, the obligation here is that
        // exactly one `Stopped` carrying the receiver arrives.
        let mut returned_rx = loop {
            let inbox_msg =
                tokio::time::timeout(std::time::Duration::from_secs(5), inbox_rx.recv())
                    .await
                    .expect("Stopped must arrive within 5s")
                    .expect("inbox open");
            match inbox_msg {
                crate::ColonyMsg::WorkDone { path } => {
                    assert_eq!(path.as_str(), "/probe");
                }
                crate::ColonyMsg::Stopped { path, receiver } => {
                    assert_eq!(path.as_str(), "/probe");
                    break receiver;
                }
                other => panic!(
                    "expected ColonyMsg::Stopped, got a different variant: {:?}",
                    std::mem::discriminant(&other)
                ),
            }
        };

        // Task ended cleanly (returned, not panicked).
        tokio::time::timeout(std::time::Duration::from_secs(5), join)
            .await
            .expect("task must return within 5s")
            .expect("task returned cleanly");

        // At most the in-flight message completed; the rest is unread in the
        // returned receiver.
        let done = handled.load(Ordering::SeqCst);
        assert!(
            done <= 1,
            "at most the in-flight handle() completes, got {done}"
        );
        // The remainder (2 - done) is drainable from the returned receiver.
        let mut remainder = 0;
        while returned_rx.try_recv().is_ok() {
            remainder += 1;
        }
        assert_eq!(
            done + remainder,
            2,
            "all messages accounted for (handled + remainder)"
        );
    }

    /// GH #47, long-running form: the handler half returns its ticket. The I/O
    /// half never takes one — it is a source, not a delivery. The count is exact
    /// because the drain runs to the end of the inbox: the task is joined first,
    /// so every sender is gone and `recv()` terminates.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_long_running_returns_its_ticket_after_handle() {
        use crate::long_running_cell::LongRunningCell;
        use meclaw_core::{OriginSink, OutputSink, Path};
        use std::future::Future;
        use std::sync::Arc;
        use tokio::sync::Semaphore;

        // One delivered message and three I/O events. `run_io` parks after its
        // three sends so the events channel stays open — the loop then ends on
        // the mailbox close, after both rendezvous have fired.
        struct Sourcey {
            handled: Arc<Semaphore>,
            events_seen: Arc<Semaphore>,
        }
        struct SourceIo;
        impl LongRunningCell for Sourcey {
            type Event = ();
            type Reconfig = ();
            type Io = SourceIo;
            fn split_io(&mut self) -> Self::Io {
                SourceIo
            }
            #[allow(clippy::manual_async_fn)]
            fn run_io(
                _io: Self::Io,
                events_tx: mpsc::Sender<Self::Event>,
                mut reconfig_rx: mpsc::Receiver<Self::Reconfig>,
            ) -> impl Future<Output = ()> + Send {
                async move {
                    for _ in 0..3 {
                        if events_tx.send(()).await.is_err() {
                            return;
                        }
                    }
                    // Park until the handler frame drops `reconfig_tx`.
                    while reconfig_rx.recv().await.is_some() {}
                }
            }
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a mut self,
                _msg: meclaw_core::Message,
                _sink: &'a OutputSink,
                _db: &'a mut crate::DbConn,
                _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
            ) -> impl Future<Output = ()> + Send + 'a {
                async move {
                    self.handled.add_permits(1);
                }
            }
            #[allow(clippy::manual_async_fn)]
            fn handle_event<'a>(
                &'a mut self,
                _event: Self::Event,
                _sink: &'a OriginSink,
                _db: &'a mut crate::DbConn,
            ) -> impl Future<Output = ()> + Send + 'a {
                async move {
                    self.events_seen.add_permits(1);
                }
            }
        }

        let handled = Arc::new(Semaphore::new(0));
        let events_seen = Arc::new(Semaphore::new(0));
        let (mb_tx, mb_rx) = mpsc::channel::<Message>(8);
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let (colony_tx, mut colony_rx) = mpsc::channel::<crate::ColonyMsg>(32);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = crate::DbConn::wrap(conn, None);

        let join = tokio::spawn(cell_task_long_running(
            Path::new("/lr"),
            mb_rx,
            out_tx,
            64,
            Sourcey {
                handled: handled.clone(),
                events_seen: events_seen.clone(),
            },
            db,
            None,            // peace_tx
            Some(colony_tx), // colony_inbox_tx
            None,            // stop_rx
            None,            // death_ack
            None,            // blob_store
            None,            // consumes
            Default::default(),
        ));

        mb_tx
            .send(MessageBuilder::new(Path::new("/lr")).build())
            .await
            .unwrap();

        // Both rendezvous before the mailbox closes: the delivery ran, and so
        // did all three events. Whatever tickets exist are drawn by now.
        let _h = tokio::time::timeout(std::time::Duration::from_secs(30), handled.acquire())
            .await
            .expect("handle() must run")
            .expect("semaphore open");
        let _e = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            events_seen.acquire_many(3),
        )
        .await
        .expect("all three events must be handled")
        .expect("semaphore open");

        drop(mb_tx);
        tokio::time::timeout(std::time::Duration::from_secs(30), join)
            .await
            .expect("the task must end on mailbox close")
            .expect("task returned cleanly");

        let mut tickets = 0;
        while let Some(msg) = colony_rx.recv().await {
            if let crate::ColonyMsg::WorkDone { path } = msg {
                assert_eq!(path.as_str(), "/lr");
                tickets += 1;
            }
        }
        assert_eq!(
            tickets, 1,
            "exactly the one delivery draws a ticket — the three I/O events draw none"
        );
    }

    /// Phase-13.5 Lifecycle-3b step-3.5 (A4): colony-initiated peace-stop on a
    /// long-running (dual-task) cell. The `stop_rx` lives inside `handler_loop`
    /// (which owns the mailbox); on stop it returns the mailbox `Receiver` via
    /// `ColonyMsg::Stopped`, breaks, and ends gracefully. The outer task sees
    /// `handler_join` finished, aborts ONLY `io_join` (abort → Cancelled, not
    /// `is_panic` → no `resume_unwind`), fires `peace_tx` and returns.
    ///
    /// Proof obligations: (1) the self-driving io polling provably ends (counter
    /// freezes after stop); (2) exactly one `Stopped` carrying the receiver;
    /// (3) peace fired (→ watcher Ok → NO `CellDied`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_long_running_stop_freezes_io_fires_peace_returns_receiver() {
        use crate::long_running_cell::LongRunningCell;
        use meclaw_core::{OriginSink, OutputSink, Path};
        use std::future::Future;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Self-driving timer-like long-running cell: run_io bumps `polls` on a
        // short interval until its channels close (abort). `handle()` blocks on
        // a gate so the test can fire stop while a handle() is in flight.
        struct TimerLike {
            polls: Arc<AtomicUsize>,
            handled: Arc<AtomicUsize>,
            gate: Arc<tokio::sync::Semaphore>,
            entered: Arc<tokio::sync::Semaphore>,
        }
        struct TimerIo {
            polls: Arc<AtomicUsize>,
        }
        impl LongRunningCell for TimerLike {
            type Event = ();
            type Reconfig = ();
            type Io = TimerIo;
            fn split_io(&mut self) -> Self::Io {
                TimerIo {
                    polls: self.polls.clone(),
                }
            }
            #[allow(clippy::manual_async_fn)]
            fn run_io(
                io: Self::Io,
                events_tx: mpsc::Sender<Self::Event>,
                mut reconfig_rx: mpsc::Receiver<Self::Reconfig>,
            ) -> impl Future<Output = ()> + Send {
                async move {
                    loop {
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
                                io.polls.fetch_add(1, Ordering::SeqCst);
                                // Keep events_tx alive (don't let the channel close).
                                if events_tx.is_closed() { break; }
                            }
                            rc = reconfig_rx.recv() => {
                                if rc.is_none() { break; }
                            }
                        }
                    }
                }
            }
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a mut self,
                _msg: meclaw_core::Message,
                _sink: &'a OutputSink,
                _db: &'a mut crate::DbConn,
                _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
            ) -> impl Future<Output = ()> + Send + 'a {
                async move {
                    self.entered.add_permits(1);
                    let _p = self.gate.acquire().await.unwrap();
                    self.handled.fetch_add(1, Ordering::SeqCst);
                }
            }
            #[allow(clippy::manual_async_fn)]
            fn handle_event<'a>(
                &'a mut self,
                _event: Self::Event,
                _sink: &'a OriginSink,
                _db: &'a mut crate::DbConn,
            ) -> impl Future<Output = ()> + Send + 'a {
                async move {}
            }
        }

        let polls = Arc::new(AtomicUsize::new(0));
        let handled = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let entered = Arc::new(tokio::sync::Semaphore::new(0));
        let (mb_tx, mb_rx) = mpsc::channel::<meclaw_core::Message>(8);
        let (out_tx, _out_rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
        let (peace_tx, mut peace_rx) = tokio::sync::oneshot::channel::<()>();
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = tokio::sync::oneshot::channel::<()>();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = crate::DbConn::wrap(conn, None);

        let join = tokio::spawn(cell_task_long_running(
            Path::new("/lr"),
            mb_rx,
            out_tx,
            64,
            TimerLike {
                polls: polls.clone(),
                handled: handled.clone(),
                gate: gate.clone(),
                entered: entered.clone(),
            },
            db,
            Some(peace_tx),
            Some(inbox_tx),
            Some(stop_rx),
            Some(death_ack_tx),
            None,
            None,
            Default::default(),
        ));

        // Queue a first message (blocks in handle() on the gate) and a second
        // remainder message that must NOT be processed after stop.
        mb_tx
            .send(meclaw_core::MessageBuilder::new(Path::new("/lr")).build())
            .await
            .unwrap();
        mb_tx
            .send(meclaw_core::MessageBuilder::new(Path::new("/lr")).build())
            .await
            .unwrap();

        // Wait until the first handle() is in flight (blocked on the gate).
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), entered.acquire())
            .await
            .expect("handle() must enter within 5s")
            .unwrap();

        // Wait until io has provably polled at least once (the freeze-after-stop
        // assertion below only means something if io was running).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while polls.load(Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline, "io never polled");
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        // Fire stop while handle() is in flight, then release the gate so the
        // in-flight handle() completes. Biased stop-arm then wins over a new
        // recv → the second message stays unprocessed.
        stop_tx.send(()).unwrap();
        gate.add_permits(8);

        // Peace must fire.
        let peace = tokio::time::timeout(std::time::Duration::from_secs(5), &mut peace_rx).await;
        assert!(
            peace.is_ok() && peace.unwrap().is_ok(),
            "peace must fire on long-running stop"
        );

        // Outer task returns cleanly (graceful, not panic).
        tokio::time::timeout(std::time::Duration::from_secs(5), join)
            .await
            .expect("task must return within 5s")
            .expect("task returned cleanly (not panicked)");

        // death_ack fires after cell.db close (TermAckGuard in handler_loop frame).
        let ack = tokio::time::timeout(std::time::Duration::from_secs(5), death_ack_rx).await;
        assert!(
            ack.is_ok() && ack.unwrap().is_ok(),
            "death_ack must fire after cell.db close on long-running stop"
        );

        // io polling provably froze: snapshot after task end, wait, re-snapshot.
        let frozen = polls.load(Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        assert_eq!(
            polls.load(Ordering::SeqCst),
            frozen,
            "io polling must be frozen after stop (io_join aborted)"
        );

        // Exactly one Stopped carrying the receiver with the remainder. GH #47:
        // the in-flight delivery's ticket sits ahead of it — tickets are
        // bookkeeping, the obligation here is one Stopped and nothing else.
        let mut returned_rx: Option<mpsc::Receiver<meclaw_core::Message>> = None;
        while let Ok(inbox_msg) = inbox_rx.try_recv() {
            match inbox_msg {
                crate::ColonyMsg::WorkDone { path } => assert_eq!(path.as_str(), "/lr"),
                crate::ColonyMsg::Stopped { path, receiver } => {
                    assert_eq!(path.as_str(), "/lr");
                    assert!(
                        returned_rx.is_none(),
                        "exactly one inbox message (Stopped), no Sleep/extra"
                    );
                    returned_rx = Some(receiver);
                }
                _ => panic!("expected ColonyMsg::Stopped"),
            }
        }
        let mut returned_rx = returned_rx.expect("Stopped must have been sent before task end");
        // At most the in-flight handle() completed; the rest is unread.
        let done = handled.load(Ordering::SeqCst);
        assert!(
            done <= 1,
            "at most the in-flight handle() completes, got {done}"
        );
        let mut remainder = 0;
        while returned_rx.try_recv().is_ok() {
            remainder += 1;
        }
        assert_eq!(
            done + remainder,
            2,
            "all messages accounted for (handled + remainder)"
        );
        assert!(remainder >= 1, "the post-stop message stays unprocessed");
    }

    // Cap-Substrat-Invariante via deterministischem Rendezvous (umgebaut 2026-05-22,
    // pre-phase-9 cleanup). Two async-aware counting semaphores replace the
    // original 5s busy-wait polling loop:
    //
    // 1. `arrival` rendezvous: every worker does `add_permits(1)` AFTER
    //    fetch_add+fetch_max. The test waits via `acquire_many(2)`. SeqCst ordering
    //    guarantees: if the test gets through, two workers are past fetch_add →
    //    max_observed >= 2. 3 messages at max_concurrency=2 test the cap's upper
    //    bound: a wrongly admitted concurrent 3rd worker would have done fetch_add
    //    gemacht → max_observed==3 → assert failt.
    //
    // 2. `completion` rendezvous: every worker does `add_permits(1)` AFTER
    //    fetch_sub. The test waits via `acquire_many(3)`. Necessary because worker
    //    tasks are `tokio::spawn`'d by the dispatcher and detached (no JoinHandle,
    //    ephemeral by design — see the module docs above). `join.await` joins ONLY
    //    the dispatcher, not the workers. Without this sync the drain assert
    //    `concurrent_now==0` would race a late worker that has not done fetch_sub yet.
    /// GH #47, stateless form: every one of N concurrent workers returns its own
    /// ticket. This is the fan-out case the roadmap called out as subtle.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stateless_dispatcher_returns_one_ticket_per_message() {
        use crate::stateless_cell::StatelessCell;

        struct EchoStateless;
        impl StatelessCell for EchoStateless {
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a self,
                _msg: meclaw_core::Message,
                sink: &'a meclaw_core::OutputSink,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {
                    let _ = sink
                        .push(meclaw_core::CellOutput {
                            target: Path::new("/out"),
                            content: meclaw_core::JsonValue::Bool(true),
                        })
                        .await;
                }
            }
        }

        let (in_tx, in_rx) = mpsc::channel::<Message>(8);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let (colony_tx, mut colony_rx) = mpsc::channel::<crate::ColonyMsg>(32);

        let join = tokio::spawn(stateless_dispatcher(
            Path::new("/d"),
            in_rx,
            out_tx,
            std::sync::Arc::new(EchoStateless),
            4,               // max concurrency
            None,            // message_timeout
            None,            // peace_tx
            None,            // stop_rx
            Some(colony_tx), // colony_inbox_tx
            None,            // death ack
            None,            // blob store
            None,            // consumes
        ));

        for _ in 0..5 {
            in_tx
                .send(MessageBuilder::new(Path::new("/d")).build())
                .await
                .unwrap();
        }

        let mut emissions = 0;
        while emissions < 5 {
            tokio::time::timeout(std::time::Duration::from_secs(30), out_rx.recv())
                .await
                .expect("all five must emit")
                .expect("emission");
            emissions += 1;
        }

        let mut tickets = 0;
        while tickets < 5 {
            match tokio::time::timeout(std::time::Duration::from_secs(30), colony_rx.recv())
                .await
                .expect("all five tickets must come back")
                .expect("colony message")
            {
                crate::ColonyMsg::WorkDone { path } => {
                    assert_eq!(path.as_str(), "/d");
                    tickets += 1;
                }
                _ => panic!("expected WorkDone"),
            }
        }
        assert_eq!(tickets, 5);
        join.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stateless_dispatcher_caps_concurrency_at_max_and_processes_all() {
        use crate::stateless_cell::StatelessCell;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Semaphore;

        struct Counting {
            concurrent_now: Arc<AtomicUsize>,
            max_observed: Arc<AtomicUsize>,
            arrival: Arc<Semaphore>,
            release: Arc<Semaphore>,
            completion: Arc<Semaphore>,
        }
        #[allow(clippy::manual_async_fn)]
        impl StatelessCell for Counting {
            fn handle<'a>(
                &'a self,
                _msg: meclaw_core::Message,
                _sink: &'a meclaw_core::OutputSink,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {
                    let now = self.concurrent_now.fetch_add(1, Ordering::SeqCst) + 1;
                    self.max_observed.fetch_max(now, Ordering::SeqCst);
                    self.arrival.add_permits(1);
                    let _ = self.release.acquire().await.unwrap();
                    self.concurrent_now.fetch_sub(1, Ordering::SeqCst);
                    self.completion.add_permits(1);
                }
            }
        }

        let concurrent_now = Arc::new(AtomicUsize::new(0));
        let max_observed = Arc::new(AtomicUsize::new(0));
        let arrival = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let completion = Arc::new(Semaphore::new(0));
        let cell = Arc::new(Counting {
            concurrent_now: concurrent_now.clone(),
            max_observed: max_observed.clone(),
            arrival: arrival.clone(),
            release: release.clone(),
            completion: completion.clone(),
        });

        let (in_tx, in_rx) = mpsc::channel(16);
        let (out_tx, _out_rx) = mpsc::channel::<meclaw_core::CellEmission>(16);
        let join = tokio::spawn(super::stateless_dispatcher(
            meclaw_core::Path::new("/x"),
            in_rx,
            out_tx,
            cell,
            2usize,
            None, // message_timeout
            None, // peace_tx
            None, // stop_rx
            None, // colony_inbox_tx
            None, // death_ack
            None, // blob_store
            None,
        ));

        // 3 messages = max_concurrency + 1. With a correct cap only the first 2
        // workers block on `release`, the 3rd waits in the dispatcher semaphore
        // (un-spawned, no fetch_add).
        for _ in 0..3 {
            in_tx
                .send(meclaw_core::MessageBuilder::new(meclaw_core::Path::new("/x")).build())
                .await
                .unwrap();
        }

        // Rendezvous 1 — arrival: returns as soon as two workers are past fetch_add.
        // The 5s timeout is a failure marker, not a tolerance.
        let _arrival_permit =
            tokio::time::timeout(std::time::Duration::from_secs(5), arrival.acquire_many(2))
                .await
                .expect(
                    "arrival rendezvous never fired — dispatcher failed to admit 2 concurrent \
             workers (Cap-Lower-Bound violated)",
                )
                .expect("arrival semaphore closed unexpectedly");

        assert_eq!(
            max_observed.load(Ordering::SeqCst),
            2,
            "concurrency must be capped at max_concurrency=2 (Cap-Upper-Bound)"
        );

        release.add_permits(10);
        drop(in_tx);
        join.await.unwrap();

        // Rendezvous 2 — completion: returns as soon as all 3 workers are past
        // fetch_sub. Detached-task race-free: without this sync the drain assert would
        // race a late worker that still has to finish fetch_sub after the dispatcher
        // exits.
        let _completion_permit = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            completion.acquire_many(3),
        )
        .await
        .expect("completion rendezvous never fired — workers did not drain within 5s")
        .expect("completion semaphore closed unexpectedly");

        assert_eq!(
            concurrent_now.load(Ordering::SeqCst),
            0,
            "all workers must drain to zero"
        );
        assert_eq!(
            max_observed.load(Ordering::SeqCst),
            2,
            "max_observed stays at max_concurrency throughout drain"
        );
    }

    /// Paket-3 P3-B3: the stateless Concept-B backstop fires when a worker's
    /// `handle()` hangs longer than `message_timeout`. The worker must emit the
    /// generic backstop error (`error_code:"message_timeout"`) to the input's
    /// `reply_to` and END (freeing its permit) — there is NO restart, NO
    /// CellDied, NO peace. The dispatcher stays alive and processes a subsequent
    /// message normally (proving the permit was freed).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stateless_dispatcher_backstop_fires_and_dispatcher_survives() {
        use crate::stateless_cell::StatelessCell;
        use meclaw_core::Path;
        use std::sync::Arc;

        // Hangs on the first message (reply_to "/back"), echoes on the second.
        struct HangOnceThenEcho;
        #[allow(clippy::manual_async_fn)]
        impl StatelessCell for HangOnceThenEcho {
            fn handle<'a>(
                &'a self,
                msg: meclaw_core::Message,
                sink: &'a meclaw_core::OutputSink,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {
                    if msg.reply_to.as_ref().map(|p| p.as_str()) == Some("/back") {
                        // Hang forever — only the backstop can end this worker.
                        std::future::pending::<()>().await;
                    } else {
                        let _ = sink
                            .push(meclaw_core::CellOutput {
                                target: Path::new("/echoed"),
                                content: meclaw_core::JsonValue::Bool(true),
                            })
                            .await;
                    }
                }
            }
        }

        // max_concurrency = 1: if the hung worker's permit were NOT freed, the
        // dispatcher would wedge at `acquire_owned()` and the second message
        // could never be processed — making the survival assert load-bearing.
        let cell = Arc::new(HangOnceThenEcho);
        let (in_tx, in_rx) = mpsc::channel::<meclaw_core::Message>(8);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let join = tokio::spawn(super::stateless_dispatcher(
            Path::new("/x"),
            in_rx,
            out_tx,
            cell,
            1usize,                                     // max_concurrency
            Some(std::time::Duration::from_millis(50)), // message_timeout
            None,                                       // peace_tx
            None,                                       // stop_rx
            None,                                       // colony_inbox_tx
            None,                                       // death_ack
            None,                                       // blob_store
            None,
        ));

        // First message hangs → backstop fires → error to "/back".
        in_tx
            .send(
                meclaw_core::MessageBuilder::new(Path::new("/x"))
                    .reply_to(Path::new("/back"))
                    .build(),
            )
            .await
            .unwrap();

        let em = tokio::time::timeout(std::time::Duration::from_secs(5), out_rx.recv())
            .await
            .expect("backstop error must arrive within 5s")
            .expect("outputs channel open");
        assert_eq!(em.target.as_str(), "/back");
        assert_eq!(em.content["header"]["finish_reason"], "error");
        assert_eq!(em.content["header"]["error_code"], "message_timeout");
        meclaw_core::validate_ubf_body(&em.content)
            .expect("backstop error body must be valid UBF (else Colony would DLQ it)");

        // The dispatcher is still alive (the worker only dropped its permit).
        assert!(
            !join.is_finished(),
            "dispatcher must stay alive after a worker backstop (worker ends, dispatcher runs)"
        );

        // Second message (no "/back" reply_to) is processed normally → proves the
        // hung worker's permit was freed (max_concurrency = 1).
        in_tx
            .send(meclaw_core::MessageBuilder::new(Path::new("/x")).build())
            .await
            .unwrap();
        let em2 = tokio::time::timeout(std::time::Duration::from_secs(5), out_rx.recv())
            .await
            .expect("subsequent message must be processed within 5s (permit freed)")
            .expect("outputs channel open");
        assert_eq!(em2.target.as_str(), "/echoed");
        assert_eq!(em2.content, meclaw_core::JsonValue::Bool(true));

        drop(in_tx);
        join.await.unwrap();
    }

    /// Paket-3 P3-B3: `message_timeout = None` is behavior-neutral — a worker
    /// whose `handle()` takes ~100ms completes normally and emits its own output;
    /// no backstop error, no early worker death.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stateless_dispatcher_none_message_timeout_does_not_wrap() {
        use crate::stateless_cell::StatelessCell;
        use meclaw_core::Path;
        use std::sync::Arc;

        struct SlowEcho;
        #[allow(clippy::manual_async_fn)]
        impl StatelessCell for SlowEcho {
            fn handle<'a>(
                &'a self,
                _msg: meclaw_core::Message,
                sink: &'a meclaw_core::OutputSink,
            ) -> impl std::future::Future<Output = ()> + Send + 'a {
                async move {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let _ = sink
                        .push(meclaw_core::CellOutput {
                            target: Path::new("/out"),
                            content: meclaw_core::JsonValue::Bool(true),
                        })
                        .await;
                }
            }
        }

        let cell = Arc::new(SlowEcho);
        let (in_tx, in_rx) = mpsc::channel::<meclaw_core::Message>(8);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let join = tokio::spawn(super::stateless_dispatcher(
            Path::new("/x"),
            in_rx,
            out_tx,
            cell,
            4usize,
            None, // message_timeout — no wrapper
            None, // peace_tx
            None, // stop_rx
            None, // colony_inbox_tx
            None, // death_ack
            None, // blob_store
            None,
        ));

        in_tx
            .send(meclaw_core::MessageBuilder::new(Path::new("/x")).build())
            .await
            .unwrap();

        // The cell's own emission (not a backstop error) arrives normally.
        let em = tokio::time::timeout(std::time::Duration::from_secs(5), out_rx.recv())
            .await
            .expect("emission within 5s")
            .expect("outputs channel open");
        assert_eq!(em.target.as_str(), "/out");
        assert_eq!(em.content, meclaw_core::JsonValue::Bool(true));

        drop(in_tx);
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_task_propagates_input_message_headers_to_sink() {
        use meclaw_core::serde_json::{Value, json};
        use meclaw_core::{Cell, CellEmission, CellOutput, MessageBuilder, OutputSink, Path};
        use std::future::Future;

        struct EchoHeaders;
        impl Cell for EchoHeaders {
            #[allow(clippy::manual_async_fn)]
            fn handle(
                &mut self,
                _msg: meclaw_core::Message,
                sink: &OutputSink,
            ) -> impl Future<Output = ()> + Send {
                let sink = sink.clone();
                async move {
                    let _ = sink
                        .push(CellOutput {
                            target: Path::new("/out"),
                            content: Value::Null,
                        })
                        .await;
                }
            }
        }

        let (in_tx, in_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let _join = tokio::spawn(cell_task(
            Path::new("/me"),
            in_rx,
            out_tx,
            EchoHeaders,
            None,
            None,
            None,
        ));

        let mut headers = meclaw_core::serde_json::Map::new();
        headers.insert("session_id".into(), json!("s1"));
        let msg = MessageBuilder::new(Path::new("/me"))
            .context(headers)
            .build();
        in_tx.send(msg).await.unwrap();

        let em = out_rx.recv().await.unwrap();
        assert_eq!(
            em.input_headers.context.get("session_id"),
            Some(&json!("s1"))
        );
    }

    /// Behaviour-neutrality proof (phase-10a.0): cells that emit via `OutputSink`
    /// (stateful/stateless) still deliver `Some(input_msg.id)` as
    /// `parent_message_id`. `None` only occurs for event-originated emissions via
    /// `OriginSink` (Phase-10-A).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cell_emission_via_output_sink_keeps_parent_as_some() {
        use meclaw_core::{
            Cell, CellEmission, CellOutput, JsonValue, MessageBuilder, OutputSink, Path,
        };
        use std::future::Future;

        struct EmitNull;
        impl Cell for EmitNull {
            #[allow(clippy::manual_async_fn)]
            fn handle(
                &mut self,
                _msg: meclaw_core::Message,
                sink: &OutputSink,
            ) -> impl Future<Output = ()> + Send {
                let sink = sink.clone();
                async move {
                    let _ = sink
                        .push(CellOutput {
                            target: Path::new("/dst"),
                            content: JsonValue::Null,
                        })
                        .await;
                }
            }
        }

        let (in_tx, in_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let _join = tokio::spawn(cell_task(
            Path::new("/c"),
            in_rx,
            out_tx,
            EmitNull,
            None,
            None,
            None,
        ));

        let msg = MessageBuilder::new(Path::new("/c")).build();
        let input_id = msg.id;
        in_tx.send(msg).await.unwrap();
        let em = out_rx.recv().await.unwrap();
        assert_eq!(
            em.parent_message_id,
            Some(input_id),
            "OutputSink::push lifts parent to Some — behavior-neutral for non-source cells"
        );
    }

    // ── Phase-13.5 A8 T6 (Step B): delivery-boundary blob resolution ─────────

    #[tokio::test]
    async fn resolve_blob_passes_inline_body_through_unchanged() {
        let mut msg = MessageBuilder::new(Path::new("/c")).build();
        msg.body = meclaw_core::Body::Inline(meclaw_core::serde_json::json!({"k": 1}));
        let deliver = resolve_blob_for_delivery(&mut msg, &None, &None).await;
        assert!(deliver, "inline body is always deliverable");
        assert!(matches!(msg.body, meclaw_core::Body::Inline(_)));
    }

    #[tokio::test]
    async fn resolve_blob_passes_through_when_no_store_wired() {
        let id = meclaw_core::Uuid::now_v7();
        let mut msg = MessageBuilder::new(Path::new("/c")).build();
        msg.body = meclaw_core::Body::Blob(id);
        // Plain test path: no blob-store → blob passes through unchanged.
        let deliver = resolve_blob_for_delivery(&mut msg, &None, &None).await;
        assert!(deliver);
        assert!(matches!(msg.body, meclaw_core::Body::Blob(b) if b == id));
    }

    #[tokio::test]
    async fn resolve_blob_replaces_body_with_inline_value() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let value =
            meclaw_core::serde_json::json!({"system": "s", "messages": [{"origin": "user"}]});
        let bytes = meclaw_core::serde_json::to_vec(&value).unwrap();
        let blob_ref = store
            .write_streaming(bytes.as_slice(), "application/json", None)
            .await
            .unwrap();
        let mut msg = MessageBuilder::new(Path::new("/c")).build();
        msg.body = meclaw_core::Body::Blob(blob_ref.blob_id);
        let deliver = resolve_blob_for_delivery(&mut msg, &Some(store), &None).await;
        assert!(deliver);
        match msg.body {
            // Spec Z.1363: every cell sees a transparent inline body.
            meclaw_core::Body::Inline(v) => assert_eq!(v, value),
            meclaw_core::Body::Blob(_) => {
                panic!("blob must be resolved to inline at the delivery boundary")
            }
        }
    }

    #[tokio::test]
    async fn resolve_blob_unresolvable_dead_letters_and_skips_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let (dlq_tx, mut dlq_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let unknown = meclaw_core::Uuid::now_v7();
        let mut msg = MessageBuilder::new(Path::new("/c")).build();
        msg.body = meclaw_core::Body::Blob(unknown);
        let deliver = resolve_blob_for_delivery(&mut msg, &Some(store), &Some(dlq_tx)).await;
        assert!(!deliver, "unresolvable blob → caller must skip handle()");
        match dlq_rx.recv().await {
            Some(crate::ColonyMsg::DeadLetterMessage { reason, .. }) => {
                assert!(
                    matches!(reason, crate::DeadLetterReason::BlobUnavailable),
                    "undeliverable blob is dead-lettered as blob_unavailable"
                );
            }
            _ => panic!("expected a DeadLetterMessage on the colony inbox channel"),
        }
    }

    // ── GH #19 (D-025): in-message pointers at the same boundary ────────────

    /// Write a UBF body document into `store` and return its blob id.
    async fn put_body(
        store: &crate::DiskBlobStore,
        doc: meclaw_core::JsonValue,
    ) -> meclaw_core::Uuid {
        let bytes = meclaw_core::serde_json::to_vec(&doc).unwrap();
        store
            .write_streaming(bytes.as_slice(), "application/json", None)
            .await
            .unwrap()
            .blob_id
    }

    fn inline(body: meclaw_core::JsonValue) -> Message {
        let mut msg = MessageBuilder::new(Path::new("/c")).build();
        msg.body = meclaw_core::Body::Inline(body);
        msg
    }

    /// The boundary expands in-message pointers, not just whole-body blobs.
    #[tokio::test]
    async fn resolve_expands_in_message_pointers_of_an_inline_body() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let turn = meclaw_core::serde_json::json!({"origin":"user","type":"text","text":"blobbed"});
        let id = put_body(&store, meclaw_core::serde_json::json!({"messages":[turn]})).await;
        let mut msg = inline(meclaw_core::serde_json::json!({
            "messages": [{"messages_id": id.to_string()}]
        }));
        assert!(resolve_blob_for_delivery(&mut msg, &Some(store), &None).await);
        let meclaw_core::Body::Inline(body) = &msg.body else {
            panic!("body must stay inline")
        };
        assert_eq!(
            body["messages"][0]["text"], "blobbed",
            "the pointer must be replaced before handle(): {body}"
        );
    }

    /// The whole-body offload and the in-message pointers compose: a blob body
    /// that itself carries a pointer is resolved in one pass at the boundary.
    #[tokio::test]
    async fn resolve_expands_pointers_inside_a_whole_body_blob() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let turn = meclaw_core::serde_json::json!({"origin":"user","type":"text","text":"nested"});
        let inner = put_body(&store, meclaw_core::serde_json::json!({"messages":[turn]})).await;
        let outer = put_body(
            &store,
            meclaw_core::serde_json::json!({"messages":[{"messages_id": inner.to_string()}]}),
        )
        .await;
        let mut msg = MessageBuilder::new(Path::new("/c")).build();
        msg.body = meclaw_core::Body::Blob(outer);
        assert!(resolve_blob_for_delivery(&mut msg, &Some(store), &None).await);
        let meclaw_core::Body::Inline(body) = &msg.body else {
            panic!("whole-body blob must resolve to inline")
        };
        assert_eq!(body["messages"][0]["text"], "nested");
    }

    /// An unresolvable pointer dead-letters under the SAME code a missing
    /// whole-body blob uses, and the cell is not called.
    #[tokio::test]
    async fn resolve_unresolvable_pointer_dead_letters_as_blob_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let (dlq_tx, mut dlq_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let mut msg = inline(meclaw_core::serde_json::json!({
            "messages": [{"messages_id": meclaw_core::Uuid::now_v7().to_string()}]
        }));
        let deliver = resolve_blob_for_delivery(&mut msg, &Some(store), &Some(dlq_tx)).await;
        assert!(!deliver, "unresolvable pointer → caller must skip handle()");
        match dlq_rx.recv().await {
            Some(crate::ColonyMsg::DeadLetterMessage { reason, .. }) => {
                assert_eq!(reason.as_code(), "blob_unavailable");
            }
            _ => panic!("expected a DeadLetterMessage on the colony inbox channel"),
        }
    }

    /// A chain past `blob_max_recursion_depth` dead-letters under the canonical
    /// `blob_recursion_too_deep` — the string the spec has always promised and
    /// that had no producer until now.
    #[tokio::test]
    async fn resolve_too_deep_pointer_chain_dead_letters_as_recursion_too_deep() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(
            crate::DiskBlobStore::new(dir.path())
                .unwrap()
                .with_max_recursion_depth(1),
        );
        let turn = meclaw_core::serde_json::json!({"origin":"user","type":"text","text":"leaf"});
        let leaf = put_body(&store, meclaw_core::serde_json::json!({"messages":[turn]})).await;
        let top = put_body(
            &store,
            meclaw_core::serde_json::json!({"messages":[{"messages_id": leaf.to_string()}]}),
        )
        .await;
        let (dlq_tx, mut dlq_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let probe = meclaw_core::serde_json::json!({
            "messages": [{"messages_id": top.to_string()}]
        });
        let mut msg = inline(probe.clone());
        let deliver = resolve_blob_for_delivery(&mut msg, &Some(store), &Some(dlq_tx)).await;
        assert!(!deliver, "a runaway chain → caller must skip handle()");
        match dlq_rx.recv().await {
            Some(crate::ColonyMsg::DeadLetterMessage { reason, message }) => {
                assert_eq!(reason.as_code(), "blob_recursion_too_deep");
                assert!(
                    matches!(&message.body, meclaw_core::Body::Inline(b) if *b == probe),
                    "the dead letter carries the ORIGINAL body, not a half-expanded one"
                );
            }
            _ => panic!("expected a DeadLetterMessage on the colony inbox channel"),
        }
    }

    // ── GH #86: system-tree `{text_id}` leaves at the same boundary ─────────

    /// The boundary expands `system` leaves too, into the plain string
    /// container the tree's consumers expect.
    #[tokio::test]
    async fn resolve_expands_system_text_id_leaves_of_an_inline_body() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let turn = meclaw_core::serde_json::json!({"origin":"user","type":"text","text":"persona"});
        let id = put_body(&store, meclaw_core::serde_json::json!({"messages":[turn]})).await;
        let mut msg = inline(meclaw_core::serde_json::json!({
            "system": {"identity": {"body": {"text_id": id.to_string()}}}
        }));
        assert!(resolve_blob_for_delivery(&mut msg, &Some(store), &None).await);
        let meclaw_core::Body::Inline(body) = &msg.body else {
            panic!("body must stay inline")
        };
        assert_eq!(
            body["system"]["identity"]["body"],
            meclaw_core::serde_json::json!({"text": "persona"}),
            "the leaf must be a {{\"text\": …}} container before handle(): {body}"
        );
    }

    /// The whole-body offload and the system leaves compose, like the
    /// `messages[]` class does.
    #[tokio::test]
    async fn resolve_expands_system_pointers_inside_a_whole_body_blob() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let turn = meclaw_core::serde_json::json!({"origin":"user","type":"text","text":"nested"});
        let inner = put_body(&store, meclaw_core::serde_json::json!({"messages":[turn]})).await;
        let outer = put_body(
            &store,
            meclaw_core::serde_json::json!({
                "system": {"facts": {"n": {"text_id": inner.to_string()}}}
            }),
        )
        .await;
        let mut msg = MessageBuilder::new(Path::new("/c")).build();
        msg.body = meclaw_core::Body::Blob(outer);
        assert!(resolve_blob_for_delivery(&mut msg, &Some(store), &None).await);
        let meclaw_core::Body::Inline(body) = &msg.body else {
            panic!("whole-body blob must resolve to inline")
        };
        assert_eq!(body["system"]["facts"]["n"]["text"], "nested");
    }

    /// An unresolvable system leaf reports the same code the other classes use.
    #[tokio::test]
    async fn resolve_unresolvable_system_pointer_dead_letters_as_blob_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let (dlq_tx, mut dlq_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let mut msg = inline(meclaw_core::serde_json::json!({
            "system": {"identity": {"body": {"text_id": meclaw_core::Uuid::now_v7().to_string()}}}
        }));
        let deliver = resolve_blob_for_delivery(&mut msg, &Some(store), &Some(dlq_tx)).await;
        assert!(!deliver, "unresolvable system leaf → skip handle()");
        match dlq_rx.recv().await {
            Some(crate::ColonyMsg::DeadLetterMessage { reason, .. }) => {
                assert_eq!(reason.as_code(), "blob_unavailable");
            }
            _ => panic!("expected a DeadLetterMessage on the colony inbox channel"),
        }
    }

    /// A system-leaf chain past the limit reports the canonical recursion code.
    #[tokio::test]
    async fn resolve_too_deep_system_chain_dead_letters_as_recursion_too_deep() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(
            crate::DiskBlobStore::new(dir.path())
                .unwrap()
                .with_max_recursion_depth(1),
        );
        let turn = meclaw_core::serde_json::json!({"origin":"user","type":"text","text":"leaf"});
        let leaf = put_body(&store, meclaw_core::serde_json::json!({"messages":[turn]})).await;
        let top = put_body(
            &store,
            meclaw_core::serde_json::json!({"messages":[{"text_id": leaf.to_string()}]}),
        )
        .await;
        let (dlq_tx, mut dlq_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let probe = meclaw_core::serde_json::json!({
            "system": {"identity": {"body": {"text_id": top.to_string()}}}
        });
        let mut msg = inline(probe.clone());
        let deliver = resolve_blob_for_delivery(&mut msg, &Some(store), &Some(dlq_tx)).await;
        assert!(!deliver, "a runaway system chain → skip handle()");
        match dlq_rx.recv().await {
            Some(crate::ColonyMsg::DeadLetterMessage { reason, message }) => {
                assert_eq!(reason.as_code(), "blob_recursion_too_deep");
                assert!(
                    matches!(&message.body, meclaw_core::Body::Inline(b) if *b == probe),
                    "the dead letter carries the ORIGINAL tree, not a half-expanded one"
                );
            }
            _ => panic!("expected a DeadLetterMessage on the colony inbox channel"),
        }
    }

    /// The all-or-nothing promise across the PAIR of pointer classes: a body
    /// whose `messages[]` resolves fine and whose `system` tree does not is
    /// dead-lettered exactly as it arrived. Each pass is already atomic on its
    /// own; this pins that the boundary does not commit the first one's result
    /// when the second fails.
    #[tokio::test]
    async fn resolve_dead_letters_the_original_body_when_only_the_system_half_fails() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let turn = meclaw_core::serde_json::json!({"origin":"user","type":"text","text":"good"});
        let good = put_body(&store, meclaw_core::serde_json::json!({"messages":[turn]})).await;
        let (dlq_tx, mut dlq_rx) = mpsc::channel::<crate::ColonyMsg>(8);
        let probe = meclaw_core::serde_json::json!({
            "system": {"identity": {"body": {"text_id": meclaw_core::Uuid::now_v7().to_string()}}},
            "messages": [{"messages_id": good.to_string()}]
        });
        let mut msg = inline(probe.clone());
        let deliver = resolve_blob_for_delivery(&mut msg, &Some(store), &Some(dlq_tx)).await;
        assert!(!deliver, "a failing half → skip handle()");
        match dlq_rx.recv().await {
            Some(crate::ColonyMsg::DeadLetterMessage { reason, message }) => {
                assert_eq!(reason.as_code(), "blob_unavailable");
                assert!(
                    matches!(&message.body, meclaw_core::Body::Inline(b) if *b == probe),
                    "the resolved messages[] half must NOT survive the failed system half"
                );
            }
            _ => panic!("expected a DeadLetterMessage on the colony inbox channel"),
        }
    }

    /// A body with no pointers must not touch the store at all — the pre-checks
    /// are what keep an ordinary conversation free of resolution work. Proven
    /// with a store rooted at a path that would fail every read.
    #[tokio::test]
    async fn resolve_leaves_a_pointerless_body_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(crate::DiskBlobStore::new(dir.path()).unwrap());
        let body = meclaw_core::serde_json::json!({
            "messages": [{"origin":"user","type":"text","text":"plain"}]
        });
        let mut msg = inline(body.clone());
        assert!(resolve_blob_for_delivery(&mut msg, &Some(store), &None).await);
        assert!(matches!(&msg.body, meclaw_core::Body::Inline(b) if *b == body));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stateless_dispatcher_fires_peace_on_stop_not_on_mailbox_close() {
        use crate::stateless_cell::StatelessCell;
        use std::future::Future;
        use std::sync::Arc;

        struct NoopSL;
        impl StatelessCell for NoopSL {
            #[allow(clippy::manual_async_fn)]
            fn handle<'a>(
                &'a self,
                _m: meclaw_core::Message,
                _s: &'a meclaw_core::OutputSink,
            ) -> impl Future<Output = ()> + Send + 'a {
                async move {}
            }
        }

        // --- stop arm fires peace ---
        let (mb_tx, mb_rx) = mpsc::channel::<meclaw_core::Message>(8);
        let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
        let (peace_tx, peace_rx) = tokio::sync::oneshot::channel::<()>();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let join = tokio::spawn(stateless_dispatcher(
            Path::new("/sl"),
            mb_rx,
            out_tx,
            Arc::new(NoopSL),
            4,
            None, // message_timeout
            Some(peace_tx),
            Some(stop_rx),
            None, // colony_inbox_tx
            None, // death_ack
            None, // blob_store
            None,
        ));
        stop_tx.send(()).unwrap();
        let peace = tokio::time::timeout(std::time::Duration::from_secs(5), peace_rx).await;
        assert!(
            peace.is_ok() && peace.unwrap().is_ok(),
            "peace MUST fire on the stop arm"
        );
        join.await.unwrap();
        drop(mb_tx);

        // --- mailbox-close does NOT fire peace (genuine Normal exit, F5) ---
        let (mb_tx2, mb_rx2) = mpsc::channel::<meclaw_core::Message>(8);
        let (out_tx2, _out_rx2) = mpsc::channel::<CellEmission>(8);
        let (peace_tx2, peace_rx2) = tokio::sync::oneshot::channel::<()>();
        let join2 = tokio::spawn(stateless_dispatcher(
            Path::new("/sl2"),
            mb_rx2,
            out_tx2,
            Arc::new(NoopSL),
            4,
            None,
            Some(peace_tx2),
            None, // no stop arm
            None,
            None,
            None,
            None,
        ));
        drop(mb_tx2); // close mailbox → recv()==None → genuine normal exit
        join2.await.unwrap();
        assert!(
            peace_rx2.await.is_err(),
            "peace MUST NOT fire on mailbox-close → watcher Err → DeathKind::Normal → remove"
        );
    }

    /// GH #47: the guard reports exactly once, on every exit path, and an
    /// inert guard says nothing.
    #[tokio::test]
    async fn a_work_guard_reports_once_when_it_drops() {
        let (tx, mut rx) = mpsc::channel::<crate::ColonyMsg>(4);
        {
            let _g = WorkGuard::new(Path::new("/a"), Some(tx.clone()));
        }
        // `ColonyMsg` carries boxed spawn closures and join handles, so it is not
        // `Debug`; the failure arm names the expectation instead of the value.
        match rx.try_recv() {
            Ok(crate::ColonyMsg::WorkDone { path }) => assert_eq!(path.as_str(), "/a"),
            _ => panic!("expected one WorkDone"),
        }
        assert!(rx.try_recv().is_err(), "the guard must report exactly once");
    }

    #[tokio::test]
    async fn an_inert_work_guard_reports_nothing() {
        let (tx, mut rx) = mpsc::channel::<crate::ColonyMsg>(4);
        {
            let _g = WorkGuard::new(Path::new("/a"), None);
        }
        drop(tx);
        assert!(rx.try_recv().is_err(), "a None guard must stay silent");
    }

    /// The load-bearing half: a handler that PANICS still gives its ticket back,
    /// because the unwind runs the guard's `Drop`. Without this a panicking cell
    /// would hold the drain until the deadline on every restart.
    #[tokio::test]
    async fn a_work_guard_reports_through_a_panicking_scope() {
        let (tx, mut rx) = mpsc::channel::<crate::ColonyMsg>(4);
        let handle = tokio::spawn(async move {
            let _g = WorkGuard::new(Path::new("/boom"), Some(tx));
            panic!("handler exploded");
        });
        assert!(handle.await.is_err(), "the task must have panicked");
        match rx.try_recv() {
            Ok(crate::ColonyMsg::WorkDone { path }) => assert_eq!(path.as_str(), "/boom"),
            _ => panic!("a panicking handler must still return its ticket"),
        }
    }
}
