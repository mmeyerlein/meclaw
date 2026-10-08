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

/// GH #515: the refresh interval -- 4 s under Telegram's ~5 s decay, one full
/// second of margin for a slow round trip.
pub const TYPING_INTERVAL: Duration = Duration::from_secs(4);

/// How often the typing status is refreshed, and how long a single turn may keep
/// refreshing it before the keeper gives up.
///
/// `interval` is [`TYPING_INTERVAL`]. `max_total` is the backstop of the turn
/// (GH #1099, [`TypingCadence::for_backstop`]): the keeper stands exactly as
/// long as the answer may still come, and the answer ends it earlier. It used
/// to be a fixed 60 s, so a turn on a slow path (a 300 s cogny backstop) went
/// silent in the chat after a minute while it was still working. `Default` is
/// the colony's default backstop (`meclaw_colony::DEFAULT_MESSAGE_TIMEOUT_MS`).
#[derive(Debug, Clone, Copy)]
pub struct TypingCadence {
    /// Delay between two `sendChatAction` calls of the same turn.
    pub interval: Duration,
    /// Ceiling on one turn's keeper. Reached without an answer, it stops.
    pub max_total: Duration,
}

impl TypingCadence {
    /// GH #1099: the production cadence for a turn whose answer may take up to
    /// `backstop`.
    pub fn for_backstop(backstop: Duration) -> Self {
        Self {
            interval: TYPING_INTERVAL,
            max_total: backstop,
        }
    }
}

impl TypingCadence {
    /// GH #1099: the production cadence of a connector, from its
    /// `typing_max_ms` param (the answering cell's backstop) and the backstop
    /// the colony resolved for it (`message_timeout_default_ms` unless it
    /// declares one). Neither (`0`/`-1`, no backstop at all) → the colony's
    /// default backstop still bounds a keeper whose turn was abandoned.
    pub fn for_connector(typing_max_ms: Option<u64>, resolved_backstop: Option<Duration>) -> Self {
        Self::for_backstop(
            typing_max_ms
                .map(Duration::from_millis)
                .or(resolved_backstop)
                .unwrap_or(Duration::from_millis(
                    meclaw_colony::DEFAULT_MESSAGE_TIMEOUT_MS,
                )),
        )
    }
}

