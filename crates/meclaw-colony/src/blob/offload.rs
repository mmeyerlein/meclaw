//! GH #1418: the blob offload of an oversized body, beside the colony loop.
//!
//! Before, `offload_oversized` awaited `DiskBlobStore::write_streaming`
//! (eight `tokio::fs` hops, two `fsync`s) inside the colony loop for every
//! body of at least `blob_inline_max_bytes`. Under host load that write was the
//! step the loop went quiet in: 27 of 27 lab events, 500-1260 ms per write,
//! load1 median 14 on the lab host -- and while it waited, no cell and no
//! sender of the whole colony got anything routed.
//!
//! Now the loop hands the body to ONE infrastructure task per colony (spec
//! overview § Concurrency: process-wide infrastructure actors), which writes
//! it and hands the message back through the colony inbox; the loop then logs
//! and delivers it on the same path as any routed message. Durability is
//! unchanged (`fsync` → rename, sidecar last as the commit marker); so is the
//! order of one sender's messages ([`OffloadState`]). A full queue or a dead
//! task falls back to the inline write. No lock, no atomic: the state lives in
//! the loop, the task owns its channel ends.

use crate::colony::ColonyMsg;
use meclaw_core::{Body, Message, Path};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::oneshot;

/// Capacity of the offload queue. A queued job holds its body once (the
/// serialized bytes; the message carries a placeholder), so queue plus `held`
/// keep at most 512 jobs times one serialized copy (plus the one the task
/// writes): 32 MiB at the 64 KiB default of `blob_inline_max_bytes`, 1 GiB only
/// if every body were 2 MiB. Past a full queue a sender that waits for nothing
/// writes inline as before.
pub(crate) const OFFLOAD_QUEUE_CAPACITY: usize = 256;

/// Cap of the held jobs. At the cap the loop stops taking cell emissions (the
/// guard of its output arm) until returns free the queue: the flood source
/// meets backpressure instead of growing `held`.
pub(crate) const OFFLOAD_HELD_CAPACITY: usize = OFFLOAD_QUEUE_CAPACITY;

/// One message on its way through the offload task.
///
/// While `bytes` is set the message carries `Body::Inline(Null)` as a
/// placeholder, so the job holds its body once; a failed or stopped write
/// restores the inline body from `bytes` ([`OffloadJob::restore_body`]).
/// `bytes == None` is a message that only waits its turn behind an earlier
/// offload of the same sender.
pub struct OffloadJob {
    /// The routing sender the message was routed for (`route()`'s `sender_path`).
    pub(crate) sender: Path,
    /// The message; its body is a placeholder while `bytes` is set.
    pub(crate) msg: Message,
    /// The serialized body to write; `None` = pass through without writing.
    /// A job back from the task always has `None` (written, or restored).
    pub(crate) bytes: Option<Vec<u8>>,
    /// Serialized size of the body, kept for the log line after `bytes` is gone.
    pub(crate) len: usize,
    /// The `logged` flag of the routing call that deferred it.
    pub(crate) logged: bool,
    /// GH #1015 replay check of the work loop that deferred it:
    /// `Some(replayed_source)` for a cell's emission (its follow-ups ask
    /// `replay_logged` and the log, cascades get derived ids), `None` for
    /// anything else. The job's return walks the rest of its work with it.
    pub(crate) origin: Option<bool>,
    /// When the loop queued it (monotonic), for `queue_wait_ms`.
    pub(crate) queued_at: tokio::time::Instant,
    /// The task died before it took this job: the loop routes it itself, on
    /// the inline path (the inline write of `route_with_log_inner`).
    pub(crate) orphaned: bool,
    /// The colony stopped the task before this job was written: the shutdown
    /// teardown dead-letters it instead of delivering it.
    pub(crate) stopped: bool,
}

