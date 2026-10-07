//! Phase-10-C: I/O sub-task for `ProxyCell`. Defines the frame types
//! between handler and I/O. `run_io` follows in T7–T9. `ProxyReconfig`
//! is an empty enum — `proxy` needs no handler→I/O reconfig flow
//! (offset lives autonomously in the I/O task; see T4 plan rationale).

use crate::proxy::telegram::{TelegramClient, TelegramError};
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;

/// I/O → Handler: a user update from Telegram. Handler persists
/// `update_id + 1` into `cell.db.update_cursor` BEFORE the OriginSink
/// emit (Phase-5 canon: state-before-emit).
#[derive(Debug, Clone)]
pub enum ProxyEvent {
    /// A user message from Telegram. Platform fields are propagated as
    /// headers (spec cell-types.md Z.370).
    UserMessage {
        /// Telegram `update_id`. Cursor persistence: `save_offset(update_id + 1)`.
        update_id: i64,
        /// Telegram `chat.id` (mandatory in Telegram API's `Message` type).
        chat_id: i64,
        /// Telegram `from.id` (optional — may be absent on service messages).
        user_id: Option<i64>,
        /// Telegram `message.message_id` (optional — not set on some update
        /// types; e.g. `edited_message` has one, `callback_query` does not;
        /// 10-C consumes only `message`-typed updates, so usually present).
        message_id: Option<i64>,
        /// `message.text`.
        text: String,
    },
    /// GH #907: a `message.document`. `parse_update` yields it with
    /// `DocumentContent::NotFetched`; `run_io` fetches the bytes (`getFile` +
    /// download, capped at the client's `max_document_bytes`) before the
    /// handler sees it, so the handler never waits on the network.
    Document {
        /// Telegram `update_id` (cursor as for `UserMessage`).
        update_id: i64,
        /// Telegram `chat.id`.
        chat_id: i64,
        /// Telegram `from.id`.
        user_id: Option<i64>,
        /// Telegram `message.message_id`.
        message_id: Option<i64>,
        /// `message.caption`, empty when the document came without one.
        caption: String,
        /// `document.file_id`, what `getFile` is asked with.
        file_id: String,
        /// `document.file_name`, or `document`/`document.pdf` without one.
        name: String,
        /// `document.mime_type`, or `application/octet-stream` without one.
        mime: String,
        /// `document.file_size` as Telegram announced it.
        size: Option<u64>,
        /// The bytes, or why there are none.
        content: DocumentContent,
    },
    /// GH #907: an update the connector does not read (a photo, a voice note, a
    /// sticker, an edit). It carries only its id: the handler persists the
    /// cursor past it and emits nothing, so the update is acknowledged instead
    /// of being fetched again after every restart.
    Skipped {
        /// Telegram `update_id`.
        update_id: i64,
    },
    /// GH #1059: no sealed bot token arrived within the round's wait. Sent by
    /// the SLEEPING I/O half (a connector of a grant polls nothing until its
    /// box opened); the handler asks again with a fresh recipient. Never a
    /// Telegram update, so it moves no cursor.
    CredentialRoundExpired {
        /// The round that starts now (1 = the `on_start` question).
        round: u32,
        /// Its wait in ms (doubling, capped at `credential_backoff_max_ms`).
        wait_ms: u64,
    },
}

impl ProxyEvent {
    /// The Telegram `update_id` of any event -- what the cursor moves past.
    pub fn update_id(&self) -> i64 {
        match self {
            ProxyEvent::UserMessage { update_id, .. }
            | ProxyEvent::Document { update_id, .. }
            | ProxyEvent::Skipped { update_id } => *update_id,
            // Not an update: `run_io` only moves the cursor for polled events,
            // and -1 could not move it past anything.
            ProxyEvent::CredentialRoundExpired { .. } => -1,
        }
    }
}

