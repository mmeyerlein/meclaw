//! GH #850 (R-SN-5, ADR-0045): a full cell mailbox overflows — it never stops
//! the colony's routing loop.
//!
//! Before this module, `route()` delivered with `entry.handle.send(msg).await`
//! and the waiter was the routing loop itself: one cell that read slower than
//! its producers stopped the whole colony until the heartbeat watchdog ended the
//! process (ADR-0004, measured as the colony deaths behind GH #850). The owner's
//! ruling replaces the blocking delivery with an overflow per cell:
//!
//! * **Normal case unchanged.** A message for a cell whose overflow is empty and
//!   whose mailbox has room still goes through `route()` — one hash lookup on an
//!   (almost always empty) map and one capacity read decide that, nothing else.
//! * **Overflow.** Mailbox full, or the overflow already holds something (order!)
//!   → the message is appended to the cell's overflow. Stage 1 is a queue in
//!   memory; a long flood moves the oldest blocks to stage 2, the
//!   `mailbox_overflow` table in `colony.db`, which stores only the message id —
//!   the message is already in `message_log`, logged exactly once when the
//!   overflow accepted it.
//! * **Drain.** One task per overflowing cell owns its queue and delivers with
//!   `Sender::reserve` — outside the loop, so the loop never waits. Order: rescued
//!   mailbox messages first (they are older), then disk, then memory.
//! * **Caps.** Per cell (messages and bytes, memory and disk together) and one
//!   memory ceiling for all cells. Only above the per-cell cap does a message die,
//!   as dead letter `mailbox_full` — loudly, one warning per second and cell.
//!
//! **An overflow belongs to one mailbox, not to a path** (GH #850 review I-1,
//! I-2). It is keyed by path for the routing decision, but it remembers the
//! mailbox its drain task delivers into. A respawn hands it the successor's
//! mailbox (the same cell, restarted — GH #18); anything else that puts a
//! different mailbox at the path — a `replace_nodes` lift (GH #682/#688), a
//! disconnect or a failure parking the cell on a fresh channel — makes the
//! overflow the displaced cell's: it is *retired* (dead-lettered
//! `cell_inactive`, like the rest of a disconnected mailbox) and a message for
//! the newcomer starts a new *generation*. Likewise an overflow that came back
//! from disk is adopted only by a cell that is active and not failed; for any
//! other cell, and for a path nobody registers by the end of the boot, it is
//! retired at once.
//!
//! **No lock** (AGENTS.md: no `Mutex`/`RwLock`/atomics in colony state,
//! OR-SN-39). The colony task owns the counters ([`Overflow`]); the drain task
//! owns the queue; they talk by message — [`OverflowCmd`] down an unbounded
//! channel, [`OverflowReport`] back through the colony inbox. The ruling's "small
//! lock per cell" is this ownership split: the counter in the colony task is the
//! only thing the routing decision reads, and it moves only on the colony task.
//!
//! **One producer per mailbox.** While a cell's overflow holds anything, the
//! colony delivers to that cell only through the overflow; while it is empty, the
//! drain task has nothing to send. So the mailbox never has two producers racing,
//! and `free_capacity() > 0` right before `route()` means `route()`'s send does
//! not wait (the colony task is the only other producer, and it is busy calling
//! `route()`). A retired generation only dead-letters; it never delivers again
//! once its `Abandon` is applied.

use crate::dead_letter::DeadLetterReason;
use meclaw_core::{Message, Path};
use std::collections::{HashMap, VecDeque};
use tokio::sync::{mpsc, oneshot};

/// Messages per disk read and per spill transaction.
const BLOCK: usize = 256;

/// Deliveries the drain task makes before it reports to the colony. The report
/// is what moves the counter the routing decision reads, and what lets the
/// colony wake a parked cell the drain task delivered into.
const REPORT_EVERY: u64 = 64;

/// The detail a `mailbox_full` dead letter carries when a persisted overflow row
/// names a message that is not in `message_log`.
pub(crate) const MISSING_FROM_LOG: &str = "missing_from_log";

/// The five `colony.json` caps of the overflow (OR-SN-40).
#[derive(Debug, Clone, Copy)]
pub(crate) struct OverflowConfig {
    spill_messages: u64,
    spill_bytes: u64,
    memory_bytes: u64,
    cap_messages: u64,
    cap_bytes: u64,
}

impl OverflowConfig {
    /// Read the five caps off `colony.json`.
    pub(crate) fn from_colony(c: &crate::ColonyConfig) -> Self {
        Self {
            spill_messages: c.mailbox_overflow_spill_messages,
            spill_bytes: c.mailbox_overflow_spill_bytes,
            memory_bytes: c.mailbox_overflow_memory_bytes,
            cap_messages: c.mailbox_overflow_cap_messages,
            cap_bytes: c.mailbox_overflow_cap_bytes,
        }
    }
}

impl Default for OverflowConfig {
    fn default() -> Self {
        Self::from_colony(&crate::ColonyConfig::default())
    }
}

/// What the overflow did, for a test to observe (`ColonyTaskConfig::with_overflow_probe`).
///
/// Test-only in purpose — production never installs a probe, and then nothing is
/// sent. It exists so that "the normal path never reads the table" is a counted
/// fact rather than an argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverflowProbe {
    /// A cell's overflow came into being.
    Entered(Path),
    /// A drain task read this many rows of the `mailbox_overflow` table.
    TableRead(Path, usize),
    /// The colony wrote this many rows of the table (a spill).
    TableWrite(Path, usize),
    /// The colony deleted this many rows of the table.
    TableDelete(Path, usize),
}

/// One message in an overflow queue.
pub(crate) struct Queued {
    /// Order within the cell, from one colony-wide sequence. Only messages from
    /// the router carry a real one; rescued messages (which never reach disk
    /// from the front queue before they are delivered) carry 0 — they never spill.
    seq: i64,
    msg: Message,
    bytes: u64,
    /// `true` when the router took an in-flight ticket for this message (GH #47)
    /// — a dead-lettered message gives it back, a rescued one never held one.
    ticket: bool,
}

/// What the colony tells a drain task.
///
/// `Push` carries a whole `Message` and dwarfs `Spill`/`Abandon`; boxing it
/// would cost an allocation per overflowing message for no gain (the channel is
/// unbounded and the command is moved, not copied) — same trade as `ColonyMsg`.
#[allow(clippy::large_enum_variant)]
pub(crate) enum OverflowCmd {
    /// Append a message the router accepted.
    Push(Queued),
    /// Put rescued mailbox messages BEFORE everything (they are older).
    Front(Vec<Queued>),
    /// The memory ceiling of all overflows is exceeded: spill a block.
    Spill,
    /// The cell has a new mailbox (a respawn).
    Handle(mpsc::Sender<Message>),
    /// The mailbox this overflow was for will not take it (the cell is gone,
    /// failed, disconnected or displaced): dead-letter everything held.
    Abandon,
}

/// A report from a drain task to the colony (`ColonyMsg::Overflow`).
pub struct OverflowReport(Report);

#[allow(clippy::large_enum_variant)]
enum Report {
    /// Messages left the overflow — delivered, or dead-lettered.
    Settled {
        path: Path,
        generation: u64,
        tally: Tally,
        /// Disk rows delivered or dead-lettered; the colony deletes them.
        disk_ids: Vec<String>,
        /// Dead-lettered messages, with whether each held a ticket.
        dead: Vec<(Message, DeadLetterReason, bool)>,
    },
    /// Stage 1 → stage 2: write these rows (none: the colony asked for a spill
    /// and stage 1 was empty by then), then fire `ack`.
    Spill {
        path: Path,
        generation: u64,
        rows: Vec<(i64, String, i64)>,
        bytes: u64,
        ack: oneshot::Sender<()>,
    },
    /// The mailbox behind the drain task's sender is closed.
    Stalled { path: Path, generation: u64 },
}

