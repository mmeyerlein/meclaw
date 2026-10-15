//! The Rust side's own hang marker over a browser driver's run (re-check tv-bn-layout B,
//! M1/M1-R; orchestrator decision on tv-bn-lb.2).
//!
//! Rust leads the number: the run's marker is `meclaw_testing::deadline::BROWSER_RUN_*`,
//! scaled by the lane, and goes to the driver as `MECLAW_WEBKIT_RUN_MS`, so the driver
//! ends a hung run itself and names the proof it hung in. What runs OUTSIDE that marker
//! -- node's start, the browser's launch, `browser.close()` and the report after it -- had
//! nothing over it but the harness kill (`.config/nextest.toml`), and a kill prints no
//! diagnosis. This marker sits over the whole child: the run's marker plus
//! [`outside`]. On expiry the child gets SIGTERM first (the driver then reports
//! `stopped by SIGTERM` and closes its browser), a HANG mark later it and everything it
//! started get SIGKILL, the marker checks under a HANG mark that every one of them is
//! gone, and the test reports `HANG:` with the survivors, the last state line and the
//! tail of stderr. The nextest leash stays the net behind it
//! (`708_... the_run_marker_sits_under_the_leash_with_room` holds the order).
//!
//! A hang mark, never a verdict: a run that ends, ends with its own output, however long
//! it took under the mark.
#![allow(dead_code)]

use std::fmt;
use std::process::{ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use meclaw_testing::deadline::{
    BROWSER_RUN_COLONY, BROWSER_RUN_OUTSIDE, BROWSER_RUN_SHEET, FLOOR, HANG, scaled, time_factor,
};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// The driver's own run marker, as the driver reads it (`display-browser-lab.mjs`).
pub const RUN_MS_VAR: &str = "MECLAW_WEBKIT_RUN_MS";

/// The tag a marked child carries in its environment and hands to everything it starts:
/// how the marker finds a process that left the child's parent chain -- a `setsid`
/// daemon, a grandchild whose parent ended and that now hangs off init (re-check
/// tv-bn-layout B fix3 H1). Chromium and WebKit pass their environment on.
pub const RUN_TAG_VAR: &str = "MECLAW_RUN_TAG";

/// What runs outside the driver's run marker and inside this one
/// (`deadline::BROWSER_RUN_OUTSIDE`), scaled by the lane like every in-test deadline
/// (TV-Z3; re-check tv-bn-layout B fix2, M1).
pub fn outside() -> Duration {
    scaled(BROWSER_RUN_OUTSIDE)
}

/// The driver's run marker for one colony line, scaled by the lane.
pub fn colony_run() -> Duration {
    scaled(BROWSER_RUN_COLONY)
}

/// The driver's run marker for one part of the sheet line, scaled by the lane.
pub fn sheet_run() -> Duration {
    scaled(BROWSER_RUN_SHEET)
}

/// A child that outlived its marker: SIGTERM first, SIGKILL for it and everything it
/// started a HANG mark later, and what it had written until then.
pub struct Hung {
    pub what: String,
    pub mark: Duration,
    pub factor: f64,
    /// `Some` when the child ended on the SIGTERM, with the status it ended with.
    pub ended_on_term: Option<ExitStatus>,
    /// The child itself had ended when the marker fired: what held its streams open was
    /// a process outside its parent chain, found by [`RUN_TAG_VAR`].
    pub child_had_ended: bool,
    /// The child, its descendants and every process carrying its [`RUN_TAG_VAR`], as
    /// found in `/proc` when the marker fired and again right before the SIGKILL.
    pub killed: Vec<u32>,
    /// The ones of `killed` still alive (same start time, not a zombie) a HANG mark
    /// after the SIGKILL -- empty when the tree is gone.
    pub survivors: Vec<u32>,
    /// What `kill` said when it could not send a signal (it did not start, or it failed
    /// for a process it was meant to reach).
    pub signal_errors: Vec<String>,
    /// Processes of the own user, started after the child, whose `environ` the tag scan
    /// could not read (a non-dumpable one answers EACCES): they may carry the tag, so
    /// `alive after it: []` is no all-clear while one is named here. Not killed --
    /// nothing proves they belong to the run.
    pub unscanned: Vec<u32>,
    /// Why a look into `/proc` failed (the directory, the child's `stat`, a blocking
    /// task): a scan that could not look is named, never read as "nothing there".
    pub scan_errors: Vec<String>,
    pub stdout: String,
    pub stderr: String,
}

/// The last `n` lines of `text`, or a note that there were none.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return "    (nothing)".to_string();
    }
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Hung {
    /// The last state the driver printed: its report (`{`), a `RUN ` or a `LOG ` line.
    pub fn last_state(&self) -> &str {
        self.stdout
            .lines()
            .rev()
            .find(|l| l.starts_with('{') || l.starts_with("RUN ") || l.starts_with("LOG "))
            .unwrap_or("(none printed)")
    }
}

