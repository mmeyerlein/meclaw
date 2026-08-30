//! GH #515: the sign of life a long turn owes the chat.
//!
//! A message arrives, the topology behind the connector spends seconds to tens
//! of seconds producing an answer, and until this module existed the chat stayed
//! completely silent for that whole time. Telegram's primitive for it is
//! `sendChatAction` with `action=typing`: the client renders "typing…" and
//! NOTHING is posted into the conversation, which is what makes it usable at all
//! — a connector that writes "still working" into the chat has changed the
//! transcript the agent behind it will read back later.
//!
//! Two facts shape the mechanism:
//!
//! - Telegram drops the status after roughly five seconds, so a single call
//!   covers only the first moment of a turn. It has to REPEAT.
//! - Nothing tells the connector that a turn was abandoned. So the repeater
//!   carries its own deadline; the answer cancels it, and if no answer ever
//!   comes the deadline does.
//!
//! Where it sits: in the handler sub-task of `ProxyCell`, which is the one place
//! that sees both ends of a turn — `handle_event` (the update arrived) starts a
//! keeper, `handle` (the assistant turn came back on the inbound edge) stops it.
//! The I/O sub-task sees only the poll and could not stop anything.

use crate::proxy::telegram::TelegramClient;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// How often the typing status is refreshed, and how long a single turn may keep
/// refreshing it before the keeper gives up.
///
/// The production values are `Default`: a 4 s interval under Telegram's ~5 s
/// decay (one full interval of margin for a slow round trip), and a 60 s ceiling
/// — long enough for the slow turns this exists for, short enough that a turn
/// which died somewhere in the topology stops pretending within a minute.
#[derive(Debug, Clone, Copy)]
pub struct TypingCadence {
    /// Delay between two `sendChatAction` calls of the same turn.
    pub interval: Duration,
    /// Ceiling on one turn's keeper. Reached without an answer, it stops.
    pub max_total: Duration,
}

impl Default for TypingCadence {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(4),
            max_total: Duration::from_secs(60),
        }
    }
}

/// The live keepers, one per chat at most.
///
/// Keyed by `chat_id`, so a second incoming turn in the same chat REPLACES its
/// predecessor's keeper instead of stacking a second repeater on the same chat.
/// Growth is bounded twice over: an entry exists only while a chat has a turn in
/// flight, every keeper self-terminates at `max_total`, and finished handles are
/// pruned on each start. `Drop` aborts what is left — a dropped cell must not
/// leave detached tasks calling the Bot API.
pub struct TypingKeepers {
    live: HashMap<i64, JoinHandle<()>>,
    cadence: TypingCadence,
}

impl Default for TypingKeepers {
    fn default() -> Self {
        Self::new(TypingCadence::default())
    }
}

impl TypingKeepers {
    /// Builds an empty registry with the given cadence.
    pub fn new(cadence: TypingCadence) -> Self {
        Self {
            live: HashMap::new(),
            cadence,
        }
    }

    /// Replaces the cadence. Takes effect for keepers started AFTER the call;
    /// a running keeper carries the cadence it was started with.
    pub fn set_cadence(&mut self, cadence: TypingCadence) {
        self.cadence = cadence;
    }

    /// The cadence new keepers are started with.
    pub fn cadence(&self) -> TypingCadence {
        self.cadence
    }

    /// Number of keepers currently held (finished-but-unpruned included).
    pub fn len(&self) -> usize {
        self.live.len()
    }

    /// Whether the registry holds no keeper at all.
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Starts (or restarts) the keeper for `chat_id`.
    ///
    /// Called from `handle_event`, i.e. the moment an incoming turn is accepted
    /// — before the emission, because the point of the whole mechanism is that
    /// the user sees something within the first moment rather than after the
    /// topology has had its say.
    pub fn start(&mut self, client: &TelegramClient, chat_id: i64, request_timeout: Duration) {
        self.prune();
        let cadence = self.cadence;
        let client = client.clone();
        let handle = tokio::spawn(async move {
            let started = Instant::now();
            let mut ticks: u32 = 0;
            loop {
                match client
                    .send_chat_action(chat_id, "typing", request_timeout)
                    .await
                {
                    Ok(()) => {
                        if ticks == 0 {
                            // GH #515: the greppable proof that a turn showed a
                            // sign of life. INFO once per turn; the refreshes
                            // that follow are DEBUG, so a busy colony does not
                            // pay a log line every four seconds per chat.
                            tracing::info!(
                                chat_action = "typing",
                                chat_id,
                                interval_ms = cadence.interval.as_millis() as u64,
                                max_total_ms = cadence.max_total.as_millis() as u64,
                                "typing indicator started for a running turn"
                            );
                        } else {
                            tracing::debug!(chat_action = "typing", chat_id, ticks, "refreshed");
                        }
                    }
                    Err(e) => {
                        // A missing sign of life must never cost the turn its
                        // answer, so this is logged and nothing else — no
                        // emission, no failure code, and the next tick tries
                        // again. The turn's own deadline ends it either way.
                        tracing::warn!(
                            chat_action = "typing",
                            chat_id,
                            error = format!("{e:?}"),
                            "typing indicator failed (the turn is unaffected)"
                        );
                    }
                }
                ticks += 1;
                if started.elapsed().saturating_add(cadence.interval) > cadence.max_total {
                    tracing::debug!(
                        chat_action = "typing",
                        chat_id,
                        ticks,
                        "typing indicator stopped - no answer within max_total"
                    );
                    return;
                }
                tokio::time::sleep(cadence.interval).await;
            }
        });
        if let Some(previous) = self.live.insert(chat_id, handle) {
            previous.abort();
        }
    }

    /// Stops the keeper for `chat_id` if one is running.
    ///
    /// Called from `handle` once the `sendMessage` attempt for that chat has
    /// completed — after, not before: the status should stand until the answer
    /// is actually on the wire.
    pub fn stop(&mut self, chat_id: i64) {
        if let Some(handle) = self.live.remove(&chat_id) {
            handle.abort();
        }
        self.prune();
    }

    /// Drops handles whose keeper already ran out its own deadline. Keeps the
    /// map at the size of the chats with a turn in flight rather than at the
    /// size of every chat the connector has ever seen.
    fn prune(&mut self) {
        self.live.retain(|_, h| !h.is_finished());
    }
}

impl Drop for TypingKeepers {
    fn drop(&mut self) {
        for (_, handle) in self.live.drain() {
            handle.abort();
        }
    }
}
