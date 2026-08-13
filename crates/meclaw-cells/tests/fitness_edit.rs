//! Track T (#104) — fitness battery for the `edit` cell.
//!
//! Patching code is the highest-risk tool operation an agent performs: a
//! silently wrong edit compiles into a wrong program. The battery pins the
//! contract of `docs/cell-types.md` § edit (phase-7 slice-2):
//!
//! - `find_replace` replaces ALL occurrences and reports the exact count —
//!   there is deliberately no unique-match guard (pinned; the gap is a
//!   fitness-report finding);
//! - 0 matches is the typed `pattern_not_found` error and leaves the file
//!   untouched — which also makes a repeated (already applied) edit loud
//!   instead of silently double-applied;
//! - `insert_at_line` is 1-based insert-BEFORE with typed range errors;
//! - multi-byte UTF-8 (umlauts, emoji) replaces cleanly, never split;
//! - the security boundary is the same fence as the file cell's.

#[path = "support_fitness.rs"]
mod support;

use meclaw_cells::EditCellFactory;
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;
use std::sync::Arc;
use support::{ToolRig, assert_error, assert_normal_result, header_of, text_of};

fn rig(base: &std::path::Path) -> ToolRig {
    ToolRig::spawn(
        Arc::new(EditCellFactory) as Arc<dyn CellFactory>,
        "/edit",
        json!({"base_path": base.to_str().unwrap(), "max_concurrency": 2}),
    )
}

fn fr(path: &str, find: &str, replace: &str) -> meclaw_core::JsonValue {
    json!({"op": "find_replace", "path": path, "find": find, "replace": replace})
}

fn ins(path: &str, line: u64, content: &str) -> meclaw_core::JsonValue {
    json!({"op": "insert_at_line", "path": path, "line": line, "content": content})
}

fn read(td: &tempfile::TempDir, name: &str) -> String {
    std::fs::read_to_string(td.path().join(name)).unwrap()
}

// ------------------------------------------------------------- find_replace

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn find_replace_replaces_every_occurrence_and_reports_the_count() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(
        td.path().join("calc.py"),
        "def add(a, b):\n    return a - b  # BUG\n\ndef sub(a, b):\n    return a - b\n",
    )
    .unwrap();
    let mut r = rig(td.path());

    let em = r.call(fr("calc.py", "a - b", "a + b"), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "operation"), "find_replace");
    assert_eq!(
        header_of(&em, "matches_changed"),
        2,
        "replace-ALL is the documented contract — BOTH occurrences changed"
    );
    let status: meclaw_core::JsonValue = meclaw_core::serde_json::from_str(&text_of(&em)).unwrap();
    assert_eq!(status["matches_changed"], 2);
    assert_eq!(
        read(&td, "calc.py"),
        "def add(a, b):\n    return a + b  # BUG\n\ndef sub(a, b):\n    return a + b\n",
        "note: the ambiguous second site changed TOO — there is no unique-match \
         guard (fitness finding, see receipt)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_unique_pattern_changes_exactly_one_site() {
    // The safe agent idiom today: include enough context to make the pattern
    // unique, then verify matches_changed == 1. The battery pins that this
    // idiom WORKS — the count is exact, not approximate.
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(
        td.path().join("calc.py"),
        "def add(a, b):\n    return a - b\n\ndef sub(a, b):\n    return a - b\n",
    )
    .unwrap();
    let mut r = rig(td.path());

    let em = r
        .call(
            fr(
                "calc.py",
                "add(a, b):\n    return a - b",
                "add(a, b):\n    return a + b",
            ),
            "c1",
        )
        .await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "matches_changed"), 1);
    assert_eq!(
        read(&td, "calc.py"),
        "def add(a, b):\n    return a + b\n\ndef sub(a, b):\n    return a - b\n",
        "only the contextually-unique site changed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_matches_is_pattern_not_found_and_the_file_is_untouched() {
    let td = tempfile::TempDir::new().unwrap();
    let original = "print('hello')\n";
    std::fs::write(td.path().join("a.py"), original).unwrap();
    let mut r = rig(td.path());

    let em = r.call(fr("a.py", "no_such_pattern", "x"), "c1").await;
    assert_error(&em, "pattern_not_found");
    assert_eq!(
        header_of(&em, "matches_changed"),
        0,
        "the error path reports matches_changed 0 explicitly (B.1 parity)"
    );
    assert_eq!(read(&td, "a.py"), original, "the file was not touched");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repeated_edit_is_loud_not_silently_double_applied() {
    // The retry case: an agent re-sends an edit whose first application
    // succeeded (lost result, timeout upstream). Because the pattern is gone
    // after the first application, the second attempt FAILS loudly — for
    // patterns that vanish on application, the edit is effectively
    // retry-detectable. (A replace whose pattern survives, e.g. swapping two
    // occurrences pairwise, does NOT have this property — agent beware.)
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.py"), "x = 1\n").unwrap();
    let mut r = rig(td.path());

    let em = r.call(fr("a.py", "x = 1", "x = 2"), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "matches_changed"), 1);
    assert_eq!(read(&td, "a.py"), "x = 2\n");

    let em = r.call(fr("a.py", "x = 1", "x = 2"), "c2").await;
    assert_error(&em, "pattern_not_found");
    assert_eq!(
        read(&td, "a.py"),
        "x = 2\n",
        "the second attempt changed nothing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_byte_utf8_replaces_cleanly() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(
        td.path().join("gruss.txt"),
        "Grüße an Müller & Familie Müller 🦀\n",
    )
    .unwrap();
    let mut r = rig(td.path());

    // Umlaut as pattern — three two-byte sites.
    let em = r.call(fr("gruss.txt", "ü", "ue"), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "matches_changed"), 3);
    assert_eq!(
        read(&td, "gruss.txt"),
        "Grueße an Mueller & Familie Mueller 🦀\n"
    );

    // Emoji (4-byte sequence) as pattern and as replacement.
    let em = r.call(fr("gruss.txt", "🦀", "🐍!"), "c2").await;
    assert_normal_result(&em, "c2");
    assert_eq!(header_of(&em, "matches_changed"), 1);
    let after = read(&td, "gruss.txt");
    assert_eq!(after, "Grueße an Mueller & Familie Mueller 🐍!\n");
    assert!(
        std::str::from_utf8(after.as_bytes()).is_ok(),
        "no byte-split, the file stays valid UTF-8"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_find_pattern_is_invalid_input() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.txt"), "x").unwrap();
    let mut r = rig(td.path());

    let em = r.call(fr("a.txt", "", "y"), "c1").await;
    assert_error(&em, "invalid_input");
}