impl OffloadJob {
    /// The inline body back from `bytes` (a failed or stopped write, a held
    /// job the loop routes itself). Never panics: bytes that do not parse
    /// (they came from `to_vec` of the same value) leave the placeholder and
    /// log.
    pub(crate) fn restore_body(&mut self) {
        if let Some(bytes) = self.bytes.take() {
            match meclaw_core::serde_json::from_slice(&bytes) {
                Ok(value) => self.msg.body = Body::Inline(value),
                Err(e) => {
                    tracing::error!(error = %e, "offload: body restore failed — placeholder kept");
                }
            }
        }
    }
}

/// The offload task: one per colony with a blob store.
pub(crate) struct OffloadTask;

impl OffloadTask {
    /// Spawn the task. It takes jobs FIFO, writes the ones that carry bytes,
    /// and hands every job back as [`ColonyMsg::Offloaded`] with `send().await`
    /// -- never `try_send`, so no job is lost to a full inbox. Only this task
    /// sends back, never the loop itself (a self-send from the loop could wedge
    /// it on its own full inbox). The task ends when the loop drops its sender
    /// or the inbox closes. `stop` (the shutdown) cuts a pending write; from
    /// then on every job goes back unwritten with `stopped` set.
    pub(crate) fn spawn(
        mut rx: mpsc::Receiver<OffloadJob>,
        mut stop: oneshot::Receiver<()>,
        store: Arc<crate::DiskBlobStore>,
        inbox_tx: mpsc::Sender<ColonyMsg>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut stopped = false;
            while let Some(mut job) = rx.recv().await {
                if !stopped {
                    // A closed stop channel (the state is gone) stops as well.
                    stopped = !matches!(stop.try_recv(), Err(oneshot::error::TryRecvError::Empty));
                }
                if !stopped && let Some(bytes) = job.bytes.as_deref() {
                    let write = async {
                        #[cfg(test)]
                        seam::stall().await;
                        store.write_streaming(bytes, "application/json", None).await
                    };
                    let written = tokio::select! {
                        biased;
                        _ = &mut stop => None,
                        r = write => Some(r),
                    };
                    match written {
                        Some(Ok(blob_ref)) => {
                            job.msg.body = Body::Blob(blob_ref.blob_id);
                            job.bytes = None;
                        }
                        // Best effort as before #1418: the body rides on inline.
                        Some(Err(e)) => {
                            tracing::error!(error = %e, "offload: blob write failed — keeping inline");
                            job.restore_body();
                        }
                        None => stopped = true,
                    }
                }
                if stopped && job.bytes.is_some() {
                    job.restore_body();
                    job.stopped = true;
                }
                if inbox_tx.send(ColonyMsg::Offloaded(job)).await.is_err() {
                    break;
                }
            }
        })
    }
}

/// What the loop does with a message it asked [`OffloadState::defer`] about.
pub(crate) enum Deferral {
    /// The job is queued (or held, or orphaned); the message comes back as
    /// [`ColonyMsg::Offloaded`].
    Deferred,
    /// Nothing to write and nothing to wait for: route it on now.
    Route(Message),
    /// No task to take it (full queue, no store, task gone) and the sender
    /// waits for nothing: the inline write, as before the offload task existed.
    Inline(Message),
}

/// The loop's half of the offload: the sender, the jobs each routing sender
/// still has out, and the jobs held back while the queue was full.
pub(crate) struct OffloadState {
    tx: Option<mpsc::Sender<OffloadJob>>,
    /// Fired by [`OffloadState::shut`]: the task cuts its pending write.
    stop: Option<oneshot::Sender<()>>,
    /// The task, awaited by the shutdown teardown.
    task: Option<tokio::task::JoinHandle<()>>,
    /// Routing sender → offload jobs still out (queued, writing, held, or
    /// back in the inbox and not yet routed). While a sender has one, every
    /// further message of it that is bound for a cell or a hive goes through
    /// the queue as well (`bytes: None`), so none overtakes an earlier one --
    /// the queue and the inbox are FIFO. An entry is removed at 0.
    pending: HashMap<Path, u32>,
    /// Jobs of a waiting sender that met a full queue. They keep their order
    /// and move into the queue as it frees; an inline write would overtake.
    held: VecDeque<OffloadJob>,
    /// Jobs the dead task never took, in order; the loop takes them before
    /// its inbox ([`OffloadState::take_orphan`]) and routes them itself.
    orphans: VecDeque<OffloadJob>,
}

