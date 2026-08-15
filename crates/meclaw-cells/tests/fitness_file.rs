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
async fn every_escape_attempt_answers_the_same_the_fence_is_no_existence_oracle() {
    // GH #107 (redefines the T-pin `a_missing_target_outside_the_boundary_
    // reports_not_found`): the boundary check is LEXICAL and runs BEFORE the
    // filesystem is touched. A missing target outside the fence used to say
    // `not_found` while an existing one said `path_outside_boundary` — a
    // (weak) existence oracle for the world outside. Now every escape answers
    // identically, whatever is or is not out there.
    let outer = tempfile::TempDir::new().unwrap();
    let base = outer.path().join("boundary");
    std::fs::create_dir(&base).unwrap();
    std::fs::write(outer.path().join("exists.txt"), b"x").unwrap();
    std::fs::create_dir(outer.path().join("adir")).unwrap();
    let mut r = rig(&base);

    for (i, args) in [
        // Existing / missing / deep — indistinguishable.
        op("read", "../exists.txt"),
        op("read", "../no-such-file-t104.txt"),
        op("read", "../../../../../../etc/passwd"),
        op("stat", "../exists.txt"),
        op("stat", "../no-such-file-t104.txt"),
        op("list", "../adir"),
        op("list", "../no-such-dir"),
        // Climbing out and back in is refused too: deciding that would mean
        // resolving names OUTSIDE the fence, which is the oracle being closed.
        op("read", "../boundary/inside.txt"),
        op("read", "sub/../../exists.txt"),
        // Write paths carry the same oracle otherwise (missing parent =>
        // io_error), so they get the same pre-check.
        write_op("../evil.txt", "x"),
        write_op("../no-such-dir/evil.txt", "x"),
    ]
    .into_iter()
    .enumerate()
    {
        let em = r.call(args.clone(), &format!("c{i}")).await;
        assert_error(&em, "path_outside_boundary");
        assert!(
            !text_of(&em).contains("no-such"),
            "the refusal does not echo what was probed: {:?}",
            text_of(&em)
        );
    }

    assert!(
        !outer.path().join("evil.txt").exists(),
        "nothing was written outside"
    );

    // Positive control: `..` INSIDE the fence is ordinary path arithmetic and
    // still works — the check refuses escapes, not the character sequence.
    std::fs::create_dir(base.join("sub")).unwrap();
    std::fs::write(base.join("inside.txt"), b"ok").unwrap();
    let em = r.call(op("read", "sub/../inside.txt"), "ctrl").await;
    assert_normal_result(&em, "ctrl");
    assert_eq!(text_of(&em), "ok");
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
async fn a_non_utf8_file_is_a_typed_io_error_in_the_default_text_mode() {
    // Binary artefacts (a .pyc, a compiled object) live in every workspace.
    // The DEFAULT read path is UTF-8-only, and the refusal is typed. The way
    // to actually look at those bytes is `mode: "base64"` (GH #106, below).
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

// -------------------------------------------- base64 + byte ranges (GH #106)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn base64_mode_reads_a_binary_file_that_text_mode_refuses() {
    // The point of the mode: a `.pyc`, an object file, a PNG header — bytes an
    // agent must be able to LOOK at, not just be refused on.
    let td = tempfile::TempDir::new().unwrap();
    let raw = [0xFFu8, 0xFE, 0x00, 0x42, 0x0A];
    std::fs::write(td.path().join("blob.bin"), raw).unwrap();
    let mut r = rig(td.path());

    let em = r
        .call(
            json!({"op": "read", "path": "blob.bin", "mode": "base64"}),
            "c1",
        )
        .await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "operation"), "read");
    assert_eq!(text_of(&em), "//4AQgo=", "standard alphabet, padded");
    assert_eq!(
        header_of(&em, "encoding"),
        "base64",
        "the mode is announced in the header — the payload is not text"
    );
    assert_eq!(
        header_of(&em, "bytes"),
        raw.len(),
        "`bytes` stays the RAW byte count, not the encoded length"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_default_read_contract_is_untouched_by_the_new_arguments() {
    // Pin: no `mode`, no `offset`, no `limit` ⇒ byte-identical to before,
    // including the ABSENCE of the `encoding` header.
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());
    r.call(write_op("a.py", "x = 1\n"), "c0").await;

    let em = r.call(op("read", "a.py"), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(text_of(&em), "x = 1\n");
    assert_eq!(header_of(&em, "bytes"), 6);
    assert!(
        header_of(&em, "encoding").is_null(),
        "text mode carries no encoding header"
    );

    // Explicit "text" is the same thing spelled out.
    let em = r
        .call(json!({"op": "read", "path": "a.py", "mode": "text"}), "c2")
        .await;
    assert_normal_result(&em, "c2");
    assert_eq!(text_of(&em), "x = 1\n");
    assert!(header_of(&em, "encoding").is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offset_and_limit_are_a_byte_range_in_both_modes() {
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());
    r.call(write_op("big.txt", "0123456789"), "c0").await;

    let em = r
        .call(
            json!({"op": "read", "path": "big.txt", "offset": 3, "limit": 4}),
            "c1",
        )
        .await;
    assert_normal_result(&em, "c1");
    assert_eq!(text_of(&em), "3456");
    assert_eq!(header_of(&em, "bytes"), 4, "bytes counts the SLICE");

    // limit alone = a head; offset alone = a tail.
    let em = r
        .call(json!({"op": "read", "path": "big.txt", "limit": 2}), "c2")
        .await;
    assert_eq!(text_of(&em), "01");
    let em = r
        .call(json!({"op": "read", "path": "big.txt", "offset": 8}), "c3")
        .await;
    assert_eq!(text_of(&em), "89");

    // A range that runs past the end is clamped, not an error.
    let em = r
        .call(
            json!({"op": "read", "path": "big.txt", "offset": 8, "limit": 999}),
            "c4",
        )
        .await;
    assert_eq!(text_of(&em), "89");

    // Same range in base64.
    let em = r
        .call(
            json!({"op": "read", "path": "big.txt", "offset": 3, "limit": 4,
                   "mode": "base64"}),
            "c5",
        )
        .await;
    assert_eq!(text_of(&em), "MzQ1Ng==");
    assert_eq!(header_of(&em, "bytes"), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_offset_at_or_past_eof_is_an_empty_read_not_an_error() {
    // The paging signal: an agent walking a large file needs "you are at the
    // end" as a RESULT, not as a failure it has to classify.
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());
    r.call(write_op("a.txt", "abc"), "c0").await;

    let em = r
        .call(json!({"op": "read", "path": "a.txt", "offset": 3}), "c1")
        .await;
    assert_normal_result(&em, "c1");
    assert_eq!(text_of(&em), "");
    assert_eq!(header_of(&em, "bytes"), 0);

    let em = r
        .call(
            json!({"op": "read", "path": "a.txt", "offset": 9000, "mode": "base64"}),
            "c2",
        )
        .await;
    assert_normal_result(&em, "c2");
    assert_eq!(text_of(&em), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_byte_range_that_splits_a_character_is_a_typed_io_error_in_text_mode() {
    // The range is BYTE semantics — it knows nothing about characters. When a
    // slice lands mid-character, text mode refuses with the SAME code as any
    // other non-UTF-8 read, and the text points at the mode that can do it.
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());
    r.call(write_op("u.txt", "äöü"), "c0").await; // 2 bytes per char

    let em = r
        .call(json!({"op": "read", "path": "u.txt", "limit": 1}), "c1")
        .await;
    assert_error(&em, "io_error");
    assert!(
        text_of(&em).to_lowercase().contains("utf-8") && text_of(&em).contains("base64"),
        "the refusal names the problem AND the way out: {:?}",
        text_of(&em)
    );

    // base64 hands the same half-character over without complaint.
    let em = r
        .call(
            json!({"op": "read", "path": "u.txt", "limit": 1, "mode": "base64"}),
            "c2",
        )
        .await;
    assert_normal_result(&em, "c2");
    assert_eq!(text_of(&em), "ww==");

    // A range ON the character boundary is plain text again.
    let em = r
        .call(json!({"op": "read", "path": "u.txt", "limit": 2}), "c3")
        .await;
    assert_eq!(text_of(&em), "ä");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_read_arguments_are_invalid_input() {
    let td = tempfile::TempDir::new().unwrap();
    let mut r = rig(td.path());
    r.call(write_op("a.txt", "abc"), "c0").await;

    for (i, args) in [
        json!({"op": "read", "path": "a.txt", "mode": "binary"}), // unknown mode
        json!({"op": "read", "path": "a.txt", "mode": 7}),
        json!({"op": "read", "path": "a.txt", "limit": 0}), // a 0-byte read is no read
        json!({"op": "read", "path": "a.txt", "limit": -1}),
        json!({"op": "read", "path": "a.txt", "offset": "3"}),
        // Range args belong to `read`. Silently ignoring them on a write would
        // let a caller believe in a partial write that never happened.
        json!({"op": "write", "path": "a.txt", "content": "x", "offset": 1}),
        json!({"op": "list", "path": ".", "limit": 1}),
        json!({"op": "stat", "path": "a.txt", "mode": "base64"}),
    ]
    .into_iter()
    .enumerate()
    {
        let em = r.call(args.clone(), &format!("c{i}")).await;
        assert_error(&em, "invalid_input");
    }

    assert_eq!(
        std::fs::read_to_string(td.path().join("a.txt")).unwrap(),
        "abc",
        "no rejected call touched the file"
    );
}
