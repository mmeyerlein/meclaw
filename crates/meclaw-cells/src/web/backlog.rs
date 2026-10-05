//! GH #1006: how far behind each viewer's socket is, told to the app.
//!
//! Every viewer has a send queue (`run_connection`, 64 frames) that the fan-out
//! fills with `try_send` and the socket's write loop empties onto the wire. A
//! queue that fills marks the viewer for a whole-tree resync (GH #414) — and
//! until this module the app writing the page learned none of it: it saw how
//! fast the cell answered, never whether the bytes reached the screen. Measured
//! on a 2-D world app: the cell answered 8–10 bundles/s while the browser got
//! 0 frames for 2–5 s and then a burst of 25–29 frames/s (~500 KB/s). The app
//! guessed a fixed pacing of 250 ms between tranches (green; 200 ms was just
//! red) — one guess for a fast wall screen and a slow phone alike.
//!
//! So the cell **measures and reports**; it never throttles. The meter lives in
//! the I/O half, per connection, as atomics: the fan-out task counts a frame in,
//! the connection's write loop counts it out after `sink.send` returns, and
//! neither waits on the other. Cost per frame: one `Instant::now()` and a
//! handful of relaxed atomic operations, no allocation (measured against the
//! push it rides on by `tests/gh1006_a_slow_viewer_is_reported_measure.rs`).
//!
//! The report is opt-in (`viewer_events: ["backlog"]`) and edge-triggered: one
//! `high` when a viewer crosses a threshold, at most one repeat per second while
//! it stays there, one `clear` when it falls below half of both. An event is a
//! message in the log (R-94), so it must never be per frame.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

use crate::web::cell::WebEvent;
use crate::web::socket::ViewerMsg;

/// The default byte threshold for `high`: 256 KB outstanding on one socket.
///
/// About six of the measured app's slot diffs (~40 KB each) — a queue that holds
/// more than that is already seconds behind on a phone.
pub const DEFAULT_HIGH_BYTES: u64 = 256 * 1024;

/// The default age threshold for `high`: the oldest outstanding frame has
/// waited 250 ms.
///
/// WHY 250: the measured app's fixed pacing between tranches was green at
/// 250 ms and just red at 200 ms — a frame older than one tranche
/// interval means the next tranche lands on top of an unwritten one.
pub const DEFAULT_HIGH_MS: u64 = 250;

/// How often a viewer that stays `high` is reported again, with fresh numbers.
const REPEAT: Duration = Duration::from_secs(1);

/// "No frame outstanding" in [`Meter::head`].
const NONE: u64 = u64::MAX;

/// What this life of the cell reports, read from the params once at spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BacklogPolicy {
    /// Whether `viewer:backlog` is reported at all (`viewer_events` names
    /// `"backlog"`). Off by default, and then nothing is ever sent to the
    /// handler: the display behaves byte for byte as before.
    pub report: bool,
    /// `backlog_high_bytes`.
    pub high_bytes: u64,
    /// `backlog_high_ms`.
    pub high_ms: u64,
}

impl Default for BacklogPolicy {
    fn default() -> Self {
        Self {
            report: false,
            high_bytes: DEFAULT_HIGH_BYTES,
            high_ms: DEFAULT_HIGH_MS,
        }
    }
}

/// One frame in a viewer's queue, stamped when it was put there.
#[derive(Debug)]
pub struct Queued {
    /// The frame itself.
    pub msg: ViewerMsg,
    /// When it entered the queue — the age the meter reports.
    pub at: Instant,
    /// Its size on the wire, counted in and out with the same number.
    pub bytes: u64,
}

impl Queued {
    fn new(msg: ViewerMsg) -> Self {
        let bytes = match &msg {
            ViewerMsg::Frame(text) => text.len() as u64,
            ViewerMsg::Binary(b) => b.len() as u64,
            ViewerMsg::Close => 0,
        };
        Self {
            msg,
            at: Instant::now(),
            bytes,
        }
    }
}