impl fmt::Display for Hung {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let term = match self.ended_on_term {
            _ if self.child_had_ended => {
                "the child had ended; a process outside its tree holds its streams".to_string()
            }
            Some(s) => format!("it ended on SIGTERM ({s})"),
            None => "it did not end on SIGTERM".to_string(),
        };
        write!(
            f,
            "HANG: {} ran past {} s (its marker, factor {}); {term}, then SIGKILL to {:?}, \
             alive after it: {:?}{}{}{}\n\
             last state: {}\nlast lines on stderr:\n{}",
            self.what,
            self.mark.as_secs(),
            self.factor,
            self.killed,
            self.survivors,
            if self.signal_errors.is_empty() {
                String::new()
            } else {
                format!("; kill failed: {}", self.signal_errors.join(" | "))
            },
            if self.unscanned.is_empty() {
                String::new()
            } else {
                format!("; unscanned (environ unreadable): {:?}", self.unscanned)
            },
            if self.scan_errors.is_empty() {
                String::new()
            } else {
                format!("; scan failed: {}", self.scan_errors.join(" | "))
            },
            self.last_state(),
            tail(&self.stderr, 20),
        )
    }
}

/// Every chunk `r` yields, as it arrives, until it closes.
fn pump<R: AsyncRead + Unpin + Send + 'static>(mut r: R) -> mpsc::UnboundedReceiver<Vec<u8>> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// Both streams of a child, read into buffers while it runs.
struct Streams {
    out_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    err_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    out_open: bool,
    err_open: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Streams {
    /// Read both streams to their end, then wait for the child. Cancel-safe: what was
    /// read stays in the buffers when the caller's marker drops this future.
    async fn drain_to_end(&mut self, child: &mut Child) -> std::io::Result<ExitStatus> {
        while self.out_open || self.err_open {
            tokio::select! {
                c = self.out_rx.recv(), if self.out_open => match c {
                    Some(c) => self.stdout.extend(c),
                    None => self.out_open = false,
                },
                c = self.err_rx.recv(), if self.err_open => match c {
                    Some(c) => self.stderr.extend(c),
                    None => self.err_open = false,
                },
            }
        }
        child.wait().await
    }

    /// Read both streams until the child itself has ended -- not its streams: a process
    /// below it may still hold them.
    async fn until_exit(&mut self, child: &mut Child) -> std::io::Result<ExitStatus> {
        loop {
            tokio::select! {
                st = child.wait() => return st,
                c = self.out_rx.recv(), if self.out_open => match c {
                    Some(c) => self.stdout.extend(c),
                    None => self.out_open = false,
                },
                c = self.err_rx.recv(), if self.err_open => match c {
                    Some(c) => self.stderr.extend(c),
                    None => self.err_open = false,
                },
            }
        }
    }

    /// Read both streams to their end, the child aside: after the SIGKILL the last bytes
    /// may still be on their way through the pumps (re-check B fix2, H1). Cancel-safe.
    async fn drain_to_eof(&mut self) {
        while self.out_open || self.err_open {
            tokio::select! {
                c = self.out_rx.recv(), if self.out_open => match c {
                    Some(c) => self.stdout.extend(c),
                    None => self.out_open = false,
                },
                c = self.err_rx.recv(), if self.err_open => match c {
                    Some(c) => self.stderr.extend(c),
                    None => self.err_open = false,
                },
            }
        }
    }
}

/// One process as `/proc` names it: the pid and its start time (field 22 of `stat`), so
/// a pid the kernel has handed to a new process is never taken for the old one.
#[derive(Clone, Copy)]
struct Proc {
    pid: u32,
    start: u64,
}

/// State, parent pid and start time of `pid`, off `/proc/<pid>/stat`: `Ok(None)` when
/// the process is gone, `Err` when its `stat` could not be read or parsed -- a read that
/// failed is no "it ended" (re-check tv-bn M2, M1).
fn stat_read(pid: u32) -> Result<Option<(char, u32, u64)>, String> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("/proc/{pid}/stat: {e}")),
    };
    let parsed: Option<(char, u32, u64)> = (|| {
        let after = stat.get(stat.rfind(')')? + 2..)?;
        let fields: Vec<&str> = after.split_whitespace().collect();
        let state = fields.first()?.chars().next()?;
        Some((
            state,
            fields.get(1)?.parse().ok()?,
            fields.get(19)?.parse().ok()?,
        ))
    })();
    parsed
        .map(Some)
        .ok_or_else(|| format!("/proc/{pid}/stat does not parse: {stat:?}"))
}

