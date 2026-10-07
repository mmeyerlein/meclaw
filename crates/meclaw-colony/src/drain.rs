//! GH #47: the ledger the shutdown drain waits on.
//!
//! A message that the colony has handed to a cell is work in flight until that
//! cell's `handle()` has returned. The colony cannot see the second half — a
//! mailbox that has gone empty means the cell TOOK the message, not that it is
//! done with it (`docs/defer-register.md` § Async cell shutdown drain: an empty
//! mailbox is not a finished handler). So the two halves are recorded at the
//! two places that can see them:
//!
//! * the colony takes a ticket in `route_with_log`, at the `pre_routable`
//!   predicate — BEFORE the message enters the mailbox, so there is no window in
//!   which the message is neither queued nor accounted for;
//! * the cell task gives it back via `ColonyMsg::WorkDone` when its handler is
//!   done, from a guard whose `Drop` also runs on a panic, on a backstop
//!   cancellation and on a task abort.
//!
//! The ledger is plain `colony_task`-local state. It is deliberately NOT an
//! `Arc<AtomicI64>` shared with the cells: the substrate's concurrency model is
//! one task per actor with its state inside the task, and a shared counter would
//! be exactly the atomic that `AGENTS.md` rules out — besides costing a twelfth
//! `CellFactory::spawn_cell` parameter across every cell type.

use meclaw_core::Path;
use std::collections::HashMap;

/// Outstanding deliveries per cell path.
///
/// An entry is only present while its count is non-zero; `leave` removes the
/// key on the last ticket, so `busy_paths` never names a settled cell.
///
/// Every method has a production caller: `enter` in `route_with_log`, `leave` in
/// the `ColonyMsg::WorkDone` arm, `forget` in the death/sleep/stop/rescue arms,
/// `total` in the quiescence check and the deadline warning, `busy_paths` in
/// that warning. The `#[allow(dead_code)]` this impl block used to carry came
/// out with the last of them (GH #47 Task 12), verified empirically: clippy is
/// green without it.
#[derive(Debug, Default)]
pub(crate) struct DrainLedger {
    owed: HashMap<Path, u32>,
    /// GH #850: the overflow of every cell whose mailbox was full. It rides in
    /// the ledger (OR-SN.K2.1) because both answer what the colony still owes a
    /// cell, and the ledger already reaches every call site of the router —
    /// including those inside the mutation handling, which this strand does not
    /// touch. A colony whose overflow is not empty is never quiescent, whatever
    /// the tickets say.
    ///
    /// Tickets (GH #850 review M-3, OR-SN.K2.4): a message the router puts into
    /// an overflow takes a ticket like any delivery, and it holds it until the
    /// cell handled it — unless the cell dies, sleeps or stops first: then
    /// [`Self::forget`] clears the path's debt, overflow tickets included, as
    /// it always has. What is still in the overflow then is guarded by the
    /// overflow check above; what it later delivers is ticketless, like a
    /// rescued mailbox (GH #18), and is guarded only by the mailbox backlog —
    /// a message a cell has taken out of its mailbox and not finished can be
    /// cut by a shutdown, the same class GH #47 names for rescued mail. Keeping
    /// those tickets instead would need the drain task's unreported deliveries,
    /// and one message lost in the death (the one the cell was handling) would
    /// then hold a ticket nobody ever returns: every later drain would run into
    /// its deadline. A dead-lettered overflow message gives its ticket back
    /// with [`Self::leave`], which is forgiving.
    pub(crate) overflow: crate::overflow::Overflow,
    /// GH #1068: messages routed to a cell whose mailbox its dying task had
    /// already closed. `route()` (byte-frozen) would send into the closed
    /// mailbox, fail, and drop the message with one warning line; the wrapper
    /// parks it here instead, keyed by the dead cell's birth path, and the
    /// `CellDied` that is on its way hands it to the successor right behind the
    /// rescued mailbox (or dead-letters it with that mailbox). It rides in the
    /// ledger for the reason the overflow does: the ledger reaches every call
    /// site of the router.
    parked_for_successor: HashMap<Path, Vec<meclaw_core::Message>>,
    /// GH #1068 review F2: open deliveries a dead letter ended outside the
    /// router -- a rescued mailbox or held mail with no successor. Each was
    /// logged with its `delivery_open` row when it was routed; a dead letter
    /// is an end, never a replay, so the row is closed. The colony's next
    /// dead-letter flush sends the closes right behind the dead letters
    /// ([`Self::take_deliveries_to_close`]); the functions that dead-letter
    /// here are synchronous and cannot send themselves.
    deliveries_to_close: Vec<(meclaw_core::Uuid, Path)>,
}

impl DrainLedger {
    /// GH #850: read the persisted overflow counters once at boot and hold one
    /// ticket per persisted message.
    pub(crate) fn hydrate_overflow(&mut self, conn: &rusqlite::Connection) {
        for (path, n) in self.overflow.hydrate(conn) {
            let owed = self.owed.entry(path).or_insert(0);
            *owed = owed.saturating_add(u32::try_from(n).unwrap_or(u32::MAX));
        }
    }

