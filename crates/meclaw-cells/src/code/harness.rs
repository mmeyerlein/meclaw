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

/// GH #1060: the first bytes of a job frame. A plain stdin document never
/// starts this way (`wire::build_stdin_json` writes `{"body":` first), so the
/// harness tells the two apart without parsing every document.
pub(crate) const JOB_FRAME_MARK: &str = "{\"meclaw_job\":";

/// GH #1060: the line a warm/resident child reads for a job that carries a
/// credential: the document (as a JSON string, so the bytes the body reads are
/// exactly the plain line's) plus ONE environment entry of this job. The
/// harness sets the entry before the body runs and removes it after, also when
/// the body raises (`harness.py`, `_run`). A job without a credential is sent
/// as the plain document, byte for byte as before.
///
/// Written by hand rather than through `json!`: the key order is the mark.
pub(crate) fn job_frame(document: &str, env_name: &str, env_value: &str) -> String {
    let mut env = Map::new();
    env.insert(
        env_name.to_string(),
        JsonValue::String(env_value.to_string()),
    );
    format!(
        "{JOB_FRAME_MARK}1,\"env\":{},\"document\":{}}}",
        JsonValue::Object(env),
        JsonValue::String(document.to_string())
    )
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
        drive_in(None, script, persistent, docs)
    }

    /// [`drive`] with the child's working directory set: the directory `-c`
    /// puts first on `sys.path` as `''`.
    fn drive_in(
        cwd: Option<&std::path::Path>,
        script: &str,
        persistent: bool,
        docs: &[&str],
    ) -> Vec<JsonValue> {
        let mut command = std::process::Command::new("python3");
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        let mut child = command
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

    /// GH #1060: the frame starts with the mark the harness looks for, and the
    /// document inside is the plain line, byte for byte.
    #[test]
    fn gh1060_a_job_frame_carries_the_plain_document_and_one_entry() {
        let doc = r#"{"body":{"a":"\u00e9\n"},"envelope":{},"params":{}}"#;
        let frame = job_frame(doc, "MECLAW_CREDENTIAL", "stub-secret-0");
        assert!(frame.starts_with(JOB_FRAME_MARK), "{frame}");
        assert!(!frame.contains('\n'), "one line");
        let v: JsonValue = meclaw_core::serde_json::from_str(&frame).unwrap();
        assert_eq!(v["document"], doc);
        assert_eq!(v["env"]["MECLAW_CREDENTIAL"], "stub-secret-0");
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

    /// The cold run of `script` in `cwd`, exactly as `cell.rs` `build_command`
    /// starts a small inline script: `python3 -c <script>`, the document on
    /// stdin. Returns its stdout.
    fn cold_in(cwd: &std::path::Path, script: &str) -> String {
        let mut child = std::process::Command::new("python3")
            .current_dir(cwd)
            .args(["-c", script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("python3 on PATH");
        writeln!(child.stdin.as_mut().expect("piped"), r#"{{"body":{{}}}}"#).unwrap();
        let out = child.wait_with_output().expect("the cold run ends");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A working directory with a `json.py` in it that leaves a mark when it is
    /// imported -- the shadow module a gate script guards against by
    /// dropping `''` from `sys.path` before its first import (GH #1026).
    fn shadowed_dir() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            dir.path().join("json.py"),
            "open(__file__ + '.imported', 'w').close()\nPLANTED = True\n",
        )
        .expect("plant json.py");
        dir
    }

    /// GH #1026: the harness's own imports never come from the working
    /// directory, and a script that drops `''` before importing `json` gets the
    /// standard library one -- as its cold run does. Before the fix the harness
    /// imported `json` itself with `''` first on `sys.path`: the planted module
    /// ran inside the harness and the script's filter came too late.
    #[test]
    fn a_script_that_drops_the_working_directory_never_sees_its_json() {
        let script = "import sys\n\
                      if sys.path and sys.path[0] == '': del sys.path[0]\n\
                      import json\n\
                      sys.stdout.write(str(getattr(json, 'PLANTED', False)))\n";
        for persistent in [false, true] {
            let dir = shadowed_dir();
            let cold = cold_in(dir.path(), script);
            assert_eq!(cold, "False", "the cold baseline");
            assert!(!dir.path().join("json.py.imported").exists());
            let frames = drive_in(
                Some(dir.path()),
                script,
                persistent,
                [r#"{"body":{}}"#; 2].as_slice(),
            );
            assert_eq!(
                frames.len(),
                2,
                "persistent={persistent}: the harness answers: {frames:?}"
            );
            for f in &frames {
                assert_eq!(f["stdout"], cold, "persistent={persistent}: as cold: {f}");
            }
            assert!(
                !dir.path().join("json.py.imported").exists(),
                "persistent={persistent}: nothing imported the planted json.py"
            );
        }
    }

    /// The other half: a script that does NOT drop `''` imports the planted
    /// module, as its cold run does -- the harness does not quietly make warm
    /// safer than cold either, it makes it the same. And the harness keeps
    /// framing its answers with its own `json`, whatever the script imported
    /// under that name.
    #[test]
    fn a_script_that_keeps_the_working_directory_sees_what_cold_sees() {
        let script = "import sys, json\n\
                      sys.stdout.write(str(getattr(json, 'PLANTED', False)))\n";
        for persistent in [false, true] {
            let dir = shadowed_dir();
            let cold = cold_in(dir.path(), script);
            assert_eq!(cold, "True", "the cold baseline");
            let frames = drive_in(
                Some(dir.path()),
                script,
                persistent,
                [r#"{"body":{}}"#; 2].as_slice(),
            );
            assert_eq!(
                frames.len(),
                2,
                "persistent={persistent}: the harness answers: {frames:?}"
            );
            for f in &frames {
                assert_eq!(f["stdout"], cold, "persistent={persistent}: as cold: {f}");
                assert_eq!(f["exit_code"], 0);
            }
        }
    }

    /// GH #1026, review of Z-M2: dropping a harness module from `sys.modules`
    /// is half the job when its parent package stays -- importing a submodule
    /// also binds it as an attribute on the parent. Measured with python
    /// 3.12.3: `collections` is loaded at a cold start, `collections.abc` is
    /// not (it comes with the harness's `traceback`), and before this test
    /// `hasattr(collections, "abc")` was `False` cold but `True` warm. A
    /// script then reached `collections.abc` warm without any import, and a
    /// filter on that name never ran. So the script lists every
    /// `parent.child` submodule bound on a loaded package at its start; cold,
    /// warm and resident must list the same, on every message. The middle
    /// message raises: the harness's own `traceback` reads
    /// `collections.abc.Sequence` (python 3.12 `traceback.py`), so the
    /// traceback must still travel -- the first fix killed the child there --
    /// and the message after it must still find no binding.
    #[test]
    fn a_purged_submodule_is_not_reachable_through_its_parent() {
        let script = "import sys\n\
                      doc = sys.stdin.read()\n\
                      M = type(sys)\n\
                      c = sys.modules.get('collections')\n\
                      bound = sorted(\n\
                      \x20   p + '.' + k\n\
                      \x20   for p, m in list(sys.modules.items()) if isinstance(m, M)\n\
                      \x20   for k, v in list(vars(m).items())\n\
                      \x20   if isinstance(v, M) and getattr(v, '__name__', None) == p + '.' + k\n\
                      )\n\
                      sys.stdout.write(str(c is not None and hasattr(c, 'abc')) + '|' + ','.join(bound))\n\
                      if 'raise' in doc: raise ValueError('boom')\n";
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cold = cold_in(dir.path(), script);
        assert!(cold.contains('|'), "the cold baseline ran: {cold:?}");
        for persistent in [false, true] {
            let frames = drive_in(
                Some(dir.path()),
                script,
                persistent,
                [
                    r#"{"body":{}}"#,
                    r#"{"body":{"raise":true}}"#,
                    r#"{"body":{}}"#,
                ]
                .as_slice(),
            );
            assert_eq!(
                frames.len(),
                3,
                "persistent={persistent}: the harness answers: {frames:?}"
            );
            for f in &frames {
                assert_eq!(
                    f["stdout"], cold,
                    "persistent={persistent}: collections.abc and every bound submodule as cold: {f}"
                );
            }
            assert_eq!(frames[0]["exit_code"], 0, "persistent={persistent}");
            assert_eq!(frames[2]["exit_code"], 0, "persistent={persistent}");
            assert_eq!(frames[1]["exit_code"], 1, "persistent={persistent}");
            let err = frames[1]["stderr"].as_str().unwrap_or_default();
            assert!(
                err.contains("ValueError: boom"),
                "persistent={persistent}: the traceback travels: {err}"
            );
        }
    }

    /// The cold run of `script` in `cwd` with `doc` on stdin: (exit code,
    /// stdout, stderr), for tests whose cold baseline raises or needs its own
    /// document.
    fn cold_full(cwd: &std::path::Path, script: &str, doc: &str) -> (i32, String, String) {
        let mut child = std::process::Command::new("python3")
            .current_dir(cwd)
            .args(["-c", script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("python3 on PATH");
        writeln!(child.stdin.as_mut().expect("piped"), "{doc}").unwrap();
        let out = child.wait_with_output().expect("the cold run ends");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// The last `n` non-empty lines of a traceback: the exception and its
    /// notes, without the frames (the harness adds its own `_run` frame).
    fn tail_lines(text: &str, n: usize) -> Vec<String> {
        let lines: Vec<String> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        lines[lines.len().saturating_sub(n)..].to_vec()
    }

    /// GH #1026, re-review of Z-M2: a script that imports `collections.abc`
    /// itself and then raises an exception with a note -- the one path where
    /// `traceback` reads `collections.abc.Sequence` -- gets the same answer
    /// warm and resident as cold: its own module bound on `collections` and
    /// in `sys.modules`, exit 1, the exception and the note on stderr. The
    /// harness's `traceback` formats with its private `collections` and
    /// leaves the script's binding alone, on every message.
    #[test]
    fn a_raise_after_the_script_imported_collections_abc_itself_answers_as_cold() {
        let script = "import sys\n\
                      import collections.abc\n\
                      doc = sys.stdin.read()\n\
                      c = sys.modules['collections']\n\
                      own = sys.modules.get('collections.abc')\n\
                      sys.stdout.write('%s|%s' % (hasattr(c, 'abc'), own is not None and own is vars(c).get('abc')))\n\
                      e = ValueError('boom')\n\
                      e.add_note('noted')\n\
                      raise e\n";
        let dir = tempfile::TempDir::new().expect("tempdir");
        let (code, out, err) = cold_full(dir.path(), script, r#"{"body":{}}"#);
        assert_eq!(
            (code, out.as_str()),
            (1, "True|True"),
            "the cold baseline: {err}"
        );
        let cold_tail = tail_lines(&err, 2);
        assert_eq!(
            cold_tail,
            ["ValueError: boom", "noted"],
            "the cold baseline"
        );
        for persistent in [false, true] {
            let frames = drive_in(
                Some(dir.path()),
                script,
                persistent,
                [r#"{"body":{}}"#; 3].as_slice(),
            );
            assert_eq!(
                frames.len(),
                3,
                "persistent={persistent}: the harness answers: {frames:?}"
            );
            for f in &frames {
                assert_eq!(f["exit_code"], 1, "persistent={persistent}: {f}");
                assert_eq!(
                    f["stdout"],
                    out.as_str(),
                    "persistent={persistent}: as cold: {f}"
                );
                assert_eq!(
                    tail_lines(f["stderr"].as_str().unwrap_or_default(), 2),
                    cold_tail,
                    "persistent={persistent}: the traceback as cold: {f}"
                );
            }
        }
    }

    /// GH #1026, re-review of Z-M2: the first fix bound `collections.abc`
    /// back on the shared `collections` for as long as the harness printed a
    /// traceback, and a thread of the script saw that window (2496 hits in
    /// 200 raises; the switch interval set low here makes it show on every
    /// run). Now the harness's `traceback` has a private `collections`, and
    /// the shared one never carries `abc` unless the script imports it. A
    /// watcher thread started by the first message checks
    /// `"abc" in vars(collections)` in a loop while 300 messages raise; the
    /// last message reports the hits. Cold, the same watcher runs over 300
    /// raises the script catches itself (a cold run has one uncaught raise,
    /// on its way out). Warm and resident must never see what cold never
    /// sees.
    #[test]
    fn a_thread_of_the_script_never_sees_collections_abc_while_the_harness_reports_a_raise() {
        let script = "import sys, json, threading\n\
                      op = json.loads(sys.stdin.read())['body'].get('op', 'cold')\n\
                      def watch(st):\n\
                      \x20   c = sys.modules['collections']\n\
                      \x20   while not st['stop'].is_set():\n\
                      \x20       st['checks'] += 1\n\
                      \x20       if 'abc' in vars(c): st['hits'] += 1\n\
                      def start():\n\
                      \x20   sys.setswitchinterval(1e-6)\n\
                      \x20   st = {'stop': threading.Event(), 'checks': 0, 'hits': 0}\n\
                      \x20   threading.Thread(target=watch, args=(st,), daemon=True).start()\n\
                      \x20   return st\n\
                      def report(st):\n\
                      \x20   st['stop'].set()\n\
                      \x20   sys.stdout.write('hits=%d watched=%s' % (st['hits'], st['checks'] > 0))\n\
                      if op == 'cold':\n\
                      \x20   st = start()\n\
                      \x20   for _ in range(300):\n\
                      \x20       try: raise ValueError('boom')\n\
                      \x20       except ValueError: pass\n\
                      \x20   report(st)\n\
                      if op == 'start': sys.gh1026_watch = start()\n\
                      if op == 'raise': raise ValueError('boom')\n\
                      if op == 'report': report(sys.gh1026_watch)\n";
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cold = cold_in(dir.path(), script);
        assert_eq!(cold, "hits=0 watched=True", "the cold baseline");
        let mut docs = vec![r#"{"body":{"op":"start"}}"#];
        docs.extend([r#"{"body":{"op":"raise"}}"#; 300]);
        docs.push(r#"{"body":{"op":"report"}}"#);
        for persistent in [false, true] {
            let frames = drive_in(Some(dir.path()), script, persistent, &docs);
            assert_eq!(
                frames.len(),
                docs.len(),
                "persistent={persistent}: the harness answers every message"
            );
            for f in &frames[1..frames.len() - 1] {
                assert_eq!(f["exit_code"], 1, "persistent={persistent}: {f}");
                assert!(
                    f["stderr"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("ValueError: boom"),
                    "persistent={persistent}: the traceback travels: {f}"
                );
            }
            assert_eq!(
                frames[frames.len() - 1]["stdout"],
                cold.as_str(),
                "persistent={persistent}: the watcher saw collections.abc bound"
            );
        }
    }

    /// The names in `listed` (comma-separated, as the scripts below print
    /// `sorted(sys.modules)`) that `baseline` does not have, and the other way
    /// round -- the readable part of a failed comparison.
    fn module_diff(listed: &str, baseline: &str) -> (Vec<String>, Vec<String>) {
        let set = |s: &str| -> std::collections::BTreeSet<String> {
            s.split(',')
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect()
        };
        let (a, b) = (set(listed), set(baseline));
        (
            a.difference(&b).cloned().collect(),
            b.difference(&a).cloned().collect(),
        )
    }

    /// Drive `script` through a raising message and a reporting one, warm and
    /// resident, in `cwd`, and hold the report's `sys.modules` to the cold
    /// run's. The traceback of the raise must still travel.
    fn modules_after_a_raise_match_cold(cwd: &std::path::Path, script: &str, raised: &str) {
        let cold = cold_in(cwd, script);
        assert!(!cold.is_empty(), "the cold baseline ran");
        for persistent in [false, true] {
            let frames = drive_in(
                Some(cwd),
                script,
                persistent,
                [r#"{"body":{"raise":true}}"#, r#"{"body":{}}"#].as_slice(),
            );
            assert_eq!(
                frames.len(),
                2,
                "persistent={persistent}: the harness answers: {frames:?}"
            );
            assert_eq!(frames[0]["exit_code"], 1, "persistent={persistent}");
            let err = frames[0]["stderr"].as_str().unwrap_or_default();
            assert!(
                err.contains(raised),
                "persistent={persistent}: the traceback travels: {err}"
            );
            let listed = frames[1]["stdout"].as_str().unwrap_or_default();
            let (extra, missing) = module_diff(listed, &cold);
            assert!(
                extra.is_empty() && missing.is_empty(),
                "persistent={persistent}: sys.modules after the raise differs from cold -- \
                 extra {extra:?}, missing {missing:?}"
            );
        }
    }

    /// GH #1026, re-review of Z-M2: `traceback` imports `ast` inside its
    /// functions for the caret anchors of every frame whose source line it
    /// reads (python 3.12 `traceback.py` line 590), and `ast` brings `_ast`
    /// and `contextlib`. A raise inside `json.loads` is such a frame. Before
    /// the fix the three sat in `sys.modules` after the first raise, warm and
    /// resident, where a cold run has none of them. The harness now hands its
    /// `traceback` these modules from private references.
    #[test]
    fn a_raise_through_a_frame_with_source_leaves_sys_modules_as_cold() {
        let script = "import sys, json\n\
                      doc = sys.stdin.read()\n\
                      if 'raise' in doc: json.loads('{bad')\n\
                      sys.stdout.write(','.join(sorted(sys.modules)))\n";
        let dir = tempfile::TempDir::new().expect("tempdir");
        modules_after_a_raise_match_cold(dir.path(), script, "JSONDecodeError");
    }

    /// The same for a frame whose source line is not ASCII: `traceback` then
    /// also imports `unicodedata` to measure the line (python 3.12
    /// `traceback.py` line 647). The frame lives in a module of the working
    /// directory that the script imports itself, cold and warm alike.
    #[test]
    fn a_raise_through_a_non_ascii_source_line_leaves_sys_modules_as_cold() {
        let script = "import sys\n\
                      doc = sys.stdin.read()\n\
                      import accent_mod\n\
                      if 'raise' in doc: accent_mod.boom({'clé': None})\n\
                      sys.stdout.write(','.join(sorted(sys.modules)))\n";
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            dir.path().join("accent_mod.py"),
            "def boom(d):\n    return d['clé']['é'] + 1\n",
        )
        .expect("plant accent_mod.py");
        modules_after_a_raise_match_cold(dir.path(), script, "TypeError");
    }

    /// A filter the script installs after a raise decides `ast` and
    /// `contextlib` as it does cold: refused. Before the fix the harness's
    /// `traceback` had imported both during the raise, so the next `import`
    /// was answered from `sys.modules` and the filter never ran. The filter
    /// also records every name it is asked: a raise while it is installed
    /// (message 3) must not ask it anything, because the harness's own
    /// imports never reach the import system (before the fix `traceback`'s
    /// `import ast` went through the script's finder).
    #[test]
    fn a_filter_set_after_a_raise_refuses_what_traceback_needs_as_cold() {
        let script = "import sys, json\n\
                      op = json.loads(sys.stdin.read())['body'].get('op')\n\
                      if op == 'raise': json.loads('{bad')\n\
                      class Ask:\n\
                      \x20   @staticmethod\n\
                      \x20   def find_spec(name, path=None, target=None):\n\
                      \x20       sys.gh1026_asked.append(name)\n\
                      \x20       if name in ('ast', '_ast', 'contextlib'): raise ImportError('refused: ' + name)\n\
                      \x20       return None\n\
                      if not hasattr(sys, 'gh1026_asked'):\n\
                      \x20   sys.gh1026_asked = []\n\
                      \x20   sys.meta_path.insert(0, Ask)\n\
                      out = []\n\
                      for n in ('ast', 'contextlib'):\n\
                      \x20   try:\n\
                      \x20       __import__(n)\n\
                      \x20       out.append('allowed')\n\
                      \x20   except ImportError:\n\
                      \x20       out.append('refused')\n\
                      sys.stdout.write(','.join(out) + '|' + ','.join(sys.gh1026_asked))\n\
                      sys.gh1026_asked.clear()\n";
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cold = cold_in(dir.path(), script);
        assert_eq!(cold, "refused,refused|ast,contextlib", "the cold baseline");
        for persistent in [false, true] {
            let frames = drive_in(
                Some(dir.path()),
                script,
                persistent,
                [
                    r#"{"body":{"op":"raise"}}"#,
                    r#"{"body":{}}"#,
                    r#"{"body":{"op":"raise"}}"#,
                    r#"{"body":{}}"#,
                ]
                .as_slice(),
            );
            assert_eq!(
                frames.len(),
                4,
                "persistent={persistent}: the harness answers: {frames:?}"
            );
            for i in [0, 2] {
                assert_eq!(frames[i]["exit_code"], 1, "persistent={persistent}");
                let err = frames[i]["stderr"].as_str().unwrap_or_default();
                assert!(
                    err.contains("JSONDecodeError"),
                    "persistent={persistent}: message {i}: the traceback travels: {err}"
                );
            }
            for i in [1, 3] {
                assert_eq!(
                    frames[i]["stdout"],
                    cold.as_str(),
                    "persistent={persistent}: message {i}: the script's filter decides as cold, \
                     and the harness asked it nothing"
                );
            }
        }
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
