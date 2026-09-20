//! CDP over a pipe: no port, no socket, no new dependency.
//!
//! Chromium's `--remote-debugging-pipe` speaks the Chrome DevTools Protocol
//! over two file descriptors, 3 in and 4 out, with NUL-separated JSON on both.
//! `std` and `tokio` hand a child three descriptors and no way to ask for two
//! more, so the child is started through `/bin/sh`, which has that way:
//!
//! ```text
//! sh -c 'exec "$0" "$@" 3<&0 4>&1 1>&2' <browser> <flags…>
//! ```
//!
//! `3<&0` puts the child's stdin on fd 3, `4>&1` puts its stdout on fd 4, and
//! `1>&2` — **after** the other two, which is the whole trick — moves the
//! browser's own chatter to stderr, where the cell's log picks it up. Measured
//! against the packaged browser on 2026-09-18: `Browser.getVersion` answers
//! over it (OR-G5). The alternative, a WebSocket on a loopback port, would open
//! a port and need a crate, and this substrate adds neither.
//!
//! # Three tasks, and why not fewer (OR-G35)
//!
//! `AsyncBufReadExt::read_until` is **not cancel-safe**: dropped mid-read it
//! takes the bytes it had already consumed with it. So it never sits in a
//! `select!` arm. The reader is a task of its own that does nothing but frame,
//! the mux is a task that owns the correlation table and the write half, and
//! the I/O half of the cell is the third. They share nothing but channels — a
//! `Mutex` around the correlation table would be the forbidden shape and would
//! not even solve the cancel problem.

use crate::browser::error::BrowserError;
use crate::browser::params::BrowserParams;
use crate::stdio_child::{ChildReaper, ChildSpec, StdioChild};
use meclaw_core::serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, oneshot};

/// The shell line that moves the pipes onto fd 3 and fd 4.
///
/// The ORDER is load-bearing: `1>&2` has to come last, or fd 4 would end up
/// pointing at stderr and the browser's answers would arrive in the log.
pub const FD_SHIM: &str = "exec \"$0\" \"$@\" 3<&0 4>&1 1>&2";

/// How many framed messages may queue between the reader and the mux.
const INBOUND_QUEUE: usize = 256;

/// How many calls may be in flight before a caller waits for room.
const REQUEST_QUEUE: usize = 64;

/// One thing the browser said.
#[derive(Debug, Clone, PartialEq)]
pub enum CdpIn {
    /// An answer to a call this cell made.
    Reply {
        /// The id the call carried.
        id: u64,
        /// The `result` object, or the browser's own error text.
        result: Result<Value, String>,
    },
    /// Something the browser said on its own.
    Event(CdpEvent),
}

/// One CDP event.
#[derive(Debug, Clone, PartialEq)]
pub struct CdpEvent {
    /// `Target.targetInfoChanged`, `Page.loadEventFired`, …
    pub method: String,
    /// Which attached session it belongs to, when it belongs to one.
    pub session_id: Option<String>,
    /// The event's own parameters.
    pub params: Value,
}

/// The mux's correlation table: a call id, when its waiter stops being worth
/// keeping, and where the answer goes.
type Pending = HashMap<u64, (tokio::time::Instant, oneshot::Sender<Result<Value, String>>)>;

/// One call on its way to the mux.
struct CdpRequest {
    method: &'static str,
    params: Value,
    session_id: Option<String>,
    answer: oneshot::Sender<Result<Value, String>>,
}

/// The writing end of a browser's pipe.
///
/// It holds no id counter and no table: the mux owns both, so this value has no
/// interior mutability at all and `call` takes `&self` honestly.
pub struct CdpPipe {
    requests: mpsc::Sender<CdpRequest>,
    external_timeout: Duration,
}

