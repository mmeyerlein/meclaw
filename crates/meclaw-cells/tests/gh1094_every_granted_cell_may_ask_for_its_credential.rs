//! GH #1094 -- every shipped cell that holds a vault grant may send its own
//! `credential_request` under emission validation.
//!
//! Measured in 0.62.0: eight `llm` cells (builder/compose, curator/summarizer,
//! display/judge, file-space/summarizer, memory-hive/{closer, dialectic,
//! dreamer, judge}) took a `credential_grant_id`, yet their contract still
//! required `hop.finish_reason` on every emission. The request carries only
//! the route and the grant, so a colony that validates emissions (debug
//! builds, or `strict_validation`) dropped it as `contract_violation` before
//! it left -- the same defect #1092 repaired for the registry's translator.
//! The repair is the contract word `optional_on_route: ["credential_request"]`
//! on every required hop key the request does not carry.
//!
//! The central check in the colony (`colony.rs`, "emission violates
//! contract.emits") runs `meclaw_core::validate_emits` against the cell's
//! compiled `contract.emits`; this lock runs exactly that function against
//! the exact request content (`credential::request_content`, the one form
//! every grant round sends) for every config that names a grant, and against
//! a request a real `llm` cell emitted for every `llm` config. `code` cells
//! are left out: the colony skips them centrally and their round pushes the
//! request past the in-cell check.
//!
//! | claim | test |
//! |---|---|
//! | every granted non-code cell's contract admits its credential request | [`gh1094_every_granted_cell_contract_admits_its_credential_request`] |
//! | the request an `llm` cell really emits passes every granted `llm` contract | [`gh1094_the_emitted_llm_request_passes_every_granted_llm_contract`] |

use meclaw_cells::LlmParams;
use meclaw_cells::credential::request_content;
use meclaw_cells::llm::LlmCell;
use meclaw_colony::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{
    Body, CellEmission, CompiledEmits, EmitsBlock, Headers, MessageBuilder, OutputSink, Path, Uuid,
    validate_emits,
};
use std::path::{Path as FsPath, PathBuf};
use tokio::sync::mpsc;

/// The cells #1094 named. A missing file is a tree without the template
/// library (GH #49); a present one must be found by the scan below.
const NAMED: &[&str] = &[
    "builder/compose",
    "curator/summarizer",
    "display/judge",
    "file-space/summarizer",
    "memory-hive/closer",
    "memory-hive/dialectic",
    "memory-hive/dreamer",
    "memory-hive/judge",
];

fn repo() -> PathBuf {
    FsPath::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn walk(dir: &FsPath, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// A granted cell: its params or settings name `credential_grant_id`, it
/// declares `contract.emits`, and the colony checks it centrally.
struct Granted {
    path: PathBuf,
    cell: String,
    emits: CompiledEmits,
}

fn granted() -> Vec<Granted> {
    let mut files = Vec::new();
    for root in ["templates", "examples"] {
        walk(&repo().join(root), &mut files);
    }
    let mut out = Vec::new();
    for path in files {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(cfg) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let has_grant = cfg["params"].get("credential_grant_id").is_some()
            || cfg["contract"]["settings"]
                .get("credential_grant_id")
                .is_some();
        // `cell` is the type word, or an object carrying it (`{"type": "llm"}`).
        let cell = cfg["cell"]
            .as_str()
            .or_else(|| cfg["cell"]["type"].as_str())
            .unwrap_or_default()
            .to_string();
        let Some(emits) = cfg["contract"].get("emits") else {
            continue;
        };
        if !has_grant || cell.is_empty() || cell == "code" || cell == "ref" {
            continue;
        }
        let block: EmitsBlock = meclaw_core::serde_json::from_value(emits.clone())
            .unwrap_or_else(|e| panic!("{}: contract.emits parses: {e}", path.display()));
        let emits = CompiledEmits::compile(&block)
            .unwrap_or_else(|e| panic!("{}: contract.emits compiles: {e}", path.display()));
        out.push(Granted { path, cell, emits });
    }
    out
}

fn name_of(g: &Granted) -> String {
    g.path
        .strip_prefix(repo())
        .unwrap_or(&g.path)
        .display()
        .to_string()
}

fn assert_named_are_scanned(all: &[Granted]) {
    for named in NAMED {
        let p = repo().join("templates").join(named).join("config.json");
        let Ok(_raw) = std::fs::read_to_string(&p) else {
            continue; // GH #49: the template library does not travel with every tree
        };
        assert!(
            all.iter()
                .any(|g| g.path.ends_with(format!("{named}/config.json"))),
            "{named} holds a grant and is checked"
        );
    }
}

#[test]
fn gh1094_every_granted_cell_contract_admits_its_credential_request() {
    let all = granted();
    assert_named_are_scanned(&all);
    let request = request_content(
        "grant:openrouter@gh1094/cell",
        &"00".repeat(32),
        "call-gh1094",
    );
    let refused: Vec<String> = all
        .iter()
        .filter_map(|g| {
            validate_emits(&request, &g.emits)
                .err()
                .map(|e| format!("{} ({}): {e}", name_of(g), g.cell))
        })
        .collect();
    assert!(
        refused.is_empty(),
        "a granted cell's own credential_request would be dropped as \
         contract_violation:\n{}",
        refused.join("\n")
    );
}

#[tokio::test]
async fn gh1094_the_emitted_llm_request_passes_every_granted_llm_contract() {
    let td = tempfile::TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db"))
        .expect("cell.db");
    let mut db = meclaw_colony::DbConn::wrap(conn, None);
    let mut c = LlmCell::new(
        LlmParams::parse(&json!({
            "provider": "openai", "model": "gpt-4o-mini", "api_key": "",
            "credential_grant_id": "grant:openrouter@gh1094/llm",
            "base_url": "http://127.0.0.1:9/v1", "external_timeout_ms": 5_000u64,
            "credential_wait_ms": 600_000u64, "credential_wait_max": 16usize,
        }))
        .expect("params"),
        reqwest::Client::builder().build().expect("http client"),
    );
    let (tx, mut rx) = mpsc::channel::<CellEmission>(64);
    let sink = OutputSink::new(
        tx.clone(),
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        Headers::new(),
        None,
    );
    let turn = MessageBuilder::new(Path::new("/llm"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": "ping"}]}),
        ))
        .build();
    c.handle(turn, &sink, &mut db).await;
    let mut seen = Vec::new();
    while let Ok(em) = rx.try_recv() {
        seen.push(em.content);
    }
    let [request] = &seen
        .iter()
        .filter(|e| e["header"]["route"] == "credential_request")
        .collect::<Vec<_>>()[..]
    else {
        panic!("exactly one credential request: {seen:?}");
    };

    let all = granted();
    assert_named_are_scanned(&all);
    let llm: Vec<&Granted> = all.iter().filter(|g| g.cell == "llm").collect();
    let refused: Vec<String> = llm
        .iter()
        .filter_map(|g| {
            validate_emits(request, &g.emits)
                .err()
                .map(|e| format!("{}: {e}", name_of(g)))
        })
        .collect();
    assert!(
        refused.is_empty(),
        "the llm cell's own credential_request would be dropped as \
         contract_violation:\n{}",
        refused.join("\n")
    );
}