/// A process-wide origin, so an `Instant` fits in an `AtomicU64`.
fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn nanos(at: Instant) -> u64 {
    // Never NONE: 584 years of process uptime would be needed to reach it.
    at.saturating_duration_since(epoch()).as_nanos() as u64
}

/// What one viewer's queue holds right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reading {
    /// Frames queued plus the one being written.
    pub frames: u64,
    /// Their bytes.
    pub bytes: u64,
    /// How long the oldest of them has waited, 0 when there is none.
    pub oldest_ms: u64,
    /// Resync marks over this connection's life (GH #414).
    pub resyncs_total: u64,
}

/// One connection's backlog meter.
///
/// Shared between exactly two writers — the fan-out (in) and the connection's
/// write loop (out) — and the readers of the `viewers` op. Atomics only; the
/// one lock guards the route and session id, written at join and read only
/// when a report is sent.
pub struct Meter {
    frames: AtomicU64,
    bytes: AtomicU64,
    /// Enqueue time of the oldest outstanding frame, as [`nanos`]; [`NONE`]
    /// when the queue is empty.
    head: AtomicU64,
    /// A resync is owed (the queue was full and the tree has not got in yet).
    owed: AtomicBool,
    /// Resync marks since the last report.
    resyncs_since: AtomicU64,
    resyncs_total: AtomicU64,
    /// The level last reported.
    high: AtomicBool,
    /// When `high` was last reported, as [`nanos`].
    last_report: AtomicU64,
    /// `(route, session_id)` once the page joined.
    ident: std::sync::Mutex<Option<(String, String)>>,
    /// Where reports go; `None` when this life does not report.
    reporter: Option<(mpsc::Sender<WebEvent>, BacklogPolicy)>,
}

impl Meter {
    /// A meter for one connection. `events` is used only when `policy.report`.
    pub fn new(policy: BacklogPolicy, events: Option<mpsc::Sender<WebEvent>>) -> Self {
        Self {
            frames: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            head: AtomicU64::new(NONE),
            owed: AtomicBool::new(false),
            resyncs_since: AtomicU64::new(0),
            resyncs_total: AtomicU64::new(0),
            high: AtomicBool::new(false),
            last_report: AtomicU64::new(0),
            ident: std::sync::Mutex::new(None),
            reporter: events.filter(|_| policy.report).map(|tx| (tx, policy)),
        }
    }

    /// A meter that counts and never reports — tests and the default.
    pub fn silent() -> Self {
        Self::new(BacklogPolicy::default(), None)
    }

    /// The page joined: reports name this route and session from now on.
    pub fn joined(&self, route: &str, session_id: &str) {
        if let Ok(mut ident) = self.ident.lock() {
            *ident = Some((route.to_string(), session_id.to_string()));
        }
    }

    /// The session id this connection joined under, if it did.
    pub fn session_id(&self) -> Option<String> {
        self.ident
            .lock()
            .ok()
            .and_then(|i| i.as_ref().map(|(_, s)| s.clone()))
    }

    /// Count a frame in. Called BEFORE it enters the queue, so the write loop
    /// can never count out a frame that was not counted in yet.
    fn counted_in(&self, at: Instant, bytes: u64) {
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        if self.frames.fetch_add(1, Ordering::AcqRel) == 0 {
            // The queue was empty, so this frame is the oldest. A plain store:
            // a stale head from a racing write-out must not survive (see
            // `written`).
            self.head.store(nanos(at), Ordering::Release);
        } else {
            self.head.fetch_min(nanos(at), Ordering::AcqRel);
        }
    }

    /// Undo [`Self::counted_in`] for a frame the queue refused.
    fn not_queued(&self, at: Instant, bytes: u64) {
        self.bytes.fetch_sub(bytes, Ordering::Relaxed);
        if self.frames.fetch_sub(1, Ordering::AcqRel) == 1 {
            let _ =
                self.head
                    .compare_exchange(nanos(at), NONE, Ordering::AcqRel, Ordering::Relaxed);
        }
    }

