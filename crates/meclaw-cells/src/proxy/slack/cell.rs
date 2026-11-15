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
use super::db::{
    RememberedReaction, claim_thread, forget_reaction, mark_envelope_seen, owns_thread,
    remember_reaction, remembered_reactions,
};
use super::emit::{build_user_turn_content, composite_chat_id, split_chat_id};
use super::io::{
    SlackEvent, SlackInbound, SlackIoConfig, SlackReconfig, SlackTokenSlot, SlackTokenWait,
    run_slack_io_into, wait_for_slack_tokens,
};
use super::params::SlackParams;
use super::reaction::{RemoveRetry, SlackReaction, remove_until_confirmed};
use super::wire::SlackEventKind;
use crate::credential::CredentialSlots;
use crate::proxy::io::CredentialWait;
use crate::proxy::typing::{TypingCadence, TypingKeepers};
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
    /// GH #1059: a token (or both) comes sealed from the vault. `None` = both
    /// tokens are literals (the one-release transition, OR-VG-4).
    credential: Option<SlackCredential>,
    /// GH #1530: the emoji of the running turn's reaction (`params
    /// .typing_reaction`); `None` = off, no reaction call at all.
    typing_reaction: Option<String>,
    /// GH #1530: the keepers of the running turns' reactions, one per chat
    /// (`context.chat_id`). `handle_event` starts one, `handle` ends it with
    /// the first answer without `interim`, the ceiling ends it otherwise.
    typing: TypingKeepers<String>,
    /// GH #1530: whether the reactions a previous run left standing were
    /// taken away already (once per start, as soon as the bot token is
    /// there).
    left_behind_cleared: bool,
    /// GH #1530: the task taking them away; aborted when the cell goes, like
    /// the held keepers. Its rows stay until a removal is confirmed, so an
    /// abort costs nothing but the next start's try.
    left_behind: Option<tokio::task::JoinHandle<()>>,
    /// OR-d54g.acktyp.2: how a failed `reactions.remove` is tried again.
    remove_retry: RemoveRetry,
    /// OR-d54g.acktyp.2: the keepers' end of the report lane -- each confirmed
    /// removal is reported here, and the handler forgets its row.
    reaction_reports: mpsc::UnboundedSender<SlackEvent>,
    /// OR-d54g.acktyp.2: the other end, until the I/O half takes it in
    /// `split_io` and hands each report on as an event. The state stays in
    /// the tasks: no mutex, no atomics, only the handler touches the `cell.db`.
    reaction_reports_rx: Option<mpsc::UnboundedReceiver<SlackEvent>>,
}

impl Drop for SlackCell {
    fn drop(&mut self) {
        if let Some(task) = self.left_behind.take() {
            task.abort();
        }
    }
}

/// GH #1059: the grants of a Slack connector whose tokens the vault delivers.
/// Key pairs and tokens live in RAM only, so every (re)start asks again.
struct SlackCredential {
    /// `params.app_token_grant_id`.
    app_grant: Option<String>,
    /// `params.bot_token_grant_id`.
    bot_grant: Option<String>,
    /// One slot per grant. Nothing is ever parked in them: the I/O half
    /// connects nowhere before the boxes, and an outbound send before the bot
    /// token is answered `credential_pending` at once.
    slots: CredentialSlots<()>,
    /// The I/O half's waiting schedule.
    wait: CredentialWait,
}

impl SlackCredential {
    /// Every grant this connector spends, each once (the same grant id for
    /// both tokens is one question, not two).
    fn grants(&self) -> Vec<String> {
        let mut grants: Vec<String> = Vec::new();
        for g in [&self.app_grant, &self.bot_grant].into_iter().flatten() {
            if !grants.contains(g) {
                grants.push(g.clone());
            }
        }
        grants
    }