/// Counts of one report.
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    /// Messages that left the overflow.
    n: u64,
    /// Their bytes.
    bytes: u64,
    /// Of those, the bytes that left MEMORY (front or stage 1).
    mem_bytes: u64,
    /// Of those, the bytes that left the FRONT (rescued messages).
    front_bytes: u64,
}

/// Per-generation counters, owned by the colony task.
struct CellOverflow {
    path: Path,
    generation: u64,
    /// Messages in the overflow (memory + disk), the one number the routing
    /// decision reads.
    pending: u64,
    pending_bytes: u64,
    /// Bytes of this overflow held in memory (front + stage 1).
    mem_bytes: u64,
    /// Of `mem_bytes`, the rescued front — memory a spill cannot free.
    front_bytes: u64,
    /// Came back from disk at boot: its drain task starts reading the table.
    hydrated: bool,
    /// Rows of this generation on disk have `floor < seq <= disk_upto` — a new
    /// generation at the same path never reads an older one's rows, and a
    /// retired one never reads a newer one's.
    floor: i64,
    disk_upto: i64,
    /// `None` until the drain task runs (an overflow hydrated from disk at boot
    /// waits for its cell to register).
    feed: Option<mpsc::UnboundedSender<OverflowCmd>>,
    /// The mailbox the drain task delivers into (`None` before it runs).
    mailbox: Option<mpsc::Sender<Message>>,
    /// A `Spill` for the memory ceiling is on its way.
    spill_requested: bool,
}

/// Where a drain task reports and reads.
#[derive(Debug, Clone)]
struct Wiring {
    report_tx: mpsc::Sender<crate::ColonyMsg>,
    db_path: std::path::PathBuf,
    probe: Option<mpsc::UnboundedSender<OverflowProbe>>,
}

/// "Once per second, and how many since" — for warnings a flood would
/// otherwise repeat per message.
#[derive(Debug, Default)]
struct Throttle {
    last: Option<std::time::Instant>,
    since: u64,
}

impl Throttle {
    /// Count one occurrence; `Some(count)` when it is time to say so.
    fn tick(&mut self) -> Option<u64> {
        let now = std::time::Instant::now();
        self.since += 1;
        if self
            .last
            .is_none_or(|t| now.duration_since(t) >= std::time::Duration::from_secs(1))
        {
            let n = self.since;
            self.last = Some(now);
            self.since = 0;
            Some(n)
        } else {
            None
        }
    }
}

/// The colony's half of every cell's overflow.
///
/// Lives inside [`crate::drain::DrainLedger`] (OR-SN.K2.1): the ledger already
/// travels to every call site of `route_with_log`, and both answer the same
/// question — what the colony still owes a cell.
pub(crate) struct Overflow {
    /// The current generation per path — the one the routing decision reads.
    cells: HashMap<Path, CellOverflow>,
    /// Generations being dead-lettered (their mailbox will not take them),
    /// by generation. They hold no path: a new generation may stand there.
    retired: HashMap<u64, CellOverflow>,
    /// Bytes of all overflows held in memory.
    mem_bytes: u64,
    cfg: OverflowConfig,
    /// `None` = not wired to a running colony (unit tests of the router): then
    /// a full mailbox is handed to `route()` exactly as before this module.
    wiring: Option<Wiring>,
    next_generation: u64,
    /// One colony-wide order for the `seq` column (OR-SN.K2.2): every
    /// generation's rows lie above the rows of every older one at its path.
    next_seq: i64,
    /// Refusals at the cap, per cell.
    cap_warned: HashMap<Path, Throttle>,
    /// Overflows coming into being, per cell (GH #850 review M-4).
    enter_warned: HashMap<Path, Throttle>,
    /// States that cannot happen while the counters are consistent (M-2):
    /// logged, never silent, never per message.
    defect_warned: Throttle,
}

impl std::fmt::Debug for Overflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overflow")
            .field("cells", &self.cells.len())
            .field("retired", &self.retired.len())
            .field("pending", &self.total_pending())
            .field("mem_bytes", &self.mem_bytes)
            .finish()
    }
}

impl Default for Overflow {
    fn default() -> Self {
        Self {
            cells: HashMap::new(),
            retired: HashMap::new(),
            mem_bytes: 0,
            cfg: OverflowConfig::default(),
            wiring: None,
            next_generation: 1,
            next_seq: 1,
            cap_warned: HashMap::new(),
            enter_warned: HashMap::new(),
            defect_warned: Throttle::default(),
        }
    }
}

/// The estimate the byte caps count: the length of the body's JSON, or of the
/// blob id of an offloaded body.
pub(crate) fn body_bytes(msg: &Message) -> u64 {
    match &msg.body {
        meclaw_core::Body::Inline(v) => meclaw_core::serde_json::to_vec(v)
            .map(|b| b.len() as u64)
            .unwrap_or(0),
        meclaw_core::Body::Blob(_) => 36,
    }
}

/// What [`Overflow::accept`] did with a message.
pub(crate) enum Accepted {
    /// It is in the overflow; the caller takes the in-flight ticket.
    Queued,
    /// The cell's overflow is at its cap: dead-letter it as `mailbox_full`.
    Refused(Message),
}

/// What the colony has to do after a report (the parts that need the registry).
pub(crate) enum Followup {
    /// Nothing.
    None,
    /// Messages were delivered to this cell — wake it if it is parked with mail.
    Delivered(Path),
    /// The drain task's mailbox is closed — decide: new handle, or abandon.
    Stalled(Path),
}

impl Overflow {
    /// An overflow wired to a running colony.
    pub(crate) fn wired(
        cfg: OverflowConfig,
        report_tx: mpsc::Sender<crate::ColonyMsg>,
        db_path: std::path::PathBuf,
        probe: Option<mpsc::UnboundedSender<OverflowProbe>>,
    ) -> Self {
        Self {
            cfg,
            wiring: Some(Wiring {
                report_tx,
                db_path,
                probe,
            }),
            ..Self::default()
        }
    }