impl CdpPipe {
    /// One CDP round trip, under the A-timeout (`params.external_timeout_ms`).
    ///
    /// Every round trip, without exception: this cell is long-running, so
    /// `cell.timeout` is `-1` and there is no message-timeout backstop behind
    /// it. A call nobody answers is `cdp_timeout` and names its method.
    pub async fn call(
        &self,
        method: &'static str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, BrowserError> {
        let (answer, wait) = oneshot::channel();
        let request = CdpRequest {
            method,
            params,
            session_id: session_id.map(str::to_string),
            answer,
        };
        // ONE deadline over both halves, not one each: two `external_timeout_ms`
        // in a row let a call take twice what the operator wrote, and the
        // sentence it ends with says the A-timeout ran out.
        let deadline = tokio::time::Instant::now() + self.external_timeout;
        match tokio::time::timeout_at(deadline, self.requests.send(request)).await {
            Err(_) => {
                return Err(BrowserError::CdpTimeout {
                    method: method.to_string(),
                });
            }
            Ok(Err(_)) => {
                return Err(BrowserError::BrowserCrashed(format!(
                    "{method}: the browser is gone"
                )));
            }
            Ok(Ok(())) => {}
        }
        match tokio::time::timeout_at(deadline, wait).await {
            Err(_) => Err(BrowserError::CdpTimeout {
                method: method.to_string(),
            }),
            // The mux drops a waiter it cannot answer, which is what the death
            // of the browser looks like from here.
            Ok(Err(_)) => Err(BrowserError::BrowserCrashed(format!(
                "{method}: the browser died while the call was outstanding"
            ))),
            Ok(Ok(Err(e))) => Err(BrowserError::NavigateFailed(format!("{method}: {e}"))),
            Ok(Ok(Ok(v))) => Ok(v),
        }
    }
}

