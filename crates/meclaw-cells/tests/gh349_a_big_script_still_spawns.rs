//! GH #349 — a `code` cell whose `script_inline` exceeds the platform's
//! per-argv-string limit must still spawn.
//!
//! Linux caps a **single** `argv` string at `MAX_ARG_STRLEN` = `32 * PAGE_SIZE`
//! = 131 072 bytes, independent of `ARG_MAX`. The substrate used to hand the
//! whole inline script to the runner as one argv string (`<runner> -c <script>`),
//! so every `code` cell above that line failed at `spawn()` with
//! `Argument list too long (os error 7)` — `templates/memory-hive/recall`
//! crossed it and its read path could not start at all.
//!
//! **These tests drive the real production spawn path** — `CodeCell::handle`,
//! the same code the colony runs — not a test-side pipe. Every existing probe
//! of a shipped script feeds the interpreter over stdin (`python3 -`), which has
//! no cap; that is exactly why the suite stayed green while the shipped cell
//! could not boot. The pin therefore has to go through `handle`.
//!
//! What is pinned here:
//!
//! 1. a >131 072 byte inline script spawns and answers;
//! 2. the DOCUMENT still travels to the child on **stdin** (unchanged — the
//!    script reads its envelope from fd 0 and echoes a field of it back);
//! 3. `__name__ == "__main__"` in the script, as under `-c`;
//! 4. the same holds under `trust: "restricted"` with nothing but the runtime
//!    set declared — `cell-types.md` § `code` promises that a `script_inline`
//!    needs no filesystem declaration of its own, and a materialised script
//!    must not quietly break that promise.

use meclaw_cells::code::{CodeCell, CodeParams, Script};
use meclaw_cells::sandbox::SandboxProfile;
use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

/// The Linux per-argv-string cap on the target platform. Named here so the
/// test says WHY the script has the size it has.
const MAX_ARG_STRLEN: usize = 32 * 4096;

