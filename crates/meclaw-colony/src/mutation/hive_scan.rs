//! GH #1176: the config scans of the mutation path, off the colony worker.
//!
//! Every mutation read the `config.json` of every hive in the colony, several
//! times: the sealed ports and the hive contracts in validation, the required
//! drains against the post-state edge table, and the slot table plus the
//! delivery boundaries once the door had returned -- and the `config.json` of
//! every template, for its cell type. Each of those readers is a synchronous
//! loop of `std::fs::read_to_string` plus a full serde parse. They ran on the
//! colony's worker with no yield and no beat (AGENTS.md rule 13), and the
//! stretch they made was the one the watchdog saw as silence: the
//! `edges_applied` phase of a ten-edge mutation measured 63 ms idle and
//! 444 ms p50 / 926 ms max with twelve colonies on one host, and the lab trip
//! that opened the issue sat 5.2 s in exactly that phase.
//!
//! [`scan_hives`] runs one such scan in ONE `spawn_blocking` (the whole scan,
//! not one task per file: the measured hand-off cost is in the strand report)
//! and beats the mutation's pulse once per config, BEFORE that config's read,
//! under a label that names it. A read that never returns therefore stays a
//! silent, stuck work item -- the beats are progress, never a timer.
//!
//! The same per-config announcement carries the operation timeout (rule 12):
//! the async side waits at most [`scan_timeout`] of charged time for the next
//! config (see [`within_charged`]), and when it does not come the scan ends
//! with [`HiveScanTimeout`], which every caller turns into a clean refusal
//! (`hive_config_timeout`) or, for the slot rebuild, into a closed hive
//! boundary and a retry on the next pass.
//!
//! A scan given up on keeps its blocking thread until the read returns. The
//! [`HiveScanGate`] the colony owns lets at most ONE scan be in flight: the
//! next caller waits for the abandoned one up to its own timeout and, when it
//! is still stuck, ends with the same timeout without starting a second read
//! -- a disk that never answers holds one blocking thread, not one per pass.

use crate::watchdog::{WORK_ITEM_DENIAL_FACTOR, WorkItem, WorkPulse, denial_deduction};
use meclaw_core::Path;
use std::time::Duration;

/// The read timeout of a scan whose pulse knows no supervisor (tests,
/// `--validate`): twice the work-item budget of the shipped watchdog
/// (5 s = `WORK_ITEM_BUDGET_FACTOR` 10 × threshold 5 × 100 ms).
pub(crate) const HIVE_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a scan waits for the next config before it gives up (AGENTS.md
/// rule 12): twice the work-item budget of the supervisor the pulse reports
/// to, [`HIVE_READ_TIMEOUT`] when it reports to none.
///
/// Deliberately ABOVE the budget, whatever the watchdog is configured to: a
/// read that hangs is declared stuck by the supervisor first, which under
/// `on_trip = exit` is the verdict. The timeout is the clean end where the
/// process stays up (`log-only`), so the colony loop is not held by one read
/// forever. One read of one `config.json` is sub-millisecond on an idle disk.
///
/// GH #1199: twice the budget of CHARGED time. The supervisor does not charge
/// a silent item the CPU the host denied it (GH #1216), so under a quota its
/// verdict comes up to [`WORK_ITEM_DENIAL_FACTOR`] budgets late; a scan
/// under a supervisor credits the same denial from the same source
/// ([`within_charged`]) and stays above that verdict. A host that denies
/// nothing waits exactly this long.
pub(crate) fn scan_timeout(pulse: &WorkPulse) -> Duration {
    pulse
        .budget()
        .map_or(HIVE_READ_TIMEOUT, |b| b.saturating_mul(2))
}

/// Awaits `fut` for `timeout` of charged time (GH #1199, K10): the wait less
/// the CPU denial the supervisor of `pulse` deducts from a silence, by the
/// same rule ([`denial_deduction`]) and read from the same source
/// ([`WorkPulse::read_denial`]). `Err` carries the timeout that ran out:
/// `timeout` extended by the credited denial.
///
/// A pulse with no supervisor (no budget) has no verdict to stay above and
/// waits `timeout` of plain time. Otherwise the denial is read only when a
/// wait outlives a fifth of `timeout` -- the fifth the supervisor charges
/// whatever the host denies, so no credit is lost -- and again at each
/// deadline, never per config: an honest read costs nothing. A host that
/// denies nothing ends the wait at exactly `timeout`; one that denies all of
/// it at [`WORK_ITEM_DENIAL_FACTOR`] × `timeout`, never later.
async fn within_charged<F: std::future::Future>(
    timeout: Duration,
    pulse: &WorkPulse,
    fut: F,
) -> Result<F::Output, Duration> {
    if pulse.budget().is_none() {
        return tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| timeout);
    }
    tokio::pin!(fut);
    let start = tokio::time::Instant::now();
    let fifth = timeout / WORK_ITEM_DENIAL_FACTOR;
    if let Ok(out) = tokio::time::timeout(fifth, &mut fut).await {
        return Ok(out);
    }
    let base = pulse.read_denial();
    let last = start + timeout.saturating_mul(WORK_ITEM_DENIAL_FACTOR);
    let mut deadline = start + timeout;
    loop {
        if let Ok(out) = tokio::time::timeout_at(deadline, &mut fut).await {
            return Ok(out);
        }
        let now = tokio::time::Instant::now();
        let waited = now - start;
        let denied = pulse.read_denial().max_growth_since(&base);
        let charged = waited.saturating_sub(denial_deduction(waited, denied));
        if charged >= timeout || deadline >= last {
            return Err(deadline - start);
        }
        // At the rate the wait has been charged so far; at least the fifth,
        // so a denial that sets in late cannot shrink the steps for ever, and
        // never past the last deadline.
        let rest = (timeout - charged).as_nanos() * waited.as_nanos() / charged.as_nanos().max(1);
        let rest = Duration::from_nanos(u64::try_from(rest).unwrap_or(u64::MAX))
            .max(fifth)
            .min(last.saturating_duration_since(now));
        deadline = now + rest;
    }
}

