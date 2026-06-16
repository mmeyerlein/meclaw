//! Phase-10-C: `ProxyCell` — implementiert `LongRunningCell` aus 10-A.
//! Handler ist DB-Authority (Cursor-Persist + Inbound-Sink-Calls); I/O
//! pollt Telegram in einer Endlos-Loop. State single-threaded im Handler-
//! Sub-Task (kein Mutex — Phase-1-Disziplin).

use crate::proxy::io::{ProxyEvent, ProxyReconfig, RunIoConfig, run_io};
use crate::proxy::telegram::TelegramClient;
use meclaw_colony::{DbConn, LongRunningCell};
use meclaw_core::{Message, OriginSink, OutputSink, Path};
use std::future::Future;
use tokio::sync::mpsc;

/// `proxy`-Cell. State lebt single-threaded im Handler-Sub-Task von
/// `cell_task_long_running`. `initial_io_cfg` wird einmal von `split_io`
/// rausgezogen + an die I/O-Task uebergeben.
pub struct ProxyCell {
    /// Reqwest-Client-Clone (Arc-internal) fuer `handle`/`sendMessage`-Calls.
    pub(crate) client: TelegramClient,
    /// Routing-Target fuer User-Source-Messages (W4 Pflichtfeld).
    pub(crate) emit_to: Path,
    /// A-Timeout fuer `sendMessage` aus `handle` (W7). β: mutable, Weg A (live).
    pub(crate) send_timeout_ms: u64,
    /// β: live poll-config (Weg B) — held so a params-update can merge over it
    /// and signal the I/O-task via `ProxyReconfig::SetPolling`.
    pub(crate) long_poll_timeout_ms: u64,
    /// β: live poll-config (Weg B).
    pub(crate) long_poll_request_secs: u64,
    /// β: live Telegram API base URL (Weg B). On a params-update the handler
    /// rebuilds `self.client` (sendMessage) AND signals the I/O-task to rebuild
    /// its client — both via `TelegramClient::with_base_url` (bot_token rehold).
    pub(crate) base_url: String,
    /// β: live `query_timeout_ms` (Weg C, cell.db ops via DbConn).
    pub(crate) query_timeout_ms: u64,
    /// I/O-Initialkonfig, wird durch `split_io` einmalig konsumiert.
    pub(crate) initial_io_cfg: Option<RunIoConfig>,
}

impl ProxyCell {
    /// Konstruktor. `initial_offset` kommt aus der Factory (sync `load_offset`
    /// aus `cell.db`, W9 Resume-Pfad). `client` ist der initial gebaute
    /// Reqwest-Client; `split_io` cloned ihn intern (ueber `RunIoConfig`).
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

/// I/O-lokale State-Struktur. Single-Owner (vom I/O-Sub-Task by-value
/// gehalten). Kein Mutex, kein Arc.
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

    /// I/O-Sub-Task — delegiert an `crate::proxy::io::run_io`.
    /// `+ Send` ist load-bearing (AFIT bindet kein Send; `tokio::spawn`
    /// in `cell_task_long_running` braucht es). `clippy::manual_async_fn`
    /// ist stable-1.95 False-Positive — siehe Pattern in
    /// `crates/meclaw-colony/src/long_running_cell.rs:96-110` und
    /// `crates/meclaw-cells/src/timer/cell.rs:63-70`.
    #[allow(clippy::manual_async_fn)]
    fn run_io(
        io: Self::Io,
        events_tx: mpsc::Sender<Self::Event>,
        reconfig_rx: mpsc::Receiver<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send {
        run_io(io.cfg, events_tx, reconfig_rx)
    }

    /// Inbound-Sink-Pfad (W4–W7, W12). Extrahiert `chat_id` aus dem
    /// `context`-Fach der Message (Standard-Header-Konvention; Befund 1),
    /// sucht den letzten assistant-Text-Turn in
    /// `body.messages[]` und ruft `TelegramClient::send_message` mit
    /// `send_timeout_ms` (A-Timeout). Pure-Sink-Disziplin (cell-types.md
    /// Z.372): bei Erfolg KEIN OutputSink-Emit. Fehlerpfade (`invalid_body`,
    /// `missing_chat_id`, `missing_assistant_turn`, `send_failed`) gehen
    /// über `emit::emit_inbound_error` (T13).
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
        reconfig_tx: &'a mpsc::Sender<Self::Reconfig>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            // 1. Body muss inline-lesbar sein (messages[]-Extraktion unten).
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

            // β: params-update slot (config.md § Zugriff Z.20), handled FIRST.
            // Mutable: send_timeout_ms (Weg A), long_poll_*/base_url (Weg B → I/O
            // via reconfig_tx; base_url also rebuilds self.client), query_timeout_ms
            // (Weg C → DbConn). Immutable: bot_token, emit_to. params-only → silent.
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
                        self.send_timeout_ms = new_ov.send_timeout_ms; // Weg A
                        self.query_timeout_ms = new_ov.query_timeout_ms; // Weg C
                        db.set_query_timeout(Some(std::time::Duration::from_millis(
                            self.query_timeout_ms,
                        )));
                        // Weg B: poll-config + base_url. Rebuild the handler's own
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
            // 2. chat_id aus dem `context`-Fach (Standard-Header-Konvention,
            //    overview § Standard-Header-Konvention — `chat_id` lebt im
            //    persistenten `context`; cell-types.md § proxy: Reply-Routing
            //    "über chat_id aus den Headers"). Befund 1: vorher las der
            //    Inbound `body.header.chat_id`, aber Colony strippt jedes
            //    emittierte `content.header` ins `hop`-Fach (`split_content_header`),
            //    das bei der nächsten Emission verfällt — in einer gerouteten
            //    Topologie sah das Reply-Leg nie eine `chat_id` und starb als
            //    `missing_chat_id`.
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

            // 3. Letzten assistant-Turn aus messages[] extrahieren.
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
                // W12 (Marcus 2026-05-24): KEIN silent drop. Error-Reply
                // analog W5/W6 — symmetrische Inbound-Fehlerklassifikation.
                crate::proxy::emit::emit_inbound_error(
                    sink,
                    &msg,
                    "missing_assistant_turn",
                    "messages[] has no assistant-text turn",
                )
                .await;
                return;
            };

            // 4. sendMessage-Call (W7 A-Timeout via Client). Fehler → T13.
            let timeout = std::time::Duration::from_millis(self.send_timeout_ms);
            match self.client.send_message(chat_id, &text, timeout).await {
                Ok(()) => {} // Pure Sink: kein Emit bei Erfolg.
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

    /// Phase-5-Kanon (State-vor-Emit): persistiert den Update-Cursor
    /// (`save_offset(update_id + 1)`) VOR der OriginSink-Emission.
    /// Bei Cell-Crash zwischen Persist und Emit verliert die Topologie
    /// höchstens ein Event; ein Crash zwischen Emit und Persist würde
    /// dasselbe Event nach Restart re-deliver'n — deshalb Persist zuerst.
    /// Emission läuft via `OriginSink` (parent=None, fresh trace), Target
    /// ist das in `params.emit_to` konfigurierte Routing-Ziel.
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

            // 1. State-vor-Emit (Phase-5-Kanon): Cursor persistieren VOR Emit.
            let next_offset = update_id + 1;
            // Weg C: cell.db op under query_timeout_ms (via DbConn::call_with_timeout).
            let _ = db
                .call_with_timeout(move |c| crate::proxy::db::save_offset(c, next_offset))
                .await;

            // 2. UBF-Content bauen + via OriginSink (parent=None, fresh trace).
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