impl Default for TypingCadence {
    fn default() -> Self {
        Self::for_backstop(Duration::from_millis(
            meclaw_colony::DEFAULT_MESSAGE_TIMEOUT_MS,
        ))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped `cell.message_timeout` of a template cell, read from disk.
    /// A shipped template file, read without an `exists` guard (delta review
    /// of fix1): the lock below is about the shipped numbers, so a template
    /// that went missing or moved fails it by name instead of passing it
    /// silently.
    fn shipped_json(rel: &str) -> meclaw_core::serde_json::Value {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../templates")
            .join(rel);
        let raw = std::fs::read_to_string(&p)
            .unwrap_or_else(|e| panic!("no such template file {}: {e}", p.display()));
        meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    fn shipped_backstop_ms(rel: &str) -> u64 {
        let v = shipped_json(rel);
        v["cell"]["message_timeout"]
            .as_u64()
            .unwrap_or_else(|| panic!("{rel}: no message_timeout"))
    }

    fn templates_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates")
    }

    /// Every `config.json` under `dir`, as a path relative to the templates root.
    fn config_files(dir: &std::path::Path, out: &mut Vec<String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                config_files(&path, out);
            } else if path.file_name().is_some_and(|n| n == "config.json") {
                let rel = path.strip_prefix(templates_root()).unwrap();
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    /// The proxy configs that carry `typing_max_ms`, each with the cells that
    /// answer its turn. The first row is the shipped connector and is checked
    /// unconditionally; a row whose template is not in this tree (the export
    /// leaves `egon` out, GH #1099 follow-up) is no claim about this tree.
    const TYPING_CEILINGS: &[(&str, &[&str])] = &[
        (
            "telegram-connector/config.json",
            &["cogny/brain/config.json", "talky/brain/config.json"],
        ),
        ("egon/proxy/config.json", &["egon/worker/config.json"]),
    ];

    /// The ceiling lock runs in every tree that ships the templates, public
    /// or private: the connector row is read without a guard, the rows of
    /// templates the tree does not hold are not read, and a proxy config that
    /// sets `typing_max_ms` without a row here fails by name -- every shipped
    /// ceiling names the backstop of the cells that answer it.
    #[test]
    fn every_shipped_typing_ceiling_is_the_backstop_of_its_answering_cells() {
        let mut found = Vec::new();
        config_files(&templates_root(), &mut found);
        let carriers: Vec<&String> = found
            .iter()
            .filter(|rel| shipped_json(rel)["params"]["typing_max_ms"].is_u64())
            .collect();
        for rel in &carriers {
            assert!(
                TYPING_CEILINGS.iter().any(|(proxy, _)| proxy == rel),
                "{rel} sets typing_max_ms but names no answering cells in TYPING_CEILINGS"
            );
        }
        let (connector, _) = TYPING_CEILINGS[0];
        assert!(
            carriers.iter().any(|rel| *rel == connector),
            "the shipped connector {connector} carries no typing_max_ms"
        );
        for (proxy, answering) in TYPING_CEILINGS {
            if *proxy != connector && !templates_root().join(proxy).is_file() {
                continue;
            }
            let backstop = answering
                .iter()
                .map(|rel| shipped_backstop_ms(rel))
                .max()
                .unwrap();
            assert_eq!(
                shipped_json(proxy)["params"]["typing_max_ms"].as_u64(),
                Some(backstop),
                "{proxy} types for the backstop of the cells that answer it"
            );
        }
    }

    /// Delta review of fix1: the ceiling lock reads its templates without an
    /// `exists` guard -- a missing file is a failure that names it.
    #[test]
    #[should_panic(expected = "no such template file")]
    fn a_missing_template_fails_the_ceiling_lock() {
        shipped_json("telegram-connector/no-such-cell/config.json");
    }

    /// GH #1099: the keeper stands as long as the answering turn may run --
    /// a 300 s backstop keeps "typing…" up past the old fixed 60 s; the answer
    /// still ends it earlier (`stop`, locked in `gh515_*`).
    ///
    /// Review fix1: the ceiling is the backstop of the cell that ANSWERS, not
    /// the proxy's. The shipped connector has no backstop of its own
    /// (`timeout: -1`), so before it typed for the colony default (121 s) while
    /// a cogny turn may run for its whole backstop (56 min); now it carries
    /// `typing_max_ms` = the longest backstop of the cells that answer a
    /// member's chat turn, and a row that moves a brain's backstop fails here.
    #[test]
    fn the_ceiling_is_the_backstop_of_the_answering_turn() {
        let v = shipped_json("telegram-connector/config.json");
        let answering = ["cogny/brain/config.json", "talky/brain/config.json"]
            .iter()
            .map(|rel| shipped_backstop_ms(rel))
            .max()
            .unwrap();
        let typing_max_ms = v["params"]["typing_max_ms"].as_u64();
        assert_eq!(
            typing_max_ms,
            Some(answering),
            "the shipped connector types for the answering turn's backstop"
        );
        // The proxy's own (absent) backstop does not cut it short.
        let shipped = TypingCadence::for_connector(typing_max_ms, None);
        assert_eq!(shipped.max_total, Duration::from_millis(answering));
        assert!(
            shipped.max_total > Duration::from_millis(meclaw_colony::DEFAULT_MESSAGE_TIMEOUT_MS)
        );
        let slow = TypingCadence::for_connector(Some(300_000), Some(Duration::from_secs(121)));
        assert_eq!(slow.max_total, Duration::from_secs(300));
        assert!(slow.max_total > Duration::from_secs(60));
        assert_eq!(slow.interval, TYPING_INTERVAL);
        assert_eq!(
            TypingCadence::for_connector(None, Some(Duration::from_secs(400))).max_total,
            Duration::from_secs(400)
        );
        assert_eq!(
            TypingCadence::for_connector(None, None).max_total,
            Duration::from_millis(meclaw_colony::DEFAULT_MESSAGE_TIMEOUT_MS)
        );
    }
}