    /// No cell has anything in its overflow, and nothing is being settled.
    pub(crate) fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.retired.is_empty()
    }

    /// Messages in all overflows, retired ones included.
    pub(crate) fn total_pending(&self) -> u64 {
        self.cells
            .values()
            .chain(self.retired.values())
            .map(|c| c.pending)
            .sum()
    }

    /// The routing decision (OR-SN-38): does a message for `path` have to go
    /// through the overflow? Yes when the overflow is wired and either already
    /// holds something for this cell (order) or the mailbox is full. The empty
    /// map answers without hashing.
    pub(crate) fn must_overflow(&self, path: &Path, handle: &meclaw_core::ActorHandle) -> bool {
        self.wiring.is_some()
            && ((!self.cells.is_empty() && self.cells.contains_key(path))
                || handle.free_capacity() == 0)
    }

    /// Read the persisted overflow counters — once, at boot.
    ///
    /// Each cell with rows gets its counters back and waits for its cell to
    /// register ([`Self::adopt`]) or for the end of the boot
    /// ([`Self::unstarted`]).
    ///
    /// Returns `(cell, messages)` per cell, so the ledger can hold one in-flight
    /// ticket per persisted message like it does for every message it accepts.
    pub(crate) fn hydrate(&mut self, conn: &rusqlite::Connection) -> Vec<(Path, u64)> {
        let rows: rusqlite::Result<Vec<(String, i64, i64, i64)>> = conn
            .prepare(
                "SELECT cell_path, count(*), coalesce(sum(bytes), 0), coalesce(max(seq), 0)
                 FROM mailbox_overflow GROUP BY cell_path",
            )
            .and_then(|mut stmt| {
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                    .collect()
            });
        let rows = match rows {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "mailbox overflow: the persisted counters could not be read — rows on disk \
                     stay there until the next boot reads them"
                );
                return Vec::new();
            }
        };
        let mut tickets = Vec::with_capacity(rows.len());
        for (path, count, bytes, max_seq) in rows {
            let generation = self.next_generation;
            self.next_generation += 1;
            self.next_seq = self.next_seq.max(max_seq.saturating_add(1));
            tracing::warn!(
                target = %path,
                pending = count,
                bytes,
                "mailbox overflow survived a restart — delivering it in order once the cell is up"
            );
            let path = Path::new(&path);
            self.cells.insert(
                path.clone(),
                CellOverflow {
                    path: path.clone(),
                    generation,
                    pending: count.max(0) as u64,
                    pending_bytes: bytes.max(0) as u64,
                    mem_bytes: 0,
                    front_bytes: 0,
                    hydrated: true,
                    floor: 0,
                    disk_upto: max_seq,
                    feed: None,
                    mailbox: None,
                    spill_requested: false,
                },
            );
            tickets.push((path, count.max(0) as u64));
        }
        tickets
    }

    /// A cell registered at `path`. An overflow that came back from disk starts
    /// draining into its mailbox — but only when the cell `accepts` it (active,
    /// not failed; GH #850 review I-1): a disconnected or failed cell would never
    /// be woken to read it, so its overflow is dead-lettered at once, the way a
    /// message routed to it is. A running overflow for ANOTHER mailbox at the
    /// path (a second registration displaced the first) is retired too.
    pub(crate) fn adopt(&mut self, path: &Path, handle: &meclaw_core::ActorHandle, accepts: bool) {
        let Some(cell) = self.cells.get(path) else {
            return;
        };
        if !accepts {
            self.retire(path, "its cell registered inactive or failed");
            return;
        }
        match &cell.mailbox {
            None => {
                if let Some(w) = self.wiring.clone()
                    && let Some(cell) = self.cells.get_mut(path)
                {
                    start_drain(cell, handle.sender_clone(), &w, self.cfg);
                }
            }
            Some(m) if !handle.same_mailbox(m) => {
                self.retire(path, "another mailbox registered at its path");
            }
            Some(_) => {}
        }
    }

    /// Overflows whose drain task never started — hydrated at boot, and no cell
    /// has registered for them yet. At the end of the boot apply each is adopted
    /// by the cell now at its path or retired (GH #850 review I-1).
    pub(crate) fn unstarted(&self) -> Vec<Path> {
        self.cells
            .values()
            .filter(|c| c.feed.is_none())
            .map(|c| c.path.clone())
            .collect()
    }

    /// Append a routed message to the overflow of `path`.
    ///
    /// `routed` is the message exactly as `route()` would have delivered it
    /// (TTL spent, target resolved). The caller has logged it and takes the
    /// in-flight ticket on [`Accepted::Queued`]. `sender` names who sent it, for
    /// the warning when the overflow comes into being.
    pub(crate) fn accept(
        &mut self,
        handle: &meclaw_core::ActorHandle,
        path: &Path,
        routed: Message,
        sender: &Path,
    ) -> Accepted {
        // GH #850 review I-2: the overflow at this path is for another mailbox —
        // the cell standing here now is not the one it was queued for.
        if self
            .cells
            .get(path)
            .and_then(|c| c.mailbox.as_ref())
            .is_some_and(|m| !handle.same_mailbox(m))
        {
            self.retire(path, "another cell now stands at its path");
        }
        let bytes = body_bytes(&routed);
        let (pending, pending_bytes) = self
            .cells
            .get(path)
            .map_or((0, 0), |c| (c.pending, c.pending_bytes));
        if pending.saturating_add(1) > self.cfg.cap_messages
            || pending_bytes.saturating_add(bytes) > self.cfg.cap_bytes
        {
            self.warn_cap(path, pending, pending_bytes);
            return Accepted::Refused(routed);
        }
        if !self.cells.contains_key(path) {
            self.warn_enter(path, sender, &routed, handle.max_capacity());
        }
        // The cell first: a new generation's floor is the sequence BEFORE its
        // first message, so the message's own row is inside its range.
        if self.cell(path, handle).is_none() {
            return Accepted::Refused(routed);
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        let Some(cell) = self.cells.get_mut(path) else {
            return Accepted::Refused(routed);
        };
        cell.pending += 1;
        cell.pending_bytes += bytes;
        cell.mem_bytes += bytes;
        let q = Queued {
            seq,
            msg: routed,
            bytes,
            ticket: true,
        };
        let unsent = match cell.feed.as_ref() {
            Some(f) => f.send(OverflowCmd::Push(q)).err().map(|e| e.0),
            None => None,
        };
        if let Some(OverflowCmd::Push(q)) = unsent {
            // The drain task is gone — it only ends when its feed is dropped or
            // the colony is, so this is a defect. Never silent: the message is
            // handed back as refused, and the lost counters are named.
            self.lose(path, bytes);
            return Accepted::Refused(q.msg);
        }
        self.mem_bytes += bytes;
        self.enforce_memory_ceiling();
        Accepted::Queued
    }

    /// Put rescued mailbox messages at the front of `path`'s overflow (creating
    /// it when it does not exist). They carry no ticket (GH #18/#47: a rescued
    /// mailbox is delivered past the router). Hands the messages back when no
    /// overflow can take them (not wired, or its drain task is gone) — the
    /// caller dead-letters them; they are never dropped here.
    pub(crate) fn front(
        &mut self,
        handle: &meclaw_core::ActorHandle,
        path: &Path,
        messages: Vec<Message>,
    ) -> Result<(), Vec<Message>> {
        if messages.is_empty() {
            return Ok(());
        }
        let queued: Vec<Queued> = messages
            .into_iter()
            .map(|msg| Queued {
                seq: 0,
                bytes: body_bytes(&msg),
                msg,
                ticket: false,
            })
            .collect();
        let n = queued.len() as u64;
        let bytes: u64 = queued.iter().map(|q| q.bytes).sum();
        let Some(cell) = self.cell(path, handle) else {
            return Err(queued.into_iter().map(|q| q.msg).collect());
        };
        let sent = match cell.feed.as_ref() {
            Some(f) => f.send(OverflowCmd::Front(queued)),
            None => return Err(queued.into_iter().map(|q| q.msg).collect()),
        };
        if let Err(mpsc::error::SendError(OverflowCmd::Front(back))) = sent {
            self.lose(path, 0);
            return Err(back.into_iter().map(|q| q.msg).collect());
        }
        if let Some(cell) = self.cells.get_mut(path) {
            cell.pending += n;
            cell.pending_bytes += bytes;
            cell.mem_bytes += bytes;
            cell.front_bytes += bytes;
        }
        self.mem_bytes += bytes;
        Ok(())
    }

    /// Point `path`'s drain task at the mailbox of a respawned cell (the same
    /// cell, restarted — GH #18). Only the `CellDied` call site and a stale
    /// `Stalled` for the same mailbox call this.
    pub(crate) fn rehandle(&mut self, path: &Path, handle: &meclaw_core::ActorHandle) {
        let wiring = self.wiring.clone();
        let cfg = self.cfg;
        let Some(cell) = self.cells.get_mut(path) else {
            return;
        };
        cell.mailbox = Some(handle.sender_clone());
        let gone = match &cell.feed {
            Some(f) => f.send(OverflowCmd::Handle(handle.sender_clone())).is_err(),
            None => {
                if let Some(w) = wiring {
                    start_drain(cell, handle.sender_clone(), &w, cfg);
                }
                false
            }
        };
        if gone {
            self.lose(path, 0);
        }
    }

    /// The cell at `path` will not take its overflow: dead-letter what it holds.
    pub(crate) fn abandon(&mut self, path: &Path) {
        self.retire(path, "its cell is gone, failed or disconnected");
    }

    /// Whether `path` has an overflow.
    pub(crate) fn holds(&self, path: &Path) -> bool {
        self.cells.contains_key(path)
    }

    /// Whether `path`'s overflow delivers into the mailbox of `handle`.
    pub(crate) fn drains_into(&self, path: &Path, handle: &meclaw_core::ActorHandle) -> bool {
        self.cells
            .get(path)
            .and_then(|c| c.mailbox.as_ref())
            .is_some_and(|m| handle.same_mailbox(m))
    }

    /// Whether `path`'s overflow delivers into a mailbox that is still open —
    /// i.e. not the one of the cell that just died there.
    pub(crate) fn drains_into_an_open_mailbox(&self, path: &Path) -> bool {
        self.cells
            .get(path)
            .and_then(|c| c.mailbox.as_ref())
            .is_some_and(|m| !m.is_closed())
    }

    /// Retire the current generation at `path`: its drain task dead-letters
    /// everything it holds (`cell_inactive`), its counters settle under
    /// `retired` until it is empty, and the path is free for a new generation
    /// at once. A generation hydrated from disk that never ran gets a drain task
    /// for the purpose, with a mailbox nobody reads (it never delivers — the
    /// `Abandon` is the first thing it sees).
    fn retire(&mut self, path: &Path, why: &str) {
        let Some(mut cell) = self.cells.remove(path) else {
            return;
        };
        if cell.pending == 0 {
            self.mem_bytes = self.mem_bytes.saturating_sub(cell.mem_bytes);
            return;
        }
        tracing::warn!(
            target = %path.as_str(),
            pending = cell.pending,
            reason = "cell_inactive",
            "mailbox overflow retired — {why}; dead-lettering what it holds (cell_inactive)"
        );
        if cell.feed.is_none()
            && let Some(w) = self.wiring.clone()
        {
            let (nobody, _dropped) = mpsc::channel::<Message>(1);
            start_drain(&mut cell, nobody, &w, self.cfg);
        }
        let sent = cell
            .feed
            .as_ref()
            .is_some_and(|f| f.send(OverflowCmd::Abandon).is_ok());
        if !sent {
            self.defect(
                path,
                "the drain task of a retired overflow is gone — what it held is lost",
            );
            self.mem_bytes = self.mem_bytes.saturating_sub(cell.mem_bytes);
            return;
        }
        self.retired.insert(cell.generation, cell);
    }

    /// Apply a drain task's report. Writes go through `log_tx` (the colony is
    /// the single owner of the writer, FIX 2); dead letters into `dead_letters`;
    /// tickets of dead-lettered messages back into `tickets_back` (one path per
    /// ticket). The registry-dependent rest is the caller's ([`Followup`]).
    ///
    /// A report from a generation the colony no longer knows is a defect: it is
    /// logged, and its dead letters and table writes are carried out anyway —
    /// nothing a drain task hands back is dropped (GH #850 review M-2).
    pub(crate) async fn on_report(
        &mut self,
        report: OverflowReport,
        log_tx: &mpsc::Sender<crate::persist::writer::ColonyWriteOp>,
        dead_letters: &mut VecDeque<crate::DeadLetter>,
        tickets_back: &mut Vec<Path>,
    ) -> Followup {
        match report.0 {
            Report::Settled {
                path,
                generation,
                tally,
                disk_ids,
                dead,
            } => {
                let delivered = tally.n > dead.len() as u64;
                let mut current = false;
                match self.slot(&path, generation) {
                    Some((cell, is_current)) => {
                        current = is_current;
                        cell.pending = cell.pending.saturating_sub(tally.n);
                        cell.pending_bytes = cell.pending_bytes.saturating_sub(tally.bytes);
                        cell.mem_bytes = cell.mem_bytes.saturating_sub(tally.mem_bytes);
                        cell.front_bytes = cell.front_bytes.saturating_sub(tally.front_bytes);
                        let empty = cell.pending == 0;
                        self.mem_bytes = self.mem_bytes.saturating_sub(tally.mem_bytes);
                        if empty {
                            // Dropping the feed ends the drain task, which holds nothing.
                            if is_current {
                                self.cells.remove(&path);
                            } else {
                                self.retired.remove(&generation);
                            }
                        }
                    }
                    None => self.defect(
                        &path,
                        "a settlement from an overflow generation the colony does not know — \
                         applying its dead letters and deletes anyway",
                    ),
                }
                if !disk_ids.is_empty() {
                    self.probe(OverflowProbe::TableDelete(path.clone(), disk_ids.len()));
                    let _ = log_tx
                        .send(crate::persist::writer::ColonyWriteOp::DeleteOverflow {
                            cell_path: path.as_str().to_string(),
                            ids: disk_ids,
                        })
                        .await;
                }
                for (msg, reason, ticket) in dead {
                    if ticket {
                        tickets_back.push(path.clone());
                    }
                    let sender_path = msg.reply_to.clone().unwrap_or_else(|| Path::new("/"));
                    crate::colony::push_dead_letter(
                        dead_letters,
                        crate::DeadLetter {
                            sender_path,
                            original_target: msg.target.clone(),
                            resolved_target: path.clone(),
                            message: msg,
                            reason,
                        },
                    );
                }
                if delivered && current {
                    Followup::Delivered(path)
                } else {
                    Followup::None
                }
            }
            Report::Spill {
                path,
                generation,
                rows,
                bytes,
                ack,
            } => {
                match self.slot(&path, generation) {
                    Some((cell, _)) => {
                        cell.mem_bytes = cell.mem_bytes.saturating_sub(bytes);
                        cell.spill_requested = false;
                        self.mem_bytes = self.mem_bytes.saturating_sub(bytes);
                    }
                    // An EMPTY answer with no slot is the end of a legitimate
                    // race, not a defect (review of K2, M-a): the colony asked
                    // for a spill, stage 1 had emptied meanwhile, and the
                    // settlement the drain task sent first took `pending` to 0
                    // and removed the generation. Nothing to write; the ack
                    // below clears the request.
                    None if rows.is_empty() => {}
                    None => self.defect(
                        &path,
                        "a spill from an overflow generation the colony does not know — \
                         writing it anyway, the next boot settles it",
                    ),
                }
                if rows.is_empty() {
                    let _ = ack.send(());
                } else {
                    self.probe(OverflowProbe::TableWrite(path.clone(), rows.len()));
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let _ = log_tx
                        .send(crate::persist::writer::ColonyWriteOp::InsertOverflow {
                            cell_path: path.as_str().to_string(),
                            rows,
                            enqueued_at: now,
                            ack: Some(ack),
                        })
                        .await;
                }
                self.enforce_memory_ceiling();
                Followup::None
            }
            Report::Stalled { path, generation } => match self.cells.get(&path) {
                Some(c) if c.generation == generation => Followup::Stalled(path),
                // A retired generation is dead-lettering already; an unknown
                // one has nothing left the colony counts.
                _ => Followup::None,
            },
        }
    }

    /// The counters a report of `(path, generation)` belongs to, and whether
    /// they are the path's current generation.
    fn slot(&mut self, path: &Path, generation: u64) -> Option<(&mut CellOverflow, bool)> {
        if self
            .cells
            .get(path)
            .is_some_and(|c| c.generation == generation)
        {
            return self.cells.get_mut(path).map(|c| (c, true));
        }
        self.retired
            .get_mut(&generation)
            .filter(|c| c.path == *path)
            .map(|c| (c, false))
    }

    /// The counters of `path`, creating them (and the drain task) on first use.
    /// `None` only when the overflow is not wired.
    ///
    /// A cell hydrated from disk has counters but no task yet; its task starts
    /// here too, told that its disk is not empty.
    fn cell(
        &mut self,
        path: &Path,
        handle: &meclaw_core::ActorHandle,
    ) -> Option<&mut CellOverflow> {
        let w = self.wiring.clone()?;
        if !self.cells.contains_key(path) {
            let generation = self.next_generation;
            self.next_generation += 1;
            self.probe(OverflowProbe::Entered(path.clone()));
            let floor = self.next_seq - 1;
            self.cells.insert(
                path.clone(),
                CellOverflow {
                    path: path.clone(),
                    generation,
                    pending: 0,
                    pending_bytes: 0,
                    mem_bytes: 0,
                    front_bytes: 0,
                    hydrated: false,
                    floor,
                    disk_upto: floor,
                    feed: None,
                    mailbox: None,
                    spill_requested: false,
                },
            );
        }
        let cfg = self.cfg;
        let cell = self.cells.get_mut(path)?;
        if cell.feed.is_none() {
            start_drain(cell, handle.sender_clone(), &w, cfg);
        }
        Some(cell)
    }

    /// The drain task of `path`'s current generation is gone: a defect, since it
    /// only ends when its feed is dropped or the colony is. The counters are
    /// dropped with a loud line; `extra_mem` is memory the caller counted into
    /// the cell but not yet into the colony total.
    fn lose(&mut self, path: &Path, extra_mem: u64) {
        let Some(cell) = self.cells.remove(path) else {
            return;
        };
        tracing::error!(
            target = %path.as_str(),
            pending = cell.pending,
            "mailbox overflow: the drain task of this cell is gone — its overflow is lost"
        );
        self.mem_bytes = self
            .mem_bytes
            .saturating_sub(cell.mem_bytes.saturating_sub(extra_mem));
    }

    /// Above the memory ceiling of all overflows: ask the cell holding the most
    /// SPILLABLE bytes (stage 1 — a rescued front cannot go to disk, GH #850
    /// review M-6) to spill a block, one request at a time per cell.
    fn enforce_memory_ceiling(&mut self) {
        if self.mem_bytes <= self.cfg.memory_bytes {
            return;
        }
        let spillable = |c: &CellOverflow| c.mem_bytes.saturating_sub(c.front_bytes);
        let Some((_, cell)) = self
            .cells
            .iter_mut()
            .filter(|(_, c)| !c.spill_requested && spillable(c) > 0 && c.feed.is_some())
            .max_by_key(|(_, c)| spillable(c))
        else {
            return;
        };
        if let Some(f) = &cell.feed
            && f.send(OverflowCmd::Spill).is_ok()
        {
            cell.spill_requested = true;
        }
    }

    /// One `warn!` per second and cell when an overflow comes into being — a
    /// cell at the edge of its capacity opens and closes one per message. Names
    /// the cell, the sender and the trace like the GH #162 line it replaced, and
    /// how many overflows opened since the last line.
    fn warn_enter(&mut self, path: &Path, sender: &Path, msg: &Message, capacity: usize) {
        if let Some(opened) = self.enter_warned.entry(path.clone()).or_default().tick() {
            tracing::warn!(
                target = %path.as_str(),
                sender = %sender.as_str(),
                trace_id = %msg.trace_id,
                mailbox_capacity = capacity,
                opened,
                reason = "mailbox_overflow",
                "target mailbox is FULL — further messages for it wait in its overflow, in \
                 order, and the colony keeps routing. If this repeats, the cell is too slow \
                 for its producers or an edge is multiplying messages (cell.mailbox_size \
                 raises the buffer, it does not fix a loop)"
            );
        }
    }

    /// One `warn!` per second and cell for refusals at the cap, naming how many
    /// were refused since the last one.
    fn warn_cap(&mut self, path: &Path, pending: u64, pending_bytes: u64) {
        let (cap_messages, cap_bytes) = (self.cfg.cap_messages, self.cfg.cap_bytes);
        if let Some(refused) = self.cap_warned.entry(path.clone()).or_default().tick() {
            tracing::warn!(
                target = %path.as_str(),
                pending,
                pending_bytes,
                refused,
                cap_messages,
                cap_bytes,
                reason = "mailbox_full",
                "the overflow of this cell is at its cap — dead-lettering as mailbox_full \
                 (colony.json mailbox_overflow_cap_messages / mailbox_overflow_cap_bytes)"
            );
        }
    }

    /// A state that cannot happen while the counters are consistent: one
    /// `error!` per second for all of them, with how many since.
    fn defect(&mut self, path: &Path, what: &str) {
        if let Some(count) = self.defect_warned.tick() {
            tracing::error!(target = %path.as_str(), count, "mailbox overflow: {what}");
        }
    }

    fn probe(&self, event: OverflowProbe) {
        if let Some(p) = self.wiring.as_ref().and_then(|w| w.probe.as_ref()) {
            let _ = p.send(event);
        }
    }
}

