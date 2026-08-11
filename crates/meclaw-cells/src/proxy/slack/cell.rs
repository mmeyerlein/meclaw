//! P12 S16/S17/S20: `SlackCell` — the handler half of the Slack variant.
//!
//! Same double-task shape as the Telegram variant: the handler owns the
//! database and the outbound API calls, the I/O task owns the socket, and state
//! lives single-threaded inside each. No mutex, no shared state.
//!
//! The addressing rule (D-5) in one place, because it decides both what the bot
//! answers and where the answer lands:
//!
//! | trigger                        | behaviour                             |
//! |--------------------------------|---------------------------------------|
//! | mention in the channel root    | opens a thread at the mention's `ts`  |
//! | mention inside a thread        | stays in that thread                  |
//! | direct message                 | no thread at all                      |
//! | message in a thread we own     | processed (no re-mention needed)      |
//! | anything else in a channel     | ignored (R5)                          |
//!
//! The last row is the one that makes a bot tolerable in a shared channel: it
//! does not jump into conversations it was not invited to.

use super::client::SlackClient;
use super::db::{claim_thread, mark_envelope_seen, owns_thread};
use super::emit::{build_user_turn_content, split_chat_id};
use super::io::{SlackInbound, SlackIoConfig, SlackReconfig, run_slack_io};
use super::params::SlackParams;
use super::wire::SlackEventKind;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Message, OriginSink, OutputSink, Path};
use std::future::Future;
use tokio::sync::mpsc;

/// Seconds since the Unix epoch, for the dedup and ownership timestamps.
fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The Slack platform variant of the `proxy` cell type.
pub struct SlackCell {
    client: SlackClient,
    emit_to: Path,
    thread_follow: bool,
    initial_io_cfg: Option<SlackIoConfig>,
}

/// I/O-local state. Single owner, held by value by the I/O sub-task.
pub struct SlackIo {
    cfg: SlackIoConfig,
}

impl SlackCell {
    /// Builds the cell from validated params plus the client the factory made.
    pub fn new(params: &SlackParams, client: SlackClient) -> Self {
        let io_client = client.clone();
        Self {
            client,
            emit_to: params.emit_to.clone(),
            thread_follow: params.thread_follow,
            initial_io_cfg: Some(SlackIoConfig {
                client: io_client,
                bot_user_id: params.bot_user_id.clone(),
                connect_timeout_ms: params.connect_timeout_ms,
                min_uptime_ms: 5_000,
                // Replaced by the substrate via `attach_liveness`.
                liveness: meclaw_colony::IoLivenessMark::disabled(),
            }),
        }
    }

    /// Decides the thread this event belongs to, or that it is not ours.
    ///
    /// Returns `None` when rule R5 rejects the event. The DB read only happens
    /// for the one case that needs it — an unmentioned threaded channel message
    /// — so ordinary traffic costs no query.
    async fn resolve_thread(
        &self,
        event: &super::wire::SlackUserEvent,
        db: &mut DbConn,
    ) -> Option<Option<String>> {
        let is_dm = event.channel_type.as_deref() == Some("im");
        match event.kind {
            // A mention is always for us. In the root it opens a thread at its
            // own ts; inside a thread it stays there. A mention in a DM keeps
            // the DM's threadless shape.
            SlackEventKind::Mention => {
                if is_dm {
                    Some(None)
                } else {
                    Some(Some(
                        event.thread_ts.clone().unwrap_or_else(|| event.ts.clone()),
                    ))
                }
            }
            SlackEventKind::Message => {
                if is_dm {
                    // Every DM is addressed to us by construction.
                    return Some(None);
                }
                let thread_ts = event.thread_ts.clone()?;
                if !self.thread_follow {
                    return None;
                }
                let channel = event.channel.clone();
                let t = thread_ts.clone();
                let owned = db
                    .call_with_timeout(move |c| owns_thread(c, &channel, &t))
                    .await;
                match owned {
                    Ok(Ok(true)) => Some(Some(thread_ts)),
                    // Not ours, unreadable, or the query timed out: stay silent.
                    // Speaking up on a failed ownership check would mean barging
                    // into a stranger's thread on a database hiccup.
                    _ => None,
                }
            }
        }
    }
}

impl LongRunningCell for SlackCell {
    type Event = SlackInbound;
    type Reconfig = SlackReconfig;
    type Io = SlackIo;

    fn split_io(&mut self) -> Self::Io {
        SlackIo {
            cfg: self
                .initial_io_cfg
                .take()
                .expect("split_io called twice — the colony calls it exactly once"),
        }
    }

