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
#![allow(dead_code)]

use meclaw_core::serde_json::{self as sj, Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Output, Stdio};

/// The substrate's warm/resident harness, byte for byte.
const HARNESS: &str = include_str!("../../src/code/harness.py");

/// Children kept per thread. A test that writes many distinct scripts (one
/// mutation per case) would otherwise keep one idle interpreter per script;
/// past this many they are all let go and booted again on demand.
const MAX_CHILDREN: usize = 16;

struct Warm {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Warm {
    fn boot(script: &str) -> Self {
        let mut child = Command::new("python3")
            .arg("-c")
            .arg(HARNESS)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("python3");
        let mut stdin = child.stdin.take().expect("stdin");
        let boot = json!({"script": script, "persistent": false});
        writeln!(stdin, "{boot}").expect("boot frame");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
        }
    }

    fn run(&mut self, document: &str) -> Output {
        let stdin = self.stdin.as_mut().expect("stdin");
        stdin
            .write_all(document.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .expect("the warm child takes the document");
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).expect("an answer frame");
        assert!(n > 0, "the warm python child died without an answer");
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
        // End of input ends the harness's loop; then the child is reaped.
        drop(self.stdin.take());
        let _ = self.child.wait();
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
        let warm = children
            .entry(script.to_string())
            .or_insert_with(|| Warm::boot(script));
        Some(warm.run(document))
    })
}