/// Start the drain task of one generation, delivering into `mailbox`.
fn start_drain(
    cell: &mut CellOverflow,
    mailbox: mpsc::Sender<Message>,
    w: &Wiring,
    cfg: OverflowConfig,
) {
    let (feed, cmds) = mpsc::unbounded_channel();
    cell.mailbox = Some(mailbox.clone());
    let drain = Drain {
        path: cell.path.clone(),
        generation: cell.generation,
        sender: mailbox,
        stalled: false,
        mailbox_epoch: 0,
        stall_reported: false,
        cmds,
        report_tx: w.report_tx.clone(),
        db_path: w.db_path.clone(),
        probe: w.probe.clone(),
        spill_messages: cfg.spill_messages,
        spill_bytes: cfg.spill_bytes,
        front: VecDeque::new(),
        disk: VecDeque::new(),
        disk_more: cell.hydrated,
        disk_after: cell.floor,
        disk_upto: cell.disk_upto,
        mem: VecDeque::new(),
        mem_bytes: 0,
        tally: Tally::default(),
        disk_ids: Vec::new(),
        dead: Vec::new(),
    };
    tokio::spawn(drain.run());
    cell.feed = Some(feed);
}

/// A message read back from stage 2.
#[allow(clippy::large_enum_variant)]
enum DiskItem {
    /// The row and its message.
    Found {
        id: String,
        msg: Message,
        bytes: u64,
    },
    /// The row names a message `message_log` does not have.
    Missing { id: String, bytes: u64 },
}