    /// The write loop took the frame stamped `at` off the queue: it is now the oldest outstanding
    /// frame (the queue is FIFO).
    pub fn writing(&self, at: Instant) {
        self.head.store(nanos(at), Ordering::Release);
    }

    /// The frame stamped `at` is on the wire. After the last outstanding frame the head is
    /// cleared — by compare-and-swap on this frame's own stamp, so a frame
    /// queued in between (which stored its own stamp) is not forgotten. While
    /// frames remain, the head stays on this frame's stamp until the loop takes
    /// the next one: an upper bound for the instant in between.
    pub fn written(&self, at: Instant, bytes: u64) {
        self.bytes.fetch_sub(bytes, Ordering::Relaxed);
        if self.frames.fetch_sub(1, Ordering::AcqRel) == 1 {
            let _ =
                self.head
                    .compare_exchange(nanos(at), NONE, Ordering::AcqRel, Ordering::Relaxed);
        }
        self.check();
    }

    /// The queue was full and the viewer now owes a whole-tree resync. Counted
    /// once per debt, not once per refused frame.
    pub fn resync_owed(&self) {
        if !self.owed.swap(true, Ordering::AcqRel) {
            self.resyncs_since.fetch_add(1, Ordering::Relaxed);
            self.resyncs_total.fetch_add(1, Ordering::Relaxed);
        }
        self.check();
    }

    /// The tree (or the next frame) got in: the debt is paid.
    pub fn resync_settled(&self) {
        self.owed.store(false, Ordering::Release);
    }

    /// What the queue holds right now.
    pub fn reading(&self) -> Reading {
        self.reading_at(Instant::now())
    }

    fn reading_at(&self, now: Instant) -> Reading {
        let frames = self.frames.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        let oldest_ms = if frames == 0 || head == NONE {
            0
        } else {
            nanos(now).saturating_sub(head) / 1_000_000
        };
        Reading {
            frames,
            bytes: if frames == 0 {
                0
            } else {
                self.bytes.load(Ordering::Relaxed)
            },
            oldest_ms,
            resyncs_total: self.resyncs_total.load(Ordering::Relaxed),
        }
    }

    /// Decide whether a report is due, and send it.
    ///
    /// Runs where the numbers change — after a frame is queued, after one is
    /// written, on a resync mark — so no timer is needed: a viewer that is
    /// `high` either keeps receiving frames (a check per frame) or keeps
    /// draining (a check per write). Free when this life does not report.
    pub fn check(&self) {
        self.check_at(Instant::now());
    }

