//! Phase-7-Close T2: verifies that all five StatelessCell::handle impls
//! have non-empty doc-comments above them.
//!
//! Pragmatischer Audit: liest die fünf Cell-Source-Dateien und prüft,
//! dass die Zeile direkt vor `fn handle<'a>` mit `///` beginnt.

use std::path::PathBuf;

fn cell_file(name: &str) -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("src").join(name)
}

fn assert_handle_has_doc_comment(path: &PathBuf) {
    let content = std::fs::read_to_string(path).expect("read cell file");
    let lines: Vec<&str> = content.lines().collect();
    let handle_idx = lines
        .iter()
        .position(|l| l.trim_start().starts_with("fn handle<'a>("))
        .unwrap_or_else(|| panic!("no `fn handle<'a>(` line in {path:?}"));
    let prev = lines[..handle_idx]
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_else(|| panic!("no non-empty line before fn handle in {path:?}"));
    assert!(
        prev.trim_start().starts_with("///"),
        "fn handle in {path:?} is missing a doc-comment; previous non-empty line: {prev:?}"
    );
}

#[test]
fn file_cell_handle_has_doc_comment() {
    assert_handle_has_doc_comment(&cell_file("file.rs"));
}

#[test]
fn bash_cell_handle_has_doc_comment() {
    assert_handle_has_doc_comment(&cell_file("bash.rs"));
}

#[test]
fn edit_cell_handle_has_doc_comment() {
    assert_handle_has_doc_comment(&cell_file("edit.rs"));
}

#[test]
fn web_fetch_cell_handle_has_doc_comment() {
    assert_handle_has_doc_comment(&cell_file("web_fetch.rs"));
}

#[test]
fn web_search_cell_handle_has_doc_comment() {
    assert_handle_has_doc_comment(&cell_file("web_search.rs"));
}