/// Where the next message comes from.
#[derive(Clone, Copy)]
enum Source {
    Front,
    Disk,
    Mem,
}

/// One generation's drain task: owns the overflow queue, delivers in order.
struct Drain {
    path: Path,
    generation: u64,
    sender: mpsc::Sender<Message>,
    /// The mailbox behind `sender` is closed; waiting for `Handle`/`Abandon`.
    stalled: bool,
    /// Counts `Handle` commands, so a permit taken before one is not used after.
    mailbox_epoch: u64,
    stall_reported: bool,
    cmds: mpsc::UnboundedReceiver<OverflowCmd>,
    report_tx: mpsc::Sender<crate::ColonyMsg>,
    db_path: std::path::PathBuf,
    probe: Option<mpsc::UnboundedSender<OverflowProbe>>,
    spill_messages: u64,
    spill_bytes: u64,
    /// Rescued mailbox messages — delivered before everything else.
    front: VecDeque<Queued>,
    /// Rows read back from disk, not yet delivered.
    disk: VecDeque<DiskItem>,
    /// Rows past `disk_after` may exist on disk.
    disk_more: bool,
    /// The highest `seq` read from disk so far (starts at the generation's floor).
    disk_after: i64,
    /// The highest `seq` of this generation on disk — what it spilled, or what
    /// the boot found. Reads never go past it.
    disk_upto: i64,
    /// Stage 1.
    mem: VecDeque<Queued>,
    mem_bytes: u64,
    /// What the next report carries.
    tally: Tally,
    disk_ids: Vec<String>,
    dead: Vec<(Message, DeadLetterReason, bool)>,
}