    /// The token slots a grant fills.
    fn slots_of(&self, grant: &str) -> Vec<SlackTokenSlot> {
        let mut out = Vec::new();
        if self.app_grant.as_deref() == Some(grant) {
            out.push(SlackTokenSlot::App);
        }
        if self.bot_grant.as_deref() == Some(grant) {
            out.push(SlackTokenSlot::Bot);
        }
        out
    }

    /// The bot token has not arrived yet (only ever true with a bot grant).
    fn bot_pending(&self) -> bool {
        self.bot_grant
            .as_deref()
            .is_some_and(|g| self.slots.secret(g).is_none())
    }
}

/// I/O-local state. Single owner, held by value by the I/O sub-task.
pub struct SlackIo {
    cfg: SlackIoConfig,
    /// GH #1059: `Some` = sleep until the vault's tokens arrived.
    wait: Option<SlackTokenWait>,
    /// OR-d54g.acktyp.2: the report lane, handed on to the handler.
    reports: Option<mpsc::UnboundedReceiver<SlackEvent>>,
}

/// OR-d54g.acktyp.2: hands every report of a confirmed removal on to the
/// handler, as an event like any other of the I/O half. Ends only when the
/// handler is gone; with no lane (a test plays the I/O half) it never ends.
async fn hand_on_reports(
    reports: Option<mpsc::UnboundedReceiver<SlackEvent>>,
    events_tx: mpsc::Sender<SlackEvent>,
) {
    let Some(mut reports) = reports else {
        return std::future::pending().await;
    };
    while let Some(report) = reports.recv().await {
        if events_tx.send(report).await.is_err() {
            return; // handler gone → the cell tears down
        }
    }
    // Every sender gone: the cell itself is gone.
    std::future::pending().await
}

impl SlackCell {
    /// Builds the cell from validated params plus the client the factory made.
    pub fn new(params: &SlackParams, client: SlackClient) -> Self {
        let io_client = client.clone();
        // OR-d54g.acktyp.2: unbounded, because a report must never wait on a
        // full lane -- it is at most one per reaction the connector set.
        let (reports_tx, reports_rx) = mpsc::unbounded_channel();
        Self {
            client,
            emit_to: params.emit_to.clone(),
            thread_follow: params.thread_follow,
            initial_io_cfg: Some(SlackIoConfig {
                client: io_client,
                bot_user_id: params.bot_user_id.clone(),
                connect_timeout_ms: params.connect_timeout_ms,
                idle_timeout_ms: params.idle_timeout_ms,
                min_uptime_ms: 5_000,
                // Replaced by the substrate via `attach_liveness`.
                liveness: meclaw_colony::IoLivenessMark::disabled(),
            }),
            credential: None,
            typing_reaction: params.typing_reaction.clone(),
            typing: TypingKeepers::new(TypingCadence::for_connector(params.typing_max_ms, None)),
            left_behind_cleared: false,
            left_behind: None,
            remove_retry: RemoveRetry {
                retries: params.typing_remove_retries,
                gap: std::time::Duration::from_millis(params.typing_remove_retry_ms),
            },
            reaction_reports: reports_tx,
            reaction_reports_rx: Some(reports_rx),
        }
    }

    /// OR-d54g.acktyp.2: the far end of the report lane. The I/O half takes it
    /// in `split_io`; a test that drives the handler alone takes it here and
    /// hands each report to `handle_event` itself, as the I/O half would.
    pub fn take_reaction_reports(&mut self) -> Option<mpsc::UnboundedReceiver<SlackEvent>> {
        self.reaction_reports_rx.take()
    }