    /// [`Meter::check`] at `now`: the clock injected, so a lock can run a
    /// minute of edges without waiting one.
    fn check_at(&self, now: Instant) {
        let Some((_, policy)) = &self.reporter else {
            return;
        };
        let r = self.reading_at(now);
        let owed = self.owed.load(Ordering::Acquire);
        let marks = self.resyncs_since.load(Ordering::Relaxed);
        let over =
            owed || marks > 0 || r.bytes >= policy.high_bytes || r.oldest_ms >= policy.high_ms;
        let under = !owed
            && marks == 0
            && r.bytes < policy.high_bytes / 2
            && r.oldest_ms < policy.high_ms / 2;
        let now_n = nanos(now);
        if over {
            if !self.high.load(Ordering::Acquire) {
                if self
                    .high
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    let last = self.last_report.swap(now_n, Ordering::AcqRel);
                    if !self.report(r, true) {
                        // Not delivered: the next check tries again.
                        self.last_report.store(last, Ordering::Release);
                        self.high.store(false, Ordering::Release);
                    }
                }
            } else {
                let last = self.last_report.load(Ordering::Acquire);
                if now_n.saturating_sub(last) >= REPEAT.as_nanos() as u64
                    && self
                        .last_report
                        .compare_exchange(last, now_n, Ordering::AcqRel, Ordering::Relaxed)
                        .is_ok()
                    && !self.report(r, true)
                {
                    self.last_report.store(last, Ordering::Release);
                }
            }
        } else if under
            && self
                .high
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            && !self.report(r, false)
        {
            self.high.store(true, Ordering::Release);
        }
    }

    /// Hand one report to the handler. `try_send`, never `send().await`: the
    /// events channel is the browsers' too (capacity 64), and waiting on it in
    /// the I/O half would build exactly the jam this meter reports. A report
    /// that does not fit is retried by the next check.
    fn report(&self, r: Reading, high: bool) -> bool {
        let Some((tx, _)) = &self.reporter else {
            return true;
        };
        let Some((route, session_id)) = self.ident.lock().ok().and_then(|i| i.clone()) else {
            // Not joined yet: nobody to name. Retried once it is.
            return false;
        };
        let resyncs = self.resyncs_since.swap(0, Ordering::AcqRel);
        let sent = tx
            .try_send(WebEvent::Backlog {
                session_id,
                route,
                frames: r.frames,
                bytes: r.bytes,
                oldest_ms: r.oldest_ms,
                resyncs,
                high,
            })
            .is_ok();
        if !sent {
            self.resyncs_since.fetch_add(resyncs, Ordering::Relaxed);
        }
        sent
    }
}

/// A viewer's queue, with its meter: the only way a frame gets in.
#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::Sender<Queued>,
    meter: Arc<Meter>,
}

impl Outbox {
    /// Wrap a queue and its meter.
    pub fn new(tx: mpsc::Sender<Queued>, meter: Arc<Meter>) -> Self {
        Self { tx, meter }
    }

    /// The meter, for the write loop and the `viewers` op.
    pub fn meter(&self) -> &Arc<Meter> {
        &self.meter
    }

    /// Queue a message without waiting; the shape of the fan-out (GH #414).
    pub fn try_send(&self, msg: ViewerMsg) -> Result<(), TrySendError<()>> {
        let q = Queued::new(msg);
        let (at, bytes) = (q.at, q.bytes);
        self.meter.counted_in(at, bytes);
        match self.tx.try_send(q) {
            Ok(()) => {
                self.meter.check();
                Ok(())
            }
            Err(e) => {
                self.meter.not_queued(at, bytes);
                Err(match e {
                    TrySendError::Full(_) => TrySendError::Full(()),
                    TrySendError::Closed(_) => TrySendError::Closed(()),
                })
            }
        }
    }

