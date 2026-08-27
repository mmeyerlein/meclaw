//! The `code` cell's stdin document, for tests that run a shipped script
//! directly through `python3` instead of through a colony.
//!
//! The substrate hands a script a three-object document — `envelope`, `body`
//! and `params` — and there is exactly one place in the tree that builds it
//! (`meclaw_cells::code::wire::build_stdin_json`). A test that pipes its own
//! JSON into the script has to agree with that shape, or it pins a wire that
//! does not exist. This helper is that agreement, written once: hand it the
//! message the way a test naturally spells it (envelope fields and body slots
//! side by side, optionally a `params` object) and it returns the document the
//! substrate would have built.

use meclaw_core::serde_json::{Map, Value, json};

/// The envelope fields the substrate lifts out of the flat spelling.
const ENVELOPE_KEYS: &[&str] = &[
    "header",
    "target",
    "reply_to",
    "trace_id",
    "parent_message_id",
    "correlation_id",
    "ttl",
];

/// Turn a flatly spelled message into the `code` cell's stdin document.
///
/// Envelope fields move into `envelope`, a `params` key becomes the top-level
/// `params` object (`{}` when absent), and everything else is a body slot.
/// A non-object input yields an empty document rather than a panic, so a test
/// that pipes deliberate garbage still exercises the script's own guard.
#[must_use]
pub fn code_stdin(flat: &Value) -> Value {
    let mut envelope = Map::new();
    let mut body = Map::new();
    let mut params = json!({});
    if let Value::Object(o) = flat {
        for (k, v) in o {
            if k == "params" {
                params = v.clone();
            } else if ENVELOPE_KEYS.contains(&k.as_str()) {
                envelope.insert(k.clone(), v.clone());
            } else {
                body.insert(k.clone(), v.clone());
            }
        }
    }
    json!({"envelope": envelope, "body": body, "params": params})
}

/// The same document, ready to be written to a child process' stdin.
#[must_use]
pub fn code_stdin_bytes(flat: &Value) -> Vec<u8> {
    code_stdin(flat).to_string().into_bytes()
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string — the substitution the colony performs at instantiation.
///
/// A test that runs a shipped `script_inline` runs it BEFORE any colony has
/// touched it, so the `${…}` tokens are still in the source. Resolving them the
/// way the colony does is what makes the script under test the script that
/// ships.
///
/// # Panics
/// If the script contains an unterminated `${`.
#[must_use]
pub fn resolve_script_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The `params.script_inline` of a shipped `config.json`, with `${…}` resolved.
///
/// # Panics
/// If the file is unreadable, is not JSON, or carries no `params.script_inline`.
#[must_use]
pub fn shipped_script(config_path: &str) -> String {
    let raw = std::fs::read_to_string(config_path).unwrap_or_else(|e| panic!("{config_path}: {e}"));
    let v: Value =
        meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{config_path}: {e}"));
    resolve_script_vars(
        v["params"]["script_inline"]
            .as_str()
            .unwrap_or_else(|| panic!("{config_path}: no params.script_inline")),
    )
}

/// Run a shipped script over a real stdin document and return the raw output.
///
/// The script is handed to `python3` **on stdin** rather than in argv: a single
/// argv string is capped at 128 KiB and the shipped scripts are within sight of
/// it (GH #279). The stdin document goes in the same way, as a `StringIO` the
/// program installs before it executes the script, so the script reads exactly
/// what the substrate would have handed it.
///
/// # Panics
/// If `python3` cannot be spawned or the child cannot be waited on.
#[must_use]
pub fn run_shipped_script(script: &str, stdin_doc: &str) -> std::process::Output {
    use std::io::Write;
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        meclaw_core::serde_json::to_string(script).expect("script is a string"),
        meclaw_core::serde_json::to_string(stdin_doc).expect("doc is a string"),
    );
    let mut child = std::process::Command::new("python3")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run a shipped script over a flatly spelled message and return its ONE
/// emission.
///
/// The convenience path for the common case: [`code_stdin`] builds the
/// document, [`run_shipped_script`] runs it, and the stdout is parsed the way
/// the substrate parses it — an object is one message, an array is N
/// (`meclaw_cells::code::wire::parse_stdout_json`). A script that emitted an
/// array here is a script whose test wanted [`emit_all`].
///
/// # Panics
/// If the script exits non-zero, emits no JSON, or emits more than one message.
#[must_use]
pub fn emit_one(script: &str, flat: &Value) -> Value {
    let mut all = emit_all(script, flat);
    assert_eq!(
        all.len(),
        1,
        "expected exactly one emission, got {}",
        all.len()
    );
    all.remove(0)
}

/// Every emission of a shipped script over a flatly spelled message.
///
/// # Panics
/// If the script exits non-zero or emits something that is not JSON.
#[must_use]
pub fn emit_all(script: &str, flat: &Value) -> Vec<Value> {
    let out = run_shipped_script(script, &code_stdin(flat).to_string());
    assert!(
        out.status.success(),
        "the script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not json ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flat_spelling_is_split_into_the_three_objects() {
        let doc = code_stdin(&json!({
            "header": {"hop": {"route": "in_turn"}},
            "messages": [{"origin": "user", "type": "text", "text": "hi"}],
            "params": {"window_size": 7}
        }));
        assert_eq!(doc["envelope"]["header"]["hop"]["route"], "in_turn");
        assert_eq!(doc["body"]["messages"][0]["text"], "hi");
        assert_eq!(doc["params"]["window_size"], 7);
        assert_eq!(doc.as_object().map(Map::len), Some(3));
    }

    #[test]
    fn the_three_objects_are_there_even_for_an_empty_message() {
        let doc = code_stdin(&json!({}));
        assert_eq!(doc, json!({"envelope": {}, "body": {}, "params": {}}));
    }
}
