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
//!
//! GH #1504: "the answer" is the first message of the turn WITHOUT
//! `hop.interim`. An interim is sent while the turn goes on, so it leaves the
//! keeper standing; and because Telegram clients drop the status when a
//! message of the bot arrives, the keeper is asked to show it again at once
//! ([`TypingKeepers::show_again`]) instead of at its next regular tick.
//!
//! GH #1530: the keeper is shared by every platform that shows a running turn;
//! what a platform does on the wire is its hook ([`TypingSign`]). A sign that
//! decays (Telegram's status) is shown again every interval and ending the
//! keeper aborts it; a sign that stays until it is taken away (a Slack
//! reaction) is shown once, and the keeper's own task takes it away at the
//! answer, at a repeat or at the ceiling. Either way the state lives in the
//! keeper's task -- no mutex, no atomics -- and the cell never waits on it.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

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

/// GH #1530: one platform's hook into the keeper -- how the sign of a running
/// turn is shown, and whether it has to be taken away again.
///
/// The keeper below holds what every platform shares (one task per chat, the
/// ceiling, the answer ends it, a failed call is logged and nothing else);
/// what a call looks like on the wire lives only in the implementations of
/// this trait. Each call carries its own operation timeout (hard rule 12):
/// the implementation wraps it, the keeper never waits on a call without one.
pub trait TypingSign: Send + Sync + 'static {
    /// The word the keeper's log lines carry as `chat_action`.
    fn kind(&self) -> &'static str;

    /// Whether the sign decays on its own and has to be shown again every
    /// [`TypingCadence::interval`]. A sign that does not decay is shown once.
    fn decays(&self) -> bool;

    /// Whether the sign stays until it is taken away ([`Self::take_away`]).
    /// `false`: the sign decays, ending the keeper is aborting its task.
    fn needs_take_away(&self) -> bool;

    /// Shows the sign once, under its own operation timeout.
    fn show(&self) -> impl std::future::Future<Output = Result<(), String>> + Send;

    /// Takes the sign away; every platform call it makes carries its own
    /// operation timeout, and it may try again a bounded number of times
    /// (Slack, OR-d54g.acktyp.2). Only called when [`Self::needs_take_away`]
    /// says so.
    fn take_away(&self) -> impl std::future::Future<Output = Result<(), String>> + Send;
}

/// One chat's keeper: its task, the lane that asks it for a tick now, and,
/// for a sign that has to be taken away, the lane that ends it.
struct Keeper {
    task: JoinHandle<()>,
    /// GH #1504: capacity 1 -- one pending request is all a keeper can use,
    /// a second one before its next tick says nothing new.
    again: mpsc::Sender<()>,
    /// GH #1530: `Some` for a sign that stays until it is taken away. Ending
    /// the keeper sends here (or drops it) and the task takes the sign away
    /// itself, after the call in flight -- an abort would leave the sign
    /// standing. `None`: the sign decays, ending is aborting.
    end: Option<oneshot::Sender<()>>,
}

impl Keeper {
    /// Ends the keeper: a sign that decays by aborting the task, a sign that
    /// stays by asking the task to take it away. Never waits.
    fn end(self) {
        match self.end {
            Some(end) => {
                let _ = end.send(());
            }
            None => self.task.abort(),
        }
    }
}

/// The live keepers, one per chat at most.
///
/// Keyed by the platform's chat address, so a second incoming turn in the same
/// chat REPLACES its predecessor's keeper instead of stacking a second sign on
/// the same chat (the predecessor is ended like an answered turn). Growth is
/// bounded twice over: an entry exists only while a chat has a turn in flight,
/// every keeper self-terminates at `max_total`, and finished handles are pruned
/// on each start. `Drop` aborts every keeper still held. A keeper already
/// ended (answer, replacement) is no longer held: it finishes taking its sign
/// away on its own, bounded by the timeouts and tries of that call.
pub struct TypingKeepers<K> {
    live: HashMap<K, Keeper>,
    cadence: TypingCadence,
}

impl<K> Default for TypingKeepers<K> {
    fn default() -> Self {
        Self::new(TypingCadence::default())
    }
}

impl<K> TypingKeepers<K> {
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
}

