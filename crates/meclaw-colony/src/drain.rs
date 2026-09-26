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
