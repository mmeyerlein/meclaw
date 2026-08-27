//! What makes this builder concretely buildable where a general harness is not:
//! its tool vocabulary is CLOSED, and every tool in it answers a refusal this
//! system has actually produced
//! (`plans/welle-2026-08-27/receipts/s12-luna-run.md`).
//!
//! So the ban is on the general-harness vocabulary — a shell, the open web, a
//! file path, and above all a hand. None of them is missing by oversight; each
//! would be a different template.
//!
//! `seeded_tools()` / `tool_schema()` below run the SHIPPED `brief` script
//! through the same pair `builder_brief_mutation_grammar.rs` uses
//! (`meclaw_testing::{shipped_script, emit_one}`) and read `system.tools` off
//! the emitted body: what the model is TOLD it may call is the thing under
//! test, not what a grep finds in the file.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};
use std::path::{Path, PathBuf};

const TOOLS: &[&str] = &[
    "librarian_search",
    "catalogue_lookup",
    "graph_read",
    "registry_read",
];

/// Names a builder tool slot must never carry.
const BANNED: &[&str] = &[
    "bash",
    "shell",
    "web_fetch",
    "web_search",
    "apply_manifest",
    "submit",
    "file_read",
    "file_write",
];

const BRIEF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/brief/config.json"
);

fn builder_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/builder")
}

fn every_config(dir: &Path, out: &mut Vec<(PathBuf, Value)>) {
    for e in std::fs::read_dir(dir).expect("builder template dir") {
        let p = e.expect("dir entry").path();
        if p.is_dir() {
            every_config(&p, out);
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            let raw = std::fs::read_to_string(&p).expect("config readable");
            out.push((p, meclaw_core::serde_json::from_str(&raw).expect("parses")));
        }
    }
}

fn configs() -> Vec<(PathBuf, Value)> {
    let mut out = Vec::new();
    every_config(&builder_root(), &mut out);
    assert!(
        !out.is_empty(),
        "the builder template has no config.json at all"
    );
    out
}

/// One rendered briefing — the same execution `builder_brief_mutation_grammar.rs`
/// performs, kept in step with it deliberately.
fn briefed_body() -> Value {
    emit_one(
        &shipped_script(BRIEF),
        &json!({
            "target": "/os/builder/brief",
            "header": {
                "hop": {"route": "brief", "stage": "briefed", "hits": 3},
                "context": {},
            },
            "ttl": 64,
            "messages": [
                {"origin": "user", "type": "text", "id": "",
                 "text": "a collector, an llm summarizer and a store, wired in a chain"},
                {"origin": "tool", "type": "tool_result", "id": "",
                 "text": "### config.md -- required_drains (spec) [d-17]\na drain is …"}
            ],
        }),
    )
}

/// The tool names the briefing seeds into `system.tools`.
fn seeded_tools() -> Vec<String> {
    briefed_body()["system"]["tools"]
        .as_object()
        .expect("system.tools — the schemas the briefing seeds")
        .keys()
        .cloned()
        .collect()
}

/// One seeded tool's function object, parsed out of its `text` leaf.
fn tool_schema(name: &str) -> Value {
    let body = briefed_body();
    let text = body["system"]["tools"][name]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("{name}: each tool is one text leaf"));
    meclaw_core::serde_json::from_str(text).expect("a stringified function object")
}

/// The `brief` script is where the schemas are written, so this reads the
/// rendered prompt rather than grepping the file: what the model is TOLD it may
/// call is the thing under test.
fn seeded_tool_names() -> Vec<String> {
    seeded_tools()
}

#[test]
fn the_builder_seeds_exactly_the_four_tools_and_no_others() {
    let mut got = seeded_tool_names();
    got.sort();
    let mut want: Vec<String> = TOOLS.iter().map(|s| (*s).to_string()).collect();
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn no_builder_file_names_a_general_harness_tool() {
    for (path, cfg) in &configs() {
        let raw = meclaw_core::serde_json::to_string(cfg).expect("re-serialise");
        for banned in BANNED {
            // `submit` appears legitimately in prose about who applies; the ban
            // is on it being a TOOL NAME, so it is checked as a quoted name.
            // Re-serialising is what makes that precise: a word inside prose is
            // inside a JSON string, so its own quotes come back ESCAPED
            // (`\"submit\"`) and cannot match the bare `"submit"` a key or a
            // schema-level name would produce.
            let as_tool = format!("\"{banned}\"");
            assert!(
                !raw.contains(&as_tool),
                "{}: {banned} is named as a tool — the builder's vocabulary is \
                 four reads, and a hand is not one of them",
                path.display()
            );
        }
    }
}

#[test]
fn no_builder_tool_schema_takes_a_filesystem_path() {
    for name in seeded_tool_names() {
        let schema = tool_schema(&name);
        let props = schema["function"]["parameters"]["properties"]
            .as_object()
            .expect("parameters declared");
        for key in props.keys() {
            assert!(
                !matches!(key.as_str(), "path" | "file" | "filename" | "dir" | "cwd"),
                "{name}: parameter {key} is a filesystem address — this hive \
                 writes nothing to disk of any kind"
            );
        }
    }
}