/// How a wait for room in the mailbox ended.
#[allow(clippy::large_enum_variant)]
enum Woke {
    Cmd(Option<OverflowCmd>),
    Room(Result<mpsc::OwnedPermit<Message>, mpsc::error::SendError<()>>),
}

impl Drain {
    /// The loop. Ends when the colony drops the feed (the overflow is settled,
    /// or the colony is gone) or when a report cannot reach the colony.
    async fn run(mut self) {
        loop {
            // Everything the colony already said, in order, before the next pick.
            loop {
                match self.cmds.try_recv() {
                    Ok(cmd) => {
                        if !self.apply(cmd).await {
                            return;
                        }
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => return,
                }
            }
            if !self.spill_while_over().await {
                return;
            }
            if self.front.is_empty() && self.disk.is_empty() && self.disk_more {
                self.read_disk_block().await;
            }
            self.settle_missing_head();
            let Some(source) = self.next_source() else {
                // Unread disk rows that could not be read this pass (the read
                // failed and backed off): read again, never wait on news first.
                if self.disk_more {
                    continue;
                }
                // Nothing to deliver: say what happened, then wait for news.
                if !self.report().await {
                    return;
                }
                match self.cmds.recv().await {
                    Some(cmd) => {
                        if !self.apply(cmd).await {
                            return;
                        }
                    }
                    None => return,
                }
                continue;
            };
            if self.stalled {
                if !self.report().await {
                    return;
                }
                if !self.stall_reported {
                    self.stall_reported = true;
                    if !self
                        .send_report(Report::Stalled {
                            path: self.path.clone(),
                            generation: self.generation,
                        })
                        .await
                    {
                        return;
                    }
                }
                match self.cmds.recv().await {
                    Some(cmd) => {
                        if !self.apply(cmd).await {
                            return;
                        }
                    }
                    None => return,
                }
                continue;
            }
            // An owned permit (on a cheap clone of the sender) so the permit does
            // not borrow `self` while the head is taken.
            match self.sender.clone().try_reserve_owned() {
                Ok(permit) => {
                    if let Some(msg) = self.take(source) {
                        permit.send(msg);
                    }
                    if self.tally.n >= REPORT_EVERY && !self.report().await {
                        return;
                    }
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    // About to wait: report first, so the colony's counter and a
                    // parked cell's wake never lag behind a wait.
                    if !self.report().await {
                        return;
                    }
                    let woke = tokio::select! {
                        biased;
                        cmd = self.cmds.recv() => Woke::Cmd(cmd),
                        room = self.sender.clone().reserve_owned() => Woke::Room(room),
                    };
                    match woke {
                        Woke::Cmd(Some(cmd)) => {
                            if !self.apply(cmd).await {
                                return;
                            }
                        }
                        Woke::Cmd(None) => return,
                        Woke::Room(Ok(permit)) => {
                            // A command that arrived meanwhile may take time
                            // (`Spill` waits for the commit, `Abandon` reads the
                            // disk), and the cell may die meanwhile: a permit
                            // held across that would put a message into a
                            // mailbox already rescued, lost without a dead
                            // letter (GH #850 review M-1). So a permit is only
                            // used when nothing came in; otherwise it is given
                            // back and the loop picks again.
                            match self.cmds.try_recv() {
                                Ok(cmd) => {
                                    drop(permit);
                                    if !self.apply(cmd).await {
                                        return;
                                    }
                                }
                                Err(mpsc::error::TryRecvError::Disconnected) => return,
                                Err(mpsc::error::TryRecvError::Empty) => {
                                    self.settle_missing_head();
                                    match self.next_source().and_then(|src| self.take(src)) {
                                        Some(msg) => {
                                            permit.send(msg);
                                        }
                                        None => drop(permit),
                                    }
                                }
                            }
                        }
                        Woke::Room(Err(_)) => self.stalled = true,
                    }
                }
                Err(mpsc::error::TrySendError::Closed(_)) => self.stalled = true,
            }
        }
    }

    /// Apply one command. `false` = the colony is gone.
    async fn apply(&mut self, cmd: OverflowCmd) -> bool {
        match cmd {
            OverflowCmd::Push(q) => {
                self.mem_bytes += q.bytes;
                self.mem.push_back(q);
                true
            }
            OverflowCmd::Front(v) => {
                for q in v.into_iter().rev() {
                    self.front.push_front(q);
                }
                true
            }
            OverflowCmd::Spill => self.spill_requested_block().await,
            OverflowCmd::Handle(sender) => {
                self.sender = sender;
                self.mailbox_epoch += 1;
                self.stalled = false;
                self.stall_reported = false;
                true
            }
            OverflowCmd::Abandon => self.abandon_all().await,
        }
    }

    /// The source of the next delivery: front, then disk, then memory. Disk
    /// rows that may still be unread block memory — they are older.
    fn next_source(&self) -> Option<Source> {
        if !self.front.is_empty() {
            Some(Source::Front)
        } else if !self.disk.is_empty() {
            Some(Source::Disk)
        } else if self.disk_more {
            // Unread rows that could not be read right now: nothing may overtake
            // them. The loop reads again on the next pass.
            None
        } else if !self.mem.is_empty() {
            Some(Source::Mem)
        } else {
            None
        }
    }

    /// Take the head of `source` and count it as delivered. `None` when the
    /// head turned out to be a row whose message is gone (dead-lettered here) —
    /// the caller then drops its permit and picks again.
    fn take(&mut self, source: Source) -> Option<Message> {
        let q = match source {
            Source::Front => {
                let q = self.front.pop_front();
                if let Some(q) = &q {
                    self.tally.front_bytes += q.bytes;
                }
                q
            }
            Source::Mem => {
                let q = self.mem.pop_front();
                if let Some(q) = &q {
                    self.mem_bytes = self.mem_bytes.saturating_sub(q.bytes);
                }
                q
            }
            Source::Disk => {
                return match self.disk.pop_front() {
                    Some(DiskItem::Found { id, msg, bytes }) => {
                        self.tally.n += 1;
                        self.tally.bytes += bytes;
                        self.disk_ids.push(id);
                        Some(msg)
                    }
                    Some(DiskItem::Missing { id, bytes }) => {
                        self.dead_missing(id, bytes);
                        None
                    }
                    None => None,
                };
            }
        }?;
        self.tally.n += 1;
        self.tally.bytes += q.bytes;
        self.tally.mem_bytes += q.bytes;
        Some(q.msg)
    }

    /// Dead-letter rows at the head of the disk buffer whose message is gone.
    fn settle_missing_head(&mut self) {
        while let Some(DiskItem::Missing { .. }) = self.disk.front() {
            if let Some(DiskItem::Missing { id, bytes }) = self.disk.pop_front() {
                self.dead_missing(id, bytes);
            }
        }
    }

    fn dead_missing(&mut self, id: String, bytes: u64) {
        tracing::error!(
            target = %self.path.as_str(),
            message_id = %id,
            reason = "mailbox_full",
            "mailbox overflow: a persisted row names a message missing from the message \
             log — dead-lettering it (mailbox_full, missing_from_log)"
        );
        let mut msg = meclaw_core::MessageBuilder::new(self.path.clone()).build();
        if let Ok(u) = meclaw_core::Uuid::parse_str(&id) {
            msg.id = u;
        }
        self.tally.n += 1;
        self.tally.bytes += bytes;
        self.disk_ids.push(id);
        self.dead.push((
            msg,
            DeadLetterReason::MailboxFull {
                detail: Some(MISSING_FROM_LOG.to_string()),
            },
            true,
        ));
    }

    /// Spill while stage 1 is over this cell's threshold.
    async fn spill_while_over(&mut self) -> bool {
        while !self.mem.is_empty()
            && (self.mem.len() as u64 > self.spill_messages || self.mem_bytes > self.spill_bytes)
        {
            if !self.spill_block().await {
                return false;
            }
        }
        true
    }

    /// The colony asked for a spill (memory ceiling). When stage 1 emptied in
    /// the meantime there is nothing to spill — but the colony still waits for
    /// the answer to clear its request, so it gets an empty one, after the
    /// settlement that explains why (GH #850 review M-6).
    async fn spill_requested_block(&mut self) -> bool {
        if !self.mem.is_empty() {
            return self.spill_block().await;
        }
        if !self.report().await {
            return false;
        }
        let (ack, done) = oneshot::channel();
        self.send_report(Report::Spill {
            path: self.path.clone(),
            generation: self.generation,
            rows: Vec::new(),
            bytes: 0,
            ack,
        })
        .await
            && done.await.is_ok()
    }

    /// Move the oldest block of stage 1 to disk. Waits for the commit, so a read
    /// that follows sees the rows (and the log rows before them — same FIFO
    /// writer channel). `false` = the colony is gone.
    async fn spill_block(&mut self) -> bool {
        let n = self.mem.len().min(BLOCK);
        if n == 0 {
            return true;
        }
        let block: Vec<Queued> = self.mem.drain(..n).collect();
        let bytes: u64 = block.iter().map(|q| q.bytes).sum();
        self.mem_bytes = self.mem_bytes.saturating_sub(bytes);
        let rows: Vec<(i64, String, i64)> = block
            .iter()
            .map(|q| (q.seq, q.msg.id.to_string(), q.bytes as i64))
            .collect();
        let top = rows.iter().map(|r| r.0).max().unwrap_or(self.disk_upto);
        let (ack, done) = oneshot::channel();
        if !self
            .send_report(Report::Spill {
                path: self.path.clone(),
                generation: self.generation,
                rows,
                bytes,
                ack,
            })
            .await
        {
            return false;
        }
        if done.await.is_err() {
            return false;
        }
        self.disk_upto = self.disk_upto.max(top);
        self.disk_more = true;
        true
    }

    /// Read the next block of stage 2, in order, past what was read already and
    /// never past this generation's own rows.
    async fn read_disk_block(&mut self) {
        let db_path = self.db_path.clone();
        let path = self.path.as_str().to_string();
        let (after, upto) = (self.disk_after, self.disk_upto);
        let read =
            tokio::task::spawn_blocking(move || read_block(&db_path, &path, after, upto, BLOCK))
                .await;
        let rows = match read {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => {
                tracing::error!(
                    target = %self.path.as_str(),
                    error = %e,
                    "mailbox overflow: reading the persisted overflow failed — retrying in 1 s"
                );
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                return;
            }
            Err(e) => {
                tracing::error!(
                    target = %self.path.as_str(),
                    error = %e,
                    "mailbox overflow: the read task failed — retrying in 1 s"
                );
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                return;
            }
        };
        if let Some(p) = &self.probe {
            let _ = p.send(OverflowProbe::TableRead(self.path.clone(), rows.len()));
        }
        if rows.len() < BLOCK {
            self.disk_more = false;
        }
        for (seq, item) in rows {
            self.disk_after = self.disk_after.max(seq);
            self.disk.push_back(item);
        }
    }

    /// Dead-letter everything held right now, disk included (`cell_inactive`,
    /// like a disconnected cell's mailbox remainder). `false` = colony gone.
    async fn abandon_all(&mut self) -> bool {
        let reason = DeadLetterReason::CellInactive;
        let mut count = 0usize;
        loop {
            while let Some(q) = self.front.pop_front() {
                count += 1;
                self.dead_queued(q, reason.clone(), true);
            }
            self.settle_missing_head();
            while let Some(item) = self.disk.pop_front() {
                count += 1;
                match item {
                    DiskItem::Found { id, msg, bytes } => {
                        self.tally.n += 1;
                        self.tally.bytes += bytes;
                        self.disk_ids.push(id);
                        self.dead.push((msg, reason.clone(), true));
                    }
                    DiskItem::Missing { id, bytes } => self.dead_missing(id, bytes),
                }
            }
            if !self.report().await {
                return false;
            }
            if !self.disk_more {
                break;
            }
            self.read_disk_block().await;
        }
        while let Some(q) = self.mem.pop_front() {
            count += 1;
            self.mem_bytes = self.mem_bytes.saturating_sub(q.bytes);
            self.dead_queued(q, reason.clone(), false);
            if self.dead.len() >= BLOCK && !self.report().await {
                return false;
            }
        }
        if count == 0 {
            tracing::debug!(target = %self.path.as_str(), "mailbox overflow abandoned (empty)");
        } else {
            tracing::warn!(
                target = %self.path.as_str(),
                count,
                "mailbox overflow: the mailbox it was for will not take it — dead-lettering \
                 it (cell_inactive)"
            );
        }
        self.report().await
    }

    fn dead_queued(&mut self, q: Queued, reason: DeadLetterReason, from_front: bool) {
        self.tally.n += 1;
        self.tally.bytes += q.bytes;
        self.tally.mem_bytes += q.bytes;
        if from_front {
            self.tally.front_bytes += q.bytes;
        }
        self.dead.push((q.msg, reason, q.ticket));
    }

    /// Send the counted settlement, if any. `false` = colony gone.
    async fn report(&mut self) -> bool {
        if self.tally.n == 0 && self.disk_ids.is_empty() && self.dead.is_empty() {
            return true;
        }
        let r = Report::Settled {
            path: self.path.clone(),
            generation: self.generation,
            tally: std::mem::take(&mut self.tally),
            disk_ids: std::mem::take(&mut self.disk_ids),
            dead: std::mem::take(&mut self.dead),
        };
        self.send_report(r).await
    }

    async fn send_report(&self, r: Report) -> bool {
        self.report_tx
            .send(crate::ColonyMsg::Overflow(OverflowReport(r)))
            .await
            .is_ok()
    }
}

/// One block of stage 2 of `cell_path`, oldest first, in `after < seq <= upto`,
/// each row joined with its `message_log` row. A fresh read-only connection per
/// call (ADR-0041: reads leave the loop; this one never was in it).
fn read_block(
    db_path: &std::path::Path,
    cell_path: &str,
    after: i64,
    upto: i64,
    limit: usize,
) -> rusqlite::Result<Vec<(i64, DiskItem)>> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    crate::persist::apply_busy_timeout(&conn)?;
    let mut stmt = conn.prepare(
        "SELECT o.seq, o.message_id, o.bytes,
                m.id, m.trace_id, m.parent_message_id, m.correlation_id, m.ttl,
                m.to_path, m.reply_to, m.headers, m.body_kind, m.body_payload, m.created_at
         FROM mailbox_overflow o LEFT JOIN message_log m ON m.id = o.message_id
         WHERE o.cell_path = ?1 AND o.seq > ?2 AND o.seq <= ?3
         ORDER BY o.seq LIMIT ?4",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![cell_path, after, upto, limit as i64],
        |r| {
            let seq: i64 = r.get(0)?;
            let id: String = r.get(1)?;
            let bytes: i64 = r.get(2)?;
            let found: Option<String> = r.get(3)?;
            if found.is_none() {
                return Ok((
                    seq,
                    DiskItem::Missing {
                        id,
                        bytes: bytes.max(0) as u64,
                    },
                ));
            }
            let log = LogColumns {
                trace_id: r.get(4)?,
                parent_message_id: r.get(5)?,
                correlation_id: r.get(6)?,
                ttl: r.get(7)?,
                to_path: r.get(8)?,
                reply_to: r.get(9)?,
                headers: r.get(10)?,
                body_kind: r.get(11)?,
                body_payload: r.get(12)?,
                created_at: r.get(13)?,
            };
            Ok(match message_from_log(&id, log) {
                Some(msg) => (
                    seq,
                    DiskItem::Found {
                        id,
                        msg,
                        bytes: bytes.max(0) as u64,
                    },
                ),
                None => (
                    seq,
                    DiskItem::Missing {
                        id,
                        bytes: bytes.max(0) as u64,
                    },
                ),
            })
        },
    )?;
    rows.collect()
}

