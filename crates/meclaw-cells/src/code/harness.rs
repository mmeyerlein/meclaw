//! The embedded warm/resident harness and its frame contract.
//!
//! The harness is handed to the runner in `argv` (`python3 -c <HARNESS>`) and
//! the SCRIPT travels on the boot frame instead. That is not a detail: it is
//! why the warm path has no GH #349 problem at all -- the constant below is a
//! few thousand bytes, while `templates/memory-hive/recall` is the very script
//! that crossed `MAX_ARG_STRLEN` and forced the per-spawn temp file.

use crate::code::params::Script;
use crate::process::KillingTimeoutOutput;
use meclaw_core::serde_json::{Map, Value as JsonValue};

/// The harness program. `include_str!` rather than a file next to the binary:
/// a runner that has to find a companion file on disk is a deployment problem
/// the substrate does not need (same reasoning as `meclaw-surface`'s client).
pub(crate) const HARNESS: &str = include_str!("harness.py");

/// Build the first line the child reads: what to compile, and whether the
/// globals dict survives between messages.
pub(crate) fn boot_frame(script: &Script, persistent: bool) -> JsonValue {
    let mut o = Map::new();
    match script {
        Script::Inline(code) => o.insert("script".into(), JsonValue::String(code.clone())),
        Script::Path(path) => o.insert("script_path".into(), JsonValue::String(path.clone())),
    };
    o.insert("persistent".into(), JsonValue::Bool(persistent));
    JsonValue::Object(o)
}

