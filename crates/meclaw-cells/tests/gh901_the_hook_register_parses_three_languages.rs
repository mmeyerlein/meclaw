//! GH #901 -- the hook register of a file space and its three checkers.
//!
//! `templates/file-space/guard` judges every new text version before
//! `./write` lets it land (S-27-2): JSON, TOML and Python, each with a parser
//! of the Python standard library; every other kind of file has no hook. This
//! lock holds the pure half of the shipped script, loaded with `ast` the way
//! the curator's locks load its policy (`support/curator_hive.rs`
//! `policy_scope`): the register `HOOKS` and its resolution by MIME type and
//! file ending, the three checkers with the place the parser stopped at, the
//! verdict with its limits, and that the register is the one place a language
//! is named (Bau C hangs Rust and the others in there).
//!
//! Filter: `-E 'binary(~gh901)'`.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::{read_json, repo, run_python};
use meclaw_core::serde_json::{self as sj, Value, json};

const GUARD: &str = "templates/file-space/guard/config.json";

/// R2b / GH #49: a tree without the template skips.
fn shipped() -> bool {
    repo(GUARD).is_file()
}

fn guard() -> Value {
    read_json(&repo(GUARD))
}

fn script() -> String {
    guard()["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// Imports, defs and upper-case constants of the guard script, in file order;
/// then `probe` -- a python expression over that scope, with the test's
/// arguments as `ARGS` -- printed as JSON. The part of the script that reads
/// stdin and writes the answer is not loaded.
const AST_LOADER: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
keep = [n for n in ast.parse(inp["src"]).body
        if isinstance(n, (ast.Import, ast.ImportFrom, ast.FunctionDef))
        or (isinstance(n, ast.Assign)
            and all(isinstance(t, ast.Name) and t.id.isupper() for t in n.targets))]
scope = {}
for n in keep:
    exec(compile(ast.Module(body=[n], type_ignores=[]), "guard", "exec"), scope)
scope["ARGS"] = inp.get("args")
print(json.dumps(eval(inp["probe"], scope)))
"#;

fn probe(expr: &str, args: Value) -> Value {
    let doc = json!({"src": script(), "probe": expr, "args": args});
    let out = run_python(AST_LOADER, &doc.to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the pure half does not load: {err}");
    sj::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn the_register_names_json_toml_and_python_and_nothing_else() {
    if !shipped() {
        return;
    }
    let got = probe(
        "{k: {'ext': v['ext'], 'mime': v['mime'], 'check': v['check'].__name__} \
         for k, v in HOOKS.items()}",
        Value::Null,
    );
    assert_eq!(
        got,
        json!({
            "json": {"ext": [".json"], "mime": ["application/json"], "check": "check_json"},
            "toml": {"ext": [".toml"], "mime": ["application/toml"], "check": "check_toml"},
            "python": {"ext": [".py"], "mime": ["text/x-python"], "check": "check_python"},
        }),
        "the register of #901: three languages, no hook for Markdown or plain text"
    );
}

#[test]
fn a_language_is_named_in_the_register_only() {
    if !shipped() {
        return;
    }
    // Review focus of #901: the register is the one place a language stands,
    // so Bau C adds a language by adding one entry. Each language word, each
    // ending and each MIME type occurs exactly once as a string literal.
    let src = script();
    for lit in [
        "\"json\"",
        "\"toml\"",
        "\"python\"",
        "\".json\"",
        "\".toml\"",
        "\".py\"",
        "\"application/json\"",
        "\"application/toml\"",
        "\"text/x-python\"",
    ] {
        assert_eq!(
            src.matches(lit).count(),
            1,
            "{lit} must stand in `HOOKS` and nowhere else in the guard script"
        );
    }
}

#[test]
fn the_mime_type_wins_over_the_ending_and_the_ending_ignores_case() {
    if !shipped() {
        return;
    }
    let cases = json!([
        ["conf/a.json", "", "json"],
        ["conf/A.JSON", "", "json"],
        ["x.Toml", "", "toml"],
        ["pkg/mod.py", "", "python"],
        ["pkg/MOD.PY", "", "python"],
        ["src/main.rs", "", ""],
        ["notes.md", "", ""],
        ["notes.txt", "", ""],
        ["README", "", ""],
        ["data.bin", "application/json", "json"],
        ["a.py", "application/json; charset=utf-8", "json"],
        ["a.json", "text/x-python", "python"],
        ["a.json", "TEXT/X-PYTHON", "python"],
        ["", "application/toml", "toml"],
        // an unregistered MIME type decides nothing: the ending does
        ["a.json", "text/markdown", "json"],
        ["a.md", "text/markdown", ""],
        // only the last segment of a path carries the ending
        ["dir.json/readme", "", ""],
    ]);
    let got = probe("[resolve(p, m) for p, m, _ in ARGS]", cases.clone());
    let want: Vec<Value> = cases
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c[2].clone())
        .collect();
    for (i, c) in cases.as_array().unwrap().iter().enumerate() {
        assert_eq!(got[i], want[i], "resolve({}, {})", c[0], c[1]);
    }
}

#[test]
fn json_is_checked_with_its_place() {
    if !shipped() {
        return;
    }
    let got = probe(
        "[check_json(t) for t in ARGS]",
        json!([
            "",
            "  \n",
            "{\"a\": 1}",
            "[1, 2]\n",
            "{\"a\":1,}",
            "[1,\n2",
            "\n\n  x"
        ]),
    );
    assert_eq!(
        got,
        json!([
            null,
            null,
            null,
            null,
            {"message": "Expecting property name enclosed in double quotes", "line": 1, "col": 8},
            {"message": "Expecting ',' delimiter", "line": 2, "col": 2},
            {"message": "Expecting value", "line": 3, "col": 3},
        ])
    );
}

#[test]
fn toml_is_checked_with_its_place() {
    if !shipped() {
        return;
    }
    let got = probe(
        "[check_toml(t) for t in ARGS]",
        json!([
            "",
            "a = 1\n[t]\nb = \"x\"\n",
            "a = \n",
            "a=1\na=2\n",
            "a=1\nb = [1,\n",
            "[x\n"
        ]),
    );
    assert_eq!(
        got,
        json!([
            null,
            null,
            {"message": "Invalid value", "line": 1, "col": 5},
            {"message": "Cannot overwrite a value", "line": 2, "col": 4},
            // `(at end of document)`: the place just past the last character
            {"message": "Invalid value", "line": 3, "col": 1},
            {"message": "Expected ']' at the end of a table declaration", "line": 1, "col": 3},
        ])
    );
}

#[test]
fn python_is_parsed_never_run() {
    if !shipped() {
        return;
    }
    let got = probe(
        "[check_python(t) for t in ARGS]",
        json!([
            "",
            "import os\n\ndef f(x):\n    return x + 1\n",
            // parsing this must not end the checker's process
            "raise SystemExit(3)\n",
            "def f(:\n  pass",
            "if 1:\n\tx=1\n        y=2\n",
            "x=1\n  y=2\n",
            "x = (1,\n",
            // no place from the parser: `line`/`col` 0 means "none" (OR-FH-66)
            "x = 1\u{0}\n"
        ]),
    );
    assert_eq!(
        got,
        json!([
            null,
            null,
            null,
            {"message": "invalid syntax", "line": 1, "col": 7},
            {"message": "inconsistent use of tabs and spaces in indentation", "line": 3, "col": 1},
            {"message": "unexpected indent", "line": 2, "col": 2},
            {"message": "'(' was never closed", "line": 1, "col": 5},
            {"message": "source code string cannot contain null bytes", "line": 0, "col": 0},
        ])
    );
}

#[test]
fn the_verdict_checks_text_only_and_steps_aside_where_it_cannot_judge() {
    if !shipped() {
        return;
    }
    let got = probe(
        "[verdict(b, n) for b, n in ARGS]",
        json!([
            [{"path": "a.json", "mime": "", "kind": "text", "text": "{}"}, 1_048_576],
            [{"path": "a.json", "mime": "", "kind": "text", "text": "{"}, 1_048_576],
            [{"path": "a.json", "mime": "", "kind": "binary", "text": "{"}, 1_048_576],
            [{"path": "main.rs", "mime": "", "kind": "text", "text": "fn main( {"}, 1_048_576],
            [{"path": "a.json", "mime": "", "kind": "text", "text": "{\"k\": 12345}"}, 10],
            [{"path": "a.json", "mime": "", "kind": "text", "text": "{\"k\": 1}"}, 12],
            [{"path": "a.json", "mime": "", "kind": "text"}, 1_048_576],
            [{"path": "deep.json", "mime": "", "kind": "text", "text": "[".repeat(200_000)}, 1_048_576],
        ]),
    );
    assert_eq!(
        got,
        json!([
            {"hook": "ok", "lang": "json"},
            {"hook": "failed", "lang": "json",
             "error": {"message": "Expecting property name enclosed in double quotes", "line": 1, "col": 2}},
            {"hook": "none", "lang": ""},
            {"hook": "none", "lang": ""},
            {"hook": "none", "lang": "json", "note": "too_large_to_check"},
            // exactly at the limit is still checked
            {"hook": "ok", "lang": "json"},
            {"hook": "none", "lang": "json", "note": "no_text"},
            {"hook": "none", "lang": "json", "note": "too_deep_to_check"},
        ])
    );
}

#[test]
fn the_limit_is_a_knob_of_one_mebibyte() {
    if !shipped() {
        return;
    }
    let cfg = guard();
    assert_eq!(cfg["params"]["max_check_bytes"], json!(1_048_576));
    assert_eq!(
        cfg["contract"]["settings"]["max_check_bytes"]["default"],
        json!(1_048_576)
    );
    let got = probe(
        "[max_check_bytes(p) for p in ARGS]",
        json!([{}, {"max_check_bytes": 10}, {"max_check_bytes": "x"}, {"max_check_bytes": 0}, null]),
    );
    assert_eq!(got, json!([1_048_576, 10, 1_048_576, 1_048_576, 1_048_576]));
}