/// [`stat_read`] where a process that can not be read counts as gone.
fn stat_of(pid: u32) -> Option<(char, u32, u64)> {
    stat_read(pid).ok().flatten()
}

/// Whether `p` still runs: the same process (same start time) and not a zombie. A
/// `stat` that can not be read counts as running (fail-closed: a survivor is named, an
/// unreadable one is no all-clear).
fn alive(p: Proc) -> bool {
    match stat_read(p.pid) {
        Ok(Some((state, _, start))) => start == p.start && state != 'Z' && state != 'X',
        Ok(None) => false,
        Err(_) => true,
    }
}

/// What a look into `/proc` found, what it could not look at, and why it failed.
#[derive(Default)]
struct Scan {
    procs: Vec<Proc>,
    unscanned: Vec<u32>,
    errors: Vec<String>,
}

impl Scan {
    /// A scan whose blocking task did not come back: nothing found, and that is said.
    fn failed(why: String) -> Self {
        Scan {
            errors: vec![why],
            ..Scan::default()
        }
    }

    /// `self`, then what `other` adds.
    fn and(mut self, other: Scan) -> Self {
        self.procs = joined(self.procs, other.procs);
        for p in other.unscanned {
            if !self.unscanned.contains(&p) {
                self.unscanned.push(p);
            }
        }
        for e in other.errors {
            if !self.errors.contains(&e) {
                self.errors.push(e);
            }
        }
        self
    }
}