/// GH #907: the bytes of one document (committed to the blob store by the
/// handler), or why there are none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentContent {
    /// Parsed, not yet fetched (only between `parse_update` and `run_io`).
    NotFetched,
    /// The downloaded bytes, at most `max_document_bytes`.
    Bytes(Vec<u8>),
    /// Over `max_document_bytes`: announced so, or grown so while downloading.
    TooLarge,
    /// A fixed failure code (`get_file_failed`, `download_failed`, `timeout`);
    /// never a URL -- the download URL carries the bot token.
    Failed(String),
}

/// Handler → I/O: live poll-config update (β, path B). The handler sends this
/// after a runtime `params`-update so the I/O-task's next poll uses the new
/// long-poll timeouts AND `base_url` WITHOUT a wake/respawn. `bot_token` is NOT
/// here — it is immutable; the I/O-task rebuilds its client via
/// `TelegramClient::with_base_url`, which reholds the existing token internally.
#[derive(Debug, Clone)]
pub enum ProxyReconfig {
    /// Apply new long-poll timeouts + base_url to the running I/O loop.
    SetPolling {
        /// New Telegram API base URL — the I/O-task rebuilds its client with it.
        base_url: String,
        /// New client-side A-timeout for `getUpdates`.
        long_poll_timeout_ms: u64,
        /// New Telegram-side long-poll wait (`timeout=<sec>`).
        long_poll_request_secs: u64,
    },
    /// GH #1059: the bot token, opened from the vault's sealed box. The I/O
    /// half of a grant's connector sleeps until this frame; on a running loop
    /// it swaps the token of the poll client. `Secret` prints `<sealed>`.
    Credential {
        /// The delivered bot token.
        bot_token: crate::credential::Secret,
    },
}

/// Configuration for the I/O sub-task. `split_io` of `ProxyCell` builds
/// this from parsed params + the loaded initial offset.
pub struct RunIoConfig {
    /// Telegram-Bot-API-Client (cloned in factory).
    pub client: TelegramClient,
    /// Initial offset loaded from `cell.db.update_cursor` (or 0 on first run).
    pub initial_offset: i64,
    /// Telegram-side long-poll request duration (`timeout` query param).
    pub long_poll_request_secs: u64,
    /// Client-side `tokio::time::timeout` envelope around the HTTP call. Two
    /// roles: inside `get_updates` it bounds the header phase (`send()`), and in
    /// `run_io` it is the margin of the outer hang-deadline
    /// (`long_poll_request_secs * 1000 + long_poll_timeout_ms`) that bounds the
    /// complete operation including the body read.
    pub long_poll_timeout_ms: u64,
    /// Issue #7: progress mark, set after every long poll that actually came
    /// back from Telegram. Default = disabled (reports nowhere), which is what
    /// every hand-built config in a test gets.
    pub liveness: meclaw_colony::IoLivenessMark,
}

/// Backoff state for `run_io`. Transient: exponential 1s → 60s; Permanent:
/// constant 5min. Reset to `Transient { current_ms: 0 }` after every
/// successful call — `0` means "no sleep" in the next tick.
enum BackoffState {
    /// `current_ms`: 0 after a successful call (no sleep); otherwise the
    /// next sleep value. Doubled after a failure, capped at 60_000.
    Transient {
        /// Milliseconds to sleep before the next poll.
        current_ms: u64,
    },
    /// Constant 5min sleep. Resets to `Transient { current_ms: 0 }` after
    /// the first successful call.
    Permanent,
}

impl BackoffState {
    fn next_sleep(&self) -> Duration {
        match self {
            BackoffState::Transient { current_ms } => Duration::from_millis(*current_ms),
            BackoffState::Permanent => Duration::from_millis(300_000),
        }
    }
    fn after_transient_failure(&mut self) {
        match self {
            BackoffState::Transient { current_ms } => {
                let next = if *current_ms == 0 {
                    1000
                } else {
                    (*current_ms * 2).min(60_000)
                };
                *self = BackoffState::Transient { current_ms: next };
            }
            BackoffState::Permanent => {} // stays permanent
        }
    }
    fn after_permanent_failure(&mut self) {
        *self = BackoffState::Permanent;
    }
    fn reset(&mut self) {
        *self = BackoffState::Transient { current_ms: 0 };
    }
}

