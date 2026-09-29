//! GH #901 -- the seam of the syntax hook: `in_check` straight at the shipped
//! `templates/file-space/guard` cell.
//!
//! The cell runs whole -- its `script_inline` under python3 with the stdin
//! document a `code` cell gets (envelope with hop and context, body, params
//! from the shipped config), the way `support/curator_hive.rs` runs a curator
//! cell -- and its answer is read the way the colony reads it: every emitted
//! object is one message, its `header` the new hop. `guard` has no store and
//! no edge of its own (`./write` draws both edges, OR-FH-H1), so the seam is
//! the one message in and the one answer out.
//!
//! Broken JSON, TOML and Python come back `failed` with the language, the
//! parser's message and the place; valid content comes back `ok`; a file
//! without a parser (`.rs`), a binary file and a text over `max_check_bytes`
//! come back `none`. The refusal `syntax` on `in_write` and `force` on the
//! version belong to `./write` and are locked in `gh900_*` (OR-FH-H1).
//!
//! Filter: `-E 'binary(~gh901)'`.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::{obj, read_json, repo, run_python};
use meclaw_core::serde_json::{self as sj, Map, Value, json};

const GUARD: &str = "templates/file-space/guard/config.json";

/// R2b / GH #49: a tree without the template skips.
fn shipped() -> bool {
    repo(GUARD).is_file()
}

/// One `in_check` at the cell; the shipped params, overlaid by `over`.
/// Returns every message the cell emitted, each as (hop, body).
fn check_with(
    over: Value,
    hop: Value,
    body: Value,
) -> Vec<(Map<String, Value>, Map<String, Value>)> {
    let cfg = read_json(&repo(GUARD));
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline");
    let mut params = obj(cfg["params"].clone());
    params.remove("script_inline");
    for (k, v) in obj(over) {
        assert!(params.contains_key(&k), "no such param: guard.{k}");
        params.insert(k, v);
    }
    let doc = json!({
        "envelope": {"header": {"context": {"cur_origin": "write", "cur_phase": "check"},
                                "hop": hop},
                     "target": "/x/space/guard",
                     "reply_to": ""},
        "body": body,
        "params": params,
    });
    let out = run_python(script, &doc.to_string());
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "guard exited non-zero: {err}");
    assert!(err.trim().is_empty(), "guard wrote to stderr: {err}");
    let emitted: Value = sj::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "guard: output is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    let list = match emitted {
        Value::Array(a) => a,
        other => vec![other],
    };
    list.into_iter()
        .map(|m| {
            let mut body = obj(m);
            let hop = body
                .remove("header")
                .and_then(|h| h.as_object().cloned())
                .unwrap_or_default();
            (hop, body)
        })
        .collect()
}

/// The one answer to one `in_check`, asserted to be exactly one message on
/// route `checked` carrying the asker's `op_id`; returns its body.
fn check(path: &str, mime: &str, kind: &str, text: &str) -> Value {
    check_over(json!({}), path, mime, kind, text)
}

fn check_over(over: Value, path: &str, mime: &str, kind: &str, text: &str) -> Value {
    let msgs = check_with(
        over,
        json!({"route": "in_check", "op_id": "op-901"}),
        json!({"path": path, "mime": mime, "kind": kind, "text": text}),
    );
    assert_eq!(msgs.len(), 1, "one request, one answer: {msgs:?}");
    let (hop, body) = &msgs[0];
    assert_eq!(
        Value::Object(hop.clone()),
        json!({"route": "checked", "op_id": "op-901"}),
        "the answer rides `checked` and mirrors `op_id`, nothing else"
    );
    Value::Object(body.clone())
}

fn failed(body: &Value, lang: &str, message: &str, line: i64, col: i64) {
    assert_eq!(
        body,
        &json!({"hook": "failed", "lang": lang,
                "error": {"message": message, "line": line, "col": col}}),
    );
}

#[test]
fn broken_json_toml_and_python_are_refused_with_their_place() {
    if !shipped() {
        return;
    }
    failed(
        &check(
            "conf/app.json",
            "application/json",
            "text",
            "{\n  \"a\": 1,\n}\n",
        ),
        "json",
        "Expecting property name enclosed in double quotes",
        3,
        1,
    );
    failed(
        &check("conf/app.toml", "", "text", "[server]\nport = \n"),
        "toml",
        "Invalid value",
        2,
        8,
    );
    failed(
        &check("tool/run.py", "", "text", "def run(:\n    return 1\n"),
        "python",
        "invalid syntax",
        1,
        9,
    );
    // mixed tabs and spaces are a refusal too (TabError is a SyntaxError)
    failed(
        &check(
            "tool/tabs.py",
            "text/x-python",
            "text",
            "if 1:\n\tx = 1\n        y = 2\n",
        ),
        "python",
        "inconsistent use of tabs and spaces in indentation",
        3,
        1,
    );
}