/// A scan that did not hear of its next config within the read timeout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HiveScanTimeout {
    /// Which scan it was (`hive-scan`, `post-state`, `slot-rebuild`).
    pub what: &'static str,
    /// The config whose read was last announced -- the one that did not
    /// return. `None` when not even the first read was announced.
    pub hive: Option<String>,
    /// The timeout that ran out: the read timeout, extended by the CPU
    /// denial credited under a supervisor (GH #1199).
    pub after: Duration,
    /// Set when this scan never started: an earlier scan (named here) was
    /// still stuck on [`Self::hive`] when the timeout ran out.
    pub behind: Option<&'static str>,
}

impl HiveScanTimeout {
    /// The refusal text: what was being read and how long it was given.
    pub fn message(&self) -> String {
        let ms = self.after.as_millis();
        let what = self.what;
        match (&self.hive, self.behind) {
            (Some(h), Some(prev)) => format!(
                "{what}: not started, the previous config scan ({prev}) is still reading \
                 '{h}' after another {ms} ms"
            ),
            (None, Some(prev)) => format!(
                "{what}: not started, the previous config scan ({prev}) is still running \
                 after another {ms} ms"
            ),
            (Some(h), None) => {
                format!("{what}: reading the config.json of '{h}' did not finish within {ms} ms")
            }
            (None, None) => {
                format!("{what}: the scan did not reach its first config.json within {ms} ms")
            }
        }
    }
}

/// The scan a caller gave up on, still holding its blocking thread.
#[derive(Debug)]
struct InFlight {
    task: tokio::task::JoinHandle<()>,
    what: &'static str,
    hive: Option<String>,
}

/// Single-flight for the config scans of one colony (see the module doc).
///
/// Owned by the colony task (it lives in the hive-scope table every scan reads
/// from), so it is plain state of one actor: no lock, no atomics.
#[derive(Debug, Default)]
pub(crate) struct HiveScanGate {
    in_flight: Option<InFlight>,
}

impl HiveScanGate {
    /// The scan an earlier caller gave up on, while it still holds its read,
    /// as the timeout a caller that must NOT wait for it ends with at once;
    /// a scan that has ended since is cleared and `None` comes back.
    ///
    /// GH #1176 re-check (u1 L2 / u2 L2): the slot rebuild runs on every pass
    /// of the colony loop while its flag is set. Waiting behind a lasting
    /// hang there cost each pass the full read timeout (10 s by default), so
    /// under `log-only` the colony took about one event per 10 s. The
    /// boundary is closed meanwhile anyway (fail-closed), so that pass asks
    /// and moves on.
    pub(crate) fn still_stuck(&mut self, what: &'static str) -> Option<HiveScanTimeout> {
        let prev = self.in_flight.as_ref()?;
        if prev.task.is_finished() {
            self.in_flight = None;
            return None;
        }
        Some(HiveScanTimeout {
            what,
            hive: prev.hive.clone(),
            after: Duration::ZERO,
            behind: Some(prev.what),
        })
    }
}