// ------------------------------------------------------------ insert_at_line

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_at_line_is_one_based_insert_before() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.py"), "line1\nline2\n").unwrap();
    let mut r = rig(td.path());

    // line 1 → prepend.
    let em = r.call(ins("a.py", 1, "# header\n"), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "operation"), "insert_at_line");
    assert_eq!(read(&td, "a.py"), "# header\nline1\nline2\n");

    // line = file_lines + 1 → append.
    let em = r.call(ins("a.py", 4, "# footer\n"), "c2").await;
    assert_normal_result(&em, "c2");
    assert_eq!(read(&td, "a.py"), "# header\nline1\nline2\n# footer\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_at_line_range_errors_are_typed() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.py"), "line1\nline2\n").unwrap();
    let mut r = rig(td.path());

    // 0 fails the >= 1 rule at parse time.
    let em = r.call(ins("a.py", 0, "x\n"), "c1").await;
    assert_error(&em, "invalid_input");

    // file has 2 lines, 3 appends — 4 is out of range.
    let em = r.call(ins("a.py", 4, "x\n"), "c2").await;
    assert_error(&em, "invalid_input");
    assert!(
        text_of(&em).contains("out of range"),
        "the text names the problem: {:?}",
        text_of(&em)
    );
    assert_eq!(read(&td, "a.py"), "line1\nline2\n", "nothing changed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_without_trailing_newline_merges_with_the_following_line() {
    // Pinned sharp edge: `content` is inserted VERBATIM between line slices.
    // Without its own trailing newline it fuses with the line it displaced.
    // Every content an agent inserts must end in '\n' — fitness-report note.
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.py"), "one\ntwo\n").unwrap();
    let mut r = rig(td.path());

    let em = r.call(ins("a.py", 2, "GLUED"), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(
        read(&td, "a.py"),
        "one\nGLUEDtwo\n",
        "no newline is added on the caller's behalf"
    );
}

// ------------------------------------------------------------------ boundary

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_edit_boundary_is_the_same_fence_as_the_file_cells() {
    let outer = tempfile::TempDir::new().unwrap();
    let base = outer.path().join("boundary");
    std::fs::create_dir(&base).unwrap();
    std::fs::write(outer.path().join("outside.txt"), b"pattern").unwrap();
    let mut r = rig(&base);

    let em = r.call(fr("../outside.txt", "pattern", "x"), "c1").await;
    assert_error(&em, "path_outside_boundary");
    assert_eq!(
        std::fs::read_to_string(outer.path().join("outside.txt")).unwrap(),
        "pattern",
        "the file outside the fence was not touched"
    );

    let em = r.call(fr("/etc/hostname", "x", "y"), "c2").await;
    assert_error(&em, "invalid_input");

    let em = r.call(fr("missing.txt", "x", "y"), "c3").await;
    assert_error(&em, "not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn editing_a_non_utf8_file_is_a_typed_io_error() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("blob.bin"), [0xFF, 0xFE, 0x42]).unwrap();
    let mut r = rig(td.path());

    let em = r.call(fr("blob.bin", "B", "C"), "c1").await;
    assert_error(&em, "io_error");
    assert_eq!(
        std::fs::read(td.path().join("blob.bin")).unwrap(),
        vec![0xFF, 0xFE, 0x42],
        "the binary file was not touched"
    );
}