/// The command line this cell starts its browser with.
///
/// Ours first, the operator's after: `extra_args` cannot overwrite a flag the
/// cell owns, because the parser refuses those outright
/// (`params::check_flag`), and appending keeps the refusal the only rule there
/// is. **No sandbox flag appears here at all** (R-G11): the browser's sandbox
/// belongs to the package it came from, and this cell neither switches it on
/// nor off.
pub fn flags(p: &BrowserParams, profile: &Path) -> Vec<String> {
    let profile = profile.display();
    let mut out: Vec<String> = [
        "--headless=new",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-gpu",
        "--hide-scrollbars",
        "--mute-audio",
        "--disable-extensions",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-sync",
        "--metrics-recording-only",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    out.push(format!("--user-data-dir={profile}"));
    out.push(format!("--disk-cache-dir={profile}/cache"));
    out.push("--remote-debugging-pipe".to_string());
    out.extend(p.extra_args.iter().cloned());
    out
}

/// The `ChildSpec` for one browser: the shim, the binary, the flags.
pub fn child_spec(p: &BrowserParams, profile: &Path) -> ChildSpec {
    let mut args = vec![
        "-c".to_string(),
        FD_SHIM.to_string(),
        p.chromium_path.clone(),
    ];
    args.extend(flags(p, profile));
    ChildSpec {
        program: "/bin/sh".to_string(),
        args,
        // Its own group, because a browser is a process tree: one zygote, one
        // renderer per site, a gpu process. Reaping the leader alone would
        // leave every one of them.
        process_group: true,
        kill_grace_ms: 2_000,
        sandbox: p.sandbox.clone().map(Box::new),
        ..Default::default()
    }
}

/// Start a browser and the two tasks that talk to it.
///
/// Synchronous and await-free on purpose: this runs inside the restart
/// corridor, where an `.await` is a deadlock waiting for a restart.
pub fn spawn_browser(
    p: &BrowserParams,
    profile: &Path,
) -> Result<(CdpPipe, mpsc::Receiver<CdpEvent>, ChildReaper), BrowserError> {
    std::fs::create_dir_all(profile).map_err(|e| {
        BrowserError::SpawnFailed(format!(
            "could not create the profile directory {}: {e}",
            profile.display()
        ))
    })?;
    let spec = child_spec(p, profile);
    let child = StdioChild::spawn(&spec).map_err(|e| BrowserError::SpawnFailed(e.detail()))?;
    let (pipes, reaper) = child.split();

    let (inbound_tx, inbound_rx) = mpsc::channel::<CdpIn>(INBOUND_QUEUE);
    let (requests_tx, requests_rx) = mpsc::channel::<CdpRequest>(REQUEST_QUEUE);
    let (events_tx, events_rx) = mpsc::channel::<CdpEvent>(INBOUND_QUEUE);

    tokio::spawn(read_frames(pipes.stdout, inbound_tx));
    tokio::spawn(mux(
        pipes.stdin,
        inbound_rx,
        requests_rx,
        events_tx,
        Duration::from_millis(p.external_timeout_ms),
    ));

    Ok((
        CdpPipe {
            requests: requests_tx,
            external_timeout: Duration::from_millis(p.external_timeout_ms),
        },
        events_rx,
        reaper,
    ))
}

/// The reader task: frame on NUL, parse, hand on. Nothing else.
///
/// It exists as its own task because `read_until` is not cancel-safe and must
/// therefore never share a `select!` with anything. When the pipe ends, this
/// task ends, the channel closes, and the mux reads that as the browser's
/// death.
async fn read_frames(
    mut out: BufReader<tokio::process::ChildStdout>,
    inbound: mpsc::Sender<CdpIn>,
) {
    let mut buf = Vec::with_capacity(8 * 1024);
    loop {
        buf.clear();
        match out.read_until(0, &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if buf.last() == Some(&0) {
            buf.pop();
        }
        if buf.is_empty() {
            continue;
        }
        let Ok(value) = meclaw_core::serde_json::from_slice::<Value>(&buf) else {
            // Not our frame to repair. A browser writes JSON on this pipe and
            // its chatter on stderr, so anything else here is noise.
            tracing::warn!("browser: a frame on the CDP pipe was not JSON");
            continue;
        };
        let framed = match value.get("id").and_then(Value::as_u64) {
            Some(id) => CdpIn::Reply {
                id,
                result: match value.get("error") {
                    Some(e) => Err(e
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or(&e.to_string())
                        .to_string()),
                    None => Ok(value.get("result").cloned().unwrap_or(json!({}))),
                },
            },
            None => CdpIn::Event(CdpEvent {
                method: value
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                session_id: value
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                params: value.get("params").cloned().unwrap_or(json!({})),
            }),
        };
        if inbound.send(framed).await.is_err() {
            return;
        }
    }
}

/// The mux task: hand out ids, write calls, match answers, forward events.
///
/// It owns the write half and the correlation table, which is why neither needs
/// a lock: one task, one owner. When the reader ends, every waiter is dropped,
/// and a dropped waiter is what `CdpPipe::call` reads as the browser's death —
/// so nothing is left hanging on a browser that is gone.
async fn mux(
    mut stdin: ChildStdin,
    mut inbound: mpsc::Receiver<CdpIn>,
    mut requests: mpsc::Receiver<CdpRequest>,
    events: mpsc::Sender<CdpEvent>,
    external_timeout: Duration,
) {
    let mut next_id: u64 = 1;
    // Each waiter with the moment it stops being worth keeping. Without it the
    // table only ever shrank on an ANSWER, and a call that ran into its
    // A-timeout left its row behind — `Runtime.evaluate` for `content_type` is
    // allowed to go unanswered by contract, so a silent target leaked one row
    // per page load for the life of the browser.
    let mut pending: Pending = HashMap::new();
    // Twice the caller's own deadline, so the caller's `cdp_timeout` is always
    // the sentence that gets written: a waiter dropped from here reads as "the
    // browser died while the call was outstanding", which would be a lie about
    // a browser that is merely slow.
    let keep = external_timeout.saturating_mul(2);
    loop {
        sweep(&mut pending, tokio::time::Instant::now());
        tokio::select! {
            framed = inbound.recv() => match framed {
                None => return,
                Some(CdpIn::Reply { id, result }) => {
                    if let Some((_, waiter)) = pending.remove(&id) {
                        let _ = waiter.send(result);
                    }
                }
                Some(CdpIn::Event(event)) => {
                    // Two rules in one line. A closed event channel is not the
                    // end of the browser: the calls still have to be answered,
                    // and a mux that returned here would fail every one of them
                    // with "the browser died" — about a browser that is fine.
                    // And a FULL one is not a reason to wait: this task owns the
                    // reply path, so a blocked send here runs every outstanding
                    // call into its timeout. The picture of one page is worth
                    // less than every answer of every page.
                    if let Err(tokio::sync::mpsc::error::TrySendError::Full(e)) =
                        events.try_send(event)
                    {
                        tracing::warn!(
                            method = %e.method,
                            "browser: the cell is behind on CDP events; one was dropped"
                        );
                    }
                }
            },
            request = requests.recv() => {
                let Some(request) = request else { return };
                let id = next_id;
                next_id += 1;
                let mut frame = json!({
                    "id": id,
                    "method": request.method,
                    "params": request.params,
                });
                if let Some(session) = request.session_id {
                    frame["sessionId"] = Value::String(session);
                }
                let mut bytes = frame.to_string().into_bytes();
                bytes.push(0);
                if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
                    // The pipe is gone. Dropping the waiter is the answer, and
                    // the reader's own end will close this loop.
                    return;
                }
                pending.insert(id, (tokio::time::Instant::now() + keep, request.answer));
            }
        }
    }
}

/// The correlation table's own end: drop every waiter whose time is up.
///
/// A row used to leave the table only on an ANSWER. `Runtime.evaluate` for
/// `content_type` is allowed by contract to go unanswered, so a silent target
/// leaked one row per page load for as long as the browser lived. Returns how
/// many went, which is what the test reads.
fn sweep(pending: &mut Pending, now: tokio::time::Instant) -> usize {
    let before = pending.len();
    pending.retain(|_, (until, _)| *until > now);
    before - pending.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> BrowserParams {
        BrowserParams::parse(&json!({
            "chromium_path": "/usr/bin/chromium",
            // Required since OR-G54.
            "sandbox": {"trust": "restricted", "network": "allow",
                        "limits": {"pids_max": 512}},
        }))
        .expect("params")
    }

    #[tokio::test]
    async fn a_call_that_was_never_answered_leaves_the_table() {
        let now = tokio::time::Instant::now();
        let mut pending: HashMap<
            u64,
            (tokio::time::Instant, oneshot::Sender<Result<Value, String>>),
        > = HashMap::new();
        let mut waiters = Vec::new();
        for (id, at) in [
            (1u64, now - Duration::from_secs(1)),
            (2, now - Duration::from_millis(1)),
            (3, now + Duration::from_secs(60)),
        ] {
            let (tx, rx) = oneshot::channel();
            pending.insert(id, (at, tx));
            waiters.push((id, rx));
        }
        assert_eq!(
            sweep(&mut pending, now),
            2,
            "two were past their keep-alive"
        );
        assert_eq!(pending.len(), 1);
        assert!(pending.contains_key(&3), "and the live one stayed");
        for (id, mut rx) in waiters {
            // `try_recv`, not `await`: the waiter that is still in the table
            // has a live sender, and awaiting it would wait for an answer that
            // by construction never comes.
            let closed = matches!(rx.try_recv(), Err(oneshot::error::TryRecvError::Closed));
            assert_eq!(
                closed,
                id != 3,
                "a swept waiter is a dropped sender, and nothing else is"
            );
        }
        // Nothing to do twice: the table only shrinks on an answer or on time.
        assert_eq!(sweep(&mut pending, now), 0);
    }
    #[test]
    fn the_shim_puts_the_answers_on_four_and_the_chatter_on_stderr() {
        // The order is the trick and the only thing worth pinning about the
        // string: `1>&2` after `4>&1`, or fd 4 lands on stderr.
        let three = FD_SHIM.find("3<&0").expect("stdin goes to fd 3");
        let four = FD_SHIM.find("4>&1").expect("stdout goes to fd 4");
        let chatter = FD_SHIM.find("1>&2").expect("the chatter goes to stderr");
        assert!(three < chatter && four < chatter, "{FD_SHIM}");
    }

    #[test]
    fn no_sandbox_flag_is_added_by_this_cell() {
        let flags = flags(&params(), Path::new("/tmp/p"));
        assert!(
            !flags.iter().any(|f| f.contains("sandbox")),
            "the browser's sandbox belongs to its package (R-G11): {flags:?}"
        );
        assert!(flags.iter().any(|f| f == "--remote-debugging-pipe"));
        assert!(flags.iter().any(|f| f == "--user-data-dir=/tmp/p"));
        assert!(
            !flags
                .iter()
                .any(|f| f.starts_with("--remote-debugging-port"))
        );
    }

    #[test]
    fn the_child_is_the_shell_and_the_browser_is_its_argument() {
        let spec = child_spec(&params(), Path::new("/tmp/p"));
        assert_eq!(spec.program, "/bin/sh");
        assert_eq!(spec.args[0], "-c");
        assert_eq!(spec.args[1], FD_SHIM);
        assert_eq!(
            spec.args[2], "/usr/bin/chromium",
            "the binary is $0, so \"$@\" is the flags and nothing else"
        );
        assert!(spec.process_group, "a browser is a process tree");
    }
}

// ---- the sandbox check that replaces the knob (R-G13) ---------------------

/// What the post-spawn look at the browser's own children found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxVerdict {
    /// At least one of the browser's children is in a user namespace of its
    /// own. The package's sandbox is doing its work.
    Sandboxed,
    /// The browser has children and not one of them left this namespace.
    NotSandboxed,
    /// The browser has no children yet. Not a verdict — ask again.
    NoChildrenYet,
}

