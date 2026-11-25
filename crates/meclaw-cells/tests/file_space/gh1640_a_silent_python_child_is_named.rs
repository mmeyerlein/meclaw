//! GH #1640 -- a python child of the hive harnesses that stays silent is named.
//!
//! WHY this file exists: `support/warm_python.rs` waited for its child without
//! an end -- for the document to be taken, for the answer, for the exit. A
//! child that stuck anywhere there held the test silent until the runner
//! killed it at 240 s, and the log did not say where (the run this issue is
//! about could not tell a hang from a slow lane). Each wait now ends at the
//! hang mark with a panic that names the child, and the child is gone by then.
//!
//! The mark is handed in small: the children here never come, so any mark
//! fires, and no statement depends on how long it takes.

// harness_fold: #[path = "support/warm_python.rs"]
#[allow(unused_imports)]
use crate::warm_python; // harness_fold: mod warm_python;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::Command;
use std::time::Duration;
use warm_python::Resident;

/// A hang mark for a child that never comes.
const MARK: Duration = Duration::from_millis(200);

/// A program that takes no line and never ends.
const DEAF: &str = "import threading; threading.Event().wait()";

/// What the statement panicked with.
fn said<T>(statement: impl FnOnce() -> T) -> String {
    let Err(e) = catch_unwind(AssertUnwindSafe(statement)) else {
        panic!("the silent child is waited for without a word");
    };
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("a panic with words")
}

#[test]
fn a_child_that_never_answers_is_named() {
    let script = "# gh1640 never answers\nimport threading\nthreading.Event().wait()\n";
    let mut child = Resident::bounded(script, MARK);
    let said = said(|| child.run("{\"gh1640\": \"the document\"}"));
    assert!(said.starts_with("HANG: "), "{said}");
    assert!(said.contains("gh1640 never answers"), "{said}");
    assert!(said.contains("the document"), "{said}");
    assert!(child.ended(), "the child is gone with the panic");
}

#[test]
fn a_child_that_never_takes_its_document_is_named() {
    let mut deaf = Command::new("python3");
    deaf.arg("-c").arg(DEAF);
    let mut child = Resident::over(deaf, "# gh1640 takes nothing", MARK);
    // Far past a pipe's buffer: the write itself blocks.
    let said = said(|| child.run(&"x".repeat(4 << 20)));
    assert!(said.starts_with("HANG: "), "{said}");
    assert!(said.contains("gh1640 takes nothing"), "{said}");
    assert!(child.ended(), "the child is gone with the panic");
}

#[test]
fn a_child_that_never_ends_is_named() {
    let script = "# gh1640 never ends\nimport atexit, threading\n\
                  atexit.register(threading.Event().wait)\nprint('{}')\n";
    let mut child = Resident::boot(script);
    let out = child.run("{}").expect("a one-line document");
    assert!(out.status.success(), "{out:?}");
    child.limit(MARK);
    let said = said(|| drop(child));
    assert!(said.starts_with("HANG: "), "{said}");
    assert!(said.contains("gh1640 never ends"), "{said}");
}
