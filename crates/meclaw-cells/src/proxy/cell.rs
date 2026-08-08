//! Phase-10-C: `ProxyCell` — implements `LongRunningCell` from 10-A.
//! The handler is the DB authority (cursor persist + inbound sink calls); the I/O
//! side polls Telegram in an endless loop. State is single-threaded in the
//! handler sub-task (no mutex — phase-1 discipline).

use crate::proxy::io::{ProxyEvent, ProxyReconfig, RunIoConfig, run_io};
use crate::proxy::telegram::TelegramClient;
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
    /// Initial I/O config, consumed exactly once by `split_io`.
    pub(crate) initial_io_cfg: Option<RunIoConfig>,
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
            initial_io_cfg: Some(RunIoConfig {
                client: io_client,
                initial_offset,
                long_poll_request_secs,
                long_poll_timeout_ms,
            }),
        }
    }
}

/// I/O-local state struct. Single owner (held by-value by the I/O sub-task).
/// No mutex, no Arc.
pub struct ProxyIo {
    pub(crate) cfg: RunIoConfig,
}

impl LongRunningCell for ProxyCell {
    type Event = ProxyEvent;
    type Reconfig = ProxyReconfig;
    type Io = ProxyIo;

    fn split_io(&mut self) -> Self::Io {
        ProxyIo {
            cfg: self.initial_io_cfg.take().expect("split_io called twice"),
        }
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
        run_io(io.cfg, events_tx, reconfig_rx)
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

            // 4. sendMessage call (W7 A timeout via the client). Errors → T13.
            let timeout = std::time::Duration::from_millis(self.send_timeout_ms);
            match self.client.send_message(chat_id, &text, timeout).await {
                Ok(()) => {} // Pure sink: no emit on success.
                Err(e) => {
                    crate::proxy::emit::emit_inbound_error(
                        sink,
                        &msg,
                        "send_failed",
                        &format!("{e:?}"),
                    )
                    .await;
                }
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
            let ProxyEvent::UserMessage {
                update_id,
                chat_id,
                user_id,
                message_id,
                text,
            } = event;

            // 1. State before emit (phase-5 canon): persist the cursor BEFORE emitting.
            let next_offset = update_id + 1;
            // Path C: cell.db op under query_timeout_ms (via DbConn::call_with_timeout).
            let _ = db
                .call_with_timeout(move |c| crate::proxy::db::save_offset(c, next_offset))
                .await;

            // 2. Build the UBF content + emit via OriginSink (parent=None, fresh trace).
            let content =
                crate::proxy::emit::build_user_turn_content(chat_id, user_id, message_id, &text);
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