    /// Queue a message, waiting for room; the shape of a topic link's
    /// forwarder, whose backpressure IS the wait. A frame waiting for room is
    /// already counted: it is backlog as much as one inside the queue.
    pub async fn send(&self, msg: ViewerMsg) -> Result<(), mpsc::error::SendError<()>> {
        let q = Queued::new(msg);
        let (at, bytes) = (q.at, q.bytes);
        self.meter.counted_in(at, bytes);
        match self.tx.send(q).await {
            Ok(()) => {
                self.meter.check();
                Ok(())
            }
            Err(_) => {
                self.meter.not_queued(at, bytes);
                Err(mpsc::error::SendError(()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reporting(cap: usize) -> (Outbox, mpsc::Receiver<Queued>, mpsc::Receiver<WebEvent>) {
        let (tx, rx) = mpsc::channel(cap);
        let (etx, erx) = mpsc::channel(64);
        let policy = BacklogPolicy {
            report: true,
            high_bytes: 1000,
            high_ms: 250,
        };
        let meter = Arc::new(Meter::new(policy, Some(etx)));
        meter.joined("/", "s1");
        (Outbox::new(tx, meter), rx, erx)
    }

    fn level(e: WebEvent) -> (bool, u64, u64) {
        match e {
            WebEvent::Backlog {
                high,
                bytes,
                resyncs,
                ..
            } => (high, bytes, resyncs),
            _ => panic!("a backlog report"),
        }
    }

    #[test]
    fn counted_in_and_out_the_queue_reads_empty() {
        let (out, mut rx, _e) = reporting(8);
        out.try_send(ViewerMsg::Frame("x".repeat(100))).unwrap();
        out.try_send(ViewerMsg::Frame("y".repeat(50))).unwrap();
        let r = out.meter().reading();
        assert_eq!((r.frames, r.bytes), (2, 150));
        while let Ok(q) = rx.try_recv() {
            out.meter().writing(q.at);
            out.meter().written(q.at, q.bytes);
        }
        assert_eq!(out.meter().reading(), Reading::default());
    }

    #[test]
    fn a_refused_frame_is_not_counted() {
        let (out, _rx, _e) = reporting(1);
        out.try_send(ViewerMsg::Frame("a".into())).unwrap();
        assert!(out.try_send(ViewerMsg::Frame("bb".into())).is_err());
        let r = out.meter().reading();
        assert_eq!((r.frames, r.bytes), (1, 1));
    }

    #[test]
    fn high_once_then_clear_once() {
        let (out, mut rx, mut e) = reporting(64);
        for _ in 0..5 {
            out.try_send(ViewerMsg::Frame("z".repeat(300))).unwrap();
        }
        // 1500 bytes ≥ 1000: exactly one `high`, not one per frame.
        let (high, bytes, _) = level(e.try_recv().expect("high"));
        assert!(high);
        assert!(bytes >= 1000);
        assert!(e.try_recv().is_err(), "edge-triggered: one report");
        while let Ok(q) = rx.try_recv() {
            out.meter().writing(q.at);
            out.meter().written(q.at, q.bytes);
        }
        let (high, _, _) = level(e.try_recv().expect("clear"));
        assert!(!high);
        assert!(e.try_recv().is_err());
    }

    #[test]
    fn silent_without_opt_in() {
        let (tx, _rx) = mpsc::channel(64);
        let (etx, mut erx) = mpsc::channel(8);
        let meter = Arc::new(Meter::new(BacklogPolicy::default(), Some(etx)));
        meter.joined("/", "s");
        let out = Outbox::new(tx, meter);
        for _ in 0..40 {
            out.try_send(ViewerMsg::Frame("q".repeat(10_000))).unwrap();
        }
        assert!(erx.try_recv().is_err());
        assert_eq!(out.meter().reading().bytes, 400_000);
    }

    /// T3 (GH #1006): reports are edge-triggered, never per frame. A viewer
    /// that stays `high` for sixty seconds, checked every 10 ms — as often as
    /// a frame is queued on a busy page — is reported once on the crossing and
    /// then at most once a second: 60 reports against 6 000 checks. The clock
    /// is injected, so the count is the meter's and not the host's load; the
    /// wall-clock minute (plan bound ≤ 61) is the measurement test's.
    #[test]
    fn gh1006_backlog_events_are_edge_triggered() {
        let (out, _rx, mut erx) = reporting(8);
        out.try_send(ViewerMsg::Frame("x".repeat(100))).unwrap();
        let start = Instant::now();
        let (mut highs, mut clears) = (0, 0);
        for tick in 0..6_000u64 {
            out.meter()
                .check_at(start + Duration::from_millis(tick * 10));
            while let Ok(e) = erx.try_recv() {
                if level(e).0 {
                    highs += 1;
                } else {
                    clears += 1;
                }
            }
        }
        assert_eq!(clears, 0, "the frame never left, so nothing clears");
        // Crossing at 250 ms, then one repeat per second: 0.25 s, 1.25 s, …,
        // 59.25 s.
        assert_eq!(highs, 60, "one high and one repeat a second over 60 s");
    }
}