/// I/O sub-task. `select!` over (a) reconfig-close (shutdown signal — in
/// 10-C the only reconfig path, because `ProxyReconfig` is empty) and
/// (b) a combined sleep+poll work-future. On update receipt: push events +
/// `running_offset = max(update_id) + 1`. Backoff via `BackoffState`
/// (transient expo 1s → 60s; permanent 5min).
///
/// Phase-10-A-Lesson (commit `31c15b6`): `events_tx` + `reconfig_rx`
/// are REFERENCED in this body — so auto-captured into the `async move`.
/// Defensively bound explicitly (`let events_tx = events_tx;`), so that
/// future refactors do not lose the binding.
///
/// T8-design (correction against the livelock bug): sleep + poll
/// live in ONE combined `work`-future per loop iteration, pinned INSIDE
/// the loop, wrapped in `select!` against `reconfig_rx.recv()`. This
/// guarantees (a) no race between a "sleep-arm" and a "poll-arm" (the
/// earlier two-arm-with-guard design dead-locked after the first failure
/// because the poll-arm stayed disabled until a successful poll reset the
/// backoff — which never happened), and (b) abort-during-sleep, because
/// `select!` cancels the whole `work`-future when `reconfig_rx`-close
/// fires. `tokio::pin!(work)` MUST be inside the loop: an `async {}`-block
/// is `!Unpin`, and a completed future is not re-awaitable — every
/// iteration needs a fresh `work`-future.
///
/// P15-Nachtrag (hard rule 12): the poll inside `work` carries an OUTER
/// `tokio::time::timeout` over the whole `get_updates` operation. The client
/// timeout inside `get_updates` covers the header phase only; the body read was
/// uncovered, which let a half-dead socket stall the lane silently and forever
/// (production incident 2026-08-11). Elapsed is classified `Transient` — the
/// backoff ladder takes it and the loop keeps living.
///
/// `+ Send` is load-bearing (see `TimerCell::run_io` doc / 10-A trait doc).
#[allow(clippy::manual_async_fn)]
pub fn run_io(
    cfg: RunIoConfig,
    events_tx: mpsc::Sender<ProxyEvent>,
    mut reconfig_rx: mpsc::Receiver<ProxyReconfig>,
) -> impl Future<Output = ()> + Send {
    async move {
        // Explicit binding (Phase-10-A-Lesson Second-Order-Trap):
        let events_tx = events_tx;
        let RunIoConfig {
            client,
            initial_offset,
            long_poll_request_secs,
            long_poll_timeout_ms,
            liveness,
        } = cfg;
        // Issue #7: announce first, so a lane that never manages a single poll
        // is visibly "never succeeded" rather than invisible.
        liveness.announce();
        let mut running_offset = initial_offset;
        // β (path B): poll config + base_url are mutable at runtime via
        // `ProxyReconfig::SetPolling`. The client is rebuilt live on a base_url
        // change (bot_token rehold internally — never from the update).
        let mut client = client;
        let mut long_poll_request_secs = long_poll_request_secs;
        let mut long_poll_timeout_ms = long_poll_timeout_ms;
        let mut backoff = BackoffState::Transient { current_ms: 0 };

        loop {
            let sleep_for = backoff.next_sleep();
            // Snapshot the (mutable) poll-config + a client clone (Arc-cheap) into
            // per-iteration locals so the `work` future borrows the snapshots, not
            // the outer mut state — the reconfig arm can then update them for the
            // next tick (e.g. rebuild `client` with a new base_url).
            let req_secs = long_poll_request_secs;
            let client_timeout = Duration::from_millis(long_poll_timeout_ms);
            let poll_client = client.clone();
            // Snapshot Copy-state (`i64`) per iteration into the async
            // block — avoids borrowing `running_offset` across the
            // mutation in the Ok-arm. `client` is `Clone` (Arc-internal)
            // and captured by ref via the outer `async move`; we re-use
            // it by reference inside `work` since `get_updates` only
            // needs `&self`.
            let poll_offset = running_offset;
            // ONE combined future per iteration: optional sleep, then
            // poll. Both `.await`-points under a SINGLE select!-arm — no
            // arm-competition, no livelock between sleep-arm and poll-arm.
            // select! against reconfig_rx.recv() cancels the entire
            // work-future on shutdown (W10: abort-during-sleep or
            // abort-during-poll).
            // Hard rule 12 envelope around the WHOLE `getUpdates` operation.
            // The `client_timeout` inside `get_updates` bounds only `send()`,
            // i.e. the header phase — the body read runs uncovered, so a peer
            // that answers with a head and then goes silent (NAT idle-timeout,
            // blackholed path: no FIN, no RST) hangs the lane forever. Measured
            // in production on 2026-08-11: silent proxy, zero log events, ESTAB
            // socket, updates waiting at Telegram.
            //
            // Budget = the server-side wait plus one full client envelope as
            // margin. The W7 tripwire guarantees `long_poll_timeout_ms >
            // long_poll_request_secs * 1000`, so the margin is always larger
            // than the legitimate wait — the deadline can never cut a valid
            // long poll, and it is always finite (defaults: 30 s + 35 s = 65 s).
            let hang_deadline = Duration::from_millis(
                req_secs
                    .saturating_mul(1000)
                    .saturating_add(long_poll_timeout_ms),
            );
            let work = async {
                if !sleep_for.is_zero() {
                    tokio::time::sleep(sleep_for).await;
                }
                let call = poll_client.get_updates(poll_offset, req_secs, client_timeout);
                match tokio::time::timeout(hang_deadline, call).await {
                    // GH #907: the documents of this batch are fetched HERE,
                    // inside `work`, so a shutdown cancels a download like it
                    // cancels a poll; every step of a fetch carries its own
                    // operation timeout (`client_timeout`, hard rule 12).
                    Ok(Ok(mut events)) => {
                        for ev in events.iter_mut() {
                            if let ProxyEvent::Document {
                                file_id,
                                size,
                                content,
                                ..
                            } = ev
                                && *content == DocumentContent::NotFetched
                            {
                                *content = poll_client
                                    .fetch_document(file_id, *size, client_timeout)
                                    .await;
                            }
                        }
                        Ok(events)
                    }
                    Ok(res) => res,
                    Err(_) => {
                        // Own, greppable message: this is NOT the ordinary
                        // client timeout (that one is a `Transient` from inside
                        // `get_updates`) — it is the backstop that fires when
                        // the call itself stopped making progress.
                        tracing::warn!(
                            deadline_ms = hang_deadline.as_millis() as u64,
                            "get_updates hung - killed by the outer deadline (transient, lane continues)"
                        );
                        Err(TelegramError::Transient("get_updates hung".into()))
                    }
                }
            };
            tokio::pin!(work);

            tokio::select! {
                biased;
                maybe_rc = reconfig_rx.recv() => match maybe_rc {
                    // β (path B): apply the new poll config + base_url live; next
                    // iteration's `work` snapshots the updated values + rebuilt
                    // client. `with_base_url` reholds the immutable bot_token.
                    Some(ProxyReconfig::SetPolling {
                        base_url,
                        long_poll_timeout_ms: new_to,
                        long_poll_request_secs: new_secs,
                    }) => {
                        client = client.with_base_url(&base_url);
                        long_poll_timeout_ms = new_to;
                        long_poll_request_secs = new_secs;
                        continue;
                    }
                    Some(ProxyReconfig::Credential { bot_token }) => {
                        client = client.with_bot_token(bot_token.expose());
                        continue;
                    }
                    None => break,
                },
                result = &mut work => match result {
                    Ok(events) => {
                        // Issue #7: the far side answered. This is the ONLY
                        // place with proof of a completed round trip — a poll
                        // that returns zero updates is a success too, which is
                        // exactly why event traffic cannot stand in for it.
                        liveness.mark_success();
                        backoff.reset();
                        for ev in events {
                            let update_id = ev.update_id();
                            running_offset = running_offset.max(update_id + 1);
                            if events_tx.send(ev).await.is_err() {
                                return; // handler dead → shutdown
                            }
                        }
                    }
                    Err(TelegramError::Transient(reason)) => {
                        tracing::debug!(?reason, "get_updates transient");
                        backoff.after_transient_failure();
                    }
                    // GH #468: a 409 recovers like a transient — the other
                    // consumer may stop, and a switchover is the case where it
                    // does — but it is the one failure an operator can act on,
                    // so it leaves the DEBUG bucket and names itself. Two
                    // pollers on one token used to be indistinguishable from a
                    // quiet chat.
                    Err(TelegramError::Conflict(reason)) => {
                        tracing::warn!(
                            error_code = "conflict_other_poller",
                            ?reason,
                            "getUpdates refused: another consumer holds this bot token \
                             (one getUpdates consumer per token; the lane keeps polling)"
                        );
                        backoff.after_transient_failure();
                    }
                    Err(TelegramError::Permanent(reason)) => {
                        tracing::warn!(?reason, "get_updates permanent (5min sleep)");
                        backoff.after_permanent_failure();
                    }
                }
            }
        }
    }
}

