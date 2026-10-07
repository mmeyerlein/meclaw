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
    /// Issue #50: idle deadline for the read loop, reset by every frame. See
    /// `SlackParams::idle_timeout_ms` for the derivation of the default.
    pub idle_timeout_ms: u64,
    /// How long a connection must survive before it counts as healthy and
    /// clears the backoff. Without this floor, a peer that accepts and
    /// immediately drops would reset the backoff on every cycle and turn the
    /// reconnect path into a hot loop.
    pub min_uptime_ms: u64,
    /// Issue #7: progress mark, set on every frame this connection receives.
    /// Default = disabled (reports nowhere).
    pub liveness: meclaw_colony::IoLivenessMark,
}

/// Live-updatable I/O settings (β params overlay, path B).
#[derive(Debug)]
pub enum SlackReconfig {
    /// Swap the Web API base URL; the client is rebuilt around the immutable
    /// tokens, which never cross the params surface.
    SetBaseUrl(String),
    /// GH #1059: a token the vault delivered sealed, opened by the handler. The
    /// I/O half of a grant's connector sleeps until every token it waits for
    /// came this way; `Secret` prints `<sealed>`, so a `{:?}` of this frame
    /// cannot leak it.
    Credential {
        /// Which of the two tokens this is.
        slot: SlackTokenSlot,
        /// The delivered token.
        token: crate::credential::Secret,
    },
}

/// GH #1059: the two credentials of one Slack connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackTokenSlot {
    /// The app-level token (`xapp-…`): opens the Socket Mode connection.
    App,
    /// The bot token (`xoxb-…`): posts the answers.
    Bot,
}

/// What the I/O half hands its handler.
///
/// GH #1059: an enum because a sleeping connector's I/O half also reports the
/// end of a credential round without a box — the handler then asks again. The
/// connection loop itself only ever sends [`SlackEvent::Inbound`] (via `From`).
#[derive(Debug, Clone)]
pub enum SlackEvent {
    /// A user event that survived the loop guard.
    Inbound(SlackInbound),
    /// GH #1059: no sealed token arrived within the round's wait. Never a Slack
    /// event, so it touches neither the dedup table nor thread ownership.
    CredentialRoundExpired {
        /// The round that starts now (1 = the `on_start` question).
        round: u32,
        /// Its wait in ms (doubling, capped at `credential_backoff_max_ms`).
        wait_ms: u64,
    },
}

impl From<SlackInbound> for SlackEvent {
    fn from(inbound: SlackInbound) -> Self {
        Self::Inbound(inbound)
    }
}

/// GH #1059: how a connector of a grant waits for its tokens.
#[derive(Debug, Clone, Copy)]
pub struct SlackTokenWait {
    /// The round schedule (`credential_wait_ms`, `credential_backoff_max_ms`).
    pub wait: crate::proxy::io::CredentialWait,
    /// The app token comes from the vault (`app_token_grant_id`).
    pub need_app: bool,
    /// The bot token comes from the vault (`bot_token_grant_id`).
    pub need_bot: bool,
}

/// GH #1059: the sleeping start of a grant's connector. Opens no connection
/// and calls no Web API until EVERY token that comes from the vault arrived
/// (`cfg.client` then carries them); returns `false` when the handler is gone.
///
/// Both tokens, not just the app token the socket needs: a connection opened
/// before the bot token would take events in whose answers could only fail
/// with `credential_pending` — a bot that listens but cannot speak.
///
/// Every round that ends incomplete is reported to the handler
/// ([`SlackEvent::CredentialRoundExpired`]), which asks again for whatever is
/// still missing: a lost `on_start` question would otherwise leave the
/// connector asleep for ever. A params update in between is applied to `cfg`
/// and does not restart the wait (the clock's deadline is absolute).
pub async fn wait_for_slack_tokens(
    cfg: &mut SlackIoConfig,
    wait: SlackTokenWait,
    events_tx: &mpsc::Sender<SlackEvent>,
    reconfig_rx: &mut mpsc::Receiver<SlackReconfig>,
) -> bool {
    use crate::credential_rounds::{RoundClock, Wake, next_wake};
    let SlackTokenWait {
        wait,
        mut need_app,
        mut need_bot,
    } = wait;
    let mut clock = RoundClock::start(wait.wait_ms, wait.backoff_max_ms);
    while need_app || need_bot {
        match next_wake(reconfig_rx, &mut clock).await {
            Wake::Reconfig(SlackReconfig::Credential { slot, token }) => {
                cfg.client = swap_token(&cfg.client, slot, &token);
                match slot {
                    SlackTokenSlot::App => need_app = false,
                    SlackTokenSlot::Bot => need_bot = false,
                }
            }
            Wake::Reconfig(SlackReconfig::SetBaseUrl(url)) => {
                cfg.client = cfg.client.with_base_url(&url);
            }
            Wake::Closed => return false,
            Wake::RoundExpired { round, wait_ms } => {
                let event = SlackEvent::CredentialRoundExpired { round, wait_ms };
                if events_tx.send(event).await.is_err() {
                    return false;
                }
            }
        }
    }
    true
}