    /// GH #1530: takes away every reaction a previous run of this cell left
    /// standing -- set, and then the cell went down before the turn's end
    /// (the keeper and its ceiling died with it), or its removal failed after
    /// all its tries. Runs once per start, as soon as the bot token is there
    /// (`on_start`, else the delivery of the sealed token). The calls run in a
    /// task of their own, each under the client's `send_timeout`, so neither
    /// the start nor any answer waits on Slack. Reading forgets nothing
    /// (OR-d54g.acktyp.2): a confirmed removal is reported on the lane and
    /// forgets its row there; a failed one keeps it for the next start.
    /// Independent of `typing_reaction`: a set that switched it off since
    /// still takes away what the earlier run left.
    async fn take_away_left_behind(&mut self, db: &mut DbConn) {
        if self.left_behind_cleared
            || self
                .credential
                .as_ref()
                .is_some_and(SlackCredential::bot_pending)
        {
            return;
        }
        self.left_behind_cleared = true;
        let rows = match db.call_with_timeout(|c| remembered_reactions(c)).await {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "slack proxy: reading the reactions left behind failed");
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "slack proxy: reading the reactions left behind timed out");
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        let client = self.client.clone();
        let retry = self.remove_retry;
        let reports = self.reaction_reports.clone();
        self.left_behind = Some(tokio::spawn(async move {
            for r in rows {
                match remove_until_confirmed(&client, &r, retry, &reports).await {
                    Ok(()) => tracing::info!(
                        channel = %r.channel,
                        "slack proxy: took away a reaction a previous run left standing"
                    ),
                    Err(e) => tracing::warn!(
                        channel = %r.channel,
                        error = %e,
                        "slack proxy: taking away a reaction a previous run left standing \
                         failed -- the next start tries again"
                    ),
                }
            }
        }));
    }

    /// OR-d54g.acktyp.2: Slack confirmed the removal of `reaction` (the
    /// keeper's report, or the start's) -- the `cell.db` forgets it. Only
    /// here: a row whose removal was not confirmed is what the next start
    /// takes away. A failed write is logged; the row then costs the next
    /// start one removal of nothing, which is no failure.
    async fn forget_removed(&self, db: &mut DbConn, reaction: RememberedReaction) {
        match db
            .call_with_timeout(move |c| forget_reaction(c, &reaction))
            .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "slack proxy: forgetting a removed reaction failed")
            }
            Err(e) => {
                tracing::warn!(error = %e, "slack proxy: forgetting a removed reaction timed out")
            }
        }
    }

    /// GH #1530: the cadence of the reaction keeper. The factory sets the
    /// production one (ceiling = `typing_max_ms`, else the backstop the colony
    /// resolved for this cell, GH #1099); a running keeper keeps its own.
    pub fn set_typing_cadence(&mut self, cadence: TypingCadence) {
        self.typing.set_cadence(cadence);
    }

    /// GH #1059: the app token, the bot token or both come from the vault,
    /// sealed, under their grants. The cell asks in `on_start`; its I/O half
    /// sleeps — no `apps.connections.open`, no socket — until every vaulted
    /// token arrived, and re-asks after every round that ends without them
    /// (`wait`, doubling up to its ceiling). The client this cell was built
    /// with must carry NO literal for a vaulted token (the factory blanks it):
    /// a literal next to a grant is never used, not even while the box is
    /// missing (OR-VG-4, ruling 22.09.). Both grants `None` = unchanged cell.
    #[must_use]
    pub fn with_credential(
        mut self,
        app_grant: Option<&str>,
        bot_grant: Option<&str>,
        wait: CredentialWait,
    ) -> Self {
        let app_grant = app_grant.filter(|g| !g.is_empty()).map(str::to_string);
        let bot_grant = bot_grant.filter(|g| !g.is_empty()).map(str::to_string);
        if app_grant.is_none() && bot_grant.is_none() {
            return self;
        }
        let on_expired: crate::credential::ExpiryFn<()> =
            std::sync::Arc::new(|_, _| Box::pin(async {}));
        self.credential = Some(SlackCredential {
            app_grant,
            bot_grant,
            slots: CredentialSlots::new(
                wait.wait_ms,
                crate::credential::default_credential_wait_max(),
                on_expired,
            ),
            wait,
        });
        self
    }

    /// The public recipient key of `grant`'s question in flight
    /// (tests/diagnosis; never a secret).
    pub fn credential_recipient_hex(&self, grant: &str) -> Option<String> {
        self.credential.as_ref()?.slots.recipient_hex(grant)
    }

    /// GH #1059: ask the access hive for the token of `grant`. Emitted towards
    /// `emit_to` like every other source emission of this cell; the template's
    /// edge on `hop.route == "credential_request"` decides where it goes.
    async fn ask_for_token(&mut self, sink: &OriginSink, grant: &str) {
        let Some(c) = self.credential.as_mut() else {
            return;
        };
        let content = match c.slots.request(grant) {
            Ok(content) => content,
            Err(e) => {
                tracing::error!(error = %e, "proxy: no random source for a credential request");
                return;
            }
        };
        tracing::info!(grant = %grant, "proxy: asking the vault for a slack token");
        let _ = sink
            .emit(meclaw_core::CellOutput {
                target: self.emit_to.clone(),
                content,
            })
            .await;
    }

    /// GH #1059: take a sealed delivery. Opened → the token goes to the
    /// handler's client (bot token) and to the I/O half, which connects once it
    /// holds every vaulted token. Not opened → the connector stays asleep; its
    /// next round asks again. Never echoes a value, never answered with an
    /// emission.
    async fn accept_token(
        &mut self,
        content: &meclaw_core::serde_json::Value,
        reconfig_tx: &mpsc::Sender<SlackReconfig>,
    ) {
        let Some(c) = self.credential.as_mut() else {
            tracing::warn!(
                "proxy: a sealed box arrived, but this slack connector has no app_token_grant_id \
                 or bot_token_grant_id — discarded"
            );
            return;
        };
        let (grant, token) = match c.slots.accept_sealed(content).await {
            Ok(accepted) => match c.slots.secret_handle(&accepted.grant) {
                Some(token) => (accepted.grant, token),
                None => {
                    tracing::warn!(
                        grant = %accepted.grant,
                        "proxy: the sealed slack token opened empty — the connector stays asleep"
                    );
                    return;
                }
            },
            Err(refusal) => {
                match refusal.detail() {
                    None => tracing::warn!(
                        "proxy: a sealed slack token of an earlier credential round arrived late \
                         and was discarded"
                    ),
                    Some(detail) => tracing::warn!(
                        %detail,
                        "proxy: the sealed slack token was refused — the connector stays asleep \
                         and asks again after the round's wait"
                    ),
                }
                return;
            }
        };
        let slots = c.slots_of(&grant);
        tracing::info!(grant = %grant, "proxy: slack token received sealed and opened in RAM");
        for slot in slots {
            // The handler only ever posts, so only the bot token lives here too.
            if slot == SlackTokenSlot::Bot {
                self.client = self.client.with_bot_token(token.expose());
            }
            let _ = reconfig_tx
                .send(SlackReconfig::Credential {
                    slot,
                    token: token.clone(),
                })
                .await;
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
    type Event = SlackEvent;
    type Reconfig = SlackReconfig;
    type Io = SlackIo;

    fn split_io(&mut self) -> Self::Io {
        SlackIo {
            cfg: self
                .initial_io_cfg
                .take()
                .expect("split_io called twice — the colony calls it exactly once"),
            wait: self.credential.as_ref().map(|c| SlackTokenWait {
                wait: c.wait,
                need_app: c.app_grant.is_some(),
                need_bot: c.bot_grant.is_some(),
            }),
            reports: self.reaction_reports_rx.take(),
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
        async move {
            let SlackIo {
                mut cfg,
                wait,
                reports,
            } = io;
            let mut reconfig_rx = reconfig_rx;
            // OR-d54g.acktyp.2: the reports of confirmed removals reach the
            // handler beside the socket, also while it waits for its tokens.
            let reports = hand_on_reports(reports, events_tx.clone());
            let socket = async move {
                // GH #1059: a grant's connector connects nowhere before its tokens.
                if let Some(wait) = wait
                    && !wait_for_slack_tokens(&mut cfg, wait, &events_tx, &mut reconfig_rx).await
                {
                    return; // handler gone → the cell tears down
                }
                run_slack_io_into(cfg, events_tx, reconfig_rx).await;
            };
            tokio::select! {
                () = socket => {}
                () = reports => {}
            }
        }
    }

    /// GH #1059: a grant's connector asks for its tokens before anything else,
    /// one question per grant; key pairs and tokens are RAM-only, so a respawn
    /// asks again.
    #[allow(clippy::manual_async_fn)]
    fn on_start<'a>(
        &'a mut self,
        sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            let grants = self
                .credential
                .as_ref()
                .map(SlackCredential::grants)
                .unwrap_or_default();
            for grant in grants {
                self.ask_for_token(sink, &grant).await;
            }
            // GH #1530: what a previous run left standing goes now -- or,
            // with a vaulted bot token, once it arrived.
            self.take_away_left_behind(db).await;
        }
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
        db: &'a mut DbConn,
        reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
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

            // GH #1059: a sealed token. Never touches `cell.db`, never
            // answered with an emission.
            if body_val.get("sealed").is_some() {
                self.accept_token(&body_val, reconfig_tx).await;
                // GH #1530: the bot token may be what the start waited for.
                self.take_away_left_behind(db).await;
                return;
            }
            // GH #1059: no bot token yet → nothing to post with. Answered at
            // once, and BEFORE the at-most-once booking below: a booked message
            // that was never posted would be swallowed by the replay.
            if self
                .credential
                .as_ref()
                .is_some_and(SlackCredential::bot_pending)
            {
                crate::proxy::emit::emit_inbound_error(
                    sink,
                    &msg,
                    "credential_pending",
                    "the slack bot token has not arrived from the vault yet",
                )
                .await;
                return;
            }

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

            // GH #1015 (OR-HV-61): after a crash the colony replays open
            // deliveries, so this exact message may already have been posted.
            // Book its id BEFORE the post (at-most-once, window in
            // `proxy::consumed`): a human must never get the same answer twice.
            // A replay posts nothing and answers as the original did (GH #1043
            // review): silent after a confirmed post, the original's
            // `send_failed` after a failed one, `send_failed` `unconfirmed` when
            // the crash fell before the answer was booked. A failed booking does
            // not post either, because without the record the post could be the
            // second.
            let once_key = msg.id.to_string();
            let booked = db
                .call(move |c| crate::proxy::consumed::book_once(c, &once_key))
                .await;
            let replay_detail = match booked {
                Ok(crate::proxy::consumed::Booking::First) => None,
                Ok(crate::proxy::consumed::Booking::Replay(prior)) => {
                    use crate::proxy::consumed::{Prior, UNCONFIRMED_DETAIL};
                    match prior {
                        Prior::Sent => return,
                        Prior::Failed(detail) => Some(detail),
                        Prior::Unconfirmed => Some(UNCONFIRMED_DETAIL.to_string()),
                    }
                }
                Err(e) => Some(format!("cell.db consumed record failed, not sent: {e}")),
            };
            if let Some(detail) = replay_detail {
                // GH #1530: a replayed answer is the end of its turn, as for
                // Telegram's status (GH #515). The row goes with the confirmed
                // removal (OR-d54g.acktyp.2), not here.
                self.typing.stop(&chat_id);
                crate::proxy::emit::emit_inbound_error(sink, &msg, "send_failed", &detail).await;
                return;
            }

            // The A-timeout for this call lives inside the client, built from
            // `send_timeout_ms` at construction time.
            let (channel, thread_ts) = split_chat_id(&chat_id);
            // GH #1505 (d54g, finding 16): an answer marked `repeat` carries a
            // text this turn already posted -- the model wrote its sentence
            // beside a call and wrote it again in the next round, and the person
            // read the same answer twice in the channel. It is not posted;
            // booking and settling stay those of a posted answer, so a replay
            // stays silent.
            let repeat = msg.headers.hop.get("repeat").and_then(|v| v.as_str()) == Some("1");
            let failure = if repeat {
                None
            } else {
                self.client
                    .chat_post_message(&channel, &text, thread_ts.as_deref())
                    .await
                    .err()
                    .map(|e| format!("{e}"))
            };
            // GH #1530: the answer is posted (or definitively failed, or a
            // repeat that posts nothing) -- the reaction of the turn goes.
            // After the post, not before: it stands while the post runs. An
            // interim is not the answer and leaves it standing; the ceiling
            // still ends a turn whose answer never comes. `stop` only hands
            // the end to the keeper's task, so the removal never holds the
            // answer up. The row in the `cell.db` goes only once the keeper
            // reports the removal confirmed (OR-d54g.acktyp.2).
            if msg.headers.hop.get("interim").and_then(|v| v.as_str()) != Some("1") {
                self.typing.stop(&chat_id);
            }
            // GH #1043 review: book the answer, so a replay answers like this.
            // A booking that fails leaves the row unconfirmed, and its replay
            // reports `unconfirmed` -- the honest answer without a record.
            let once_key = msg.id.to_string();
            let answer = failure.clone();
            if let Err(e) = db
                .call(move |c| {
                    crate::proxy::consumed::settle(
                        c,
                        &once_key,
                        answer.as_deref().map_or(Ok(()), Err),
                    )
                })
                .await
            {
                tracing::warn!(error = %e, "slack proxy: booking the post's answer failed");
            }
            if let Some(detail) = failure {
                crate::proxy::emit::emit_inbound_error(sink, &msg, "send_failed", &detail).await;
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
            let SlackInbound { envelope_id, event } = match event {
                SlackEvent::Inbound(inbound) => inbound,
                // GH #1059: a round ended without every vaulted token → ask
                // again for each one still missing, fresh recipient. A box that
                // opened in between makes it moot for its grant.
                SlackEvent::CredentialRoundExpired { round, wait_ms } => {
                    let pending: Vec<String> = self
                        .credential
                        .as_ref()
                        .map(|c| {
                            c.grants()
                                .into_iter()
                                .filter(|g| c.slots.secret(g).is_none())
                                .collect()
                        })
                        .unwrap_or_default();
                    for grant in pending {
                        tracing::warn!(
                            round,
                            wait_ms,
                            grant = %grant,
                            "proxy: no sealed slack token arrived in time — asking again"
                        );
                        self.ask_for_token(sink, &grant).await;
                    }
                    return;
                }
                // OR-d54g.acktyp.2: a removal Slack confirmed -- forget its row.
                SlackEvent::ReactionRemoved(reaction) => {
                    self.forget_removed(db, reaction).await;
                    return;
                }
            };
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

            // GH #1530: the sign of the running turn goes up before the
            // emission, so the person sees it in the first moment. `start`
            // only spawns the keeper's task; nothing here waits on Slack.
            // Without the bot token there is nothing to call with.
            if let Some(name) = self.typing_reaction.clone()
                && !self
                    .credential
                    .as_ref()
                    .is_some_and(SlackCredential::bot_pending)
            {
                // GH #1530: remembered before the add goes out, and forgotten
                // only once its removal is confirmed (OR-d54g.acktyp.2), so a
                // restart before the turn's end, a removal that failed, or an
                // add that failed at its timeout and still went through leaves
                // a row the next start takes away. A failed write costs the
                // sign nothing.
                let chat = composite_chat_id(&event.channel, thread_ts.as_deref());
                let remembered = RememberedReaction {
                    channel: event.channel.clone(),
                    ts: event.ts.clone(),
                    name,
                };
                let key = chat.clone();
                let row = remembered.clone();
                if let Err(e) = db
                    .call_with_timeout(move |c| remember_reaction(c, &key, &row, now))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()))
                {
                    tracing::warn!(error = %e, "slack proxy: remembering the turn's reaction failed");
                }
                self.typing.start(
                    chat,
                    SlackReaction {
                        client: self.client.clone(),
                        reaction: remembered,
                        retry: self.remove_retry,
                        reports: self.reaction_reports.clone(),
                    },
                );
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
