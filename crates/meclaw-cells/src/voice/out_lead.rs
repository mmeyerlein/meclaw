//! The lead of the model's audio at the edge that plays it (wave w2,
//! `params.audio_out_lead_ms`).
//!
//! The provider paces its output in real time and the cell passes a chunk on
//! as it comes (`connection.rs`, *Outbound framing*). The edge behind the socket
//! therefore holds exactly what the last chunk brought and no reserve, and a
//! chunk that is late by any amount is heard as silence of that length. The
//! remedy is a small stock at the edge: a chunk that arrives when nothing is
//! left to play -- the start of a spoken section, or after an underrun -- is
//! held back for the lead and then sent as ever. Every chunk after it reaches
//! the edge that much earlier than it is needed, and a delay up to the lead
//! leaves no hole. The price is paid once per section: it begins a lead later.
//!
//! This is the calculation and nothing else: arrival times and playing times
//! in, send times and order out. It reads no clock, owns no timer and knows no
//! socket; the connection that owns those asks it. It does not know the edge's
//! own playing clock either, so *played until* is an estimate, and it never
//! makes audio up: what goes out is what came in, in the order it came.
//!
//! The same numbers give the gap at the way out, which is the other thing this
//! module answers: how long, by this account, the edge had nothing to play.

use std::collections::VecDeque;
use std::time::Duration;
use tokio::time::Instant;

/// How many gaps one line keeps with their time. Beyond that they are only
/// counted ([`OutLead::gaps_beyond_cap`]): a line that stutters for an hour
/// must not grow a report of its own size, and the first few hundred already
/// say where and how long.
pub const MAX_GAPS: usize = 256;

/// One point in time as the caller read it: on the clock the calculation runs
/// on, and as an epoch for whoever reads the gaps afterwards. Both are handed
/// in, because this module reads neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    /// The monotonic time; the only one that is calculated with.
    pub at: Instant,
    /// The same moment in milliseconds since the epoch; only carried along.
    pub epoch_ms: u64,
}

/// A hole at the way out: the line had nothing to play when a chunk was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    /// When the chunk that ended the hole was sent.
    pub until: Moment,
    /// How long the line had been empty by then: the send time less the time
    /// it was played until.
    pub length: Duration,
}

/// The account of one line: what it has been sent, and what waits for the lead.
#[derive(Debug)]
pub struct OutLead<T> {
    /// How long a chunk waits that meets an empty line; zero is off.
    lead: Duration,
    /// Until when the line has something to play, by this account. `None` until
    /// the first chunk is sent and after [`Self::clear`].
    played_until: Option<Instant>,
    /// The chunks held back, in the order they came.
    waiting: VecDeque<(T, Duration)>,
    /// When the chunks held back may leave; set exactly while some wait.
    due: Option<Instant>,
    /// The holes so far, at most [`MAX_GAPS`].
    gaps: Vec<Gap>,
    /// The holes beyond them.
    gaps_beyond_cap: u64,
}

impl<T> OutLead<T> {
    /// An empty line with this lead; zero passes every chunk on as it comes.
    pub fn new(lead: Duration) -> Self {
        Self {
            lead,
            played_until: None,
            waiting: VecDeque::new(),
            due: None,
            gaps: Vec::new(),
            gaps_beyond_cap: 0,
        }
    }

    /// A chunk that plays for `plays` has arrived: the chunk itself when it is
    /// to be sent now, `None` when it waits ([`Self::due`]).
    ///
    /// Behind waiting chunks it always waits with them, also past the due time:
    /// the caller's timer may be late, and the order may not change.
    pub fn arrive(&mut self, now: Moment, chunk: T, plays: Duration) -> Option<T> {
        let empty = self.played_until.is_none_or(|until| now.at > until);
        if !self.waiting.is_empty() || (empty && !self.lead.is_zero()) {
            self.due.get_or_insert(now.at + self.lead);
            self.waiting.push_back((chunk, plays));
            return None;
        }
        self.sent(now, plays);
        Some(chunk)
    }

    /// When the waiting chunks may leave; `None` while none waits.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// The waiting chunks, in order, to be sent now; none before they are due.
    pub fn release(&mut self, now: Moment) -> Vec<T> {
        if self.due.is_none_or(|due| now.at < due) {
            return Vec::new();
        }
        self.release_all(now)
    }

    /// The waiting chunks, in order, to be sent now whether they are due or not:
    /// something is about to go out that must not go before them.
    pub fn release_all(&mut self, now: Moment) -> Vec<T> {
        self.due = None;
        let waiting = std::mem::take(&mut self.waiting);
        waiting
            .into_iter()
            .map(|(chunk, plays)| {
                self.sent(now, plays);
                chunk
            })
            .collect()
    }

    /// Audio that plays for `plays` goes out now without having been held: the
    /// account takes it in, and nothing waits for it.
    pub fn pass(&mut self, now: Moment, plays: Duration) {
        self.sent(now, plays);
    }

