//! The warm/resident runner pool: a broker task owning the queue and the idle
//! set, plus one task per child owning exactly one process.
//!
//! The broker holds no pipe and no process. That is the whole point of the
//! split — nothing here can block on a child, so one slow script cannot stall
//! the others, and the state lives in a task instead of behind a lock.

use crate::process::KillingTimeoutOutput;
use std::collections::VecDeque;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

/// One execution of the cell's script, addressed at whichever child is free.
pub(crate) struct Job {
    /// The stdin document (`wire::build_stdin_json`), already serialized.
    pub(crate) document: String,
    /// The A-timeout for this one execution (`params.external_timeout_ms`).
    pub(crate) timeout: Duration,
    /// Where the answer goes. A dropped sender IS the caller's cancellation.
    pub(crate) reply: oneshot::Sender<JobOutcome>,
}

/// What one job produced. Every variant maps onto an error path the cold runner
/// already has, which is why warm introduces no `error_code` of its own.
pub(crate) enum JobOutcome {
    /// The child answered: exit code, stdout, stderr -- the same three values a
    /// cold `with_killing_timeout` returns.
    Ran(KillingTimeoutOutput),
    /// The A-timeout elapsed and the child was killed (`script_timeout`).
    Timeout,
    /// The job could not be run at all (`io_error`).
    Io(String),
}

/// The cell's end of the pool. Cheap to clone (one `mpsc::Sender`), and holding
/// one is what keeps the pool alive -- a `CodeCell` that drops takes its
/// children with it.
#[derive(Clone)]
pub(crate) struct PoolHandle {
    jobs_tx: mpsc::Sender<Job>,
}

impl PoolHandle {
    /// Run one document and wait for its answer.
    ///
    /// The await carries the same A-timeout the child applies to the run
    /// (mirror of `mcp::stdio::rpc_over_task`): the child owns the KILL, this
    /// timer owns the guarantee that `handle()` returns at all. Both produce
    /// `script_timeout`, so the race between them has one outcome.
    pub(crate) async fn run(&self, document: String, timeout: Duration) -> JobOutcome {
        let (reply, rx) = oneshot::channel();
        let job = Job {
            document,
            timeout,
            reply,
        };
        if self.jobs_tx.send(job).await.is_err() {
            return JobOutcome::Io("runner pool is gone".into());
        }
        match tokio::time::timeout(timeout, rx).await {
            Err(_) => JobOutcome::Timeout,
            Ok(Err(_)) => JobOutcome::Io("runner dropped the reply".into()),
            Ok(Ok(outcome)) => outcome,
        }
    }
}

/// Start a pool of `size` children and return its handle.
pub(crate) fn spawn_pool(cfg: crate::code::child::ChildConfig, size: usize) -> PoolHandle {
    let size = size.max(1);
    let (jobs_tx, jobs_rx) = mpsc::channel::<Job>(size);
    tokio::spawn(pool_task(cfg, size, jobs_rx));
    PoolHandle { jobs_tx }
}