    /// GH #1068: hold `msg` for the successor of the cell born at `path`,
    /// whose dying task closed its mailbox before the message could enter it.
    ///
    /// Review F4: under the overflow's per-cell cap (`colony.json
    /// mailbox_overflow_cap_messages` / `_cap_bytes`) -- above it the message
    /// comes back and the caller dead-letters it as `mailbox_full`, as the
    /// overflow does.
    pub(crate) fn park_for_successor(
        &mut self,
        path: &Path,
        msg: meclaw_core::Message,
    ) -> Result<(), Box<meclaw_core::Message>> {
        let caps = self.overflow.caps();
        park_within(
            self.parked_for_successor.entry(path.clone()).or_default(),
            msg,
            caps,
        )
    }

    /// GH #1068: what [`Self::park_for_successor`] holds for `path`, oldest
    /// first, and nothing afterwards.
    pub(crate) fn take_parked_for_successor(&mut self, path: &Path) -> Vec<meclaw_core::Message> {
        self.parked_for_successor.remove(path).unwrap_or_default()
    }

    /// GH #1068: everything still parked (the shutdown flush dead-letters it).
    pub(crate) fn drain_parked_for_successor(&mut self) -> Vec<(Path, Vec<meclaw_core::Message>)> {
        self.parked_for_successor.drain().collect()
    }

    /// GH #1068 review F2: `msg` was routed (its `delivery_open` row is open)
    /// and ends as a dead letter here -- close the row with the next flush.
    /// The row is keyed by the path the router resolved, which a routed
    /// message carries as its `target`.
    pub(crate) fn close_delivery(&mut self, msg: &meclaw_core::Message) {
        self.deliveries_to_close.push((msg.id, msg.target.clone()));
    }

    /// GH #1068 review F2: the rows to close, oldest first, and nothing
    /// afterwards.
    pub(crate) fn take_deliveries_to_close(&mut self) -> Vec<(meclaw_core::Uuid, Path)> {
        std::mem::take(&mut self.deliveries_to_close)
    }

    /// The colony is about to put a message into this cell's mailbox.
    pub(crate) fn enter(&mut self, path: &Path) {
        *self.owed.entry(path.clone()).or_insert(0) += 1;
    }

    /// The cell reported that a handler finished.
    ///
    /// Saturating and forgiving: a `WorkDone` without a matching ticket is
    /// possible (a rescued mailbox is delivered past the router) and must not
    /// underflow.
    pub(crate) fn leave(&mut self, path: &Path) {
        if let Some(n) = self.owed.get_mut(path) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.owed.remove(path);
            }
        }
    }

    /// The cell is gone (died, slept, was disconnected). Nothing more is coming
    /// from it, so its debt is cleared rather than waited out.
    pub(crate) fn forget(&mut self, path: &Path) {
        self.owed.remove(path);
    }

    /// Total outstanding deliveries across all cells.
    pub(crate) fn total(&self) -> u32 {
        self.owed.values().copied().sum()
    }

    /// The debtors, comma separated, sorted — the honest half of a cut drain.
    pub(crate) fn busy_paths(&self) -> String {
        let mut v: Vec<&str> = self.owed.keys().map(|p| p.as_str()).collect();
        v.sort_unstable();
        v.join(",")
    }
}

/// GH #1068 review F4: append `msg` to `held` unless that passes `caps`
/// (`(messages, bytes)`, the overflow's per-cell cap); then hand it back.
fn park_within(
    held: &mut Vec<meclaw_core::Message>,
    msg: meclaw_core::Message,
    (cap_messages, cap_bytes): (u64, u64),
) -> Result<(), Box<meclaw_core::Message>> {
    let bytes: u64 = held.iter().map(crate::overflow::body_bytes).sum();
    if (held.len() as u64).saturating_add(1) > cap_messages
        || bytes.saturating_add(crate::overflow::body_bytes(&msg)) > cap_bytes
    {
        return Err(Box::new(msg));
    }
    held.push(msg);
    Ok(())
}

/// GH #47: the four observations that together mean "nothing is in flight".
///
/// * `inbox_len` — events the colony has not looked at yet
/// * `outputs_len` — emissions a cell has already made and the loop has not
///   routed yet
/// * `ledger` — deliveries handed to a cell whose handler has not reported back
/// * `mailbox_backlog` — messages sitting in cell mailboxes; the two send paths
///   that bypass the router (`deliver_rescued_mailbox`, the post-disconnect
///   channel swap) take no ticket, so this is the observation that catches them
///
/// Deliberately NOT a settle window. A cell awaiting an HTTP response satisfies
/// three of the four for the whole call; a time-based rule would cut exactly the
/// work this drain exists to save.
pub(crate) fn is_quiescent(
    ledger: &DrainLedger,
    mailbox_backlog: usize,
    inbox_len: usize,
    outputs_len: usize,
) -> bool {
    ledger.total() == 0
        && ledger.overflow.is_empty()
        && ledger.parked_for_successor.is_empty()
        && mailbox_backlog == 0
        && inbox_len == 0
        && outputs_len == 0
}

