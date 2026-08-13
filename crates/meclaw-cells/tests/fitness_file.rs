//! Track T (#104) — fitness battery for the `file` cell.
//!
//! A coding agent's writes go through this cell, so the battery pins the CRUD
//! surface AND the security boundary (`docs/cell-types.md` § file, phase-7
//! slice-1 conventions):
//!
//! - write/read/list/stat round-trip, byte-identical for UTF-8 content;
//! - the boundary rejects absolute paths, `../` traversal and symlink escapes
//!   with the documented codes;
//! - `write` refuses a missing parent with the GH #79 contract TEXT (the text
//!   is the repair instruction — it is part of the contract);
//! - large files round-trip uncut;
//! - non-UTF-8 files are a typed error, not garbage.

#[path = "support_fitness.rs"]
mod support;

use meclaw_cells::FileCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::{Value, json};
use std::sync::Arc;
use support::{ToolRig, assert_error, assert_normal_result, header_of, text_of};

fn rig(base: &std::path::Path) -> ToolRig {
    ToolRig::spawn(
        Arc::new(FileCellFactory) as Arc<dyn CellFactory>,
        "/fs",
        json!({"base_path": base.to_str().unwrap(), "max_concurrency": 2}),
    )
}

fn op(o: &str, path: &str) -> Value {
    json!({"op": o, "path": path})
}

fn write_op(path: &str, content: &str) -> Value {
    json!({"op": "write", "path": path, "content": content})
}

// ---------------------------------------------------------------------- CRUD

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_read_list_stat_round_trip() {
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());

    // write — the status text is structured JSON, bytes = content length.
    let content = "def add(a, b):\n    return a + b\n";
    let em = r.call(write_op("calc.py", content), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "operation"), "write");
    assert_eq!(header_of(&em, "bytes"), content.len());
    let status: Value = meclaw_core::serde_json::from_str(&text_of(&em)).unwrap();
    assert_eq!(status["written"], content.len());

    // read — byte-identical.
    let em = r.call(op("read", "calc.py"), "c2").await;
    assert_normal_result(&em, "c2");
    assert_eq!(header_of(&em, "operation"), "read");
    assert_eq!(text_of(&em), content);
    assert_eq!(header_of(&em, "bytes"), content.len());

    // list — sorted entries with kind and size.
    let em = r.call(op("list", "."), "c3").await;
    assert_normal_result(&em, "c3");
    let entries: Value = meclaw_core::serde_json::from_str(&text_of(&em)).unwrap();
    let arr = entries.as_array().expect("list is a JSON array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "calc.py");
    assert_eq!(arr[0]["kind"], "file");
    assert_eq!(arr[0]["size"], content.len());

    // stat — kind/size/modified.
    let em = r.call(op("stat", "calc.py"), "c4").await;
    assert_normal_result(&em, "c4");
    let stat: Value = meclaw_core::serde_json::from_str(&text_of(&em)).unwrap();
    assert_eq!(stat["kind"], "file");
    assert_eq!(stat["size"], content.len());
    assert!(stat["modified"].is_u64(), "stat carries an mtime");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn utf8_content_round_trips_byte_identical() {
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());

    let content = "# Grüße an Müller — naïve Prüfung 🦀\nwert = \"öäüß\"\n";
    let em = r.call(write_op("umlaute.py", content), "c1").await;
    assert_normal_result(&em, "c1");

    let em = r.call(op("read", "umlaute.py"), "c2").await;
    assert_eq!(
        text_of(&em),
        content,
        "multi-byte content survives unchanged"
    );
    assert_eq!(
        header_of(&em, "bytes"),
        content.len(),
        "bytes counts BYTES, not chars"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_overwrites_whole_file_no_append_mode_exists() {
    // Pin the overwrite semantics: a second write REPLACES. An agent that
    // wants to append must read-modify-write (or use edit) — worth knowing,
    // not worth guessing.
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());

    r.call(write_op("a.txt", "first version"), "c1").await;
    r.call(write_op("a.txt", "second"), "c2").await;

    let em = r.call(op("read", "a.txt"), "c3").await;
    assert_eq!(text_of(&em), "second", "write replaces, never appends");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_512_kib_file_round_trips_uncut() {
    // A generated lockfile or a vendored source easily reaches this size. The
    // rig reads the cell's own emission (pre-delivery), so this pins the CELL
    // contract: no size cap, no truncation on the file cell itself.
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());

    let content = "x".repeat(512 * 1024);
    let em = r.call(write_op("big.txt", &content), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "bytes"), content.len());

    let em = r.call(op("read", "big.txt"), "c2").await;
    assert_normal_result(&em, "c2");
    assert_eq!(text_of(&em).len(), content.len(), "nothing was cut");
    assert_eq!(text_of(&em), content, "nothing was mangled");
}

