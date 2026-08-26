//! One warm child: a task that owns exactly one process and answers one job at
//! a time.
//!
//! The shape is the `mcp` stdio precedent with the correlation map removed: a
//! runner child answers strictly in order and never speaks unbidden, so a
//! request is a write followed by the next frame. Everything that makes owning
//! a child process safe -- `kill_on_drop`, the sandbox scope, the orphan
//! journal entry, the reap -- comes from `crate::stdio_child` unchanged.

use crate::code::harness;
use crate::code::params::{CodeParams, RunnerMode};
use crate::code::pool::{Job, JobOutcome};
use crate::process::KillingTimeoutOutput;
use crate::stdio_child::{ChildExit, ChildSpec, Frame, StdioChild, StdioChildError};
use meclaw_core::serde_json::Value as JsonValue;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Grace between stdin-close and SIGKILL when a warm child is retired.
///
/// Short on purpose: the harness exits on stdin EOF within a few milliseconds,
/// and a killed child is being REPLACED, so waiting on it delays the next
/// message for nothing.
const KILL_GRACE_MS: u64 = 200;

/// Everything a child task needs to build -- and rebuild -- its process.
#[derive(Clone)]
pub(crate) struct ChildConfig {
    /// How to start the runner with the harness.
    pub(crate) spec: ChildSpec,
    /// The first line the child reads (script + namespace mode).
    pub(crate) boot: JsonValue,
}

/// The child configuration a `warm`/`resident` cell needs; `None` for `cold`.
pub(crate) fn config_for(p: &CodeParams) -> Option<ChildConfig> {
    let persistent = match p.runner_mode {
        RunnerMode::Cold => return None,
        RunnerMode::Warm => false,
        RunnerMode::Resident => true,
    };
    Some(ChildConfig {
        spec: ChildSpec {
            program: p.runner.clone(),
            args: vec!["-c".into(), harness::HARNESS.to_string()],
            env: Vec::new(),
            cwd: None,
            kill_grace_ms: KILL_GRACE_MS,
            // Both false because that is what a cold spawn does. A warm child
            // must differ from a cold one in lifetime and in nothing else.
            process_group: false,
            env_clear: false,
            sandbox: p.sandbox.clone().map(Box::new),
        },
        boot: harness::boot_frame(&p.script, persistent),
    })
}

/// The child task: pull one job, serve it, report the slot free.
pub(crate) async fn child_task(
    slot: usize,
    cfg: ChildConfig,
    mut jobs: mpsc::Receiver<Job>,
    idle: mpsc::Sender<usize>,
) {
    // The child is spawned on the FIRST job, not here: a colony full of warm
    // cells that nobody talks to must not hold a python process each.
    let mut child: Option<StdioChild> = None;
    while let Some(Job {
        document,
        timeout,
        reply,
    }) = jobs.recv().await
    {
        let outcome = serve(&cfg, &mut child, &document, timeout).await;
        let _ = reply.send(outcome);
        if idle.send(slot).await.is_err() {
            break;
        }
    }
    retire(&mut child, &cfg).await;
}

/// Run one job on this slot's child, spawning or replacing it as needed.
///
/// **The retry rule, and why it is exactly this one.** A failed WRITE means the
/// request never reached the child, so nothing ran and one respawn-and-retry is
/// free of consequence. A failed READ means the opposite: the body may have run
/// and had its side effects, so retrying would duplicate them silently. Hence
/// exactly one retry, and only on the write.
async fn serve(
    cfg: &ChildConfig,
    child: &mut Option<StdioChild>,
    document: &str,
    timeout: Duration,
) -> JobOutcome {
    // ONE deadline for the whole job (rule 12: the A-timeout is per EXECUTION,
    // so a respawn-and-retry must not silently double the budget).
    let deadline = Instant::now() + timeout;
    for attempt in 0..2u8 {
        if child.is_none() {
            match boot(cfg, deadline).await {
                Ok(c) => *child = Some(c),
                Err(e) => {
                    return JobOutcome::Io(format!("warm runner did not start: {}", e.detail()));
                }
            }
        }
        // The child borrow is re-taken per step rather than held across the
        // whole body: `retire`/`reap_as_output` need the OPTION, and a borrow
        // that outlived the write would make that impossible.
        let written = {
            let Some(live) = child.as_mut() else {
                return JobOutcome::Io("warm runner slot is empty".into());
            };
            tokio::time::timeout_at(deadline, live.write_raw(document)).await
        };
        match written {
            Ok(Ok(())) => {}
            Ok(Err(_)) if attempt == 0 => {
                retire(child, cfg).await;
                continue;
            }
            Ok(Err(e)) => {
                return JobOutcome::Io(format!("warm runner write failed: {}", e.detail()));
            }
            Err(_) => {
                retire(child, cfg).await;
                return JobOutcome::Timeout;
            }
        }
        let answered = {
            let Some(live) = child.as_mut() else {
                return JobOutcome::Io("warm runner slot is empty".into());
            };
            tokio::time::timeout_at(deadline, read_answer(live)).await
        };
        return match answered {
            Ok(Ok(out)) => JobOutcome::Ran(out),
            // The child died mid-run. Its exit status is exactly what a cold
            // run would have reported, so report that and let the shared tail
            // classify it (`script_failed` / `invalid_json`).
            Ok(Err(_)) => JobOutcome::Ran(reap_as_output(child, cfg).await),
            Err(_) => {
                retire(child, cfg).await;
                JobOutcome::Timeout
            }
        };
    }
    JobOutcome::Io("warm runner could not be reached".into())
}