/// The pids `/proc` lists; a directory (or an entry) that can not be read is an error
/// of the scan, never an empty list (re-check tv-bn M2, M1).
fn pids(errors: &mut Vec<String>) -> Vec<u32> {
    let dir = match std::fs::read_dir("/proc") {
        Ok(d) => d,
        Err(e) => {
            errors.push(format!("read_dir /proc: {e}"));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for entry in dir {
        match entry {
            Ok(e) => out.extend(e.file_name().to_str().and_then(|n| n.parse::<u32>().ok())),
            Err(e) => errors.push(format!("an entry of /proc: {e}")),
        }
    }
    out
}

/// `root` and every process below it, read off `/proc` (parent pid, field 4 of `stat`).
fn tree_of(root: u32) -> Scan {
    let mut errors = Vec::new();
    let mut table: Vec<(Proc, u32)> = Vec::new();
    for pid in pids(&mut errors) {
        match stat_read(pid) {
            Ok(Some((_, ppid, start))) => table.push((Proc { pid, start }, ppid)),
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    let Some(&(top, _)) = table.iter().find(|(p, _)| p.pid == root) else {
        return Scan {
            errors,
            ..Scan::default()
        };
    };
    let mut tree = vec![top];
    let mut i = 0;
    while i < tree.len() {
        let parent = tree[i].pid;
        tree.extend(
            table
                .iter()
                .filter(|(_, pp)| *pp == parent)
                .map(|(c, _)| *c),
        );
        i += 1;
    }
    Scan {
        procs: tree,
        unscanned: Vec::new(),
        errors,
    }
}

/// The real uid of `pid` (first field of `Uid:` in `/proc/<pid>/status`).
fn uid_of(pid: &str) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// A tag no other marked run carries: this test process's pid and a counter.
fn run_tag() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Every process whose environment carries `tag` (`/proc/<pid>/environ` reads for the
/// own user's processes). One of the own user, started at or after `since` (the
/// child's start), whose `environ` answers anything but NotFound -- a non-dumpable
/// process answers EACCES while `stat` and `status` stay readable -- may carry the tag
/// and is named `unscanned`, never skipped as "not tagged" (re-check tv-bn M2, M1).
/// Another user's process can not have inherited the tag of this test's child.
fn tagged(tag: &str, since: u64) -> Scan {
    let entry = format!("{RUN_TAG_VAR}={tag}");
    let mut scan = Scan::default();
    let me = uid_of("self");
    if me.is_none() {
        scan.errors
            .push("/proc/self/status names no uid: every unreadable environ counts".into());
    }
    for pid in pids(&mut scan.errors) {
        match std::fs::read(format!("/proc/{pid}/environ")) {
            Ok(env) => {
                if !env.split(|b| *b == 0).any(|kv| kv == entry.as_bytes()) {
                    continue;
                }
                match stat_read(pid) {
                    Ok(Some((_, _, start))) => scan.procs.push(Proc { pid, start }),
                    Ok(None) => {}
                    Err(e) => scan.errors.push(e),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                let own = match (me, uid_of(&pid.to_string())) {
                    (Some(me), Some(uid)) => uid == me,
                    _ => true,
                };
                let later = match stat_read(pid) {
                    Ok(Some((_, _, start))) => start >= since,
                    Ok(None) => false,
                    Err(_) => true,
                };
                if own && later {
                    scan.unscanned.push(pid);
                }
            }
        }
    }
    scan
}

/// `a`, then the ones of `b` it does not hold yet.
fn joined(mut a: Vec<Proc>, b: Vec<Proc>) -> Vec<Proc> {
    for p in b {
        if !a.iter().any(|q| q.pid == p.pid && q.start == p.start) {
            a.push(p);
        }
    }
    a
}

/// `kill -<signal>` to the ones of `procs` that still run (a pid that ended may already
/// name another process, re-check B fix2 H2) -- no new crate for a signal (`Cargo.toml`
/// stays). `Err` says why a signal did not go out: `kill` did not start, or it failed
/// for a process that was alive a moment before (one that ended in between is no error).
fn signal(sig: &'static str, procs: &[Proc]) -> Result<(), String> {
    let pids: Vec<u32> = procs.iter().filter(|p| alive(**p)).map(|p| p.pid).collect();
    if pids.is_empty() {
        return Ok(());
    }
    let out = std::process::Command::new("kill")
        // The ESRCH text is read below; a localised one would turn a process that ended
        // in between into a failed signal (re-check B fix3 H2).
        .env("LC_ALL", "C")
        .arg(format!("-{sig}"))
        .args(pids.iter().map(u32::to_string))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("kill -{sig} {pids:?} did not start: {e}"))?;
    let complaints: Vec<String> = String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.contains("No such process"))
        .map(str::to_string)
        .collect();
    if out.status.success() || complaints.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "kill -{sig} {pids:?} ({}): {}",
            out.status,
            complaints.join("; ")
        ))
    }
}

/// The ones of `procs` that still run once a HANG mark has passed or all have ended.
async fn survivors_of(procs: Vec<Proc>) -> Vec<u32> {
    let still = |procs: Vec<Proc>| async move {
        let all: Vec<u32> = procs.iter().map(|p| p.pid).collect();
        tokio::task::spawn_blocking(move || {
            procs
                .into_iter()
                .filter(|p| alive(*p))
                .map(|p| p.pid)
                .collect::<Vec<u32>>()
        })
        .await
        // A look that did not come back names every process (fail-closed).
        .unwrap_or(all)
    };
    let gone = meclaw_testing::wait::within_hang(|| {
        let procs = procs.clone();
        async move { still(procs).await.is_empty().then_some(()) }
    })
    .await;
    match gone {
        Some(()) => Vec::new(),
        None => still(procs).await,
    }
}

/// Run `cmd` to its end: the driver's run marker `run` goes in as [`RUN_MS_VAR`], and the
/// child runs under `run + outside`.
///
/// `Ok` is what `Command::output` gives -- the output of a child that ended, or the error
/// that it never started. `Err` is a child that ran past its marker (see [`Hung`]).
pub async fn run_marked(
    cmd: &mut Command,
    what: &str,
    run: Duration,
    outside: Duration,
) -> Result<std::io::Result<Output>, Box<Hung>> {
    let mark = run + outside;
    let tag = run_tag();
    let spawned = cmd
        .env(RUN_MS_VAR, run.as_millis().to_string())
        .env(RUN_TAG_VAR, &tag)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A dropped future does not take a child with it.
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return Ok(Err(e)),
    };
    // The child's start time, read while it surely runs: the floor below which no
    // process can have inherited its tag (`tagged`). Unknown = 0, every later process.
    let born = child.id();
    let since = tokio::task::spawn_blocking(move || born.and_then(stat_of).map(|s| s.2))
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    let mut s = Streams {
        out_rx: pump(child.stdout.take().expect("stdout is piped")),
        err_rx: pump(child.stderr.take().expect("stderr is piped")),
        out_open: true,
        err_open: true,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    match tokio::time::timeout(mark, s.drain_to_end(&mut child)).await {
        Ok(Ok(status)) => {
            return Ok(Ok(Output {
                status,
                stdout: s.stdout,
                stderr: s.stderr,
            }));
        }
        Ok(Err(e)) => return Ok(Err(e)),
        Err(_) => {}
    }
    // The tree is read BEFORE the SIGTERM: a parent that ends on it hands its children
    // to init, and they would no longer be found below it. The tag finds the ones that
    // were never below it or left it before the marker fired (re-check B fix3 H1).
    let pid = child.id();
    let first_tag = tag.clone();
    let (root, scan, child_had_ended) = tokio::task::spawn_blocking(move || {
        let mut errors = Vec::new();
        // `None` from `child.id()` is a child tokio has reaped; a `stat` that can not
        // be read is no "it ended" -- the SIGTERM grace stays (re-check tv-bn M2, M1).
        let ended = match pid.map(stat_read) {
            None | Some(Ok(None)) => true,
            Some(Ok(Some((state, _, _)))) => state == 'Z' || state == 'X',
            Some(Err(e)) => {
                errors.push(format!("the child: {e}"));
                false
            }
        };
        let below = pid.map(tree_of).unwrap_or_default();
        let root = below.procs.first().copied().filter(|r| Some(r.pid) == pid);
        let scan = Scan {
            errors,
            ..Scan::default()
        }
        .and(below)
        .and(tagged(&first_tag, since));
        (root, scan, ended)
    })
    .await
    .unwrap_or_else(|e| (None, Scan::failed(format!("the first look: {e}")), false));
    let mut signal_errors = Vec::new();
    if let Some(root) = root {
        let sent = tokio::task::spawn_blocking(move || signal("TERM", &[root])).await;
        signal_errors.extend(sent.map_err(|e| e.to_string()).and_then(|r| r).err());
    }
    let ended_on_term = if child_had_ended {
        None
    } else {
        match tokio::time::timeout(scaled(HANG), s.until_exit(&mut child)).await {
            Ok(Ok(status)) => Some(status),
            _ => None,
        }
    };
    // A second look right before the SIGKILL: what the child started in the SIGTERM's
    // grace, below it or tagged.
    let second_tag = tag.clone();
    let second = tokio::task::spawn_blocking(move || {
        let below = root
            .filter(|r| alive(*r))
            .map(|r| tree_of(r.pid))
            .unwrap_or_default();
        below.and(tagged(&second_tag, since))
    })
    .await
    .unwrap_or_else(|e| Scan::failed(format!("the second look: {e}")));
    let scan = scan.and(second);
    let tree = scan.procs.clone();
    let killed: Vec<u32> = tree.iter().map(|p| p.pid).collect();
    let to_kill = tree.clone();
    let sent = tokio::task::spawn_blocking(move || signal("KILL", &to_kill)).await;
    signal_errors.extend(sent.map_err(|e| e.to_string()).and_then(|r| r).err());
    if let Err(e) = child.start_kill() {
        // The child had ended already (`InvalidInput`) unless it says otherwise.
        if e.kind() != std::io::ErrorKind::InvalidInput {
            signal_errors.push(format!("start_kill: {e}"));
        }
    }
    // Every process of the tree is gone, or it is named: a SIGKILL that did not land
    // leaves a browser behind (re-check B fix2, M3). The survivors come from a look
    // AFTER the SIGKILL as well: a tagged process born between the second look and the
    // signal was neither killed nor named (re-check tv-bn M2, M1).
    let last_tag = tag.clone();
    let after = tokio::task::spawn_blocking(move || {
        let below = root
            .filter(|r| alive(*r))
            .map(|r| tree_of(r.pid))
            .unwrap_or_default();
        below.and(tagged(&last_tag, since))
    })
    .await
    .unwrap_or_else(|e| Scan::failed(format!("the look after the SIGKILL: {e}")));
    let scan = scan.and(after);
    let survivors = survivors_of(scan.procs.clone()).await;
    // The streams to their end (each pump hands over what it still holds), under a
    // floor mark: a survivor may hold them open.
    let _ = tokio::time::timeout(scaled(FLOOR), s.drain_to_eof()).await;
    Err(Box::new(Hung {
        what: what.to_string(),
        mark,
        factor: time_factor(),
        ended_on_term,
        child_had_ended,
        killed,
        survivors,
        signal_errors,
        unscanned: scan.unscanned,
        scan_errors: scan.errors,
        stdout: String::from_utf8_lossy(&s.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&s.stderr).into_owned(),
    }))
}
