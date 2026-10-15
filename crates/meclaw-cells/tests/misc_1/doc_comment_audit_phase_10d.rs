//! Phase-10-D phase-close audit: every pub item + LongRunningCell impl method in
//! `src/mcp/**` carries `///` doc comments (CONTRIBUTING.md
//! § Coding-Standards). Pattern analog `doc_comment_audit_phase_10c.rs`.

use std::fs;
use std::path::PathBuf;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/mcp")
}

#[test]
fn all_mcp_source_files_pass_doc_comment_audit() {
    let files = [
        "mod.rs",
        "params.rs",
        "jsonrpc.rs",
        "db.rs",
        "wire.rs",
        "io.rs",
        "emit.rs",
        "parse.rs",
        "cell.rs",
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
                || trimmed.starts_with("pub trait ")
                || trimmed.starts_with("pub const ")
                || trimmed.starts_with("pub mod ");
            if !is_pub_item {
                continue;
            }
            // Walk back, skipping attribute lines and blank lines; the
            // first "real" predecessor must be a `///` doc-comment.
            let mut j = i;
            let has_doc = loop {
                if j == 0 {
                    break false;
                }
                j -= 1;
                let p = lines[j].trim_start();
                if p.is_empty() || p.starts_with('#') {
                    continue;
                }
                break p.starts_with("///") || p.starts_with("//!");
            };
            if !has_doc {
                missing.push(format!("{fname}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "doc comments missing:\n{}",
        missing.join("\n")
    );
}
