//! Phase-10-C Phase-Close-Audit: alle pub items + LongRunningCell-Impl-
//! Methoden in `src/proxy/**` haben `///`-Doc-Comments (CLAUDE.md
//! § Coding-Standards). Pattern analog `doc_comment_audit_phase_10b.rs`.

use std::fs;
use std::path::PathBuf;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/proxy")
}

#[test]
fn all_proxy_source_files_pass_doc_comment_audit() {
    let files = [
        "mod.rs",
        "params.rs",
        "db.rs",
        "io.rs",
        "telegram.rs",
        "cell.rs",
        "emit.rs",
        "factory.rs",
    ];
    let mut missing: Vec<String> = Vec::new();
    for fname in files {
        let path = project_root().join(fname);
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {fname}: {e}"));
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            let is_pub_item = trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub async fn ")
                || trimmed.starts_with("pub struct ")
                || trimmed.starts_with("pub enum ")
                || trimmed.starts_with("pub trait ");
            if !is_pub_item {
                continue;
            }
            // Walk back, skipping attribute lines and blank lines; the first
            // "real" predecessor must be a `///` doc-comment (or `#[doc(...)]`).
            let mut j = i;
            let mut found_doc = false;
            while j > 0 {
                j -= 1;
                let p = lines[j].trim_start();
                if p.is_empty() || p.starts_with("#[") || p.starts_with("#![") {
                    continue;
                }
                found_doc = p.starts_with("///") || p.starts_with("#[doc");
                break;
            }
            if !found_doc {
                missing.push(format!("{fname}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "Missing doc-comments (CLAUDE.md § Coding-Standards):\n  {}",
        missing.join("\n  ")
    );
}