/// Does this browser's own child chain leave our user namespace?
///
/// The replacement for the sandbox knob R-G11 struck: instead of an operator
/// asserting that the browser is confined, the cell **looks** — at the
/// processes below the pid it remembers, never at a name pattern, because this
/// host runs other people's browsers and a pattern would find them.
///
/// The rule is "at least one child is elsewhere" rather than "every child is":
/// a real browser keeps helpers in the daemon's namespace on purpose — the
/// crash handler is one — while the zygote every renderer is forked from is the
/// one that unshares. A cell that refused on any shared child would refuse
/// every working browser; one that accepted on any unshared child would be the
/// knob again, in the affirmative.
pub fn renderers_are_sandboxed(pid: u32) -> SandboxVerdict {
    let Ok(ours) = std::fs::read_link("/proc/self/ns/user") else {
        // No procfs namespace links: nothing can be observed here, and the
        // caller turns that into a refusal rather than a shrug.
        return SandboxVerdict::NoChildrenYet;
    };
    let children = descendants_of(pid);
    if children.is_empty() {
        return SandboxVerdict::NoChildrenYet;
    }
    for child in children {
        match std::fs::read_link(format!("/proc/{child}/ns/user")) {
            Ok(theirs) if theirs != ours => return SandboxVerdict::Sandboxed,
            // A child that vanished between the listing and the read says
            // nothing either way.
            _ => {}
        }
    }
    SandboxVerdict::NotSandboxed
}