    /// A barge-in or a cancel: what waits is dropped, and the line counts as
    /// empty, because a cancelled client drops what it still plays. The next
    /// chunk therefore waits the lead again, and the quiet before it is no gap
    /// -- it was asked for. The gaps counted so far stay.
    pub fn clear(&mut self) {
        self.waiting.clear();
        self.due = None;
        self.played_until = None;
    }

    /// The gaps so far, oldest first, at most [`MAX_GAPS`].
    pub fn gaps(&self) -> &[Gap] {
        &self.gaps
    }

    /// How many gaps came after the list was full.
    pub fn gaps_beyond_cap(&self) -> u64 {
        self.gaps_beyond_cap
    }

    /// A chunk goes out now: the gap it ends, and what the line has to play
    /// from here on.
    fn sent(&mut self, now: Moment, plays: Duration) {
        let until = self.played_until.unwrap_or(now.at);
        let length = now.at.saturating_duration_since(until);
        if !length.is_zero() {
            if self.gaps.len() < MAX_GAPS {
                self.gaps.push(Gap { until: now, length });
            } else {
                self.gaps_beyond_cap += 1;
            }
        }
        self.played_until = Some(until.max(now.at) + plays);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAD: Duration = Duration::from_millis(20);
    /// The provider's chunk length on the telephone.
    const CHUNK: Duration = Duration::from_millis(100);
    /// An arbitrary epoch for the start of a test's line.
    const EPOCH: u64 = 1_000_000;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// The moment `at`, `start` being the zero of the line.
    fn moment(start: Instant, at: Instant) -> Moment {
        Moment {
            at,
            epoch_ms: EPOCH + at.duration_since(start).as_millis() as u64,
        }
    }

    /// Run chunks of [`CHUNK`] arriving at these milliseconds through the line,
    /// with the one timer a connection would own firing exactly when it is due.
    /// Gives each chunk's number and the millisecond it was sent at.
    fn run(line: &mut OutLead<usize>, start: Instant, arrivals: &[u64]) -> Vec<(usize, u64)> {
        let mut sent = Vec::new();
        for (n, arrival) in arrivals.iter().enumerate() {
            let now = moment(start, start + ms(*arrival));
            if let Some(due) = line.due().filter(|due| *due <= now.at) {
                release_due(line, start, due, &mut sent);
            }
            if let Some(chunk) = line.arrive(now, n, CHUNK) {
                sent.push((chunk, now.epoch_ms - EPOCH));
            }
        }
        if let Some(due) = line.due() {
            release_due(line, start, due, &mut sent);
        }
        sent
    }

    fn release_due(
        line: &mut OutLead<usize>,
        start: Instant,
        due: Instant,
        sent: &mut Vec<(usize, u64)>,
    ) {
        let now = moment(start, due);
        for chunk in line.release(now) {
            sent.push((chunk, now.epoch_ms - EPOCH));
        }
    }

    /// The gaps as (millisecond they ended, length in milliseconds).
    fn gaps(line: &OutLead<usize>) -> Vec<(u64, u64)> {
        line.gaps()
            .iter()
            .map(|g| (g.until.epoch_ms - EPOCH, g.length.as_millis() as u64))
            .collect()
    }

    /// The first chunk of a line and the first one after an underrun wait the
    /// lead; what arrives meanwhile queues behind and leaves with them.
    #[tokio::test(start_paused = true)]
    async fn a_chunk_after_an_empty_line_waits_the_lead() {
        let start = Instant::now();
        let mut line = OutLead::new(LEAD);
        // 0 waits until 20, 1 queues behind it; both are played until 220.
        // 2 finds the line empty since 220 and waits from 500 to 520.
        let sent = run(&mut line, start, &[0, 10, 500]);
        assert_eq!(sent, vec![(0, 20), (1, 20), (2, 520)]);
        assert_eq!(gaps(&line), vec![(520, 300)], "520 less played until 220");
        assert_eq!(line.due(), None, "nothing waits any more");
    }

    /// A line that still has something to play takes a chunk as it comes, up to
    /// the very moment it runs out.
    #[tokio::test(start_paused = true)]
    async fn a_chunk_on_a_filled_line_leaves_at_once() {
        let start = Instant::now();
        let mut line = OutLead::new(LEAD);
        let sent = run(&mut line, start, &[0, 100, 200, 320]);
        assert_eq!(sent, vec![(0, 20), (1, 100), (2, 200), (3, 320)]);
        assert_eq!(gaps(&line), vec![]);
    }

    /// The provider's rhythm with one chunk late: by the lead it leaves no hole,
    /// by more it does, and the hole is the whole quiet the line had.
    #[tokio::test(start_paused = true)]
    async fn a_late_chunk_within_the_lead_leaves_no_gap() {
        let start = Instant::now();
        let mut line = OutLead::new(LEAD);
        let sent = run(&mut line, start, &[0, 120, 200]);
        assert_eq!(sent, vec![(0, 20), (1, 120), (2, 200)]);
        assert_eq!(gaps(&line), vec![], "late by 20 of 20");

        // Played until 120, so 121 is empty: 1 waits to 141.
        let start = Instant::now();
        let mut line = OutLead::new(LEAD);
        let sent = run(&mut line, start, &[0, 121]);
        assert_eq!(sent, vec![(0, 20), (1, 141)]);
        assert_eq!(gaps(&line), vec![(141, 21)], "late by 21 of 20");
    }

    /// The default: every chunk leaves when it arrives, none ever waits, and
    /// the account of the gaps runs all the same.
    #[tokio::test(start_paused = true)]
    async fn lead_zero_is_todays_burst() {
        let start = Instant::now();
        let mut line = OutLead::new(Duration::ZERO);
        let sent = run(&mut line, start, &[0, 5, 10, 315, 700]);
        assert_eq!(sent, vec![(0, 0), (1, 5), (2, 10), (3, 315), (4, 700)]);
        // Played until 300, then until 415.
        assert_eq!(gaps(&line), vec![(315, 15), (700, 285)]);
        assert_eq!(line.due(), None);
    }

    /// A cancel drops what waits and never sends it; the next chunk finds an
    /// empty line, and the quiet the cancel made is no gap.
    #[tokio::test(start_paused = true)]
    async fn a_cancel_drops_the_waiting_chunks() {
        let start = Instant::now();
        let mut line = OutLead::new(LEAD);
        assert_eq!(line.arrive(moment(start, start), 0, CHUNK), None);
        assert_eq!(line.arrive(moment(start, start), 1, CHUNK), None);
        let due = line.due().expect("two chunks wait");
        line.clear();
        assert_eq!(line.due(), None);
        assert_eq!(line.release(moment(start, due)), Vec::<usize>::new());

        assert_eq!(line.arrive(moment(start, start + ms(50)), 2, CHUNK), None);
        assert_eq!(line.due(), Some(start + ms(70)), "a new lead of its own");
        assert_eq!(line.release(moment(start, start + ms(70))), vec![2]);
        // Cancelled on a line that is playing: the same, and no gap.
        line.clear();
        assert_eq!(line.arrive(moment(start, start + ms(400)), 3, CHUNK), None);
        assert_eq!(line.release(moment(start, start + ms(420))), vec![3]);
        assert_eq!(gaps(&line), vec![]);
    }

    /// A timer that fires late changes no order: what arrives past the due time
    /// still queues behind what waits, and nothing leaves before it is due.
    #[tokio::test(start_paused = true)]
    async fn a_late_timer_keeps_the_order() {
        let start = Instant::now();
        let mut line = OutLead::new(LEAD);
        assert_eq!(line.arrive(moment(start, start), 0, CHUNK), None);
        assert_eq!(
            line.release(moment(start, start + ms(19))),
            Vec::<usize>::new()
        );
        assert_eq!(line.arrive(moment(start, start + ms(35)), 1, CHUNK), None);
        assert_eq!(line.due(), Some(start + ms(20)));
        assert_eq!(line.release(moment(start, start + ms(35))), vec![0, 1]);
    }

    /// What must not go before the waiting chunks releases them early; they are
    /// counted from when they left, and what passes the line counts with them.
    #[tokio::test(start_paused = true)]
    async fn an_early_release_and_a_passing_chunk_count_as_sent() {
        let start = Instant::now();
        let mut line = OutLead::new(LEAD);
        assert_eq!(line.arrive(moment(start, start), 0, CHUNK), None);
        assert_eq!(line.arrive(moment(start, start), 1, CHUNK), None);
        assert_eq!(line.release_all(moment(start, start + ms(5))), vec![0, 1]);
        assert_eq!(line.due(), None);
        // Played until 205; what passes makes it 305.
        line.pass(moment(start, start + ms(5)), CHUNK);
        assert_eq!(
            line.arrive(moment(start, start + ms(305)), 2, CHUNK),
            Some(2)
        );
        assert_eq!(gaps(&line), vec![]);
    }

    /// The list of gaps is capped; what comes after it is counted.
    #[tokio::test(start_paused = true)]
    async fn the_gaps_are_capped_and_counted() {
        let start = Instant::now();
        let mut line = OutLead::new(Duration::ZERO);
        // One chunk each 150 ms: 50 ms of quiet before every one but the first.
        let arrivals: Vec<u64> = (0..MAX_GAPS as u64 + 4).map(|n| n * 150).collect();
        run(&mut line, start, &arrivals);
        assert_eq!(line.gaps().len(), MAX_GAPS);
        assert_eq!(gaps(&line)[0], (150, 50), "the first are the ones kept");
        assert_eq!(line.gaps_beyond_cap(), 3);
    }
}
