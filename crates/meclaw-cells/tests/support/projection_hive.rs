//! The pure half of the cells of `templates/projection` (GH #905), loaded the way
//! the file-space tables load theirs (`support/file_space_hive.rs`): the script's
//! imports, defs and upper-case constants under its own params, a probe evaluated
//! in that scope, the answer as JSON. `script_inline` knows no library, so the
//! pure functions are held to tables right where they live.
#![allow(dead_code)]

use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const TEMPLATE: &str = "templates/projection";

pub fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// R2b / GH #49: a tree without the template skips.
pub fn shipped() -> bool {
    repo(&format!("{TEMPLATE}/config.json")).is_file()
        && repo("templates/file-space/config.json").is_file()
}

pub fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

pub fn cell_config(name: &str) -> Value {
    read_json(&repo(&format!("{TEMPLATE}/{name}/config.json")))
}

pub fn script_of(name: &str) -> String {
    cell_config(name)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// Run a program under python3, the program itself on stdin.
pub fn run_python(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        sj::to_string(script).unwrap(),
        sj::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

const AST_LOADER: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
src, params = inp["src"], inp["params"]
keep = [n for n in ast.parse(src).body
        if isinstance(n, (ast.Import, ast.ImportFrom, ast.FunctionDef))
        or (isinstance(n, ast.Assign)
            and all(isinstance(t, ast.Name) and t.id.isupper() for t in n.targets))]
scope = {"P": params, "doc": {"params": params, "body": {}, "envelope": {}}}
for n in keep:
    exec(compile(ast.Module(body=[n], type_ignores=[]), "cell", "exec"), scope)
scope["ARGS"] = inp.get("args")
print(json.dumps(eval(inp["probe"], scope)))
"#;

/// The pure half of the shipped script of `cell`, and `probe` evaluated in it.
pub fn pure(cell: &str, probe: &str, args: Value) -> Value {
    let mut params: Map<String, Value> = cell_config(cell)["params"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    params.remove("script_inline");
    let doc = json!({"src": script_of(cell), "params": params, "probe": probe, "args": args});
    let out = run_python(AST_LOADER, &doc.to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "{cell}: the pure half does not load: {err}"
    );
    sj::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{cell}: not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// One call of `probe` over every input of `table` (`ARGS` = the list of
/// inputs), each answer held to its row.
pub fn check(cell: &str, probe: &str, table: &[(&str, Value, Value)]) {
    let args: Vec<Value> = table.iter().map(|(_, arg, _)| arg.clone()).collect();
    let got = pure(cell, probe, json!(args));
    let got = got.as_array().expect("the probe answers a list");
    assert_eq!(got.len(), table.len(), "one answer per row: {got:?}");
    for ((label, arg, want), got) in table.iter().zip(got) {
        assert_eq!(got, want, "{label}: input {arg}");
    }
}
