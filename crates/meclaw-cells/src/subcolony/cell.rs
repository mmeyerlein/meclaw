//! `SubcolonyCell` — the handler side of the facade.
//!
//! It owns the connection state and the translation between parent messages and
//! child frames. It owns no pipe and no process; those live in the I/O sub-task.
//!
//! Two failure classes, treated differently on purpose:
//!
//! - **Deterministic** (the child speaks another protocol, never boots, cannot be
//!   spawned): a restart would reproduce it exactly. The cell stays up and
//!   refuses every request with the reason. Burning the restart budget on a
//!   certainty would only turn one clear error into a process storm.
//! - **Transient** (the child died mid-conversation): a restart is precisely the
//!   cure. Whoever was waiting gets a typed failure FIRST — the ratified liveness
//!   slice — and then the handler panics into `one_for_one`, which boots a fresh
//!   child from the child's own `colony.db`.
//!
//! There is no task register here, and the reason is not that a request is
//! idempotent — it is not; a request can make the child write to its store.
//! The reason is that there is no automatic re-fire path: a request lives only in
//! the serve loop's pending map and in the reconfig channel, both of which die
//! with the task. Whoever asked decides whether to ask again.

use crate::stdio_child::{ChildCommand, ChildEvent};
use crate::subcolony::emit;
use crate::subcolony::io::{SubcolonyEvent, SubcolonyIo, SubcolonyReconfig};
use crate::subcolony::params::{SubcolonyOverlay, SubcolonyParams};
use crate::subcolony::wire;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Message, OriginSink, OutputSink, Uuid};
use std::future::Future;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// What the handler knows about the connection.
#[derive(Debug)]
enum ChildState {
    /// No boot verdict yet.
    Booting,
    /// The child booted and speaks our protocol.
    Ready,
    /// The child cannot serve, and a restart would not change that.
    Unusable {
        error_code: &'static str,
        detail: String,
    },
}

/// The `subcolony` cell. Single-threaded state in the handler sub-task.
pub struct SubcolonyCell {
    params: SubcolonyParams,
    initial_io: Option<SubcolonyIo>,
    state: ChildState,
    /// Cloned from the first `handle` call so `handle_event` can talk to the
    /// child too (the trait hands the reconfig sender to `handle` only).
    reconfig: Option<mpsc::Sender<SubcolonyReconfig>>,
}

impl SubcolonyCell {
    /// Build a cell from its (birth ⊕ overlay) params.
    pub fn new(params: SubcolonyParams) -> Self {
        Self {
            initial_io: Some(SubcolonyIo::new(params.clone())),
            params,
            state: ChildState::Booting,
            reconfig: None,
        }
    }
}

impl LongRunningCell for SubcolonyCell {
    type Event = SubcolonyEvent;
    type Reconfig = SubcolonyReconfig;
    type Io = SubcolonyIo;

    fn split_io(&mut self) -> Self::Io {
        self.initial_io.take().expect("split_io called twice")
    }

    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        crate::subcolony::io::run_io(io, events_tx, reconfig_rx)
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
            if self.reconfig.is_none() {
                self.reconfig = Some(reconfig_tx.clone());
            }

            // β params-update slot, handled FIRST. Only tuning moves; the
            // containment keys are immutable and a message touching one gets a
            // loud reject rather than a silent no-op.
            if let meclaw_core::Body::Inline(ref v) = msg.body
                && v.get("params").is_some()
            {
                self.apply_params_update(&msg, sink, db).await;
                return;
            }

            // A deterministic failure is reported without bothering the child.
            if let ChildState::Unusable { error_code, detail } = &self.state {
                let (code, detail) = (*error_code, detail.clone());
                emit::error(sink, &msg, code, &detail).await;
                return;
            }

            // The correlation key is generated per request, not taken from the
            // parent turn: one parent turn may fan out to several calls.
            let turn_id = Uuid::now_v7().to_string();
            let frame = match wire::request_frame(&msg, &self.params, &turn_id) {
                Ok(f) => f,
                Err(e) => {
                    emit::error(sink, &msg, e.error_code, &e.detail).await;
                    return;
                }
            };

            let (tx, rx) = oneshot::channel();
            let sent = reconfig_tx
                .send(SubcolonyReconfig::Child(ChildCommand::Send {
                    line: frame,
                    correlate: Some(turn_id),
                    reply: Some(tx),
                }))
                .await
                .is_ok();
            if !sent {
                emit::error(
                    sink,
                    &msg,
                    "subcolony_gone",
                    "the sub-colony connection is no longer reachable",
                )
                .await;
                return;
            }