#[test]
fn valid_content_passes() {
    if !shipped() {
        return;
    }
    for (path, text, lang) in [
        ("conf/app.json", "{\"a\": [1, 2, {\"b\": null}]}\n", "json"),
        (
            "conf/app.toml",
            "[server]\nport = 8080\nhost = \"127.0.0.1\"\n",
            "toml",
        ),
        (
            "tool/run.py",
            "import sys\n\ndef run():\n    sys.exit(4)\n\nrun()\n",
            "python",
        ),
        ("conf/empty.json", "", "json"),
        ("conf/empty.toml", "", "toml"),
        ("tool/empty.py", "", "python"),
    ] {
        assert_eq!(
            check(path, "", "text", text),
            json!({"hook": "ok", "lang": lang}),
            "{path}"
        );
    }
}

#[test]
fn a_file_without_a_parser_and_a_binary_file_are_not_checked() {
    if !shipped() {
        return;
    }
    assert_eq!(
        check("src/main.rs", "", "text", "fn main( {"),
        json!({"hook": "none", "lang": ""})
    );
    assert_eq!(
        check("notes/a.md", "text/markdown", "text", "# ((("),
        json!({"hook": "none", "lang": ""})
    );
    assert_eq!(
        check("img/data.json", "application/json", "binary", "{"),
        json!({"hook": "none", "lang": ""})
    );
}

#[test]
fn nothing_over_the_limit_is_checked() {
    if !shipped() {
        return;
    }
    // The shipped limit: 1 MiB, one byte over it is not parsed at all. Why 1 MiB
    // and not 4: near 4 MiB a parse took up to 9.66 s on the host (Python
    // `"x\n"` x 2 Mi, load 15), right at `external_timeout_ms` 10 000; a
    // `script_timeout` never answers `checked`, so the write would hang in
    // `pending` (Abschluss-Review H I-1, OR-FH-92).
    let big = format!("[{}", "1,".repeat(512 * 1024));
    assert_eq!(big.len(), 1024 * 1024 + 1);
    assert_eq!(
        check("big.json", "", "text", &big),
        json!({"hook": "none", "lang": "json", "note": "too_large_to_check"})
    );
    // The knob moves the limit; under it the same content is judged.
    assert_eq!(
        check_over(
            json!({"max_check_bytes": 4}),
            "a.py",
            "",
            "text",
            "x = (1,\n"
        ),
        json!({"hook": "none", "lang": "python", "note": "too_large_to_check"})
    );
    failed(
        &check_over(
            json!({"max_check_bytes": 64}),
            "a.py",
            "",
            "text",
            "x = (1,\n",
        ),
        "python",
        "'(' was never closed",
        1,
        5,
    );
}

#[test]
fn the_same_request_gets_the_same_answer() {
    if !shipped() {
        return;
    }
    let a = check("conf/app.toml", "", "text", "a=1\na=2\n");
    let b = check("conf/app.toml", "", "text", "a=1\na=2\n");
    assert_eq!(a, b);
    failed(&a, "toml", "Cannot overwrite a value", 2, 4);
}

#[test]
fn the_cell_is_a_cold_sandboxed_code_cell_without_a_store() {
    if !shipped() {
        return;
    }
    let cfg = read_json(&repo(GUARD));
    assert_eq!(cfg["cell"]["type"], json!("code"));
    assert_eq!(cfg["params"]["runner"], json!("python3"));
    assert_eq!(cfg["params"]["runner_mode"], json!("cold"));
    assert_eq!(cfg["params"]["sandbox"]["trust"], json!("restricted"));
    assert_eq!(cfg["params"]["sandbox"]["network"], json!("deny"));
    let script = cfg["params"]["script_inline"].as_str().unwrap();
    // pure: no store route, nothing run, no environment
    for word in [
        "exec(",
        "eval(",
        "os.environ",
        "open(",
        "subprocess",
        "\"route\": \"store\"",
    ] {
        assert!(!script.contains(word), "the guard script contains {word}");
    }
    assert_eq!(
        cfg["contract"]["emits"]["hop"]["route"]["values"],
        json!(["checked"])
    );
}
