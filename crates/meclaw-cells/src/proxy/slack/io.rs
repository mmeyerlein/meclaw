//! P12 S13: the Socket Mode I/O loop for one connection.
//!
//! Frame-driven, never polled. The loop blocks on the socket and reacts; the
//! only reason it ever comes back is an event — a `disconnect` frame, a close,
//! or an error. There is no tick, no interval and no "check if anything
//! arrived" (standing rule: NO POLLING). Backoff exists solely to damp repeated
//! failures before a reconnect attempt, which is error handling rather than a
//! query cycle.
//!
//! Acknowledgement ordering is deliberate and follows P12 D-7 option (a): the
//! ack goes out immediately after the frame is decoded, BEFORE the loop filter
//! and before the handler ever sees the event. Two consequences, both intended:
//! ignoring an event still acknowledges it (silence makes Slack redeliver a
//! frame we deliberately discarded), and a crash between ack and persist loses
//! at most one event — the same trade the Telegram variant already made with
//! state-before-emit.

use super::client::{SlackClient, SlackError};
use super::wire::{
    SlackFrame, SlackUserEvent, ack_frame, loop_drop_reason, parse_frame, parse_user_event,
};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Backoff for the reconnect loop.
///
/// This duplicates the shape of the Telegram variant's `BackoffState` on
/// purpose. That type is private to `proxy/io.rs`, and `proxy/io.rs` is under a
/// byte-identity gate for this work — importing it would mean editing the very
/// file whose non-modification is the regression proof. Thirty duplicated lines
/// are the cheaper side of that trade, and the roadmap carries the note to
/// unify once a third platform makes the right abstraction visible.
#[derive(Debug)]
enum Backoff {
    /// `current_ms == 0` means "reconnect immediately".
    Transient {
        /// Milliseconds to wait before the next attempt.
        current_ms: u64,
    },
    /// Dead credentials: wait a long, constant interval instead of hammering.
    Permanent,
}

impl Backoff {
    fn new() -> Self {
        Self::Transient { current_ms: 0 }
    }
    fn next_sleep(&self) -> Duration {
        match self {
            Self::Transient { current_ms } => Duration::from_millis(*current_ms),
            Self::Permanent => Duration::from_millis(300_000),
        }
    }
    fn after_transient_failure(&mut self) {
        if let Self::Transient { current_ms } = self {
            let next = if *current_ms == 0 {
                1000
            } else {
                (*current_ms * 2).min(60_000)
            };
            *self = Self::Transient { current_ms: next };
        }
    }
    fn after_permanent_failure(&mut self) {
        *self = Self::Permanent;
    }
    fn reset(&mut self) {
        *self = Self::Transient { current_ms: 0 };
    }
}

/// Runtime configuration for the Slack I/O task.
pub struct SlackIoConfig {
    /// Web API client (holds both tokens).
    pub client: SlackClient,
    /// Optional self-filter for loop rule R4.
    pub bot_user_id: Option<String>,
    /// A-timeout for `apps.connections.open` and the WebSocket handshake.
    pub connect_timeout_ms: u64,
    /// How long a connection must survive before it counts as healthy and
    /// clears the backoff. Without this floor, a peer that accepts and
    /// immediately drops would reset the backoff on every cycle and turn the
    /// reconnect path into a hot loop.
    pub min_uptime_ms: u64,
}

/// Live-updatable I/O settings (β params overlay, path B).
#[derive(Debug)]
pub enum SlackReconfig {
    /// Swap the Web API base URL; the client is rebuilt around the immutable
    /// tokens, which never cross the params surface.
    SetBaseUrl(String),
}

/// The reconnect loop: connect, pump, and reconnect on every ending.
///
/// Reconnects are event-driven — each one is caused by a `disconnect` frame, a
/// close or an error, never by a timer. The sleeps below are failure damping
/// and only ever run after something went wrong, which is what keeps this
/// inside the NO POLLING rule rather than beside it.
///
/// Shutdown is the reconfig channel closing, matching the Telegram I/O task.
pub async fn run_slack_io(
    mut cfg: SlackIoConfig,
    events_tx: mpsc::Sender<SlackInbound>,
    mut reconfig_rx: mpsc::Receiver<SlackReconfig>,
) {
    let mut backoff = Backoff::new();
    let mut own_app_id: Option<String> = None;
    let connect_timeout = Duration::from_millis(cfg.connect_timeout_ms);

    loop {
        let sleep_for = backoff.next_sleep();
        if !sleep_for.is_zero() {
            tokio::select! {
                biased;
                r = reconfig_rx.recv() => {
                    match r {
                        None => return,
                        Some(SlackReconfig::SetBaseUrl(url)) => {
                            cfg.client = cfg.client.with_base_url(&url);
                            continue;
                        }
                    }
                }
                _ = tokio::time::sleep(sleep_for) => {}
            }
        }

        let started = tokio::time::Instant::now();
        let end = tokio::select! {
            biased;
            r = reconfig_rx.recv() => {
                match r {
                    None => return,
                    Some(SlackReconfig::SetBaseUrl(url)) => {
                        cfg.client = cfg.client.with_base_url(&url);
                        continue;
                    }
                }
            }
            end = connect_and_run(
                &cfg.client,
                &events_tx,
                &mut own_app_id,
                cfg.bot_user_id.as_deref(),
                connect_timeout,
            ) => end,
        };

        let healthy = started.elapsed() >= Duration::from_millis(cfg.min_uptime_ms);
        match end {
            ConnectionEnd::Disconnect(reason) => {
                tracing::info!(%reason, "slack: server asked us to reconnect");
                if healthy {
                    backoff.reset();
                } else {
                    backoff.after_transient_failure();
                }
            }
            ConnectionEnd::Closed => {
                if healthy {
                    backoff.reset();
                } else {
                    backoff.after_transient_failure();
                }
            }
            ConnectionEnd::Transient(msg) => {
                tracing::warn!(error = %msg, "slack: connection failed, backing off");
                backoff.after_transient_failure();
            }
            ConnectionEnd::Fatal(msg) => {
                tracing::error!(error = %msg, "slack: credentials rejected");
                backoff.after_permanent_failure();
            }
        }
    }
}

