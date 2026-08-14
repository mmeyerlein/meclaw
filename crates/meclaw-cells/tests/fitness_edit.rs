//! Track T (#104) — fitness battery for the `edit` cell.
//!
//! Patching code is the highest-risk tool operation an agent performs: a
//! silently wrong edit compiles into a wrong program. The battery pins the
//! contract of `docs/cell-types.md` § edit (phase-7 slice-2):
//!
//! - `find_replace` replaces ALL occurrences and reports the exact count;
//!   the optional `expected_matches` guard (GH #105) turns that count into a
//!   precondition — without the argument the replace-ALL contract is unchanged;
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

fn fr_expect(
    path: &str,
    find: &str,
    replace: &str,
    expected: meclaw_core::JsonValue,
) -> meclaw_core::JsonValue {
    json!({"op": "find_replace", "path": path, "find": find, "replace": replace,
           "expected_matches": expected})
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
        "note: the ambiguous second site changed TOO — WITHOUT `expected_matches` \
         the guard does not exist (GH #105: the default contract is unchanged)"
    );
}

// ------------------------------------------------- expected_matches (GH #105)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_matching_expected_matches_applies_the_edit() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("calc.py"), "a - b\na - b\n").unwrap();
    let mut r = rig(td.path());

    let em = r
        .call(fr_expect("calc.py", "a - b", "a + b", json!(2)), "c1")
        .await;
    assert_normal_result(&em, "c1");
    assert_eq!(header_of(&em, "matches_changed"), 2);
    assert_eq!(read(&td, "calc.py"), "a + b\na + b\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_expected_matches_refuses_the_edit_and_names_both_numbers() {
    // GH #105: the highest-risk coding failure — an ambiguous pattern silently
    // patching a site the caller never saw. With `expected_matches` the count
    // becomes a PRECONDITION: mismatch ⇒ typed refusal, file untouched.
    let td = tempfile::TempDir::new().unwrap();
    let original = "def add(a, b):\n    return a - b\n\ndef sub(a, b):\n    return a - b\n";
    std::fs::write(td.path().join("calc.py"), original).unwrap();
    let mut r = rig(td.path());

    let em = r
        .call(fr_expect("calc.py", "a - b", "a + b", json!(1)), "c1")
        .await;
    assert_error(&em, "unexpected_match_count");
    assert_eq!(
        header_of(&em, "matches_changed"),
        0,
        "the guard path applied no edit at all"
    );
    let text = text_of(&em);
    assert!(
        text.contains('1') && text.contains('2'),
        "the refusal names BOTH numbers so the caller can repair: {text:?}"
    );
    assert_eq!(
        read(&td, "calc.py"),
        original,
        "not one byte of the file was touched"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_matches_keeps_pattern_not_found_even_under_the_guard() {
    // Precedence ruling (GH #105): `pattern_not_found` is the MORE specific
    // diagnosis — your pattern is not in the file at all — and keeps winning
    // over the count mismatch. Same code with and without the argument.
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.py"), "x = 1\n").unwrap();
    let mut r = rig(td.path());

    let em = r
        .call(fr_expect("a.py", "no_such_pattern", "y", json!(1)), "c1")
        .await;
    assert_error(&em, "pattern_not_found");
    assert_eq!(read(&td, "a.py"), "x = 1\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_malformed_expected_matches_is_invalid_input() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.py"), "x = 1\n").unwrap();
    let mut r = rig(td.path());

    // 0 is not an edit — the guard counts sites the caller INTENDS to change.
    let em = r.call(fr_expect("a.py", "x", "y", json!(0)), "c1").await;
    assert_error(&em, "invalid_input");

    // Wrong type is a shape error, not a count.
    let em = r.call(fr_expect("a.py", "x", "y", json!("1")), "c2").await;
    assert_error(&em, "invalid_input");

    let em = r.call(fr_expect("a.py", "x", "y", json!(-1)), "c3").await;
    assert_error(&em, "invalid_input");

    assert_eq!(read(&td, "a.py"), "x = 1\n", "no edit slipped through");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expected_matches_is_not_accepted_on_insert_at_line() {
    // The guard is a find_replace concept — `insert_at_line` has no match
    // count. Silently ignoring the argument would let a caller believe in a
    // guard that never runs.
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.py"), "one\n").unwrap();
    let mut r = rig(td.path());

    let em = r
        .call(
            json!({"op": "insert_at_line", "path": "a.py", "line": 1,
                   "content": "zero\n", "expected_matches": 1}),
            "c1",
        )
        .await;
    assert_error(&em, "invalid_input");
    assert_eq!(read(&td, "a.py"), "one\n");
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
async fn insert_without_trailing_newline_is_normalized_to_its_own_line() {
    // GH #108 (redefines the T-pin `insert_without_trailing_newline_merges_
    // with_the_following_line`): `content` used to be spliced VERBATIM between
    // line slices, so a missing trailing newline fused the inserted text into
    // the line it displaced — silently producing a broken file that only the
    // next compile run would report. The op is called insert_at_LINE, so the
    // cell now closes the line it was asked to insert.
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.py"), "one\ntwo\n").unwrap();
    let mut r = rig(td.path());

    let em = r.call(ins("a.py", 2, "GLUED"), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(read(&td, "a.py"), "one\nGLUED\ntwo\n");

    // Content that already ends in a newline is untouched — no double '\n'.
    let em = r.call(ins("a.py", 1, "# header\n"), "c2").await;
    assert_normal_result(&em, "c2");
    assert_eq!(read(&td, "a.py"), "# header\none\nGLUED\ntwo\n");

    // Multi-line content: only the LAST line is missing its terminator.
    let em = r.call(ins("a.py", 1, "import os\nimport sys"), "c3").await;
    assert_normal_result(&em, "c3");
    assert_eq!(
        read(&td, "a.py"),
        "import os\nimport sys\n# header\none\nGLUED\ntwo\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn empty_content_inserts_nothing_no_phantom_blank_line() {
    // GH #108 boundary: normalization completes a line the caller started. An
    // empty `content` starts none, so it stays empty — a caller who wants a
    // blank line writes "\n", which says so.
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.py"), "one\ntwo\n").unwrap();
    let mut r = rig(td.path());

    let em = r.call(ins("a.py", 2, ""), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(read(&td, "a.py"), "one\ntwo\n");

    let em = r.call(ins("a.py", 2, "\n"), "c2").await;
    assert_normal_result(&em, "c2");
    assert_eq!(read(&td, "a.py"), "one\n\ntwo\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_without_a_final_newline_still_fuses_at_append_known_limit() {
    // The MIRROR edge of GH #108, deliberately NOT fixed: when the FILE's last
    // line has no terminator, an append lands on it. Closing that line would
    // rewrite a line the caller never named — the cell normalizes its own
    // argument, never the file's existing bytes. Pinned so it stays a decision.
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("a.py"), "one\ntwo").unwrap();
    let mut r = rig(td.path());

    let em = r.call(ins("a.py", 3, "three"), "c1").await;
    assert_normal_result(&em, "c1");
    assert_eq!(read(&td, "a.py"), "one\ntwothree\n");
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

    // Inside the fence, a missing file is still an ordinary not_found.
    let em = r.call(fr("missing.txt", "x", "y"), "c3").await;
    assert_error(&em, "not_found");

    // GH #107: outside the fence it is NOT — existing and missing answer
    // identically, so the fence is no existence oracle. Same for insert.
    let em = r.call(fr("../no-such-file.txt", "x", "y"), "c4").await;
    assert_error(&em, "path_outside_boundary");
    let em = r.call(ins("../no-such-file.txt", 1, "x\n"), "c5").await;
    assert_error(&em, "path_outside_boundary");
    let em = r.call(ins("../outside.txt", 1, "x\n"), "c6").await;
    assert_error(&em, "path_outside_boundary");
    assert_eq!(
        std::fs::read_to_string(outer.path().join("outside.txt")).unwrap(),
        "pattern",
        "still untouched"
    );
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