/// Every process below `pid`, through `/proc/<pid>/task/*/children`.
///
/// Bounded: a browser's tree is tens of processes, and a runaway `/proc` walk
/// on a busy host is not worth the completeness. The pid is the one this cell
/// remembered at spawn, so the walk starts inside its own child chain and
/// cannot wander into somebody else's.
fn descendants_of(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut frontier = vec![pid];
    let mut seen = std::collections::HashSet::from([pid]);
    while let Some(parent) = frontier.pop() {
        if out.len() > 256 {
            break;
        }
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{parent}/task")) else {
            continue;
        };
        for task in tasks.flatten() {
            let Ok(list) = std::fs::read_to_string(task.path().join("children")) else {
                continue;
            };
            for child in list
                .split_whitespace()
                .filter_map(|s| s.parse::<u32>().ok())
            {
                if seen.insert(child) {
                    out.push(child);
                    frontier.push(child);
                }
            }
        }
    }
    out
}

// ---- what the cap did, after the fact -------------------------------------

/// The cgroup directory a process sits in, if this host publishes one.
///
/// Read once, while the process is alive: the answer is needed exactly when it
/// is not any more.
pub fn cgroup_of(pid: u32) -> Option<std::path::PathBuf> {
    crate::sandbox::cgroup_of(pid)
}

/// How many times the kernel OOM-killed something in `cgroup` or below it.
///
/// The one number that tells a memory cap apart from a crash. Without it an
/// operator reads "the browser died" and has no way to know that the cap they
/// wrote is what killed it — which is the whole reason the cap is allowed to be
/// written at all.
pub fn oom_kills(cgroup: &std::path::Path) -> Option<u64> {
    crate::sandbox::oom_kills(cgroup)
}