impl OffloadState {
    /// The state of a colony with `store`: spawns the task. Without a store
    /// there is nothing to write and every decision is the inline path.
    pub(crate) fn new(
        store: &Option<Arc<crate::DiskBlobStore>>,
        inbox_tx: &mpsc::Sender<ColonyMsg>,
    ) -> Self {
        let mut stop = None;
        let mut task = None;
        let tx = store.as_ref().map(|store| {
            let (tx, rx) = mpsc::channel(OFFLOAD_QUEUE_CAPACITY);
            let (stop_tx, stop_rx) = oneshot::channel();
            task = Some(OffloadTask::spawn(
                rx,
                stop_rx,
                store.clone(),
                inbox_tx.clone(),
            ));
            stop = Some(stop_tx);
            tx
        });
        Self {
            tx,
            stop,
            task,
            pending: HashMap::new(),
            held: VecDeque::new(),
            orphans: VecDeque::new(),
        }
    }

    /// Test seam: a state on a queue the test drains itself.
    #[cfg(test)]
    pub(crate) fn with_queue(tx: mpsc::Sender<OffloadJob>) -> Self {
        Self {
            tx: Some(tx),
            stop: None,
            task: None,
            pending: HashMap::new(),
            held: VecDeque::new(),
            orphans: VecDeque::new(),
        }
    }

    /// `held` is at its cap: the loop takes no cell emission until a return
    /// frees the queue.
    pub(crate) fn saturated(&self) -> bool {
        self.held.len() >= OFFLOAD_HELD_CAPACITY
    }

    /// The shutdown: drop the queue's sender and stop the task (always -- the
    /// drain already waited for quiescence up to its deadline). Returns the
    /// task to await; it hands back every job it still has, unwritten ones
    /// with `stopped` set.
    pub(crate) fn shut(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        self.tx = None;
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.take()
    }

    /// The jobs the loop still holds (held, orphans), in order, each with its
    /// inline body back: the shutdown teardown routes them itself.
    pub(crate) fn take_rest(&mut self) -> Vec<OffloadJob> {
        let mut rest: Vec<OffloadJob> = self.held.drain(..).chain(self.orphans.drain(..)).collect();
        for job in &mut rest {
            job.restore_body();
        }
        self.pending.clear();
        rest
    }

    #[cfg(test)]
    pub(crate) fn held_len(&self) -> usize {
        self.held.len()
    }

    /// Offload jobs still out (held ones included). They count as work in
    /// flight: a colony with one is not quiescent (`Settle`, the drain).
    pub(crate) fn in_flight(&self) -> usize {
        self.pending.values().map(|n| *n as usize).sum::<usize>() + self.orphans.len()
    }