/// Spawn the process and hand it the boot frame. No answer is expected.
async fn boot(cfg: &ChildConfig, deadline: Instant) -> Result<StdioChild, StdioChildError> {
    let mut child = StdioChild::spawn(&cfg.spec)?;
    let left = deadline.saturating_duration_since(Instant::now());
    child.write_frame(&cfg.boot, left).await?;
    Ok(child)
}

/// Read frames until one is JSON. A non-JSON line is diagnostics, never fatal:
/// the harness writes only frames, so anything else came past it (a C library
/// printing on fd 1) and must not end the run.
async fn read_answer(child: &mut StdioChild) -> Result<KillingTimeoutOutput, StdioChildError> {
    loop {
        match child.read_frame().await? {
            Some(Frame::Json(v)) => {
                return harness::output_from_frame(&v).map_err(StdioChildError::Read);
            }
            Some(Frame::Malformed(_)) => continue,
            None => return Err(StdioChildError::ChildGone(ChildExit::Eof)),
        }
    }
}

/// End the child and report its fate the way a cold run reports a killed
/// script: the exit code the process actually had, empty pipes.
async fn reap_as_output(child: &mut Option<StdioChild>, cfg: &ChildConfig) -> KillingTimeoutOutput {
    let exit = match child.take() {
        Some(c) => {
            c.terminate(Duration::from_millis(cfg.spec.kill_grace_ms))
                .await
        }
        None => ChildExit::Eof,
    };
    KillingTimeoutOutput {
        // `-1` for abnormal termination is the cold convention
        // (`crate::process::KillingTimeoutOutput`, Brainstorm-Decision 1.6).
        exit_code: match exit {
            ChildExit::Code(c) => c,
            _ => -1,
        },
        stdout: Vec::new(),
        stderr: Vec::new(),
    }
}