/// GH #1059: how a connector of a grant waits for its bot token.
#[derive(Debug, Clone, Copy)]
pub struct CredentialWait {
    /// Wait of the first round (`credential_wait_ms`).
    pub wait_ms: u64,
    /// Ceiling of the doubling wait (`credential_backoff_max_ms`).
    pub backoff_max_ms: u64,
}

/// GH #1059: the sleeping start of a grant's connector. Polls nothing and
/// returns `true` once the bot token arrived (`cfg.client` then carries it);
/// `false` when the handler is gone. Every round that ends without a box is
/// reported to the handler (`ProxyEvent::CredentialRoundExpired`), which asks
/// again — without that, a lost `on_start` question (broker not spawned yet,
/// vault locked, an error instead of a box) would leave the connector asleep
/// for ever: unlike an llm turn, nothing parked here starts a new round. A
/// params update in between is applied to `cfg` and does not restart the wait.
pub async fn wait_for_bot_token(
    cfg: &mut RunIoConfig,
    wait: CredentialWait,
    events_tx: &mpsc::Sender<ProxyEvent>,
    reconfig_rx: &mut mpsc::Receiver<ProxyReconfig>,
) -> bool {
    use crate::credential_rounds::{RoundClock, Wake, next_wake};
    let mut clock = RoundClock::start(wait.wait_ms, wait.backoff_max_ms);
    loop {
        match next_wake(reconfig_rx, &mut clock).await {
            Wake::Reconfig(ProxyReconfig::Credential { bot_token }) => {
                cfg.client = cfg.client.with_bot_token(bot_token.expose());
                return true;
            }
            Wake::Reconfig(ProxyReconfig::SetPolling {
                base_url,
                long_poll_timeout_ms,
                long_poll_request_secs,
            }) => {
                cfg.client = cfg.client.with_base_url(&base_url);
                cfg.long_poll_timeout_ms = long_poll_timeout_ms;
                cfg.long_poll_request_secs = long_poll_request_secs;
            }
            Wake::Closed => return false,
            Wake::RoundExpired { round, wait_ms } => {
                let event = ProxyEvent::CredentialRoundExpired { round, wait_ms };
                if events_tx.send(event).await.is_err() {
                    return false;
                }
            }
        }
    }
}