    /// Issue #7: a Socket Mode lane is frame-driven, so the mark tracks frame
    /// arrival — a silent socket that nobody closed is precisely the failure
    /// that used to be invisible.
    fn attach_liveness(io: &mut Self::Io, mark: meclaw_colony::IoLivenessMark) {
        io.cfg.liveness = mark;
    }

    /// I/O sub-task. `+ Send` is load-bearing (AFIT does not bind Send and
    /// `tokio::spawn` needs it); `manual_async_fn` is the known stable-1.95
    /// false positive, same as the Telegram and timer variants.
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        run_slack_io(io.cfg, events_tx, reconfig_rx)
    }

    /// Inbound sink leg: post the agent's answer back to Slack.
    ///
    /// Pure sink — nothing is emitted on success. Failures are announced
    /// through the shared, platform-neutral `emit_inbound_error`.
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        _db: &'a mut DbConn,
        _reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let body_val = match &msg.body {
                meclaw_core::Body::Inline(v) => v.clone(),
                _ => {
                    crate::proxy::emit::emit_inbound_error(
                        sink,
                        &msg,
                        "invalid_body",
                        "expected inline json",
                    )
                    .await;
                    return;
                }
            };

            // The address lives in `context`, put there by the promotion edge.
            // Absent means that edge is missing from the topology — fail loud,
            // because a silent drop here looks like "the bot ignored me".
            let Some(chat_id) = msg
                .headers
                .context
                .get("chat_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            else {
                crate::proxy::emit::emit_inbound_error(
                    sink,
                    &msg,
                    "missing_chat_id",
                    "context.chat_id required (promote it on the proxy out-edge)",
                )
                .await;
                return;
            };

            let text = body_val
                .get("messages")
                .and_then(|m| m.as_array())
                .and_then(|arr| {
                    arr.iter().rev().find(|t| {
                        t.get("origin").and_then(|v| v.as_str()) == Some("assistant")
                            && t.get("type").and_then(|v| v.as_str()) == Some("text")
                    })
                })
                .and_then(|t| t.get("text"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let Some(text) = text else {
                crate::proxy::emit::emit_inbound_error(
                    sink,
                    &msg,
                    "missing_assistant_turn",
                    "messages[] has no assistant-text turn",
                )
                .await;
                return;
            };

            // The A-timeout for this call lives inside the client, built from
            // `send_timeout_ms` at construction time.
            let (channel, thread_ts) = split_chat_id(&chat_id);
            match self
                .client
                .chat_post_message(&channel, &text, thread_ts.as_deref())
                .await
            {
                Ok(()) => {}
                Err(e) => {
                    crate::proxy::emit::emit_inbound_error(
                        sink,
                        &msg,
                        "send_failed",
                        &format!("{e}"),
                    )
                    .await;
                }
            }
        }
    }

    /// Source leg: dedup, decide ownership, persist, then emit.
    ///
    /// Phase-5 canon (state before emit): the envelope is recorded and the
    /// thread claimed BEFORE the emission. A crash in between loses at most one
    /// event; the other order would re-deliver the same message after a restart.
    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let SlackInbound { envelope_id, event } = event;
            let now = now_unix_seconds();

            // 1. Dedup. Slack redelivers un-acked envelopes, and an ack can be
            //    lost after we sent it, so a repeat is expected rather than
            //    exceptional.
            let env_id = envelope_id.clone();
            let first_sighting = db
                .call_with_timeout(move |c| mark_envelope_seen(c, &env_id, now))
                .await;
            match first_sighting {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) => return, // redelivery: already handled
                // A failed dedup check must not become a double emission.
                _ => return,
            }

            // 2. Is this ours, and which thread does it belong to? (R5 + D-5)
            let Some(thread_ts) = self.resolve_thread(&event, db).await else {
                return;
            };

            // 3. Claim the thread before emitting, so a follow-up that arrives
            //    while the agent is still thinking is already recognised as ours.
            if let (SlackEventKind::Mention, Some(t)) = (event.kind, thread_ts.as_ref()) {
                let channel = event.channel.clone();
                let t = t.clone();
                let _ = db
                    .call_with_timeout(move |c| claim_thread(c, &channel, &t, now))
                    .await;
            }

            // 4. Emit through the OriginSink (parent=None, fresh trace).
            let content = build_user_turn_content(&event, thread_ts.as_deref(), &event.text);
            let _ = sink
                .emit(meclaw_core::CellOutput {
                    target: self.emit_to.clone(),
                    content,
                })
                .await;
        }
    }
}
