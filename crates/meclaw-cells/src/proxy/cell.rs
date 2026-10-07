//! Phase-10-C: `ProxyCell` — implements `LongRunningCell` from 10-A.
//! The handler is the DB authority (cursor persist + inbound sink calls); the I/O
//! side polls Telegram in an endless loop. State is single-threaded in the
//! handler sub-task (no mutex — phase-1 discipline).

use crate::credential::CredentialSlots;
use crate::proxy::io::{
    CredentialWait, ProxyEvent, ProxyReconfig, RunIoConfig, run_io, wait_for_bot_token,
};
use crate::proxy::telegram::TelegramClient;
use crate::proxy::typing::{TypingCadence, TypingKeepers};
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Message, OriginSink, OutputSink, Path};
use std::future::Future;
use tokio::sync::mpsc;

/// The `proxy` cell. State lives single-threaded in the handler sub-task of
/// `cell_task_long_running`. `initial_io_cfg` is pulled out once by `split_io`
/// and handed to the I/O task.
pub struct ProxyCell {
    /// reqwest client clone (Arc internally) for the `handle`/`sendMessage` calls.
    pub(crate) client: TelegramClient,
    /// Routing target for user-source messages (W4, mandatory field).
    pub(crate) emit_to: Path,
    /// A timeout for `sendMessage` from `handle` (W7). β: mutable, path A (live).
    pub(crate) send_timeout_ms: u64,
    /// β: live poll config (path B) — held so a params update can merge over it
    /// and signal the I/O-task via `ProxyReconfig::SetPolling`.
    pub(crate) long_poll_timeout_ms: u64,
    /// β: live poll config (path B).
    pub(crate) long_poll_request_secs: u64,
    /// β: live Telegram API base URL (path B). On a params update the handler
    /// rebuilds `self.client` (sendMessage) AND signals the I/O-task to rebuild
    /// its client — both via `TelegramClient::with_base_url` (bot_token rehold).
    pub(crate) base_url: String,
    /// β: live `query_timeout_ms` (path C, cell.db ops via DbConn).
    pub(crate) query_timeout_ms: u64,
    /// GH #515: the typing keepers, one per chat with a turn in flight.
    /// `handle_event` starts one, `handle` stops it once the answer is out.
    /// This is the only place that sees both ends of a turn — the I/O sub-task
    /// sees the poll and could never stop anything.
    pub(crate) typing: TypingKeepers,
    /// Initial I/O config, consumed exactly once by `split_io`.
    pub(crate) initial_io_cfg: Option<RunIoConfig>,
    /// GH #907: the colony blob store a fetched document is committed to
    /// before its turn goes out (the turn carries the reference, never the
    /// bytes). `None` -- a cell built outside a colony -- turns every document
    /// into the fallback line `no_blob_store`.
    pub(crate) blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
    /// GH #1059: the bot token comes sealed from the vault (`bot_token_grant_id`).
    /// `None` = a literal `bot_token` (the one-release transition, OR-VG-4).
    pub(crate) credential: Option<ProxyCredential>,
}

/// GH #1059: the grant of a connector whose bot token the vault delivers.
/// Key pair and token live in RAM only, so every (re)start asks again.
pub(crate) struct ProxyCredential {
    /// `params.bot_token_grant_id`.
    pub(crate) grant: String,
    /// The slot of that grant. Nothing is ever parked in it: the I/O half
    /// polls nothing before the box, and an outbound send before it is
    /// answered `credential_pending` at once.
    pub(crate) slots: CredentialSlots<()>,
    /// The I/O half's waiting schedule.
    pub(crate) wait: CredentialWait,
}

