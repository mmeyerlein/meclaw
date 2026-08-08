//! Phase-9-Close: verifies that StoreCell::handle and CodeCell::handle
//! both have non-empty doc-comments above them — analog Phase-7
//! doc_comment_audit (see crates/meclaw-cells/tests/doc_comment_audit.rs).

use std::path::PathBuf;

fn cell_file(rel: &str) -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("src").join(rel)
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
fn store_cell_handle_has_doc_comment() {
    assert_handle_has_doc_comment(&cell_file("store/cell.rs"));
}

#[test]
fn code_cell_handle_has_doc_comment() {
    assert_handle_has_doc_comment(&cell_file("code/cell.rs"));
}