fn make_sink(otx: mpsc::Sender<meclaw_core::CellEmission>) -> OutputSink {
    OutputSink::new(
        otx,
        Path::new("/code"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    )
}

fn mk_msg() -> meclaw_core::Message {
    MessageBuilder::new(Path::new("/code"))
        .body(Body::Inline(json!({"messages":[]})))
        .reply_to(Path::new("/sink"))
        .build()
}

/// A syntactically valid Python program comfortably over [`MAX_ARG_STRLEN`],
/// which reads its envelope from stdin and reports back three facts: the
/// `envelope.target` it was handed, its own `__name__`, and its own source size.
fn oversized_script() -> String {
    // One long comment line does the padding: it is the cheapest way to make the
    // SOURCE big without making the program do anything different.
    let filler = "#".repeat(MAX_ARG_STRLEN + 10_000);
    let body = r#"
import json, sys
doc = json.load(sys.stdin)
sys.stdout.write(json.dumps({
    "messages": [{
        "origin": "tool",
        "type": "tool_result",
        "text": doc["envelope"]["target"] + "|" + __name__,
        "id": "",
    }]
}))
"#;
    let script = format!("{filler}{body}");
    assert!(
        script.len() > MAX_ARG_STRLEN,
        "the pin is only a pin above the cap: {} bytes",
        script.len()
    );
    script
}

fn params_for(script: String, sandbox: Option<SandboxProfile>) -> CodeParams {
    CodeParams {
        runner: "python3".into(),
        script: Script::Inline(script),
        external_timeout_ms: Some(30_000),
        max_concurrency: None,
        sandbox,
    }
}

/// Drive `handle` once through the production path and return every emission.
async fn run(cell: &CodeCell) -> Vec<meclaw_core::CellEmission> {
    let (otx, mut orx) = mpsc::channel(16);
    let sink = make_sink(otx);
    cell.handle(mk_msg(), &sink).await;
    drop(sink);
    let mut outs = Vec::new();
    while let Some(em) = orx.recv().await {
        outs.push(em);
    }
    outs
}

/// Assert the one emission is the script's own answer, and return its text.
fn sole_answer(outs: &[meclaw_core::CellEmission]) -> String {
    assert_eq!(outs.len(), 1, "expected exactly one emission");
    let header = &outs[0].content["header"];
    assert!(
        header["error_code"].is_null(),
        "the cell must not fail: {header}"
    );
    assert_eq!(
        header["exit_code"],
        json!(0),
        "script must exit 0: {header}"
    );
    outs[0].content["messages"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_big_script_still_spawns() {
    let cell = CodeCell::new(params_for(oversized_script(), None), false, None, false);
    let outs = run(&cell).await;
    assert_eq!(
        sole_answer(&outs),
        "/code|__main__",
        "the document must still arrive on stdin and the script must still be __main__"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_big_script_still_spawns_under_a_restricted_sandbox() {
    // A kernel without Landlock cannot enforce the property under test; skip
    // visibly rather than go red (the convention of `sandbox_isolation.rs`).
    if meclaw_cells::sandbox::landlock_abi().is_none() {
        eprintln!("SKIP: no Landlock on this kernel");
        return;
    }
    // The shipped shape: nothing but the runtime set. `cell-types.md` § `code`
    // says a `script_inline` needs no declaration of its own — whatever the
    // substrate does to get the script to the runner, that has to stay true.
    let profile = SandboxProfile::parse(&json!({
        "sandbox": {"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}}
    }))
    .expect("profile parses")
    .expect("profile present");
    let cell = CodeCell::new(
        params_for(oversized_script(), Some(profile)),
        false,
        None,
        false,
    );
    let outs = run(&cell).await;
    assert_eq!(sole_answer(&outs), "/code|__main__");
}

/// The script that broke: `templates/memory-hive/recall`, 141 063 bytes, run
/// through the production spawn path.
///
/// This one is not a constructed size — it is the shipped cell whose read path
/// stopped working. The receipt is POSITIVE: handed a tier-0 recall context, the
/// script answers with the store legs of a recall (`hop.route == "rstore"`),
/// which it can only do by having run. No store and no LLM are involved — the
/// legs are the `select` tool calls the cell emits, not their answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shipped_recall_script_spawns() {
    const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";
    let raw = std::fs::read_to_string(RECALL_CONFIG).expect("recall config");
    let v: meclaw_core::serde_json::Value =
        meclaw_core::serde_json::from_str(&raw).expect("config json");
    // `${VAR:-default}` is resolved to its default, the way the substrate would
    // resolve it at instantiation; a bare `${VAR}` resolves to the empty string.
    let script_raw = v["params"]["script_inline"]
        .as_str()
        .expect("script_inline");
    let mut script = String::with_capacity(script_raw.len());
    let mut rest = script_raw;
    while let Some(start) = rest.find("${") {
        script.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find('}').expect("unterminated ${...}");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            script.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    script.push_str(rest);
    assert!(
        script.len() > MAX_ARG_STRLEN,
        "the shipped recall script is {} bytes — below the cap this test proves nothing",
        script.len()
    );

    // The recall context an edge promotes in (the shipped `memory-hive` port
    // modifier), plus the audience keys the GH #244 gate requires on the read
    // path. Without them the cell correctly answers with nothing at all, which
    // would be a receipt nobody can read.
    let mut context = meclaw_core::serde_json::Map::new();
    for (k, v) in [
        ("recall_query", "What is my favorite color?"),
        ("memory_tier", "0"),
        ("recall_as_of", ""),
        ("recall_window_from", ""),
        ("recall_window_to", ""),
        ("audience_now", "[\"user\"]"),
        ("channel", "gh349"),
        ("session_id", "gh349"),
    ] {
        context.insert(k.into(), json!(v));
    }
    let msg = MessageBuilder::new(Path::new("/code"))
        .headers(meclaw_core::Headers::from_parts(
            context,
            meclaw_core::serde_json::Map::new(),
        ))
        .body(Body::Inline(json!({"messages":[
            {"origin":"user","type":"text","text":"What is my favorite color?"}
        ]})))
        .reply_to(Path::new("/sink"))
        .build();

    let cell = CodeCell::new(params_for(script, None), true, None, false);
    let (otx, mut orx) = mpsc::channel(16);
    let sink = make_sink(otx);
    cell.handle(msg, &sink).await;
    drop(sink);
    let mut outs = Vec::new();
    while let Some(em) = orx.recv().await {
        outs.push(em);
    }

    let rendered = meclaw_core::serde_json::to_string(
        &outs.iter().map(|e| e.content.clone()).collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(
        !rendered.contains("Argument list too long"),
        "the shipped recall script must spawn: {rendered}"
    );
    assert!(
        outs.iter()
            .any(|e| e.content["header"]["route"] == json!("rstore")),
        "the shipped recall script must answer with its store legs: {rendered}"
    );
}

/// A script comfortably UNDER the cap keeps working unchanged — the repair must
/// not become a regression for the 73 shipped cells that were never affected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_small_script_is_untouched() {
    let script = r#"
import json, sys
doc = json.load(sys.stdin)
sys.stdout.write(json.dumps({"messages": [{
    "origin": "tool", "type": "tool_result",
    "text": doc["envelope"]["target"] + "|" + __name__, "id": "",
}]}))
"#;
    assert!(script.len() < MAX_ARG_STRLEN);
    let cell = CodeCell::new(params_for(script.to_string(), None), false, None, false);
    let outs = run(&cell).await;
    assert_eq!(sole_answer(&outs), "/code|__main__");
}