/// The broker: owns the queue and the idle set, owns no pipe and no process.
///
/// **Panic-free by construction** (the AUDIT-PRE14-001 analogy, § O2 of the
/// plan): no `unwrap`, no indexing, no `expect`. There is no supervisor above
/// this task -- a stateless cell's workers are not supervised either -- so the
/// discipline the substrate applies to `handle()` applies here: every exit is
/// an ANSWER, never a silence.
async fn pool_task(
    cfg: crate::code::child::ChildConfig,
    size: usize,
    mut jobs_rx: mpsc::Receiver<Job>,
) {
    let (idle_tx, mut idle_rx) = mpsc::channel::<usize>(size);
    let mut slots: Vec<mpsc::Sender<Job>> = Vec::with_capacity(size);
    for slot in 0..size {
        let (tx, rx) = mpsc::channel::<Job>(1);
        slots.push(tx);
        tokio::spawn(crate::code::child::child_task(
            slot,
            cfg.clone(),
            rx,
            idle_tx.clone(),
        ));
    }
    // The broker's own sender is dropped so `idle_rx` can report "every child
    // task is gone" rather than parking forever.
    drop(idle_tx);
    let mut idle: VecDeque<usize> = (0..size).collect();
    let mut queue: VecDeque<Job> = VecDeque::new();
    loop {
        tokio::select! {
            // Freed slots first: a queue that could be served must be served
            // before a new job is taken in.
            biased;
            freed = idle_rx.recv() => match freed {
                Some(slot) => {
                    // FRONT, not back (GH #429): reuse the child that just
                    // finished. A freed slot returned to the BACK round-robins
                    // a serial stream across every slot, so a pool of N pays N
                    // interpreter starts and holds N children to serve one
                    // message at a time -- which defeats the lazy spawn for the
                    // commonest traffic shape there is. A burst still fans out:
                    // concurrent jobs are all queued before any slot reports
                    // itself free, so `dispatch` drains the idle set anyway.
                    // The JOB queue stays FIFO; only the idle set is a stack,
                    // and slots are interchangeable workers.
                    idle.push_front(slot);
                    dispatch(&slots, &mut idle, &mut queue);
                }
                None => break,
            },
            job = jobs_rx.recv() => match job {
                Some(job) => {
                    queue.push_back(job);
                    dispatch(&slots, &mut idle, &mut queue);
                }
                // The cell value is gone. Dropping `slots` ends every child
                // task, and `kill_on_drop` takes the processes with them.
                None => break,
            },
        }
    }
    for job in queue.drain(..) {
        let _ = job.reply.send(JobOutcome::Io("runner pool stopped".into()));
    }
}

