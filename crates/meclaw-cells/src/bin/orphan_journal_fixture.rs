//! Test fixture for GH #116: a stand-in daemon that can be crashed hard.
//!
//! It does exactly what a real daemon does around a tool child — boot the
//! orphan journal, spawn a child through the production
//! [`StdioChild::spawn`](meclaw_cells::stdio_child::StdioChild::spawn) path,
//! then keep running — and nothing else. The test SIGKILLs it, which is the
//! one teardown path on which no `Drop`, no `kill_on_drop` and no `terminate`
//! runs; the child it leaves behind is the orphan the boot reap has to find.
//!
//! Usage: `orphan_journal_fixture <colony-root> <sleep-seconds>`. Prints the
//! child's pid on stdout (one line, flushed) and then parks forever.

use meclaw_cells::stdio_child::{ChildSpec, StdioChild};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .expect("usage: orphan_journal_fixture <root> <seconds>");
    let seconds = args
        .next()
        .expect("usage: orphan_journal_fixture <root> <seconds>");

    // The boot pass: reap whatever a previous run left, then install the
    // journal so the spawn below is recorded.
    let report = meclaw_cells::orphan_journal::boot(std::path::Path::new(&root));
    eprintln!("orphan_journal_fixture: boot reap {report:?}");

    let spec = ChildSpec {
        program: "sleep".to_string(),
        args: vec![seconds],
        kill_grace_ms: 100,
        // Own process group: this is the shape the harness/mcp children have,
        // and it exercises the reaper's group sweep.
        process_group: true,
        ..ChildSpec::default()
    };
    let child = StdioChild::spawn(&spec).expect("sleep must spawn");
    println!("{}", child.pid().expect("a fresh child has a pid"));
    use std::io::Write;
    std::io::stdout().flush().expect("stdout flushes");

    // Park. `child` stays bound on purpose — dropping it would kill the very
    // process the test needs to survive us.
    std::future::pending::<()>().await;
    drop(child);
}