/// Drop the child so the next job boots a fresh one.
async fn retire(child: &mut Option<StdioChild>, cfg: &ChildConfig) {
    if let Some(c) = child.take() {
        let _ = c
            .terminate(Duration::from_millis(cfg.spec.kill_grace_ms))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::params::{CodeParams, RunnerMode, Script};
    use meclaw_core::serde_json::json;
    use tokio::sync::oneshot;

    fn params(script: &str, mode: RunnerMode) -> CodeParams {
        CodeParams {
            runner: "python3".into(),
            script: Script::Inline(script.into()),
            external_timeout_ms: Some(5_000),
            max_concurrency: None,
            sandbox: None,
            runner_mode: mode,
        }
    }

    /// Drive one child task with `docs` and collect the outcomes in order.
    async fn drive(p: &CodeParams, docs: &[&str]) -> Vec<JobOutcome> {
        let cfg = config_for(p).expect("a warm/resident params yields a config");
        let (jobs_tx, jobs_rx) = tokio::sync::mpsc::channel::<Job>(4);
        let (idle_tx, mut idle_rx) = tokio::sync::mpsc::channel::<usize>(4);
        let join = tokio::spawn(child_task(0, cfg, jobs_rx, idle_tx));
        let mut out = Vec::new();
        for d in docs {
            let (reply, rx) = oneshot::channel();
            jobs_tx
                .send(Job {
                    document: (*d).to_string(),
                    timeout: std::time::Duration::from_millis(5_000),
                    reply,
                })
                .await
                .unwrap();
            out.push(rx.await.expect("every job is answered"));
            assert_eq!(
                idle_rx.recv().await,
                Some(0),
                "the slot reports itself free"
            );
        }
        drop(jobs_tx);
        join.await.unwrap();
        out
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_jobs_run_in_the_same_process() {
        // The pid the body prints is the whole point of the mode.
        let p = params(
            "import os,sys,json; sys.stdout.write(json.dumps({'pid': os.getpid()}))",
            RunnerMode::Warm,
        );
        let outs = drive(&p, [r#"{"body":{}}"#, r#"{"body":{}}"#].as_slice()).await;
        let pid = |o: &JobOutcome| match o {
            JobOutcome::Ran(k) => {
                let v: meclaw_core::serde_json::Value =
                    meclaw_core::serde_json::from_slice(&k.stdout).unwrap();
                v["pid"].as_i64().unwrap()
            }
            _ => panic!("expected a run"),
        };
        assert_eq!(pid(&outs[0]), pid(&outs[1]), "the interpreter survived");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hanging_body_times_out_and_the_next_job_gets_a_fresh_child() {
        let p = params(
            "import os,sys,json,time\n\
             if json.load(sys.stdin)['body'].get('hang'): time.sleep(30)\n\
             sys.stdout.write(json.dumps({'pid': os.getpid()}))\n",
            RunnerMode::Warm,
        );
        let cfg = config_for(&p).unwrap();
        let (jobs_tx, jobs_rx) = tokio::sync::mpsc::channel::<Job>(4);
        let (idle_tx, mut idle_rx) = tokio::sync::mpsc::channel::<usize>(4);
        let _join = tokio::spawn(child_task(0, cfg, jobs_rx, idle_tx));
        let send = |doc: &str, ms: u64| {
            let (reply, rx) = oneshot::channel();
            let job = Job {
                document: doc.to_string(),
                timeout: std::time::Duration::from_millis(ms),
                reply,
            };
            let tx = jobs_tx.clone();
            async move {
                tx.send(job).await.unwrap();
                rx.await.unwrap()
            }
        };
        let first = send(r#"{"body":{"hang":true}}"#, 300).await;
        assert!(
            matches!(first, JobOutcome::Timeout),
            "the A-timeout answers"
        );
        assert_eq!(idle_rx.recv().await, Some(0));
        let second = send(r#"{"body":{}}"#, 5_000).await;
        assert!(
            matches!(second, JobOutcome::Ran(ref k) if k.exit_code == 0),
            "the killed child was replaced, the pool did not wedge"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_child_that_died_while_idle_is_replaced_without_losing_the_job() {
        let p = params(
            "import os,sys,json\n\
             d = json.load(sys.stdin)\n\
             sys.stdout.write(json.dumps({'n': d['body']['n']}))\n",
            RunnerMode::Warm,
        );
        let outs = drive(&p, [r#"{"body":{"n":1}}"#].as_slice()).await;
        assert!(matches!(outs[0], JobOutcome::Ran(_)));
        // The dedicated kill path is pinned end-to-end in Task 10; here the
        // point is only that a first job on a cold slot boots a child.
    }

    #[tokio::test]
    async fn a_cold_params_yields_no_child_config() {
        assert!(config_for(&params("pass", RunnerMode::Cold)).is_none());
    }

    #[tokio::test]
    async fn the_child_spec_carries_the_declared_sandbox_and_nothing_else() {
        let mut p = params("pass", RunnerMode::Warm);
        p.sandbox = Some(crate::sandbox::SandboxProfile::Trusted);
        let cfg = config_for(&p).unwrap();
        assert_eq!(cfg.spec.program, "python3");
        assert_eq!(cfg.spec.args[0], "-c");
        assert_eq!(cfg.spec.args[1], crate::code::harness::HARNESS);
        assert_eq!(
            cfg.spec.sandbox.as_deref(),
            Some(&crate::sandbox::SandboxProfile::Trusted),
            "the same profile a cold spawn applies"
        );
        assert!(!cfg.spec.env_clear, "cold does not clear the env either");
        assert!(
            !cfg.spec.process_group,
            "cold spawns no process group either"
        );
        assert_eq!(cfg.boot["persistent"], json!(false));
    }
}
