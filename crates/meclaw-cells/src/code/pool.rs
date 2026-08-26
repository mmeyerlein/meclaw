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

    fn pool(script: &str, mode: RunnerMode, size: usize) -> PoolHandle {
        let p = CodeParams {
            runner: "python3".into(),
            script: Script::Inline(script.into()),
            external_timeout_ms: Some(5_000),
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_pool_of_two_runs_two_jobs_at_once_on_two_processes() {
        let h = pool(
            "import os,sys,json,time; time.sleep(0.2); sys.stdout.write(json.dumps({'pid':os.getpid()}))",
            RunnerMode::Warm,
            2,
        );
        let t = std::time::Instant::now();
        let (a, b) = tokio::join!(
            h.run(r#"{"body":{}}"#.into(), Duration::from_millis(5_000)),
            h.run(r#"{"body":{}}"#.into(), Duration::from_millis(5_000)),
        );
        assert!(
            t.elapsed() < Duration::from_millis(2_000),
            "the two ran together"
        );
        assert_ne!(stdout_of(&a), stdout_of(&b), "two children, two pids");
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
                &h.run(
                    format!(r#"{{"body":{{"n":{n}}}}}"#),
                    Duration::from_millis(5_000),
                )
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
            set.spawn(async move {
                h.run(r#"{"body":{}}"#.into(), Duration::from_millis(5_000))
                    .await
            });
        }
        let mut answers = 0;
        while let Some(r) = set.join_next().await {
            assert!(matches!(r.unwrap(), JobOutcome::Ran(_)));
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
        let out = h
            .run(r#"{"body":{}}"#.into(), Duration::from_millis(5_000))
            .await;
        let v: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(&stdout_of(&out)).unwrap();
        let pid = v["pid"].as_i64().unwrap() as i32;
        drop(h);
        // The broker sees its job channel close, drops the slots, the child
        // tasks end and `kill_on_drop` reaps the processes.
        for _ in 0..100 {
            if unsafe { libc::kill(pid, 0) } != 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the warm child outlived its pool");
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
        let mut pids = std::collections::BTreeSet::new();
        for _ in 0..12 {
            let o = h
                .run(r#"{"body":{}}"#.into(), Duration::from_millis(5_000))
                .await;
            let v: meclaw_core::serde_json::Value =
                meclaw_core::serde_json::from_str(&stdout_of(&o)).unwrap();
            pids.insert(v["pid"].as_i64().unwrap());
        }
        assert_eq!(
            pids.len(),
            1,
            "twelve serial messages started {} interpreters: {pids:?}",
            pids.len()
        );
    }

    /// The other half of the same rule: a BURST still uses the whole pool, so
    /// the fix above buys its saving without costing parallelism.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_still_fans_out_across_the_whole_pool() {
        let h = pool(
            "import os,sys,json,time; time.sleep(0.3); sys.stdout.write(json.dumps({'pid': os.getpid()}))",
            RunnerMode::Warm,
            4,
        );
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..4 {
            let h = h.clone();
            set.spawn(async move {
                h.run(r#"{"body":{}}"#.into(), Duration::from_millis(10_000))
                    .await
            });
        }
        let mut pids = std::collections::BTreeSet::new();
        while let Some(r) = set.join_next().await {
            let v: meclaw_core::serde_json::Value =
                meclaw_core::serde_json::from_str(&stdout_of(&r.unwrap())).unwrap();
            pids.insert(v["pid"].as_i64().unwrap());
        }
        assert_eq!(
            pids.len(),
            4,
            "four concurrent jobs must meet four children"
        );
    }
}
