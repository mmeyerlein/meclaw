//! The hive harnesses' python, run the way the shipped warm runner runs it
//! (GH #1048): one `python3 -c <harness>` child per script, the script
//! compiled once, then one stdin document per line and one answer frame back.
//! The harness is the substrate's own (`src/code/harness.py`, the code cell's
//! `runner_mode: warm`), so a body starts where `python3 -c <script>` starts --
//! the module table and `sys.path` of a cold run, a fresh globals dict per
//! message (GH #1026, locked with the harness) -- and the three values that
//! come back are the ones a cold run returns.
//!
//! WHY (measured on build04, 2026-10-07, one test at a time): the cold spawn
//! was the test. `gh904_the_curator_clock_keeps_one_row` spent 45.5 s of 48.0 s
//! in 564 python processes (policy 101 ms each, intake 73 ms, writer 44 ms --
//! start-up plus compiling a script of up to 156 KB, every hop again), and
//! `gh903 semantic_ops_resolve_every_address_form_...` 36.8 s of 38.4 s in 343
//! (derive 108 ms, write 102 ms). Under the strand gate's load the same two
//! took 108.7 s and 81.1 s, over the 80 s mark (1/3 of 30 s x 8), and every
//! other test on these two harnesses paid the same toll per hop.
//!
//! Every wait for a child ends at the hang mark (GH #1640): for the document to
//! be taken, for the answer, for the exit. One thread per child holds its two
//! pipes, so the waiting side can stop waiting, end the child and name it; a
//! pipe itself has no deadline. A stuck child was a silent test until the
//! runner's kill at 240 s, with nothing in the log to tell it from a slow lane.
#![allow(dead_code)]

use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_testing::deadline::{HANG, scaled};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

/// The substrate's warm/resident harness, byte for byte.
const HARNESS: &str = include_str!("../../src/code/harness.py");

/// Children kept per thread. A test that writes many distinct scripts (one
/// mutation per case) would otherwise keep one idle interpreter per script;
/// past this many they are all let go and booted again on demand.
const MAX_CHILDREN: usize = 16;

/// Where the thread of a child stands, for the words of a hang.
const STAGES: [&str; 3] = [
    "has not taken its boot frame",
    "has not taken its document",
    "has not answered its document",
];

/// The head of a text on one line, for the words of a hang.
fn head(text: &str) -> String {
    let words: String = text.chars().take(160).collect();
    format!("({} bytes) {words:?}", text.len())
}

/// The thread of one child: the boot frame, then one answer line per document.
/// It ends with the child: on a pipe that broke, or -- the documents over --
/// when the child has closed its output. Its `answers` end with it.
fn serve(
    mut stdin: ChildStdin,
    mut stdout: BufReader<ChildStdout>,
    boot: String,
    documents: Receiver<String>,
    answers: Sender<Result<String, String>>,
    stage: Arc<AtomicU8>,
) {
    let mut failed = writeln!(stdin, "{boot}")
        .err()
        .map(|e| format!("the warm python child takes no boot frame: {e}"));
    for document in documents {
        if failed.is_none() {
            stage.store(1, Ordering::Relaxed);
            failed = stdin
                .write_all(document.as_bytes())
                .and_then(|()| stdin.write_all(b"\n"))
                .and_then(|()| stdin.flush())
                .err()
                .map(|e| format!("the warm python child takes no document: {e}"));
        }
        let answer = match failed.take() {
            Some(why) => Err(why),
            None => {
                stage.store(2, Ordering::Relaxed);
                let mut line = String::new();
                match stdout.read_line(&mut line) {
                    Ok(0) => Err("the warm python child died without an answer".to_string()),
                    Ok(_) => Ok(line),
                    Err(e) => Err(format!("the warm python child gives no answer: {e}")),
                }
            }
        };
        let over = answer.is_err();
        if answers.send(answer).is_err() || over {
            return;
        }
    }
    // End of input ends the harness's loop, and with it the child's output.
    drop(stdin);
    let _ = std::io::copy(&mut stdout, &mut std::io::sink());
}

struct Warm {
    child: Child,
    documents: Option<Sender<String>>,
    answers: Receiver<Result<String, String>>,
    stage: Arc<AtomicU8>,
    script: String,
    limit: Duration,
}

impl Warm {
    fn boot(script: &str) -> Self {
        Self::boot_as(script, false)
    }

    fn boot_as(script: &str, persistent: bool) -> Self {
        let mut command = Command::new("python3");
        command.arg("-c").arg(HARNESS);
        Self::over(command, script, persistent, scaled(HANG))
    }