#[cfg(test)]
mod quiescence_tests {
    use super::*;

    #[test]
    fn everything_empty_is_quiescent() {
        assert!(is_quiescent(&DrainLedger::default(), 0, 0, 0));
    }

    #[test]
    fn a_handler_still_running_is_not_quiescent() {
        let mut l = DrainLedger::default();
        l.enter(&Path::new("/slow"));
        assert!(
            !is_quiescent(&l, 0, 0, 0),
            "an empty mailbox is not a finished handler"
        );
    }

    #[test]
    fn a_queued_mailbox_message_is_not_quiescent() {
        assert!(!is_quiescent(&DrainLedger::default(), 1, 0, 0));
    }

    #[test]
    fn an_unrouted_emission_is_not_quiescent() {
        assert!(!is_quiescent(&DrainLedger::default(), 0, 0, 1));
    }

    #[test]
    fn an_unseen_colony_event_is_not_quiescent() {
        assert!(!is_quiescent(&DrainLedger::default(), 0, 1, 0));
    }

    /// GH #1068: a message held for a dying cell's successor is owed work.
    #[test]
    fn mail_held_for_a_successor_is_not_quiescent() {
        let mut l = DrainLedger::default();
        let path = Path::new("/dying");
        l.park_for_successor(
            &path,
            meclaw_core::MessageBuilder::new(path.clone()).build(),
        )
        .expect("far below the cap");
        assert!(!is_quiescent(&l, 0, 0, 0));
        assert_eq!(l.take_parked_for_successor(&path).len(), 1);
        assert!(is_quiescent(&l, 0, 0, 0), "taken over, nothing is held");
        assert!(l.take_parked_for_successor(&path).is_empty());
    }

    /// GH #1068 review F4: held mail has the overflow's cap -- by count and by
    /// bytes; what passes it comes back to be dead-lettered (`mailbox_full`),
    /// and what is already held stays, in order.
    #[test]
    fn held_mail_stops_at_the_overflow_cap() {
        let path = Path::new("/dying");
        let msg = |i: u64| {
            meclaw_core::MessageBuilder::new(path.clone())
                .body(meclaw_core::Body::Inline(
                    meclaw_core::serde_json::json!({ "i": i }),
                ))
                .build()
        };
        let one = crate::overflow::body_bytes(&msg(0));
        let mut held = Vec::new();
        assert!(park_within(&mut held, msg(0), (2, u64::MAX)).is_ok());
        assert!(park_within(&mut held, msg(1), (2, u64::MAX)).is_ok());
        let third = msg(2);
        let third_id = third.id;
        let back = park_within(&mut held, third, (2, u64::MAX)).expect_err("count cap");
        assert_eq!(back.id, third_id, "the refused message comes back");
        assert_eq!(held.len(), 2);
        let mut held = Vec::new();
        assert!(park_within(&mut held, msg(0), (u64::MAX, one)).is_ok());
        assert!(
            park_within(&mut held, msg(1), (u64::MAX, one)).is_err(),
            "byte cap"
        );
        assert_eq!(held.len(), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_ledger_owes_nothing() {
        let l = DrainLedger::default();
        assert_eq!(l.total(), 0);
        assert_eq!(l.busy_paths(), "");
    }

    #[test]
    fn a_ticket_taken_is_a_ticket_owed_until_it_is_given_back() {
        let mut l = DrainLedger::default();
        let p = Path::new("/a");
        l.enter(&p);
        l.enter(&p);
        assert_eq!(l.total(), 2);
        l.leave(&p);
        assert_eq!(l.total(), 1);
        l.leave(&p);
        assert_eq!(l.total(), 0);
    }

    /// `deliver_rescued_mailbox` sends straight into a mailbox without going
    /// through `route_with_log`, so its answers report a `WorkDone` for which no
    /// ticket was ever taken. That must not underflow, and it must not make the
    /// ledger negative-by-wraparound.
    #[test]
    fn giving_back_a_ticket_that_was_never_taken_is_a_no_op() {
        let mut l = DrainLedger::default();
        l.leave(&Path::new("/never-seen"));
        assert_eq!(l.total(), 0);
    }

    /// A dead cell answers nothing more. Its outstanding tickets must fall with
    /// it, or the drain would wait for a corpse until the deadline.
    #[test]
    fn forgetting_a_path_drops_all_of_its_tickets() {
        let mut l = DrainLedger::default();
        l.enter(&Path::new("/a"));
        l.enter(&Path::new("/a"));
        l.enter(&Path::new("/b"));
        l.forget(&Path::new("/a"));
        assert_eq!(l.total(), 1);
        assert_eq!(l.busy_paths(), "/b");
    }

    #[test]
    fn busy_paths_names_every_debtor_in_a_stable_order() {
        let mut l = DrainLedger::default();
        l.enter(&Path::new("/z"));
        l.enter(&Path::new("/a"));
        assert_eq!(l.busy_paths(), "/a,/z");
    }
}