impl ProxyCell {
    /// Constructor. `initial_offset` comes from the factory (sync `load_offset`
    /// from `cell.db`, W9 resume path). `client` is the initially built reqwest
    /// client; `split_io` clones it internally (through `RunIoConfig`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: TelegramClient,
        emit_to: Path,
        initial_offset: i64,
        long_poll_timeout_ms: u64,
        long_poll_request_secs: u64,
        send_timeout_ms: u64,
        query_timeout_ms: u64,
        base_url: String,
    ) -> Self {
        let io_client = client.clone();
        Self {
            client,
            emit_to,
            send_timeout_ms,
            long_poll_timeout_ms,
            long_poll_request_secs,
            base_url,
            query_timeout_ms,
            typing: TypingKeepers::default(),
            initial_io_cfg: Some(RunIoConfig {
                client: io_client,
                initial_offset,
                long_poll_request_secs,
                long_poll_timeout_ms,
                // Replaced by the substrate via `attach_liveness` when the cell
                // is spawned inside a colony.
                liveness: meclaw_colony::IoLivenessMark::disabled(),
            }),
            blob_store: None,
            credential: None,
        }
    }

    /// GH #1059: the bot token comes from the vault, sealed, under `grant`.
    /// The cell asks in `on_start`; its I/O half sleeps — no poll, no
    /// connection — until the box opened, and re-asks after every round that
    /// ends without one (`wait`, doubling up to its ceiling). The client this
    /// cell was built with must carry NO token (the factory passes `""`): a
    /// literal next to a grant is never used, not even while the box is
    /// missing (OR-VG-4, ruling 22.09.).
    #[must_use]
    pub fn with_credential(mut self, grant: &str, wait: CredentialWait) -> Self {
        let on_expired: crate::credential::ExpiryFn<()> =
            std::sync::Arc::new(|_, _| Box::pin(async {}));
        self.credential = Some(ProxyCredential {
            grant: grant.to_string(),
            slots: CredentialSlots::new(
                wait.wait_ms,
                crate::credential::default_credential_wait_max(),
                on_expired,
            ),
            wait,
        });
        self
    }

    /// The public recipient key of the bot-token question in flight
    /// (tests/diagnosis; never a secret).
    pub fn credential_recipient_hex(&self) -> Option<String> {
        let c = self.credential.as_ref()?;
        c.slots.recipient_hex(&c.grant)
    }

    /// GH #1059: ask the access hive for the bot token. Emitted towards
    /// `emit_to` like every other source emission of this cell; the template's
    /// edge on `hop.route == "credential_request"` decides where it goes.
    async fn ask_for_bot_token(&mut self, sink: &OriginSink) {
        let Some(c) = self.credential.as_mut() else {
            return;
        };
        let content = match c.slots.request(&c.grant) {
            Ok(content) => content,
            Err(e) => {
                tracing::error!(error = %e, "proxy: no random source for a credential request");
                return;
            }
        };
        tracing::info!(grant = %c.grant, "proxy: asking the vault for the bot token");
        let _ = sink
            .emit(meclaw_core::CellOutput {
                target: self.emit_to.clone(),
                content,
            })
            .await;
    }

    /// GH #1059: take a sealed delivery. Opened → the handler's client and the
    /// I/O half get the token (the I/O starts polling). Not opened → the
    /// connector stays asleep; its next round asks again. Never echoes a value.
    async fn accept_bot_token(
        &mut self,
        content: &meclaw_core::serde_json::Value,
        reconfig_tx: &mpsc::Sender<ProxyReconfig>,
    ) {
        let Some(c) = self.credential.as_mut() else {
            tracing::warn!(
                "proxy: a sealed box arrived, but this connector has no bot_token_grant_id — discarded"
            );
            return;
        };
        match c.slots.accept_sealed(content).await {
            Ok(accepted) => {
                let Some(bot_token) = c.slots.secret_handle(&accepted.grant) else {
                    tracing::warn!(
                        grant = %accepted.grant,
                        "proxy: the sealed bot token opened empty — the connector stays asleep"
                    );
                    return;
                };
                tracing::info!(grant = %accepted.grant, "proxy: bot token received sealed and opened in RAM");
                self.client = self.client.with_bot_token(bot_token.expose());
                let _ = reconfig_tx
                    .send(ProxyReconfig::Credential { bot_token })
                    .await;
            }
            Err(refusal) => match refusal.detail() {
                None => tracing::warn!(
                    "proxy: a sealed box of an earlier credential round arrived late and was discarded"
                ),
                Some(detail) => tracing::warn!(
                    %detail,
                    "proxy: the sealed bot token was refused — the connector stays asleep and asks \
                     again after the round's wait"
                ),
            },
        }
    }

    /// GH #907: the blob store documents are committed to. A builder rather
    /// than a `new` argument: the factory sets it on birth and respawn alike,
    /// and the many constructions that never see a document stay as they are.
    #[must_use]
    pub fn with_blob_store(
        mut self,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
    ) -> Self {
        self.blob_store = blob_store;
        self
    }

    /// GH #515: overrides the typing cadence (`TypingCadence::default` is the
    /// production one: 4 s under Telegram's ~5 s decay, 60 s ceiling).
    ///
    /// A test/ops seam, deliberately NOT a params surface: the numbers follow
    /// from the Bot API's own decay and from what a stuck turn may cost, not
    /// from a topology's taste, and a settable one would be a new promise on a
    /// template that is only supposed to grow a behaviour. What it buys is a
    /// test that measures the real mechanism in under two seconds instead of
    /// sitting out a 60 s ceiling.
    pub fn set_typing_cadence(&mut self, cadence: TypingCadence) {
        self.typing.set_cadence(cadence);
    }
}