/// Turn one answer frame into the very value a cold run returns.
///
/// This is the whole reason warm adds no `error_code` of its own: past this
/// function the cell cannot tell which runner produced the three values.
pub(crate) fn output_from_frame(v: &JsonValue) -> Result<KillingTimeoutOutput, String> {
    let exit_code = v
        .get("exit_code")
        .and_then(JsonValue::as_i64)
        .ok_or("runner frame without an integer exit_code")? as i32;
    let stdout = v
        .get("stdout")
        .and_then(|s| s.as_str())
        .ok_or("runner frame without a string stdout")?;
    let stderr = v
        .get("stderr")
        .and_then(|s| s.as_str())
        .ok_or("runner frame without a string stderr")?;
    Ok(KillingTimeoutOutput {
        exit_code,
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::params::Script;
    use std::io::Write;

    /// Boot the harness with `script`, feed it `docs` (one line each) and return
    /// one answer frame per document.
    fn drive(script: &str, persistent: bool, docs: &[&str]) -> Vec<JsonValue> {
        let mut child = std::process::Command::new("python3")
            .args(["-c", HARNESS])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("python3 on PATH");
        {
            let stdin = child.stdin.as_mut().expect("piped");
            let boot = boot_frame(&Script::Inline(script.to_string()), persistent);
            writeln!(stdin, "{boot}").unwrap();
            for d in docs {
                writeln!(stdin, "{d}").unwrap();
            }
        }
        let out = child.wait_with_output().expect("harness ends on stdin EOF");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| meclaw_core::serde_json::from_str(l).expect("every line is one frame"))
            .collect()
    }

    #[test]
    fn one_document_in_one_frame_out() {
        let frames = drive(
            r#"import sys,json; d=json.load(sys.stdin); print(json.dumps({"seen": d["body"]["n"]}), end="")"#,
            false,
            [
                r#"{"envelope":{},"body":{"n":1},"params":{}}"#,
                r#"{"envelope":{},"body":{"n":2},"params":{}}"#,
            ]
            .as_slice(),
        );
        assert_eq!(frames.len(), 2, "one answer per document");
        assert_eq!(frames[0]["exit_code"], 0);
        assert_eq!(frames[0]["stdout"], r#"{"seen": 1}"#);
        assert_eq!(frames[1]["stdout"], r#"{"seen": 2}"#);
        assert_eq!(frames[0]["stderr"], "");
    }

    #[test]
    fn a_raising_body_answers_exit_one_with_the_traceback_on_stderr() {
        let frames = drive(
            "raise ValueError('boom')",
            false,
            [r#"{"body":{}}"#].as_slice(),
        );
        assert_eq!(frames[0]["exit_code"], 1, "an exception is python's exit 1");
        assert_eq!(frames[0]["stdout"], "");
        let err = frames[0]["stderr"].as_str().unwrap();
        assert!(
            err.contains("ValueError: boom"),
            "the traceback travels: {err}"
        );
    }

    #[test]
    fn sys_exit_keeps_its_code_and_the_child_stays_alive() {
        let frames = drive(
            "import sys; print('x', end=''); sys.exit(3)",
            false,
            [r#"{"body":{}}"#, r#"{"body":{}}"#].as_slice(),
        );
        assert_eq!(frames.len(), 2, "sys.exit ends the BODY, not the runner");
        assert_eq!(frames[0]["exit_code"], 3);
        assert_eq!(frames[0]["stdout"], "x");
    }

    #[test]
    fn a_syntax_error_answers_every_message_the_same_way() {
        let frames = drive(
            "def (",
            false,
            [r#"{"body":{}}"#, r#"{"body":{}}"#].as_slice(),
        );
        assert_eq!(frames.len(), 2);
        for f in &frames {
            assert_eq!(
                f["exit_code"], 1,
                "a script that does not compile fails every run"
            );
            assert!(f["stderr"].as_str().unwrap().contains("SyntaxError"));
        }
    }

    #[test]
    fn a_frame_becomes_the_output_a_cold_run_produces() {
        let v = meclaw_core::serde_json::json!({"exit_code":2,"stdout":"o","stderr":"e"});
        let out = output_from_frame(&v).unwrap();
        assert_eq!(out.exit_code, 2);
        assert_eq!(out.stdout, b"o");
        assert_eq!(out.stderr, b"e");
        assert!(output_from_frame(&meclaw_core::serde_json::json!({"stdout":"o"})).is_err());
    }

    /// warm: a body that writes a global does NOT see it again. The dict is
    /// rebuilt per message, so accumulation is impossible rather than merely
    /// discouraged -- this is the property that makes `warm == cold`.
    #[test]
    fn warm_hands_every_message_a_fresh_namespace() {
        let script = "import json,sys\n\
                      n = globals().get('n', 0) + 1\n\
                      globals()['n'] = n\n\
                      sys.stdout.write(json.dumps({'n': n}))\n";
        let frames = drive(script, false, [r#"{"body":{}}"#; 3].as_slice());
        let seen: Vec<&str> = frames
            .iter()
            .map(|f| f["stdout"].as_str().unwrap())
            .collect();
        assert_eq!(seen, vec![r#"{"n": 1}"#, r#"{"n": 1}"#, r#"{"n": 1}"#]);
    }

    /// resident: the same script accumulates, because that is the mode's whole
    /// point. The two tests together are the semantic difference between the
    /// modes -- there is no third knob.
    #[test]
    fn resident_carries_its_namespace_across_messages() {
        let script = "import json,sys\n\
                      n = globals().get('n', 0) + 1\n\
                      globals()['n'] = n\n\
                      sys.stdout.write(json.dumps({'n': n}))\n";
        let frames = drive(script, true, [r#"{"body":{}}"#; 3].as_slice());
        let seen: Vec<&str> = frames
            .iter()
            .map(|f| f["stdout"].as_str().unwrap())
            .collect();
        assert_eq!(seen, vec![r#"{"n": 1}"#, r#"{"n": 2}"#, r#"{"n": 3}"#]);
    }

    /// Neither mode leaks the PREVIOUS message's document: the body reads its
    /// own line from `sys.stdin` and nothing else is in the pipe for it.
    #[test]
    fn the_body_reads_its_own_document_from_stdin() {
        let script =
            r#"import sys,json; d=json.load(sys.stdin); sys.stdout.write(str(d["body"]["n"]))"#;
        let frames = drive(
            script,
            true,
            [r#"{"body":{"n":7}}"#, r#"{"body":{"n":8}}"#].as_slice(),
        );
        assert_eq!(frames[0]["stdout"], "7");
        assert_eq!(frames[1]["stdout"], "8");
    }
}