impl<K> TypingKeepers<K>
where
    // `tracing::Value`: the log lines carry the key as the platform's own
    // type (Telegram's `chat_id` stays a number, as before GH #1530).
    K: Eq + Hash + Clone + tracing::Value + Send + 'static,
{
    /// Starts (or restarts) the keeper for `chat`, showing `sign`.
    ///
    /// Called from `handle_event`, i.e. the moment an incoming turn is accepted
    /// — before the emission, because the point of the whole mechanism is that
    /// the user sees something within the first moment rather than after the
    /// topology has had its say. Only spawns a task: nothing here waits on the
    /// platform.
    pub fn start<S: TypingSign>(&mut self, chat: K, sign: S) {
        self.prune();
        let cadence = self.cadence;
        let (again, mut asked_again) = mpsc::channel::<()>(1);
        let (end, mut ended) = if sign.needs_take_away() {
            let (tx, rx) = oneshot::channel::<()>();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let key = chat.clone();
        let task = tokio::spawn(async move {
            let chat_id = key;
            let chat_action = sign.kind();
            let started = Instant::now();
            let mut ticks: u32 = 0;
            loop {
                match sign.show().await {
                    Ok(()) => {
                        if ticks == 0 {
                            // GH #515: the greppable proof that a turn showed a
                            // sign of life. INFO once per turn; the refreshes
                            // that follow are DEBUG, so a busy colony does not
                            // pay a log line every interval per chat.
                            tracing::info!(
                                chat_action,
                                chat_id,
                                interval_ms = cadence.interval.as_millis() as u64,
                                max_total_ms = cadence.max_total.as_millis() as u64,
                                "typing indicator started for a running turn"
                            );
                        } else {
                            tracing::debug!(chat_action, chat_id, ticks, "refreshed");
                        }
                    }
                    Err(e) => {
                        // A missing sign of life must never cost the turn its
                        // answer, so this is logged and nothing else — no
                        // emission, no failure code, and the next tick tries
                        // again. The turn's own deadline ends it either way.
                        tracing::warn!(
                            chat_action,
                            chat_id,
                            error = e,
                            "typing indicator failed (the turn is unaffected)"
                        );
                    }
                }
                ticks += 1;
                // GH #1530: a sign that does not decay is not shown again;
                // it stands until the answer or the ceiling, whichever first,
                // so no further interval is booked against the ceiling.
                let next = if sign.decays() {
                    cadence.interval
                } else {
                    Duration::ZERO
                };
                // GH #1530: an answer that came while the call was in flight
                // ends the keeper before the ceiling is judged, so the log
                // never calls an answered turn unanswered.
                let answered = match ended.as_mut() {
                    Some(rx) => !matches!(rx.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
                    None => false,
                };
                if answered {
                    break;
                }
                if started.elapsed().saturating_add(next) > cadence.max_total {
                    tracing::debug!(
                        chat_action,
                        chat_id,
                        ticks,
                        "typing indicator stopped - no answer within max_total"
                    );
                    break;
                }
                // GH #1504: the next tick is due after one interval, or at
                // once when the cell asks for it (`show_again`). Either way
                // the ceiling check above ran first, so a request never
                // carries the keeper past `max_total`. A closed lane (`None`)
                // only disables its branch; the interval still runs.
                // GH #1530: for a sign that does not decay the wait runs to
                // the ceiling, and the end lane (the answer) cuts it short.
                let wait = if sign.decays() {
                    cadence.interval
                } else {
                    cadence.max_total.saturating_sub(started.elapsed())
                };
                let end_lane = async {
                    match ended.as_mut() {
                        Some(rx) => {
                            let _ = rx.await;
                        }
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    biased;
                    () = end_lane => break,
                    () = tokio::time::sleep(wait) => {
                        if !sign.decays() {
                            tracing::debug!(
                                chat_action,
                                chat_id,
                                ticks,
                                "typing indicator stopped - no answer within max_total"
                            );
                            break;
                        }
                    }
                    Some(()) = asked_again.recv(), if sign.decays() => {}
                }
            }
            if sign.needs_take_away() {
                // GH #1530: the sign stays until it is taken away, so the end
                // of the keeper is one more call. Logged like the showing;
                // nothing waits on it, and a refusal costs the turn nothing.
                match sign.take_away().await {
                    Ok(()) => {
                        tracing::debug!(chat_action, chat_id, "typing indicator taken away")
                    }
                    Err(e) => tracing::warn!(
                        chat_action,
                        chat_id,
                        error = e,
                        "taking the typing indicator away failed (the turn is unaffected)"
                    ),
                }
            }
        });
        if let Some(previous) = self.live.insert(chat, Keeper { task, again, end }) {
            previous.end();
        }
    }

    /// GH #1504: asks the keeper of `chat` to show the status again now
    /// rather than at its next regular tick.
    ///
    /// Called from `handle` after an interim message went out: the turn is
    /// still running, but the client may drop a decaying status the moment
    /// the message arrived, and the regular tick may be a whole interval away.
    /// Without a standing keeper (none started, or it ran out its ceiling) this
    /// does nothing -- the ceiling stays the ceiling. A sign that does not
    /// decay ignores the request. Never waits: the request is handed over or,
    /// with one already pending, dropped.
    pub fn show_again(&self, chat: &K) {
        if let Some(keeper) = self.live.get(chat) {
            let _ = keeper.again.try_send(());
        }
    }

    /// Ends the keeper for `chat` if one is running.
    ///
    /// Called from `handle` once the send of the turn's answer has completed —
    /// after, not before: the status should stand until the answer is actually
    /// on the wire. Never waits: a sign that has to be taken away is taken
    /// away by the keeper's own task.
    pub fn stop(&mut self, chat: &K) {
        if let Some(keeper) = self.live.remove(chat) {
            keeper.end();
        }
        self.prune();
    }

    /// Drops handles whose keeper already ran out its own deadline. Keeps the
    /// map at the size of the chats with a turn in flight rather than at the
    /// size of every chat the connector has ever seen.
    fn prune(&mut self) {
        self.live.retain(|_, k| !k.task.is_finished());
    }
}

impl<K> Drop for TypingKeepers<K> {
    fn drop(&mut self) {
        for (_, keeper) in self.live.drain() {
            keeper.task.abort();
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
        // GH #1530: the Slack connector's reaction keeper, same ceiling rule.
        (
            "slack-agent/proxy/config.json",
            &["slack-agent/worker/config.json"],
        ),
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