/// I/O-local state struct. Single owner (held by-value by the I/O sub-task).
/// No mutex, no Arc.
pub struct ProxyIo {
    pub(crate) cfg: RunIoConfig,
    /// GH #1059: `Some` = sleep until the bot token arrives (a grant's connector).
    pub(crate) wait: Option<CredentialWait>,
}

impl LongRunningCell for ProxyCell {
    type Event = ProxyEvent;
    type Reconfig = ProxyReconfig;
    type Io = ProxyIo;

    fn split_io(&mut self) -> Self::Io {
        ProxyIo {
            cfg: self.initial_io_cfg.take().expect("split_io called twice"),
            wait: self.credential.as_ref().map(|c| c.wait),
        }
    }

    /// Issue #7: the long poll is this colony's only outside connection, and a
    /// hung one is what started the issue. Take the mark.
    fn attach_liveness(io: &mut Self::Io, mark: meclaw_colony::IoLivenessMark) {
        io.cfg.liveness = mark;
    }

    /// I/O sub-task — delegates to `crate::proxy::io::run_io`.
    /// `+ Send` is load-bearing (AFIT does not bind Send; `tokio::spawn` in
    /// `cell_task_long_running` needs it). `clippy::manual_async_fn` is a
    /// stable-1.95 false positive — see the pattern in
    /// `crates/meclaw-colony/src/long_running_cell.rs:96-110` and
    /// `crates/meclaw-cells/src/timer/cell.rs:63-70`.
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            let ProxyIo { mut cfg, wait } = io;
            let mut reconfig_rx = reconfig_rx;
            // GH #1059: a grant's connector polls nothing before its token.
            if let Some(wait) = wait
                && !wait_for_bot_token(&mut cfg, wait, &events_tx, &mut reconfig_rx).await
            {
                return; // handler gone → the cell tears down
            }
            run_io(cfg, events_tx, reconfig_rx).await;
        }
    }

    /// GH #1059: a grant's connector asks for its bot token before anything
    /// else; key pair and token are RAM-only, so a respawn asks again.
    #[allow(clippy::manual_async_fn)]
    fn on_start<'a>(
        &'a mut self,
        sink: &'a OriginSink,
        _db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move { self.ask_for_bot_token(sink).await }
    }

    /// Inbound sink path (W4-W7, W12). Extracts `chat_id` from the message's
    /// `context` compartment (standard header convention; finding 1), looks for
    /// the last assistant text turn in `body.messages[]` and calls
    /// `TelegramClient::send_message` with `send_timeout_ms` (A timeout).
    /// Pure-sink discipline (cell-types.md l.372): on success there is NO
    /// OutputSink emit. The failure paths (`invalid_body`, `missing_chat_id`,
    /// `missing_assistant_turn`, `send_failed`) go through
    /// `emit::emit_inbound_error` (T13).
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
        reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            // 1. The body must be inline-readable (messages[] extraction below).
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

            // GH #1059: the sealed bot token. Never touches `cell.db`, never
            // answered with an emission.
            if body_val.get("sealed").is_some() {
                self.accept_bot_token(&body_val, reconfig_tx).await;
                return;
            }

            // β: params-update slot (config.md § Access l.20), handled FIRST.
            // Mutable: send_timeout_ms (path A), long_poll_*/base_url (path B → I/O
            // via reconfig_tx; base_url also rebuilds self.client), query_timeout_ms
            // (path C → DbConn). Immutable: bot_token, emit_to. params-only → silent.
            if let Some(params_val) = body_val.get("params") {
                let update_obj = match params_val.as_object() {
                    Some(o) => o.clone(),
                    None => {
                        crate::proxy::emit::emit_inbound_error(
                            sink,
                            &msg,
                            "invalid_input",
                            "params slot: not a JSON object",
                        )
                        .await;
                        return;
                    }
                };
                let current = crate::proxy::params::ProxyOverlay {
                    base_url: self.base_url.clone(),
                    long_poll_timeout_ms: self.long_poll_timeout_ms,
                    long_poll_request_secs: self.long_poll_request_secs,
                    send_timeout_ms: self.send_timeout_ms,
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
                                crate::proxy::emit::emit_inbound_error(
                                    sink,
                                    &msg,
                                    "invalid_input",
                                    &format!("cell.db params write failed: {e}"),
                                )
                                .await;
                                return;
                            }
                            Err(meclaw_colony::QueryTimeout::Interrupted) => {
                                crate::proxy::emit::emit_inbound_error(
                                    sink,
                                    &msg,
                                    "query_timeout",
                                    "params write exceeded query_timeout_ms",
                                )
                                .await;
                                return;
                            }
                        }
                        // Live apply across all three ways.
                        self.send_timeout_ms = new_ov.send_timeout_ms; // path A
                        self.query_timeout_ms = new_ov.query_timeout_ms; // path C
                        db.set_query_timeout(Some(std::time::Duration::from_millis(
                            self.query_timeout_ms,
                        )));
                        // Path B: poll config + base_url. Rebuild the handler's own
                        // client (sendMessage) with the new base_url (bot_token
                        // rehold internally), and signal the I/O-task to do the same.
                        self.long_poll_timeout_ms = new_ov.long_poll_timeout_ms;
                        self.long_poll_request_secs = new_ov.long_poll_request_secs;
                        if new_ov.base_url != self.base_url {
                            self.base_url = new_ov.base_url.clone();
                            self.client = self.client.with_base_url(&self.base_url);
                        }
                        let _ = reconfig_tx
                            .send(crate::proxy::io::ProxyReconfig::SetPolling {
                                base_url: self.base_url.clone(),
                                long_poll_timeout_ms: self.long_poll_timeout_ms,
                                long_poll_request_secs: self.long_poll_request_secs,
                            })
                            .await;
                    }
                    Err(e) => {
                        crate::proxy::emit::emit_inbound_error(
                            sink,
                            &msg,
                            "invalid_input",
                            &e.detail(),
                        )
                        .await;
                    }
                }
                // Standalone params-update → done (no inbound send in this message).
                return;
            }
            // GH #1059: no token yet → nothing to send with. Answered at once:
            // a reply that waited for the vault would only be a later failure.
            if let Some(c) = &self.credential
                && c.slots.secret(&c.grant).is_none()
            {
                crate::proxy::emit::emit_inbound_error(
                    sink,
                    &msg,
                    "credential_pending",
                    "the bot token has not arrived from the vault yet",
                )
                .await;
                return;
            }
            // 2. chat_id from the `context` compartment (standard header
            //    convention, overview § Standard header convention — `chat_id`
            //    lives in the persistent `context`; cell-types.md § proxy:
            //    reply routing "via chat_id from the headers"). Finding 1:
            //    previously the inbound path read `body.header.chat_id`, but
            //    colony strips every emitted `content.header` into the `hop`
            //    compartment (`split_content_header`), which decays on the next
            //    emission — in a routed topology the reply leg never saw a
            //    `chat_id` and died as `missing_chat_id`.
            let chat_id = msg.headers.context.get("chat_id").and_then(|v| v.as_i64());
            let Some(chat_id) = chat_id else {
                crate::proxy::emit::emit_inbound_error(
                    sink,
                    &msg,
                    "missing_chat_id",
                    "context.chat_id required",
                )
                .await;
                return;
            };

            // 3. Extract the last assistant turn from messages[].
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
                // W12 (ruling 2026-05-24): NO silent drop. An error reply
                // analogous to W5/W6 — symmetric inbound failure classification.
                crate::proxy::emit::emit_inbound_error(
                    sink,
                    &msg,
                    "missing_assistant_turn",
                    "messages[] has no assistant-text turn",
                )
                .await;
                return;
            };

            // 4. GH #1015 (OR-HV-61): after a crash the colony replays open
            //    deliveries, so this exact message may already have been sent.
            //    Book its id BEFORE the send (at-most-once, window in
            //    `proxy::consumed`): a human must never get the same answer
            //    twice. A replay sends nothing and answers as the original did
            //    (GH #1043 review): silent after a confirmed send, the
            //    original's `send_failed` after a failed one, `send_failed`
            //    `unconfirmed` when the crash fell before the answer was
            //    booked. A failed booking does not send either: without the
            //    record the send could be the second one.
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
                self.typing.stop(chat_id);
                crate::proxy::emit::emit_inbound_error(sink, &msg, "send_failed", &detail).await;
                return;
            }

            // 5. sendMessage call (W7 A timeout via the client). Errors → T13.
            let timeout = std::time::Duration::from_millis(self.send_timeout_ms);
            let sent = self.client.send_message(chat_id, &text, timeout).await;
            // GH #515: the answer is on the wire (or has definitively failed) —
            // the keeper for this chat has nothing left to say. Stopping AFTER
            // the call, not before, is deliberate: the status should stand for
            // the whole time the send itself takes.
            self.typing.stop(chat_id);
            let failure = sent.err().map(|e| format!("{e:?}"));
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
                tracing::warn!(error = %e, "proxy: booking the send's answer failed");
            }
            if let Some(detail) = failure {
                crate::proxy::emit::emit_inbound_error(sink, &msg, "send_failed", &detail).await;
            }
        }
    }

    /// Phase-5 canon (state before emit): persists the update cursor
    /// (`save_offset(update_id + 1)`) BEFORE the OriginSink emission.
    /// If the cell crashes between persist and emit, the topology loses at most
    /// one event; a crash between emit and persist would re-deliver the same
    /// event after a restart — hence persist first. The emission runs via
    /// `OriginSink` (parent=None, fresh trace), and the target is the routing
    /// destination configured in `params.emit_to`.
    #[allow(clippy::manual_async_fn)]
    fn handle_event<'a>(
        &'a mut self,
        event: Self::Event,
        sink: &'a OriginSink,
        db: &'a mut DbConn,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            // GH #907: an update the connector does not read only moves the
            // cursor -- persisted like every other one, so a restart does not
            // fetch it again -- and emits nothing.
            let (update_id, chat_id, content) = match event {
                // GH #1059: a round ended without a box → ask again, fresh
                // recipient. A box that opened in between makes it moot.
                ProxyEvent::CredentialRoundExpired { round, wait_ms } => {
                    if let Some(c) = &self.credential
                        && c.slots.secret(&c.grant).is_none()
                    {
                        tracing::warn!(
                            round,
                            wait_ms,
                            grant = %c.grant,
                            "proxy: no sealed bot token arrived in time — asking again"
                        );
                        self.ask_for_bot_token(sink).await;
                    }
                    return;
                }
                ProxyEvent::Skipped { update_id } => {
                    let next_offset = update_id + 1;
                    let _ = db
                        .call_with_timeout(move |c| crate::proxy::db::save_offset(c, next_offset))
                        .await;
                    return;
                }
                ProxyEvent::UserMessage {
                    update_id,
                    chat_id,
                    user_id,
                    message_id,
                    text,
                } => (
                    update_id,
                    chat_id,
                    crate::proxy::emit::build_user_turn_content(
                        chat_id, user_id, message_id, &text,
                    ),
                ),
                ProxyEvent::Document {
                    update_id,
                    chat_id,
                    user_id,
                    message_id,
                    caption,
                    name,
                    mime,
                    content,
                    ..
                } => {
                    // GH #907: the bytes go into the blob store HERE, before the
                    // cursor write below, so "cursor persisted before emission"
                    // holds unchanged; a crash after the blob write and before
                    // the cursor leaves an orphan blob and a re-fetched update,
                    // never a lost document. The write is local storage like
                    // the cursor write, so it runs under the same knob,
                    // `query_timeout_ms` (`send_timeout_ms` bounds a network
                    // call to Telegram, which this is not).
                    let stored = match content {
                        crate::proxy::io::DocumentContent::Bytes(bytes) => {
                            crate::proxy::emit::store_document(
                                self.blob_store.as_deref(),
                                bytes,
                                &mime,
                                &name,
                                std::time::Duration::from_millis(self.query_timeout_ms),
                            )
                            .await
                            .map_err(str::to_string)
                        }
                        other => Err(crate::proxy::emit::document_failure(&other)
                            .unwrap_or_else(|| "not_fetched".to_string())),
                    };
                    let turn = crate::proxy::emit::build_document_turn_content(
                        chat_id,
                        user_id,
                        message_id,
                        &caption,
                        &name,
                        stored.as_ref().map_err(String::as_str),
                    );
                    (update_id, chat_id, turn)
                }
            };

            // GH #515: the sign of life goes FIRST — before the cursor write and
            // before the emission. The whole point is that the chat sees
            // something in the first moment of the turn, not after the topology
            // behind the connector has had its say; and `start` only spawns a
            // task, so nothing here waits on Telegram.
            self.typing.start(
                &self.client,
                chat_id,
                std::time::Duration::from_millis(self.send_timeout_ms),
            );

            // 1. State before emit (phase-5 canon): persist the cursor BEFORE emitting.
            let next_offset = update_id + 1;
            // Path C: cell.db op under query_timeout_ms (via DbConn::call_with_timeout).
            let _ = db
                .call_with_timeout(move |c| crate::proxy::db::save_offset(c, next_offset))
                .await;

            // 2. Emit the UBF content via OriginSink (parent=None, fresh trace).
            let _ = sink
                .emit(meclaw_core::CellOutput {
                    target: self.emit_to.clone(),
                    content,
                })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::telegram::TelegramClient;
    use meclaw_core::Path;

    #[test]
    fn split_io_moves_client_and_offset_out() {
        let client = TelegramClient::new("http://x", "T").unwrap();
        let mut cell = ProxyCell::new(
            client,
            Path::new("/main"),
            42,
            35000,
            30,
            10000,
            5000,
            "https://api.telegram.org".into(),
        );
        let io = <ProxyCell as meclaw_colony::LongRunningCell>::split_io(&mut cell);
        assert_eq!(io.cfg.initial_offset, 42);
        assert_eq!(io.cfg.long_poll_request_secs, 30);
        assert_eq!(io.cfg.long_poll_timeout_ms, 35000);
    }
}