/// Where a browser's ceiling ended up, and what can still answer for it
/// afterwards.
///
/// Two fields and not one, because the directory that carries the ceiling is
/// not always the directory that survives the browser. A cgroup this cell
/// created lives as long as its scope value does. A scope the service manager
/// created for a re-homed child is REMOVED with its last process — measured
/// on 2026-09-20: after the ceiling killed the browser, `memory.events` was
/// already gone when the pipe closed. So a moved child also remembers the
/// nearest cgroup above it, whose `oom_kill` counter is hierarchical, and the
/// count it carried before the ceiling was written.
#[derive(Debug, Clone)]
pub struct CapSite {
    /// The cgroup the browser really sat in, read while it was alive.
    pub dir: std::path::PathBuf,
    /// The surviving ancestor and its `oom_kill` count from before the
    /// ceiling was written; `None` when `dir` outlives the browser by itself.
    pub witness: Option<crate::sandbox::OomWitness>,
}

impl CapSite {
    /// A site that is its own witness: a cgroup this cell owns.
    pub fn at(dir: std::path::PathBuf) -> Self {
        Self { dir, witness: None }
    }

    /// Kills the ceiling caused, read after the death.
    ///
    /// The child's own cgroup first, because its count is about this browser
    /// and nothing else. Only once that directory is gone does the ancestor
    /// answer, by the delta since the ceiling was written — which can name one
    /// kill too many when a sibling under the same ancestor died at the same
    /// time, and never one too few.
    fn kills(&self) -> Option<u64> {
        if let Some(n) = oom_kills(&self.dir) {
            return Some(n);
        }
        let (ancestor, before) = self.witness.as_ref()?;
        Some(oom_kills(ancestor)?.saturating_sub(*before))
    }
}

/// How a dead browser is described, with the cap's own verdict when there is
/// one.
pub fn death_detail(site: Option<&CapSite>, cause: &str) -> String {
    match site.and_then(CapSite::kills).filter(|n| *n > 0) {
        Some(n) => format!("{cause}; oom_kill={n} — params.sandbox.limits.memory_max_bytes"),
        None => cause.to_string(),
    }
}

#[cfg(test)]
mod cap_tests {
    use super::*;

    #[test]
    fn a_cgroup_without_a_kill_says_nothing_extra() {
        let td = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            td.path().join("memory.events"),
            "low 0\nhigh 0\nmax 12\noom 0\noom_kill 0\n",
        )
        .expect("events");
        assert_eq!(oom_kills(td.path()), Some(0));
        assert_eq!(
            death_detail(
                Some(&CapSite::at(td.path().to_path_buf())),
                "the browser's pipe closed"
            ),
            "the browser's pipe closed"
        );
    }

    #[test]
    fn a_cap_that_killed_says_so_and_names_the_knob() {
        let td = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            td.path().join("memory.events"),
            "low 0\nhigh 4\nmax 40\noom 3\noom_kill 2\n",
        )
        .expect("events");
        assert_eq!(oom_kills(td.path()), Some(2));
        let said = death_detail(
            Some(&CapSite::at(td.path().to_path_buf())),
            "the browser's pipe closed",
        );
        assert!(said.contains("oom_kill=2"), "{said}");
        assert!(
            said.contains("memory_max_bytes"),
            "an operator reading this has to know which knob did it: {said}"
        );
    }

    #[test]
    fn a_host_without_the_file_is_not_a_verdict() {
        let td = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(oom_kills(td.path()), None);
        assert_eq!(death_detail(None, "killed by signal"), "killed by signal");
    }

    #[test]
    fn our_own_cgroup_is_readable_where_this_host_publishes_one() {
        // Not an assertion about the host: a container without cgroup v2 has no
        // such file, and this says so rather than going red.
        match cgroup_of(std::process::id()) {
            Some(path) => assert!(path.starts_with("/sys/fs/cgroup"), "{path:?}"),
            None => eprintln!("[cgroup_of] SKIPPED: this host publishes no cgroup v2 path"),
        }
    }
}