// ------------------------------------------------------------------ boundary

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn absolute_paths_are_invalid_input_on_every_op() {
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());

    for (i, args) in [
        op("read", "/etc/passwd"),
        write_op("/tmp/escape.txt", "x"),
        op("list", "/etc"),
        op("stat", "/etc/passwd"),
    ]
    .into_iter()
    .enumerate()
    {
        let em = r.call(args, &format!("c{i}")).await;
        assert_error(&em, "invalid_input");
        assert!(
            text_of(&em).contains("must be relative"),
            "the text names the rule: {:?}",
            text_of(&em)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dot_dot_traversal_to_an_existing_target_is_path_outside_boundary() {
    // The boundary is a sibling-proof fence: a file that EXISTS right outside
    // it is refused with the boundary code, on reads and on writes.
    let outer = tempfile::TempDir::new().unwrap();
    let base = outer.path().join("boundary");
    std::fs::create_dir(&base).unwrap();
    std::fs::write(outer.path().join("secret.txt"), b"secret").unwrap();
    let mut r = rig(&base);

    let em = r.call(op("read", "../secret.txt"), "c1").await;
    assert_error(&em, "path_outside_boundary");
    assert!(!text_of(&em).contains("secret"), "nothing leaked");

    let em = r.call(write_op("../evil.txt", "x"), "c2").await;
    assert_error(&em, "path_outside_boundary");
    assert!(
        !outer.path().join("evil.txt").exists(),
        "nothing was written outside"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_symlink_escape_is_refused_at_the_resolved_target() {
    // Symlinks are followed by canonicalize, so a link INSIDE the boundary
    // pointing OUTSIDE is an escape and must be refused.
    let outer = tempfile::TempDir::new().unwrap();
    let base = outer.path().join("boundary");
    std::fs::create_dir(&base).unwrap();
    std::fs::write(outer.path().join("target.txt"), b"CONTENT_T104_LINK").unwrap();
    std::os::unix::fs::symlink(outer.path().join("target.txt"), base.join("link.txt")).unwrap();
    let mut r = rig(&base);

    let em = r.call(op("read", "link.txt"), "c1").await;
    assert_error(&em, "path_outside_boundary");
    assert!(
        !text_of(&em).contains("CONTENT_T104_LINK"),
        "nothing leaked"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_target_outside_the_boundary_reports_not_found() {
    // Known limit, pinned deliberately: the existence check runs before the
    // boundary check on the read path, so a traversal to a MISSING file says
    // `not_found`, not `path_outside_boundary`. A (weak) existence oracle
    // outside the fence — flagged in the fitness report, pinned here so a
    // future fix shows up as a conscious contract change.
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());

    let em = r.call(op("read", "../no-such-file-t104.txt"), "c1").await;
    assert_error(&em, "not_found");
}

// ----------------------------------------------------------- write contract

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_without_parent_names_the_condition_and_the_parent() {
    // GH #79: the TEXT is contract — it names the parent as the caller wrote
    // it plus the repair hint. An agent parses this to decide `mkdir -p`.
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());

    let em = r.call(write_op("notes/today.md", "x"), "c1").await;
    assert_error(&em, "io_error");
    assert_eq!(
        text_of(&em),
        "parent directory does not exist: notes (write does not create directories)"
    );

    // Control: after the parent exists, the same write succeeds.
    std::fs::create_dir(td.path().join("notes")).unwrap();
    let em = r.call(write_op("notes/today.md", "x"), "c2").await;
    assert_normal_result(&em, "c2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_through_a_file_as_parent_names_the_shape_problem() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("blocker"), b"i am a file").unwrap();
    let mut r = rig(td.path());

    let em = r.call(write_op("blocker/child.txt", "x"), "c1").await;
    assert_error(&em, "io_error");
    assert_eq!(text_of(&em), "parent path is not a directory: blocker");
}

// ------------------------------------------------------------ type mismatch

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shape_errors_are_typed_read_dir_list_file_stat_missing() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(td.path().join("adir")).unwrap();
    std::fs::write(td.path().join("afile"), b"x").unwrap();
    let mut r = rig(td.path());

    let em = r.call(op("read", "adir"), "c1").await;
    assert_error(&em, "not_a_file");

    let em = r.call(op("list", "afile"), "c2").await;
    assert_error(&em, "not_a_directory");

    let em = r.call(op("read", "missing.txt"), "c3").await;
    assert_error(&em, "not_found");

    let em = r.call(op("stat", "missing.txt"), "c4").await;
    assert_error(&em, "not_found");

    let em = r.call(json!({"op": "chmod", "path": "afile"}), "c5").await;
    assert_error(&em, "invalid_input");
}

// ------------------------------------------------------------------ non-UTF8

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_utf8_file_is_a_typed_io_error_not_garbage() {
    // Binary artefacts (a .pyc, a compiled object) live in every workspace.
    // The read path is UTF-8-only today: the refusal must be TYPED. That there
    // is no binary/base64 read mode at all is a fitness-report finding.
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("blob.bin"), [0xFF, 0xFE, 0x00, 0x42]).unwrap();
    let mut r = rig(td.path());

    let em = r.call(op("read", "blob.bin"), "c1").await;
    assert_error(&em, "io_error");
    assert!(
        text_of(&em).contains("UTF-8") || text_of(&em).contains("utf-8"),
        "the text names the encoding problem: {:?}",
        text_of(&em)
    );

    // stat and list still work on the same file — only READ is text-bound.
    let em = r.call(op("stat", "blob.bin"), "c2").await;
    assert_normal_result(&em, "c2");
    let stat: Value = meclaw_core::serde_json::from_str(&text_of(&em)).unwrap();
    assert_eq!(stat["size"], 4);
}