    /// The child `command` starts, spoken to as the harness is, each wait for
    /// it ending at `limit`.
    fn over(mut command: Command, script: &str, persistent: bool, limit: Duration) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("python3");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let boot = json!({"script": script, "persistent": persistent}).to_string();
        let (documents, asked) = channel();
        let (answer, answers) = channel();
        let stage = Arc::new(AtomicU8::new(0));
        let at = Arc::clone(&stage);
        std::thread::spawn(move || serve(stdin, stdout, boot, asked, answer, at));
        Self {
            child,
            documents: Some(documents),
            answers,
            stage,
            script: script.to_string(),
            limit,
        }
    }

    /// The child is killed and reaped, by its own handle; neither waits.
    fn end(&mut self) {
        self.documents = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn run(&mut self, document: &str) -> Output {
        let handed = self
            .documents
            .as_ref()
            .is_some_and(|d| d.send(document.to_string()).is_ok());
        assert!(
            handed,
            "the warm python child is gone; its script {}",
            head(&self.script)
        );
        let line = match self.answers.recv_timeout(self.limit) {
            Ok(Ok(line)) => line,
            Ok(Err(why)) => {
                self.end();
                panic!("{why}; its script {}", head(&self.script));
            }
            Err(RecvTimeoutError::Timeout) => {
                let at = STAGES[usize::from(self.stage.load(Ordering::Relaxed)).min(2)];
                self.end();
                panic!(
                    "HANG: the warm python child {at} (waited {} s); its script {}; its document {}",
                    self.limit.as_secs(),
                    head(&self.script),
                    head(document)
                );
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.end();
                panic!(
                    "the thread of the warm python child is gone; its script {}",
                    head(&self.script)
                );
            }
        };
        let frame: Value = sj::from_str(&line).expect("the answer frame is JSON");
        let code = frame["exit_code"].as_i64().expect("exit_code");
        let text = |k: &str| frame[k].as_str().unwrap_or_default().as_bytes().to_vec();
        Output {
            // What the OS hands a cold run's parent: the low byte, as a wait status.
            status: ExitStatus::from_raw(((code & 0xff) as i32) << 8),
            stdout: text("stdout"),
            stderr: text("stderr"),
        }
    }
}

impl Drop for Warm {
    fn drop(&mut self) {
        // Ended already: there is nothing left to wait for.
        if self.documents.take().is_none() {
            return;
        }
        // End of input ends the harness's loop; the thread reads the output to
        // its end, then the child is reaped.
        let silent = loop {
            match self.answers.recv_timeout(self.limit) {
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => break true,
                Err(RecvTimeoutError::Disconnected) => break false,
            }
        };
        // A child that has exited while something else holds its output open
        // is no hang.
        let hung = silent && !self.child.try_wait().is_ok_and(|s| s.is_some());
        self.end();
        if hung {
            let said = format!(
                "HANG: the warm python child does not end with its input (waited {} s); its script {}",
                self.limit.as_secs(),
                head(&self.script)
            );
            // A second panic while one unwinds would abort the first one's words.
            if std::thread::panicking() {
                eprintln!("{said}");
            } else {
                panic!("{said}");
            }
        }
    }
}

thread_local! {
    static CHILDREN: RefCell<HashMap<String, Warm>> = RefCell::new(HashMap::new());
}

/// Run `script` against `document`. `None` when the document cannot travel
/// as one line (the warm wire is line-JSON); the caller runs it cold then.
pub fn run(script: &str, document: &str) -> Option<Output> {
    if document.contains('\n') {
        return None;
    }
    CHILDREN.with(|children| {
        let mut children = children.borrow_mut();
        if !children.contains_key(script) && children.len() >= MAX_CHILDREN {
            children.clear();
        }
        // Taken out while it runs: a child that ends in a panic is let go, and the
        // next run of its script boots another.
        let (key, mut warm) = children
            .remove_entry(script)
            .unwrap_or_else(|| (script.to_string(), Warm::boot(script)));
        let out = warm.run(document);
        children.insert(key, warm);
        Some(out)
    })
}

/// One child of the `resident` runner (GH #1225): `persistent: true` in the
/// boot frame, so the script's globals -- `_MEM` of `./intake` -- live from
/// one message to the next the way they do in a colony. Owned by its caller
/// (one per hive, never the thread's warm table), and [`Resident::restart`]
/// is the child kill a deploy or a `script_timeout` is.
pub struct Resident {
    script: String,
    warm: Warm,
}

impl Resident {
    pub fn boot(script: &str) -> Self {
        Self {
            script: script.to_string(),
            warm: Warm::boot_as(script, true),
        }
    }

    /// [`Resident::boot`] with each wait ending at `limit` instead of the hang
    /// mark: for the pins of the mark itself.
    pub fn bounded(script: &str, limit: Duration) -> Self {
        let mut child = Self::boot(script);
        child.limit(limit);
        child
    }

    /// The child `command` starts in the place of the harness, each wait ending
    /// at `limit`.
    pub fn over(command: Command, script: &str, limit: Duration) -> Self {
        Self {
            script: script.to_string(),
            warm: Warm::over(command, script, true, limit),
        }
    }

    /// Each wait from here on ends at `limit`.
    pub fn limit(&mut self, limit: Duration) {
        self.warm.limit = limit;
    }

    /// Whether the child is gone.
    pub fn ended(&mut self) -> bool {
        self.warm.child.try_wait().is_ok_and(|s| s.is_some())
    }

    /// `None` when the document cannot travel as one line.
    pub fn run(&mut self, document: &str) -> Option<Output> {
        if document.contains('\n') {
            return None;
        }
        Some(self.warm.run(document))
    }

    /// The child killed and booted again: its globals start empty.
    pub fn restart(&mut self) {
        self.warm.end();
        self.warm = Warm::boot_as(&self.script, true);
    }
}