/// An inbound event that survived the loop guard.
#[derive(Debug, Clone)]
pub struct SlackInbound {
    /// Envelope id, carried through for handler-side dedup.
    pub envelope_id: String,
    /// The decoded user event.
    pub event: SlackUserEvent,
}

/// Why a connection ended. Every variant is a reconnect trigger except
/// `Fatal`, which means the token itself is bad.
#[derive(Debug)]
pub enum ConnectionEnd {
    /// Slack sent `disconnect` with this reason — reconnect promptly.
    Disconnect(String),
    /// Socket closed by peer or stream exhausted.
    Closed,
    /// Transient failure; reconnect after backoff.
    Transient(String),
    /// Permanent failure (dead token, missing scope) — do not hammer.
    Fatal(String),
}

/// Opens one Socket Mode connection and pumps it until it ends.
///
/// `own_app_id` is written from the `hello` frame and read by loop rule R3, so
/// it is an in/out parameter rather than a return value: the identity has to be
/// known before the first event is filtered.
pub async fn connect_and_run(
    client: &SlackClient,
    tx: &mpsc::Sender<SlackInbound>,
    own_app_id: &mut Option<String>,
    bot_user_id: Option<&str>,
    connect_timeout: Duration,
) -> ConnectionEnd {
    let url = match client.apps_connections_open().await {
        Ok(u) => u,
        Err(SlackError::Permanent(m)) => return ConnectionEnd::Fatal(m),
        Err(SlackError::Transient(m)) => return ConnectionEnd::Transient(m),
    };

    // A-timeout around the WebSocket handshake (hard rule 12): without it a
    // silent TCP peer would hang the I/O task with no operation-level guard.
    let ws =
        match tokio::time::timeout(connect_timeout, tokio_tungstenite::connect_async(&url)).await {
            Err(_) => return ConnectionEnd::Transient("ws connect: timeout".into()),
            Ok(Err(e)) => return ConnectionEnd::Transient(format!("ws connect: {e}")),
            Ok(Ok((ws, _resp))) => ws,
        };
    let (mut write, mut read) = ws.split();

    while let Some(msg) = read.next().await {
        let text = match msg {
            Ok(WsMessage::Text(t)) => t.to_string(),
            Ok(WsMessage::Close(_)) => return ConnectionEnd::Closed,
            Ok(
                WsMessage::Ping(_)
                | WsMessage::Pong(_)
                | WsMessage::Binary(_)
                | WsMessage::Frame(_),
            ) => {
                continue;
            }
            Err(e) => return ConnectionEnd::Transient(format!("ws read: {e}")),
        };

        match parse_frame(&text) {
            Ok(SlackFrame::Hello { app_id, .. }) => {
                if app_id.is_some() {
                    *own_app_id = app_id;
                }
            }
            Ok(SlackFrame::EventsApi {
                envelope_id,
                payload,
                ..
            }) => {
                // Ack first, unconditionally — see the module note on D-7.
                if write
                    .send(WsMessage::Text(ack_frame(&envelope_id).into()))
                    .await
                    .is_err()
                {
                    return ConnectionEnd::Closed;
                }
                if loop_drop_reason(&payload, own_app_id.as_deref(), bot_user_id).is_some() {
                    continue;
                }
                if let Some(event) = parse_user_event(&payload)
                    && tx.send(SlackInbound { envelope_id, event }).await.is_err()
                {
                    // Handler is gone; this connection has no consumer left.
                    return ConnectionEnd::Closed;
                }
            }
            Ok(SlackFrame::Disconnect { reason }) => return ConnectionEnd::Disconnect(reason),
            Ok(SlackFrame::Other { envelope_id, .. }) => {
                if let Some(id) = envelope_id
                    && write
                        .send(WsMessage::Text(ack_frame(&id).into()))
                        .await
                        .is_err()
                {
                    return ConnectionEnd::Closed;
                }
            }
            Err(e) => {
                // A malformed frame is not a reason to drop a healthy socket.
                tracing::warn!(error = %e, "slack: unparsable frame ignored");
            }
        }
    }
    ConnectionEnd::Closed
}