            // A-timeout (rule 12) on the one bounded operation: the child's
            // answer. Generous, because the child may be doing real work.
            let answer =
                tokio::time::timeout(Duration::from_millis(self.params.request_timeout_ms), rx)
                    .await;
            match answer {
                Ok(Ok(Ok(frame))) => match wire::reply_body(&frame) {
                    Ok(body) => emit::reply(sink, &msg, body).await,
                    Err(e) => emit::error(sink, &msg, e.error_code, &e.detail).await,
                },
                // The I/O side reported the child's fate instead of an answer.
                Ok(Ok(Err(e))) => {
                    emit::error(sink, &msg, "subcolony_gone", &e.detail()).await;
                }
                Ok(Err(_)) => {
                    emit::error(
                        sink,
                        &msg,
                        "subcolony_gone",
                        "the sub-colony connection dropped the request",
                    )
                    .await;
                }
                Err(_) => {
                    emit::error(
                        sink,
                        &msg,
                        "request_timeout",
                        "the sub-colony did not answer within request_timeout_ms",
                    )
                    .await;
                }
            }
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            match event {
                SubcolonyEvent::Ready { version } => {
                    // Reported, never asserted: a sealed child colony is
                    // expected to run a different build than its parent.
                    tracing::info!(child_version = %version, "sub-colony ready");
                    self.state = ChildState::Ready;
                }
                SubcolonyEvent::BootFailed { error_code, detail } => {
                    tracing::error!(error_code, detail = %detail, "sub-colony boot failed");
                    self.state = ChildState::Unusable { error_code, detail };
                }
                // A frame nobody was waiting for: the child topology emitted it
                // on its own. Forwarding it beats dropping it.
                SubcolonyEvent::Child(ChildEvent::Frame(v)) => match &self.params.emit_to {
                    Some(target) => match wire::reply_body(&v) {
                        Ok(body) => emit::unsolicited(sink, target.clone(), body).await,
                        Err(e) => {
                            tracing::warn!(detail = %e.detail, "unreadable unsolicited frame")
                        }
                    },
                    None => tracing::warn!(
                        "sub-colony sent an unsolicited frame and no emit_to is configured"
                    ),
                },
                SubcolonyEvent::Child(ChildEvent::Malformed { raw }) => {
                    tracing::debug!(line = %raw, "sub-colony: non-json line from child");
                }
                // Transient by assumption: whoever was waiting has already been
                // answered by the serve loop, so panicking now costs nothing and
                // buys a fresh child booted from its own colony.db.
                SubcolonyEvent::Child(ChildEvent::Exited(exit)) => {
                    panic!("sub-colony child exited: {}", exit.detail());
                }
            }
        }
    }
}

impl SubcolonyCell {
    /// Apply a runtime params update. Only the tunables move, and only for the
    /// NEXT request.
    async fn apply_params_update(&mut self, msg: &Message, sink: &OutputSink, db: &mut DbConn) {
        let meclaw_core::Body::Inline(body) = &msg.body else {
            return;
        };
        let Some(update) = body.get("params").and_then(|p| p.as_object()).cloned() else {
            emit::error(sink, msg, "invalid_input", "params slot: not a JSON object").await;
            return;
        };
        let current = SubcolonyOverlay {
            boot_timeout_ms: self.params.boot_timeout_ms,
            request_timeout_ms: self.params.request_timeout_ms,
            external_timeout_ms: self.params.external_timeout_ms,
            query_timeout_ms: self.params.query_timeout_ms,
        };
        let (new_overlay, persisted) = match crate::params_overlay::apply_update(&current, &update)
        {
            Ok(pair) => pair,
            Err(e) => {
                emit::error(sink, msg, "invalid_input", &e.detail()).await;
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
                    "query_timeout",
                    "params write exceeded query_timeout_ms",
                )
                .await;
                return;
            }
        }
        self.params.boot_timeout_ms = new_overlay.boot_timeout_ms;
        self.params.request_timeout_ms = new_overlay.request_timeout_ms;
        self.params.external_timeout_ms = new_overlay.external_timeout_ms;
        self.params.query_timeout_ms = new_overlay.query_timeout_ms;
        db.set_query_timeout(Some(Duration::from_millis(self.params.query_timeout_ms)));
    }
}