/// The `message_log` columns of one message.
struct LogColumns {
    trace_id: String,
    parent_message_id: Option<String>,
    correlation_id: Option<String>,
    ttl: i64,
    to_path: String,
    reply_to: Option<String>,
    headers: String,
    body_kind: String,
    body_payload: Option<String>,
    created_at: i64,
}

/// The inverse of the colony's log-row builder: the message as the router
/// delivers it (`ttl` is the post-decrement value the log stores, `target` the
/// resolved `to_path`). `None` when a column does not parse — the row is then
/// treated like a missing one, never delivered half.
fn message_from_log(id: &str, c: LogColumns) -> Option<Message> {
    let uuid = |s: &str| meclaw_core::Uuid::parse_str(s).ok();
    let body = match c.body_kind.as_str() {
        "blob" => meclaw_core::Body::Blob(uuid(c.body_payload.as_deref()?)?),
        _ => meclaw_core::Body::Inline(
            meclaw_core::serde_json::from_str(c.body_payload.as_deref().unwrap_or("null")).ok()?,
        ),
    };
    let parent_message_id = match c.parent_message_id.as_deref() {
        Some(s) => Some(uuid(s)?),
        None => None,
    };
    let correlation_id = match c.correlation_id.as_deref() {
        Some(s) => Some(uuid(s)?),
        None => None,
    };
    Some(Message {
        id: uuid(id)?,
        trace_id: uuid(&c.trace_id)?,
        parent_message_id,
        correlation_id,
        target: Path::new(&c.to_path),
        reply_to: c.reply_to.as_deref().map(Path::new),
        ttl: u32::try_from(c.ttl.max(0)).unwrap_or(0),
        headers: meclaw_core::serde_json::from_str(&c.headers).ok()?,
        body,
        created_at: c.created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(body_kind: &str, body_payload: Option<&str>) -> LogColumns {
        LogColumns {
            trace_id: meclaw_core::Uuid::now_v7().to_string(),
            parent_message_id: None,
            correlation_id: None,
            ttl: 7,
            to_path: "/c".into(),
            reply_to: Some("/r".into()),
            headers: "{}".into(),
            body_kind: body_kind.into(),
            body_payload: body_payload.map(str::to_owned),
            created_at: 42,
        }
    }

    /// A log row comes back as the message the router would have delivered.
    #[test]
    fn a_log_row_comes_back_as_the_routed_message() {
        let id = meclaw_core::Uuid::now_v7();
        let m = message_from_log(
            &id.to_string(),
            cols("inline", Some(r#"{"messages":[{"text":"x"}]}"#)),
        )
        .expect("a well-formed row");
        assert_eq!(m.id, id);
        assert_eq!(m.ttl, 7);
        assert_eq!(m.target.as_str(), "/c");
        assert_eq!(m.reply_to.as_ref().map(|p| p.as_str()), Some("/r"));
        assert_eq!(m.created_at, 42);
        assert!(matches!(m.body, meclaw_core::Body::Inline(_)));
        let blob = meclaw_core::Uuid::now_v7();
        let b = message_from_log(&id.to_string(), cols("blob", Some(&blob.to_string())))
            .expect("a blob row");
        assert!(matches!(b.body, meclaw_core::Body::Blob(u) if u == blob));
    }

    /// A row that does not parse is never delivered half.
    #[test]
    fn a_broken_log_row_is_none() {
        let id = meclaw_core::Uuid::now_v7().to_string();
        assert!(message_from_log(&id, cols("blob", Some("not-a-uuid"))).is_none());
        assert!(message_from_log(&id, cols("inline", Some("{not json"))).is_none());
        assert!(message_from_log("nope", cols("inline", Some("{}"))).is_none());
    }

    /// Review of K2, M-a: the colony asked for a spill, the drain task's stage 1
    /// had emptied meanwhile, so it sends the settlement first -- which takes
    /// `pending` to 0 and removes the generation -- and then the empty spill
    /// answer. That answer meets no slot, and it is the legitimate end of the
    /// race, not a defect: it is acked and nothing is logged as "cannot happen".
    #[tokio::test]
    async fn an_empty_spill_after_the_last_settlement_is_no_defect() {
        let mut o = Overflow::default();
        let (log_tx, _log_rx) = mpsc::channel(1);
        let (ack, done) = oneshot::channel();
        let mut dead = VecDeque::new();
        let mut tickets = Vec::new();
        let _ = o
            .on_report(
                OverflowReport(Report::Spill {
                    path: Path::new("/c"),
                    generation: 7,
                    rows: Vec::new(),
                    bytes: 0,
                    ack,
                }),
                &log_tx,
                &mut dead,
                &mut tickets,
            )
            .await;
        assert!(done.await.is_ok(), "the empty answer is acked");
        assert!(
            o.defect_warned.last.is_none() && o.defect_warned.since == 0,
            "an empty spill after the last settlement was logged as a defect"
        );
    }

    /// The unwired overflow (router unit tests) never diverts a message.
    #[test]
    fn an_unwired_overflow_never_diverts() {
        let (tx, _rx) = mpsc::channel(1);
        let h = meclaw_core::ActorHandle::new(Path::new("/c"), tx);
        h.try_send(meclaw_core::MessageBuilder::new(Path::new("/c")).build())
            .unwrap();
        let o = Overflow::default();
        assert!(!o.must_overflow(&Path::new("/c"), &h));
        assert!(o.is_empty());
    }
}