/// One announcement; `false` once the async side has given up on the scan.
type Step = Box<dyn Fn(&str) -> bool + Send + Sync>;

/// What a scan closure gets: the colony root and the hive paths, the latter
/// as an iterator that announces each hive before handing it out.
pub(crate) struct HiveScan {
    root: std::path::PathBuf,
    paths: Vec<Path>,
    step: Step,
}

impl HiveScan {
    /// The colony root the readers resolve hive directories against.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// The hive paths, each announced (beat + timeout clock) as the reader
    /// takes it. One reader walking the list is one read per hive; a second
    /// walk announces every hive again, as it reads every file again.
    pub fn paths(&self) -> PacedPaths<'_> {
        PacedPaths {
            inner: self.paths.iter(),
            step: &*self.step,
        }
    }

    /// Announce a read that is not a walk over [`Self::paths`] (the connect
    /// points of a sealed hive, a template's `config.json`).
    pub fn step(&self, hive: &str) {
        let _ = (self.step)(hive);
    }
}

/// The paced iterator [`HiveScan::paths`] hands out.
pub(crate) struct PacedPaths<'a> {
    inner: std::slice::Iter<'a, Path>,
    step: &'a (dyn Fn(&str) -> bool + Send + Sync),
}

impl<'a> Iterator for PacedPaths<'a> {
    type Item = &'a Path;

    /// Ends the walk once the async side has given up on the scan (GH #1176
    /// re-check u2 L3): an abandoned scan read every remaining config for a
    /// result nobody takes, on a disk that had just shown it is slow.
    fn next(&mut self) -> Option<&'a Path> {
        let p = self.inner.next()?;
        if !(self.step)(p.as_str()) {
            return None;
        }
        Some(p)
    }
}

/// Run `scan` over the configs under `root` off the colony worker, with one
/// beat per config and [`scan_timeout`] per read, behind `gate`. See the
/// module doc.
pub(crate) async fn scan_hives<T: Send + 'static>(
    gate: &mut HiveScanGate,
    root: &std::path::Path,
    paths: Vec<Path>,
    pulse: &WorkPulse,
    what: &'static str,
    scan: impl FnOnce(&HiveScan) -> T + Send + 'static,
) -> Result<T, HiveScanTimeout> {
    let timeout = scan_timeout(pulse);
    scan_hives_paced(gate, root, paths, pulse, what, timeout, |_| {}, scan).await
}

