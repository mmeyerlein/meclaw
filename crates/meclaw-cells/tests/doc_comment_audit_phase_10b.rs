//! Phase-10-B doc-comment audit (CONTRIBUTING.md § Coding standards). Checks that
//! every public item in `crates/meclaw-cells/src/timer/**/*.rs` carries a
//! `///` doc or a `#[doc(...)]` attribute. Symmetric to
//! `doc_comment_audit_phase_9.rs` (Phase-9-Audit-Test).

use std::fs;
use std::path::{Path as StdPath, PathBuf};

fn check_file(path: &StdPath, missing: &mut Vec<String>) {
    let src = fs::read_to_string(path).expect("read timer source");
    let lines: Vec<&str> = src.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        let is_public = t.starts_with("pub fn ")
            || t.starts_with("pub async fn ")
            || t.starts_with("pub struct ")
            || t.starts_with("pub enum ")
            || t.starts_with("pub trait ");
        if !is_public {
            continue;
        }
        // Spec intent (CONTRIBUTING.md § Coding standards): "every public item has
        // doc comments." Standard Rust ordering: `///` →
        // `#[derive(...)]`/`#[allow(...)]`/`#[cfg(...)]` → `pub …`. Wir
        // we skip attributes (lines starting with `#[`) and blank lines, and
        // check whether the last real predecessor line starts with `///` (or
        // `#[doc(...)]`).
        let mut j = i;
        let mut found_doc = false;
        while j > 0 {
            j -= 1;
            let p = lines[j].trim_start();
            if p.is_empty() || p.starts_with("#[") {
                continue;
            }
            found_doc = p.starts_with("///") || p.starts_with("#[doc");
            break;
        }
        if !found_doc {
            missing.push(format!("{}:{}  {}", path.display(), i + 1, l));
        }
    }
}

fn timer_src_dir() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("src").join("timer")
}

#[test]
fn all_public_items_in_timer_have_doc_comments() {
    let mut missing = Vec::new();
    let dir = timer_src_dir();
    for entry in fs::read_dir(&dir).expect("read_dir src/timer") {
        let e = entry.expect("dir entry");
        if e.path().extension().and_then(|s| s.to_str()) == Some("rs") {
            check_file(&e.path(), &mut missing);
        }
    }
    assert!(
        missing.is_empty(),
        "missing /// docs in src/timer/**:\n{}",
        missing.join("\n")
    );
}