    /// Decide for a message bound for a cell or a hive: queue it (an oversized
    /// inline body, or any message of a sender that still waits), route it on,
    /// or fall back to the inline write.
    pub(crate) fn defer(
        &mut self,
        sender: &Path,
        msg: Message,
        blob_inline_max_bytes: usize,
        logged: bool,
        origin: Option<bool>,
    ) -> Deferral {
        if self.tx.is_none() {
            return self.without_task(sender, msg, logged, origin);
        }
        let waiting = self.pending.contains_key(sender);
        let bytes = match &msg.body {
            Body::Inline(value) => match meclaw_core::serde_json::to_vec(value) {
                Ok(b) if b.len() >= blob_inline_max_bytes => Some(b),
                Ok(_) => None,
                // `offload_oversized` logs the same failure on the inline path;
                // here the body simply stays inline.
                Err(e) => {
                    tracing::error!(error = %e, "offload: body serialize failed — keeping inline");
                    None
                }
            },
            Body::Blob(_) => None,
        };
        if bytes.is_none() && !waiting {
            return Deferral::Route(msg);
        }
        let mut msg = msg;
        if bytes.is_some() {
            // One representation: the serialized bytes carry the body now.
            msg.body = Body::Inline(meclaw_core::serde_json::Value::Null);
        }
        let job = OffloadJob {
            sender: sender.clone(),
            len: bytes.as_ref().map_or(0, Vec::len),
            msg,
            bytes,
            logged,
            origin,
            queued_at: tokio::time::Instant::now(),
            orphaned: false,
            stopped: false,
        };
        // A held job is older than anything queued after it: while any is
        // held, a waiting sender's next job queues behind it.
        if waiting && !self.held.is_empty() {
            self.hold(job);
            return Deferral::Deferred;
        }
        let sent = match &self.tx {
            Some(tx) => tx.try_send(job),
            None => Err(TrySendError::Closed(job)),
        };
        match sent {
            Ok(()) => {
                *self.pending.entry(sender.clone()).or_insert(0) += 1;
                Deferral::Deferred
            }
            // OR-d53-4.z1.1: the inline write would overtake this sender's
            // queued jobs, so its job waits for the queue instead.
            Err(TrySendError::Full(job)) if waiting => {
                self.hold(job);
                Deferral::Deferred
            }
            Err(TrySendError::Full(mut job)) => {
                tracing::warn!(
                    sender = %sender.as_str(),
                    "offload queue full — writing inline"
                );
                job.restore_body();
                Deferral::Inline(job.msg)
            }
            Err(TrySendError::Closed(mut job)) => {
                self.task_gone();
                job.restore_body();
                self.without_task(sender, job.msg, logged, origin)
            }
        }
    }

    /// No task (no store, or it died): the inline path -- unless orphans of
    /// the dead task are still to be routed, then this message queues behind
    /// them, so it overtakes none of its sender's.
    fn without_task(
        &mut self,
        sender: &Path,
        msg: Message,
        logged: bool,
        origin: Option<bool>,
    ) -> Deferral {
        if self.orphans.is_empty() {
            return Deferral::Inline(msg);
        }
        self.orphans.push_back(OffloadJob {
            sender: sender.clone(),
            msg,
            // An orphan takes the inline path, where `offload_oversized`
            // decides on the size itself.
            bytes: None,
            len: 0,
            logged,
            origin,
            queued_at: tokio::time::Instant::now(),
            orphaned: true,
            stopped: false,
        });
        Deferral::Deferred
    }

    /// The task is gone (it can only end by a panic while the loop lives).
    /// The jobs it held are lost with it; the held ones become orphans the
    /// loop routes itself, and nothing counts as out any more -- the colony
    /// must still reach quiescence.
    fn task_gone(&mut self) {
        let held = self.held.len();
        let lost = self.in_flight().saturating_sub(held + self.orphans.len());
        tracing::error!(lost, held, "offload task gone — writing inline");
        self.tx = None;
        self.pending.clear();
        while let Some(mut job) = self.held.pop_front() {
            // An orphan takes the inline path with its inline body.
            job.restore_body();
            job.orphaned = true;
            self.orphans.push_back(job);
        }
    }

    fn hold(&mut self, job: OffloadJob) {
        *self.pending.entry(job.sender.clone()).or_insert(0) += 1;
        self.held.push_back(job);
    }

    /// The next job the dead task never took, if any.
    pub(crate) fn take_orphan(&mut self) -> Option<OffloadJob> {
        self.orphans.pop_front()
    }

