//! GH #1528: every `llm` cell of a curator, for a test that boots a tree from disk.
//!
//! The curator's model cells (its summarizer, and since GH #1528 its ballast
//! judge) carry `${ctx.model}`: an instantiation-side substitution a tree
//! booted from disk cannot resolve. A test that copies a composite onto disk
//! points each of them at its mock with a literal model. [`patch_llm_cells`]
//! does that for every `llm` cell directly under one curator directory,
//! found by its cell type rather than by name, so a model cell the curator
//! grows later is covered without touching each test again. Written ONCE
//! here instead of in every test that boots a talky, a cogny or a curator.
#![allow(dead_code)]

use meclaw_core::serde_json::Value;

/// Rewrites the `config.json` of every `llm` cell directly under `curator`
/// with `edit`, in directory order, and returns how many it rewrote. A
/// curator without any `llm` cell is a broken fixture: it panics.
pub fn patch_llm_cells(curator: &std::path::Path, mut edit: impl FnMut(&mut Value)) -> usize {
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(curator)
        .unwrap_or_else(|e| panic!("{}: {e}", curator.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let mut rewritten = 0;
    for dir in dirs {
        let path = dir.join("config.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut cfg: Value = meclaw_core::serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        if cfg["cell"]["type"].as_str() != Some("llm") {
            continue;
        }
        edit(&mut cfg);
        std::fs::write(
            &path,
            meclaw_core::serde_json::to_string_pretty(&cfg).expect("serialize"),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        rewritten += 1;
    }
    assert!(
        rewritten > 0,
        "{}: a curator carries at least one llm cell",
        curator.display()
    );
    rewritten
}