/// [`scan_hives`] with the read timeout as a parameter and `pace` run inside
/// every config's step -- the seam a test uses to make a read as slow as a
/// contended disk makes it, or to make one hang.
///
/// # Cancel safety (GH #1176 re-check u2 L8)
///
/// Not cancel safe: the gate learns of the blocking task only when the
/// timeout gives it up. A caller that drops this future mid-await (a
/// `select!` arm that loses) leaves a reading task the gate does not know of,
/// and the next scan starts beside it. No caller does that today; one that
/// puts a scan into a `select!` first moves the task into the gate before the
/// first await.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn scan_hives_paced<T: Send + 'static>(
    gate: &mut HiveScanGate,
    root: &std::path::Path,
    paths: Vec<Path>,
    pulse: &WorkPulse,
    what: &'static str,
    read_timeout: Duration,
    pace: impl Fn(&str) + Send + Sync + 'static,
    scan: impl FnOnce(&HiveScan) -> T + Send + 'static,
) -> Result<T, HiveScanTimeout> {
    // Single-flight: a scan an earlier caller gave up on is waited for, up to
    // this caller's own timeout, and never run beside.
    if let Some(mut prev) = gate.in_flight.take()
        && !prev.task.is_finished()
        && let Err(granted) = within_charged(read_timeout, pulse, &mut prev.task).await
    {
        // DEBUG, not WARN (GH #1176 re-check): under a lasting hang every
        // pass of the slot rebuild lands here; the caller reports the outcome
        // (a mutation refusal carries this timeout, the boundary logs once
        // per transition).
        tracing::debug!(
            scan = what,
            behind = prev.what,
            hive = prev.hive.as_deref().unwrap_or("<none>"),
            timeout_ms = read_timeout.as_millis() as u64,
            granted_ms = granted.as_millis() as u64,
            "config scan: the previous scan is still stuck, not starting another"
        );
        let err = HiveScanTimeout {
            what,
            hive: prev.hive.clone(),
            after: granted,
            behind: Some(prev.what),
        };
        gate.in_flight = Some(prev);
        return Err(err);
    }

    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let step_pulse = pulse.clone();
    let step: Step = Box::new(move |hive: &str| {
        // The announcement first: a scan the async side has already given up
        // on (timeout) must not go on beating under the mutation's label.
        if progress_tx.send(hive.to_string()).is_err() {
            return false;
        }
        step_pulse
            .with_label(WorkItem::new(format!(
                "{} op={what} hive={hive}",
                step_pulse.label()
            )))
            .tick();
        pace(hive);
        true
    });
    let root = root.to_path_buf();
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<T>();
    let mut task = tokio::task::spawn_blocking(move || {
        let _ = done_tx.send(scan(&HiveScan { root, paths, step }));
    });
    let mut current: Option<String> = None;
    loop {
        tokio::select! {
            biased;
            joined = &mut task => return finished(joined, &mut done_rx, what, current, read_timeout),
            next = within_charged(read_timeout, pulse, progress_rx.recv()) => match next {
                Ok(Some(hive)) => current = Some(hive),
                // The scan dropped its announcer: it has returned, only the
                // join is left.
                Ok(None) => {
                    let joined = (&mut task).await;
                    return finished(joined, &mut done_rx, what, current, read_timeout);
                }
                Err(granted) => {
                    tracing::warn!(
                        scan = what,
                        hive = current.as_deref().unwrap_or("<none>"),
                        timeout_ms = read_timeout.as_millis() as u64,
                        granted_ms = granted.as_millis() as u64,
                        "config scan: a read did not finish in time"
                    );
                    gate.in_flight = Some(InFlight {
                        task,
                        what,
                        hive: current.clone(),
                    });
                    return Err(HiveScanTimeout {
                        what,
                        hive: current,
                        after: granted,
                        behind: None,
                    });
                }
            },
        }
    }
}

/// The scan's task has ended: its value, or its panic (a panic inside the
/// scan stays a panic, exactly as it was on the worker).
fn finished<T>(
    joined: Result<(), tokio::task::JoinError>,
    done_rx: &mut tokio::sync::oneshot::Receiver<T>,
    what: &'static str,
    hive: Option<String>,
    after: Duration,
) -> Result<T, HiveScanTimeout> {
    match joined {
        Ok(()) => done_rx.try_recv().map_err(|_| HiveScanTimeout {
            what,
            hive,
            after,
            behind: None,
        }),
        Err(e) => match e.try_into_panic() {
            Ok(payload) => std::panic::resume_unwind(payload),
            // Cancelled: only a runtime shutting down cancels a blocking task.
            Err(_) => Err(HiveScanTimeout {
                what,
                hive,
                after,
                behind: None,
            }),
        },
    }
}

/// A scan that judges a mutation (validation, post-state) and timed out
/// refuses it: the violation `hive_config_timeout` goes into `rejection` under
/// `stage`, and the caller gets nothing to judge with. Fail-closed is decided
/// here, so a call site holds no `Err` arm of its own that could drop it.
pub(crate) fn read_or_refuse<T>(
    read: Result<T, HiveScanTimeout>,
    stage: crate::mutation::rejection::Stage,
    rejection: &mut crate::mutation::rejection::MutationRejection,
) -> Option<T> {
    match read {
        Ok(value) => Some(value),
        Err(timeout) => {
            rejection.push(crate::mutation::rejection::Violation::from_error(
                stage,
                &crate::mutation::MutationError::HiveConfigTimeout(timeout.message()),
                timeout.hive,
            ));
            None
        }
    }
}

/// The slot rebuild's verdict on its scan: the new tables, with `dirty`
/// cleared; on a timeout the timeout, `dirty` stays set so a later pass reads
/// again. The caller keeps its slot table and closes the hive boundary
/// meanwhile ([`crate::hive_boundary::sealed_shut`]): fail-closed. It logs,
/// once per transition, not this function once per pass (a lasting hang
/// repeats the pass every few seconds).
pub(crate) fn rebuilt_or_kept<T>(
    read: Result<T, HiveScanTimeout>,
    dirty: &mut bool,
) -> Result<T, HiveScanTimeout> {
    if read.is_ok() {
        *dirty = false;
    }
    read
}

#[cfg(test)]
mod tests;