/// Hand queued jobs to idle slots, in order. Synchronous on purpose: a broker
/// that awaited a child would let one slow child stall the whole pool.
fn dispatch(slots: &[mpsc::Sender<Job>], idle: &mut VecDeque<usize>, queue: &mut VecDeque<Job>) {
    while !queue.is_empty() {
        let Some(slot) = idle.pop_front() else { return };
        // A slot index with no sender cannot come back; the job stays queued.
        let Some(tx) = slots.get(slot) else { continue };
        let Some(job) = queue.pop_front() else { return };
        if let Err(err) = tx.try_send(job) {
            let job = match err {
                mpsc::error::TrySendError::Full(j) | mpsc::error::TrySendError::Closed(j) => j,
            };
            // FIFO survives a broken slot: the job goes back to the HEAD, and
            // the slot leaves the rotation until it reports itself free again.
            queue.push_front(job);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::child::config_for;
    use crate::code::params::{CodeParams, RunnerMode, Script};
    use meclaw_testing::deadline;
    use std::collections::BTreeSet;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::unix::pipe;

    /// The A-timeout of every job here (TV-Z9: SUT time by configuration):
    /// far above the test's own hang mark, so a job that never ends is named
    /// by that mark (`HANG:`), never cut short by the pool.
    fn job_wait() -> Duration {
        Duration::from_millis(deadline::product_wait_ms())
    }

    fn pool(script: &str, mode: RunnerMode, size: usize) -> PoolHandle {
        let p = CodeParams {
            runner: "python3".into(),
            script: Script::Inline(script.into()),
            external_timeout_ms: Some(deadline::product_wait_ms()),
            max_concurrency: None,
            sandbox: None,
            runner_mode: mode,
        };
        spawn_pool(config_for(&p).expect("warm/resident"), size)
    }

    fn stdout_of(o: &JobOutcome) -> String {
        match o {
            JobOutcome::Ran(k) => String::from_utf8_lossy(&k.stdout).to_string(),
            JobOutcome::Timeout => "<timeout>".into(),
            JobOutcome::Io(e) => format!("<io:{e}>"),
        }
    }

    fn pid_of(o: &JobOutcome) -> i64 {
        let out = stdout_of(o);
        let v: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(&out)
            .unwrap_or_else(|e| panic!("a job answered no pid ({e}): {out}"));
        v["pid"].as_i64().expect("a numeric pid")
    }

    /// The job of a rendezvous: announce its own pid on `arrive`, then block
    /// on `gate` until the test lets it go, then answer the pid.
    const MEET: &str = "import os,sys,json\n\
        d = json.load(sys.stdin)['body']\n\
        a = os.open(d['arrive'], os.O_WRONLY)\n\
        os.write(a, b'%d\\n' % os.getpid())\n\
        os.close(a)\n\
        g = os.open(d['gate'], os.O_RDONLY)\n\
        os.read(g, 1)\n\
        os.close(g)\n\
        sys.stdout.write(json.dumps({'pid': os.getpid()}))\n";

    /// GH #1199 (K9): concurrency as an EVENT, not a stopwatch. Every job of
    /// [`MEET`] announces itself and then waits on a gate the test opens only
    /// once `n` distinct processes have announced themselves -- so when the
    /// gate opens, `n` jobs are running at the same moment, each in its own
    /// child. A pool that ran them one after the other never gets there, and
    /// the wait ends at the hang mark (`HANG:`).
    ///
    /// Both ends are FIFOs the test holds open read-write, so no `open` ever
    /// blocks on the other side: a job's announcement never waits for the
    /// reader, and the bytes the test writes into the gate stay there for a
    /// job that opens it later.
    struct Rendezvous {
        _dir: tempfile::TempDir,
        arrive_path: std::path::PathBuf,
        gate_path: std::path::PathBuf,
        arrive: BufReader<pipe::Receiver>,
        gate: pipe::Sender,
    }

    impl Rendezvous {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let arrive_path = dir.path().join("arrive");
            let gate_path = dir.path().join("gate");
            for p in [&arrive_path, &gate_path] {
                let c = std::ffi::CString::new(p.to_str().expect("utf-8 path")).expect("no NUL");
                // SAFETY: a valid NUL-terminated path; mkfifo touches nothing else.
                let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
                assert_eq!(rc, 0, "mkfifo {}", p.display());
            }
            let arrive = pipe::OpenOptions::new()
                .read_write(true)
                .open_receiver(&arrive_path)
                .expect("open the arrival FIFO");
            let gate = pipe::OpenOptions::new()
                .read_write(true)
                .open_sender(&gate_path)
                .expect("open the gate FIFO");
            Self {
                _dir: dir,
                arrive_path,
                gate_path,
                arrive: BufReader::new(arrive),
                gate,
            }
        }

        /// The stdin document of a [`MEET`] job.
        fn document(&self) -> String {
            meclaw_core::serde_json::json!({"body": {
                "arrive": self.arrive_path.to_str().expect("utf-8 path"),
                "gate": self.gate_path.to_str().expect("utf-8 path"),
            }})
            .to_string()
        }

        /// Waits until `n` distinct processes are inside a job at once, then
        /// lets all of them go. Returns their pids.
        async fn meet(&mut self, n: usize) -> BTreeSet<i64> {
            let mut here = BTreeSet::new();
            let arrive = &mut self.arrive;
            deadline::hang(
                &format!("{n} jobs run at once, each in its own child"),
                async {
                    while here.len() < n {
                        let mut line = String::new();
                        arrive.read_line(&mut line).await.expect("read an arrival");
                        here.insert(line.trim().parse::<i64>().expect("an arrival is a pid"));
                    }
                },
            )
            .await;
            self.gate
                .write_all(&vec![b'.'; n])
                .await
                .expect("open the gate");
            here
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_pool_of_two_runs_two_jobs_at_once_on_two_processes() {
        let h = pool(MEET, RunnerMode::Warm, 2);
        let mut rv = Rendezvous::new();
        let doc = rv.document();
        let (a, b, met) = tokio::join!(
            h.run(doc.clone(), job_wait()),
            h.run(doc, job_wait()),
            rv.meet(2),
        );
        assert_eq!(met.len(), 2, "the two ran together");
        assert_ne!(pid_of(&a), pid_of(&b), "two children, two pids");
        assert_eq!(
            BTreeSet::from([pid_of(&a), pid_of(&b)]),
            met,
            "the two answers came from the two processes that met"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_pool_of_one_serves_strictly_in_order() {
        let h = pool(
            r#"import sys,json; d=json.load(sys.stdin); sys.stdout.write(str(d["body"]["n"]))"#,
            RunnerMode::Resident,
            1,
        );
        let mut got = Vec::new();
        for n in 0..5 {
            got.push(stdout_of(
                &h.run(format!(r#"{{"body":{{"n":{n}}}}}"#), job_wait())
                    .await,
            ));
        }
        assert_eq!(got, vec!["0", "1", "2", "3", "4"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn every_job_is_answered_exactly_once() {
        let h = pool(
            r#"import sys,json; sys.stdout.write(json.dumps({"ok":True}))"#,
            RunnerMode::Warm,
            2,
        );
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..12 {
            let h = h.clone();
            set.spawn(async move { h.run(r#"{"body":{}}"#.into(), job_wait()).await });
        }
        let mut answers = 0;
        while let Some(r) = set.join_next().await {
            let out = r.unwrap();
            assert_eq!(stdout_of(&out), r#"{"ok": true}"#, "every job ran");
            answers += 1;
        }
        assert_eq!(answers, 12, "no job vanished, none was answered twice");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_handle_takes_the_children_with_it() {
        let h = pool(
            "import os,sys,json; sys.stdout.write(json.dumps({'pid': os.getpid()}))",
            RunnerMode::Warm,
            1,
        );
        let out = h.run(r#"{"body":{}}"#.into(), job_wait()).await;
        let pid = pid_of(&out) as i32;
        drop(h);
        // The broker sees its job channel close, drops the slots, the child
        // tasks end and `kill_on_drop` reaps the processes.
        meclaw_testing::wait::until(
            "the warm child is gone after its pool dropped",
            || async move {
                // SAFETY: signal 0 only probes whether the pid still exists.
                let probe = unsafe { libc::kill(pid, 0) };
                probe != 0
            },
        )
        .await;
    }

    /// GH #429: a serial stream must stay on ONE child, however big the pool.
    ///
    /// The refutation that found the bug: with the freed slot returned to the
    /// back of the idle set this touched all four children.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_serial_stream_stays_on_one_child_however_big_the_pool() {
        let h = pool(
            "import os,sys,json; sys.stdout.write(json.dumps({'pid': os.getpid()}))",
            RunnerMode::Warm,
            4,
        );
        let mut pids = BTreeSet::new();
        for _ in 0..12 {
            let o = h.run(r#"{"body":{}}"#.into(), job_wait()).await;
            pids.insert(pid_of(&o));
        }
        assert_eq!(
            pids.len(),
            1,
            "twelve serial messages started {} interpreters: {pids:?}",
            pids.len()
        );
    }

    /// The other half of the same rule: a BURST still uses the whole pool, so
    /// the fix above buys its saving without costing parallelism. The four
    /// jobs meet at a [`Rendezvous`]: none of them ends before all four run.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_still_fans_out_across_the_whole_pool() {
        let h = pool(MEET, RunnerMode::Warm, 4);
        let mut rv = Rendezvous::new();
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..4 {
            let h = h.clone();
            let doc = rv.document();
            set.spawn(async move { h.run(doc, job_wait()).await });
        }
        let met = rv.meet(4).await;
        let mut pids = BTreeSet::new();
        while let Some(r) = set.join_next().await {
            pids.insert(pid_of(&r.unwrap()));
        }
        assert_eq!(
            pids.len(),
            4,
            "four concurrent jobs must meet four children"
        );
        assert_eq!(pids, met, "the four answers came from the four that met");
    }
}
