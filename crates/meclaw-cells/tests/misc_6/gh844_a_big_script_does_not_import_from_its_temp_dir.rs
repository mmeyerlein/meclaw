//! GH #844 — a `code` cell whose `script_inline` is too big for `argv` does not
//! import from the directory its temporary file lands in.
//!
//! Since GH #349 such a script is written to a per-spawn file in the SHARED
//! temporary directory and started as `python3 <path>`. That form puts the
//! script's own directory first on `sys.path`, so any module lying next to the
//! file — a `json.py` another user or another program left in `/tmp` — wins the
//! import over the standard library, and the cell runs somebody else's code.
//! The fix starts the runner as `python3 -I <path>` on that path and only there.
//!
//! **The production spawn path is driven** (`CodeCell::handle`, cold, no
//! sandbox), exactly as `gh349_a_big_script_still_spawns.rs` does it, and the
//! measurement is the script's own report from inside the interpreter: whether
//! a module planted next to its file is importable, whether that directory is
//! on `sys.path`, and whether the file is named after the process that wrote
//! it. The planted module has a name no program uses (`meclaw_gh844_<uuid>`),
//! never `json.py`: the temporary directory is shared, and a test must not
//! plant a trap for its neighbours.

use meclaw_cells::code::{CodeCell, CodeParams, RunnerMode, Script};
use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

/// The Linux per-argv-string cap: the size that forces the temp file.
const MAX_ARG_STRLEN: usize = 32 * 4096;

/// A module planted in the temporary directory, removed again however the test
/// ends.
struct Planted(std::path::PathBuf);

impl Drop for Planted {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn plant() -> (String, Planted) {
    let name = format!("meclaw_gh844_{}", Uuid::now_v7().simple());
    let path = std::env::temp_dir().join(format!("{name}.py"));
    std::fs::write(&path, "PLANTED = True\n").expect("plant the module");
    (name, Planted(path))
}

/// A program over the cap that reports, from inside the interpreter, what it
/// can see: the planted module, its own directory on `sys.path`, and the name
/// it was written under.
fn reporting_script(planted: &str) -> String {
    let filler = "#".repeat(MAX_ARG_STRLEN + 10_000);
    let body = format!(
        r#"
import importlib.util, json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
report = {{
    "planted_importable": importlib.util.find_spec("{planted}") is not None,
    "own_dir_on_path": here in [os.path.abspath(p) for p in sys.path if p],
    "isolated": sys.flags.isolated,
    "named_after_parent": os.path.basename(__file__).startswith(
        "meclaw-code-%d-" % os.getppid()),
}}
sys.stdout.write(json.dumps({{"messages": [{{
    "origin": "tool", "type": "tool_result", "id": "", "text": json.dumps(report)}}]}}))
"#
    );
    let script = format!("{filler}{body}");
    assert!(
        script.len() > MAX_ARG_STRLEN,
        "the pin is only a pin above the cap"
    );
    script
}

async fn run(script: String) -> Value {
    let cell = CodeCell::new(
        CodeParams {
            runner: "python3".into(),
            script: Script::Inline(script),
            external_timeout_ms: Some(30_000),
            max_concurrency: None,
            sandbox: None,
            runner_mode: RunnerMode::Cold,
        },
        false,
        None,
        false,
    );
    let (otx, mut orx) = mpsc::channel(16);
    let sink = OutputSink::new(
        otx,
        Path::new("/code"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new("/code"))
        .body(Body::Inline(json!({"messages": []})))
        .reply_to(Path::new("/sink"))
        .build();
    cell.handle(msg, &sink).await;
    drop(sink);
    let mut outs = Vec::new();
    while let Some(em) = orx.recv().await {
        outs.push(em);
    }
    assert_eq!(outs.len(), 1, "exactly one answer: {outs:?}");
    let header = &outs[0].content["header"];
    assert!(
        header["error_code"].is_null(),
        "the cell must not fail: {header}"
    );
    let text = outs[0].content["messages"][0]["text"]
        .as_str()
        .expect("the report is a text turn");
    meclaw_core::serde_json::from_str(text).expect("the report is JSON")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_big_script_does_not_import_from_its_temp_dir() {
    let (name, _planted) = plant();
    let report = run(reporting_script(&name)).await;
    assert_eq!(
        report["planted_importable"],
        json!(false),
        "a module lying next to the temp file must not be importable: {report}"
    );
    assert_eq!(
        report["own_dir_on_path"],
        json!(false),
        "the shared temp directory must not be on sys.path: {report}"
    );
    assert_eq!(
        report["isolated"],
        json!(1),
        "the runner runs with -I: {report}"
    );
    assert_eq!(
        report["named_after_parent"],
        json!(true),
        "the file carries the pid of the process that wrote it: {report}"
    );
}