    /// A job came back (from the task, or an orphan): it is no longer out,
    /// held jobs move up into the freed queue, and -- for a job the task
    /// handled -- the line `offload_deferred` names it. Returns the routing
    /// sender and the message, to be routed WITHOUT a second deferral.
    pub(crate) fn returned(&mut self, job: OffloadJob) -> (Path, Message) {
        if job.orphaned {
            return (job.sender, job.msg);
        }
        if let Some(n) = self.pending.get_mut(&job.sender) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.pending.remove(&job.sender);
            }
        }
        let mut gone = false;
        if let Some(tx) = &self.tx {
            while let Some(next) = self.held.pop_front() {
                match tx.try_send(next) {
                    Ok(()) => {}
                    Err(TrySendError::Full(next)) => {
                        self.held.push_front(next);
                        break;
                    }
                    Err(TrySendError::Closed(next)) => {
                        self.held.push_front(next);
                        gone = true;
                        break;
                    }
                }
            }
        }
        if gone {
            self.task_gone();
        }
        let body_kind = match job.msg.body {
            Body::Inline(_) => "inline",
            Body::Blob(_) => "blob",
        };
        // The lab counts this line in the colony log: proof the deferred
        // path ran (plan d53 T4-C § Z1 (f)).
        tracing::info!(
            sender = %job.sender.as_str(),
            target = %job.msg.target.as_str(),
            body_kind,
            bytes = job.len,
            queue_wait_ms = u64::try_from(job.queued_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            "offload_deferred"
        );
        (job.sender, job.msg)
    }
}

/// GH #1372 / #1418: fault injection for the offload pins -- the next blob
/// write on this thread (the inline write or the offload task's) hangs until
/// the given future completes. Thread-local: it acts in a current-thread test
/// runtime only.
#[cfg(test)]
pub(crate) mod seam {
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;

    type Stall = Pin<Box<dyn Future<Output = ()> + Send>>;

    thread_local! {
        static STALL: RefCell<Option<Stall>> = const { RefCell::new(None) };
    }

    /// Hang the next offload on this thread until `stall` completes.
    pub(crate) fn set(stall: impl Future<Output = ()> + Send + 'static) {
        STALL.with(|s| *s.borrow_mut() = Some(Box::pin(stall)));
    }

    /// Await the stall set by [`set`], once.
    pub(crate) async fn stall() {
        if let Some(stall) = STALL.with(|s| s.borrow_mut().take()) {
            stall.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::MessageBuilder;
    use meclaw_core::serde_json::{Value, json};

    fn big(i: usize) -> Message {
        MessageBuilder::new(Path::new("/sink"))
            .ttl(8)
            .body(Body::Inline(json!({"i": i, "pad": "x".repeat(2048)})))
            .build()
    }

    /// Pin (#1418, memory): a flood of ONE sender, three times queue plus
    /// `held`, against a task that hands jobs back one by one. The loop takes
    /// a message only while `held` is below its cap (the guard of the output
    /// arm), so `held` never passes the cap; a queued job carries its body
    /// once (serialized, the message a placeholder); the order holds and every
    /// message comes back with its body.
    #[test]
    fn a_flood_of_one_sender_stays_under_the_cap_in_order() {
        let (tx, mut rx) = mpsc::channel(OFFLOAD_QUEUE_CAPACITY);
        let mut st = OffloadState::with_queue(tx);
        let a = Path::new("/a");
        let n = 3 * (OFFLOAD_QUEUE_CAPACITY + OFFLOAD_HELD_CAPACITY);
        let mut flood = (0..n).map(big).peekable();
        let mut got = Vec::new();
        while got.len() < n {
            if flood.peek().is_some() && !st.saturated() {
                let m = flood.next().expect("peeked");
                assert!(
                    matches!(st.defer(&a, m, 1024, false, None), Deferral::Deferred),
                    "every message of the flood goes through the queue"
                );
                assert!(
                    st.held_len() <= OFFLOAD_HELD_CAPACITY,
                    "held stays under its cap ({} > {OFFLOAD_HELD_CAPACITY})",
                    st.held_len()
                );
                continue;
            }
            let mut job = rx.try_recv().expect("a job is queued while any is out");
            assert!(
                matches!(job.msg.body, Body::Inline(Value::Null)) && job.bytes.is_some(),
                "a queued job holds its body once"
            );
            job.restore_body();
            let (_, m) = st.returned(job);
            got.push(match &m.body {
                Body::Inline(v) => v["i"].as_u64().map_or(usize::MAX, |i| i as usize),
                Body::Blob(_) => usize::MAX,
            });
        }
        assert_eq!(got, (0..n).collect::<Vec<_>>(), "the order of /a holds");
        assert_eq!(st.in_flight(), 0, "nothing is left out");
    }
}