/// GH #1059: the client with one delivered token swapped in.
fn swap_token(
    client: &SlackClient,
    slot: SlackTokenSlot,
    token: &crate::credential::Secret,
) -> SlackClient {
    match slot {
        SlackTokenSlot::App => client.with_app_token(token.expose()),
        SlackTokenSlot::Bot => client.with_bot_token(token.expose()),
    }
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
    cfg: SlackIoConfig,
    events_tx: mpsc::Sender<SlackInbound>,
    reconfig_rx: mpsc::Receiver<SlackReconfig>,
) {
    run_slack_io_into(cfg, events_tx, reconfig_rx).await;
}

/// [`run_slack_io`], generic over the event type (GH #1059): the cell sends
/// [`SlackEvent`], a bare loop [`SlackInbound`] — the loop itself only ever
/// sends inbound events, converted with `From`. A separate function so the
/// public one keeps its concrete channel type.
pub(crate) async fn run_slack_io_into<E: From<SlackInbound> + Send>(
    mut cfg: SlackIoConfig,
    events_tx: mpsc::Sender<E>,
    mut reconfig_rx: mpsc::Receiver<SlackReconfig>,
) {
    let mut backoff = Backoff::new();
    let mut own_app_id: Option<String> = None;
    let connect_timeout = Duration::from_millis(cfg.connect_timeout_ms);
    let idle_timeout = Duration::from_millis(cfg.idle_timeout_ms);
    // Issue #7: announce before the first connect — a socket that never opens is
    // visibly "never succeeded" rather than invisible.
    let liveness = cfg.liveness.clone();
    liveness.announce();

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
                        // GH #1059: a token delivered again (a re-asked grant)
                        // replaces the one in use from the next connect on.
                        Some(SlackReconfig::Credential { slot, token }) => {
                            cfg.client = swap_token(&cfg.client, slot, &token);
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
                    Some(SlackReconfig::Credential { slot, token }) => {
                        cfg.client = swap_token(&cfg.client, slot, &token);
                        continue;
                    }
                }
            }
            end = connect_and_run_into(
                &cfg.client,
                &events_tx,
                &mut own_app_id,
                cfg.bot_user_id.as_deref(),
                connect_timeout,
                idle_timeout,
                &liveness,
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
///
/// Two deadlines, two shapes (issue #50). `connect_timeout` is an ordinary
/// A-timeout around a bounded operation — the handshake either completes or it
/// does not. The read loop that follows has no such boundary, so it carries
/// `idle_timeout` instead: the longest silence tolerated between two frames.
#[allow(clippy::too_many_arguments)]
pub async fn connect_and_run(
    client: &SlackClient,
    tx: &mpsc::Sender<SlackInbound>,
    own_app_id: &mut Option<String>,
    bot_user_id: Option<&str>,
    connect_timeout: Duration,
    idle_timeout: Duration,
    liveness: &meclaw_colony::IoLivenessMark,
) -> ConnectionEnd {
    connect_and_run_into(
        client,
        tx,
        own_app_id,
        bot_user_id,
        connect_timeout,
        idle_timeout,
        liveness,
    )
    .await
}

/// [`connect_and_run`], generic over the event type (GH #1059, see
/// [`run_slack_io_into`]).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn connect_and_run_into<E: From<SlackInbound> + Send>(
    client: &SlackClient,
    tx: &mpsc::Sender<E>,
    own_app_id: &mut Option<String>,
    bot_user_id: Option<&str>,
    connect_timeout: Duration,
    idle_timeout: Duration,
    liveness: &meclaw_colony::IoLivenessMark,
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
    // Issue #7: the socket is open — Slack answered `apps.connections.open` and
    // completed the WebSocket handshake, a full external round trip.
    liveness.mark_success();

    loop {
        // Issue #50: the idle deadline. `timeout` is re-created on every
        // iteration, so any arriving frame resets the clock — that is the whole
        // mechanism. On a blackholed path (NAT idle timeout, dropped route, no
        // FIN, no RST) nothing arrives at all: no frame, no ping, no error. The
        // bare `read.next().await` parked there forever and never reached any
        // of the `ConnectionEnd` returns that drive the reconnect, so the lane
        // looked idle from the outside while it was dead.
        let msg = match tokio::time::timeout(idle_timeout, read.next()).await {
            Err(_) => {
                // Transient, not fatal: silence says nothing about the
                // credentials, only about this socket. Backoff plus reconnect
                // is the whole response — no panic, no cell death.
                return ConnectionEnd::Transient(format!(
                    "ws read: idle for {idle_timeout:?}, no frame and no ping — \
                     socket presumed dead"
                ));
            }
            Ok(None) => break,
            Ok(Some(msg)) => msg,
        };
        // Issue #7: every frame — including the pings and the `hello` — is proof
        // that this connection is still carrying traffic. A Socket Mode lane is
        // frame-driven, so frame arrival is its round trip; the marks stay fresh
        // on a quiet channel and go stale on a dead one.
        liveness.mark_success();
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
                    && tx
                        .send(SlackInbound { envelope_id, event }.into())
                        .await
                        .is_err()
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
