//! GH #79: a `write` into a missing parent directory has to say so.
//!
//! The `file` cell already knows why the write failed -- the parent directory
//! it resolved is not there. What travelled back to the caller was
//! `io error during resolve: No such file or directory (os error 2)`, which is
//! the one channel an agent has and which names neither the condition nor the
//! repair. The `read` path in the same cell has always named its condition
//! (`path not found: ...`), so the asymmetry was inside one file.
//!
//! What is pinned here is the WORDING, deliberately: the text is a contract for
//! the model reading it, not an accident of which `std::io` call surfaced
//! first. The taxonomy is pinned alongside it and stays `io_error` -- #79 is a
//! message change, not a taxonomy change.
//!
//! The other `io_error` causes on the write path get their own texts, so that
//! "create the directory", "you aimed at a file", "you may not write here" are
//! three different sentences rather than one prefix.

use meclaw_cells::FileCell;
use meclaw_colony::StatelessCell;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json};
use tokio::sync::mpsc;

/// Drives one tool call through the cell and returns the single emission.
async fn invoke(cell: &FileCell, args: meclaw_core::serde_json::Value) -> CellEmission {
    let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        out_tx,
        Path::new("/file"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        10,
        meclaw_core::Headers::new(),
        None,
    );
    let body = json!({
        "messages": [{
            "origin": "assistant", "type": "tool_call",
            "text": args.to_string(), "id": "call-79"
        }]
    });
    let msg = MessageBuilder::new(Path::new("/file"))
        .reply_to(Path::new("/caller"))
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink).await;
    out_rx.recv().await.expect("emission")
}

fn cell_at(base: &std::path::Path) -> FileCell {
    FileCell {
        base_path: base.to_path_buf(),
        max_concurrency: 8,
    }
}

/// The reported case, verbatim: the workspace root exists, `notes/` does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_into_a_missing_parent_names_the_parent_and_stays_io_error() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let cell = cell_at(td.path());

    let em = invoke(
        &cell,
        json!({"op": "write", "path": "notes/hello.txt", "content": "alpha"}),
    )
    .await;

    assert_eq!(
        em.content["header"]["error_code"], "io_error",
        "#79 is a message change: the classification stays io_error"
    );
    assert_eq!(
        em.content["messages"][0]["text"],
        "parent directory does not exist: notes (write does not create directories)",
        "the write path must name the missing parent, not the io call that noticed it"
    );
}

/// A deeper miss names the whole missing parent path, because that is the
/// argument a repair (`mkdir -p`) takes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deeper_missing_parent_is_named_in_full() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let cell = cell_at(td.path());

    let em = invoke(
        &cell,
        json!({"op": "write", "path": "a/b/c.txt", "content": "x"}),
    )
    .await;

    assert_eq!(em.content["header"]["error_code"], "io_error");
    assert_eq!(
        em.content["messages"][0]["text"],
        "parent directory does not exist: a/b (write does not create directories)"
    );
}

/// A file standing where a directory was addressed is a different repair, so it
/// is a different sentence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_in_the_parent_position_says_not_a_directory() {
    let td = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(td.path().join("notes"), b"i am a file").expect("write");
    let cell = cell_at(td.path());

    let em = invoke(
        &cell,
        json!({"op": "write", "path": "notes/hello.txt", "content": "alpha"}),
    )
    .await;

    assert_eq!(em.content["header"]["error_code"], "io_error");
    assert_eq!(
        em.content["messages"][0]["text"],
        "parent path is not a directory: notes"
    );
}

/// An unreachable parent is neither absent nor a file: no `mkdir` repairs it.
///
/// Root bypasses the mode bits, so the test states its own precondition and
/// skips rather than asserting something the kernel did not do.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_parent_says_permission_denied() {
    use std::os::unix::fs::PermissionsExt;

    let td = tempfile::TempDir::new().expect("tempdir");
    let locked = td.path().join("locked");
    std::fs::create_dir_all(locked.join("sub")).expect("mkdir");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("chmod");

    // Precondition: the search bit actually bites for this process.
    let bites = std::fs::canonicalize(locked.join("sub")).is_err();
    if !bites {
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        eprintln!("skipped: this process traverses a 0o000 directory (running as root?)");
        return;
    }

    let cell = cell_at(td.path());
    let em = invoke(
        &cell,
        json!({"op": "write", "path": "locked/sub/x.txt", "content": "y"}),
    )
    .await;

    // Restore before asserting, so a failing assert does not leave an
    // undeletable TempDir behind.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    assert_eq!(em.content["header"]["error_code"], "io_error");
    assert_eq!(
        em.content["messages"][0]["text"],
        "parent directory not accessible: locked/sub (permission denied)"
    );
}

/// The parent resolved; the write itself was refused. That is a third
/// condition, and the text says at which stage it happened.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_write_names_the_write_stage() {
    use std::os::unix::fs::PermissionsExt;

    let td = tempfile::TempDir::new().expect("tempdir");
    let target = td.path().join("readonly.txt");
    std::fs::write(&target, b"old").expect("write");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o444)).expect("chmod");

    if std::fs::OpenOptions::new()
        .write(true)
        .open(&target)
        .is_ok()
    {
        eprintln!("skipped: this process writes a 0o444 file (running as root?)");
        return;
    }

    let cell = cell_at(td.path());
    let em = invoke(
        &cell,
        json!({"op": "write", "path": "readonly.txt", "content": "new"}),
    )
    .await;

    assert_eq!(em.content["header"]["error_code"], "io_error");
    let text = em.content["messages"][0]["text"]
        .as_str()
        .expect("text")
        .to_string();
    assert!(
        text.starts_with("write failed: permission denied"),
        "a refused write must name the stage and the reason, got: {text}"
    );
}

/// The read path is the reference the issue points at; it must keep its wording.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_read_path_keeps_its_own_named_condition() {
    let td = tempfile::TempDir::new().expect("tempdir");
    let cell = cell_at(td.path());

    let em = invoke(&cell, json!({"op": "read", "path": "nope.txt"})).await;

    assert_eq!(em.content["header"]["error_code"], "not_found");
    let text = em.content["messages"][0]["text"].as_str().expect("text");
    assert!(
        text.starts_with("path not found:"),
        "the read path's wording is unchanged, got: {text}"
    );
}
