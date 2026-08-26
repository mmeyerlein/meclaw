//! CodeCell: stateless cell that runs a user-supplied script (Python
//! first) as a subprocess. stdin = serialized Message; stdout = content
//! JSON (Object → 1 msg, Array → N msgs if multi_send_capable).

use crate::code::{CodeParams, params::Script, wire};
use crate::process::{KillingTimeoutErr, KillingTimeoutOutput, with_killing_timeout};
use meclaw_colony::StatelessCell;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{CellOutput, Message, OutputSink, Path};

/// Phase-9 CodeCell: stateless cell that runs a user-supplied script
/// (Python first) as a subprocess. stdin = serialized Message;
/// stdout = content JSON (Object → 1 msg, Array → N msgs if
/// `multi_send_capable`).
pub struct CodeCell {
    /// Pre-validated params (runner = "python3", script source, optional
    /// timeout + concurrency).
    pub params: CodeParams,
    /// Whether this cell may emit multiple output messages per input.
    /// Set from `contract.multi_send_capable` via the `CellFactory` trait
    /// param (Phase 11). The Phase-9 `params`-bridge has been removed.
    pub multi_send_capable: bool,
    /// Pre-compiled emits validators (P13/D-017). `None` if no `emits`
    /// declared. Set from `contract.emits` via the `CellFactory` trait param.
    pub emits: Option<std::sync::Arc<meclaw_core::CompiledEmits>>,
    /// Effective enforcement flag (resolved at spawn by Colony,
    /// `cfg!(debug_assertions) || strict_validation`).
    pub validate_emits: bool,
    /// The read-only, secret-filtered copy of the cell's raw params handed to
    /// the script on stdin under the top-level `params` key (W12 route A).
    /// Already filtered by [`wire::filter_params_for_stdin`] at construction,
    /// so no credential ever sits in this struct. Empty object by default —
    /// the production path fills it via [`CodeCell::with_stdin_params`].
    pub stdin_params: Value,
    /// The runner this cell was built with. `pub(crate)` rather than `pub`
    /// because a pool handle is crate-internal plumbing, not part of the cell's
    /// published shape.
    pub(crate) runner: RunnerHandle,
}

/// How a `CodeCell` actually runs its script.
///
/// `Cold` is not a fallback and not a degraded mode: it is the same code path
/// the cell had before the runner modes existed, reached by a value that says
/// so. The pooled variant is what `warm` and `resident` attach.
pub(crate) enum RunnerHandle {
    /// One fresh process per message.
    Cold,
    /// A pool of resident children (`warm`: `size` of them, fresh namespace per
    /// message; `resident`: exactly one, persistent namespace).
    Pooled(crate::code::pool::PoolHandle),
}

impl CodeCell {
    /// Construct a new CodeCell. Implementation detail — production
    /// entry is `CodeCellFactory`.
    #[doc(hidden)]
    pub fn new(
        params: CodeParams,
        multi_send_capable: bool,
        emits: Option<std::sync::Arc<meclaw_core::CompiledEmits>>,
        validate_emits: bool,
    ) -> Self {
        Self {
            params,
            multi_send_capable,
            emits,
            validate_emits,
            stdin_params: Value::Object(Map::new()),
            runner: RunnerHandle::Cold,
        }
    }

    /// Attach the cell's raw params, secret-filtered, as the `params` copy the
    /// script reads on stdin (W12 route A). Additive builder step: a cell built
    /// without it hands the script `"params": {}` — the pre-W12 behaviour.
    ///
    /// The filter runs HERE, not per message, so the raw credential values are
    /// dropped before they can be stored on the cell.
    #[doc(hidden)]
    #[must_use]
    pub fn with_stdin_params(mut self, raw_params: &Value) -> Self {
        self.stdin_params = wire::filter_params_for_stdin(raw_params);
        self
    }

    /// Attach the warm/resident pool the params ask for.
    ///
    /// A `cold` params returns the cell UNCHANGED, so the default path is not
    /// merely equivalent to the pre-lane one -- it is the same code. Must be
    /// called inside a tokio runtime (it spawns the broker); the production
    /// caller is `CodeCellFactory`, and a cell built by `new()` alone stays
    /// cold no matter what its params say.
    #[doc(hidden)]
    #[must_use]
    pub(crate) fn with_runner_pool(mut self, params: &CodeParams, size: usize) -> Self {
        if let Some(cfg) = crate::code::child::config_for(params) {
            self.runner = RunnerHandle::Pooled(crate::code::pool::spawn_pool(cfg, size));
        }
        self
    }

    /// The cold runner: one fresh process per message.
    ///
    /// Byte-for-byte the pre-lane path — moved out of `handle()` so the warm
    /// branch can sit BESIDE it instead of inside it. Emits its own error bodies
    /// and answers `None` when it did; `Some(out)` feeds the shared tail
    /// (headers, contract check, emission) that both runners share.
    ///
    /// `cell_path` is `msg.target`: the orphan journal labels the child with the
    /// cell's own address, and that label is part of the pre-lane behaviour, so
    /// it travels as a parameter rather than being reinvented here.
    async fn run_cold(
        &self,
        stdin_json: String,
        timeout: std::time::Duration,
        sink: &OutputSink,
        reply_target: &Path,
        cell_path: &Path,
        started: std::time::Instant,
    ) -> Option<KillingTimeoutOutput> {
        // GH #349: an inline script above the platform's per-argv-string cap
        // (Linux: MAX_ARG_STRLEN = 32 * PAGE_SIZE) cannot be handed to the
        // runner in `argv` at all — `spawn()` answers `Argument list too
        // long`. stdin is not free either: it carries the DOCUMENT, which
        // the script reads. So the script goes to a per-spawn temp file and
        // the runner is pointed at it, the same `<runner> <path>` form
        // `script_path` already uses. Scripts under the cap keep the argv
        // path unchanged. The guard unlinks the file when it drops — bound
        // for the whole run so the file outlives the child.
        let materialised = match &self.params.script {
            Script::Inline(code) => {
                match crate::code::script_file::materialise_if_oversized(code, timeout).await {
                    Ok(m) => m,
                    Err(e) => {
                        emit_spawn_error(
                            sink,
                            reply_target,
                            format!("inline script could not be materialised: {e}"),
                        )
                        .await;
                        return None;
                    }
                }
            }
            Script::Path(_) => None,
        };
        let script_file_path = materialised.as_ref().map(|m| m.path());

        let (cmd, args) = build_command(&self.params, script_file_path);
        let mut command = tokio::process::Command::new(&cmd);
        command
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // S4 (GH #35): the sandbox is installed on the command, not around
        // it. A profile that cannot be applied fails HERE, before a child
        // exists — a `restricted` cell never falls back to unsandboxed.
        //
        // GH #85: the returned scope owns the child's cgroup and must stay
        // alive until the child is reaped, so it is bound rather than
        // dropped on the spot.
        //
        // GH #349: when the script was materialised, the profile gets one
        // extra read grant — the script file itself. `cell-types.md` §
        // `code` promises a `script_inline` needs no declaration of its own,
        // and the runner has to be able to open the program it is told to
        // run. Nothing else is widened, and nothing of this reaches
        // `config.json`.
        let effective_profile = match (&self.params.sandbox, script_file_path) {
            (Some(profile), Some(path)) => Some(profile.with_readable_file(path)),
            _ => None,
        };
        let profile_in_force = effective_profile.as_ref().or(self.params.sandbox.as_ref());
        let _sandbox_scope = match profile_in_force {
            None => crate::sandbox::SandboxScope::empty(),
            Some(profile) => match crate::sandbox::apply(profile, &mut command) {
                Ok(scope) => scope,
                Err(e) => {
                    emit_spawn_error(sink, reply_target, format!("sandbox not applied: {e}")).await;
                    return None;
                }
            },
        };
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                emit_spawn_error(sink, reply_target, e.to_string()).await;
                return None;
            }
        };

        // GH #116: same crash-durable record as `bash` — see the note at
        // that spawn site. Taken before the pipes are moved out, because
        // `Child::id()` is only meaningful while the child is unreaped.
        let _journal_note = crate::orphan_journal::note_spawn(child.id(), None, cell_path.as_str());

        // Concurrent stdin write: avoids a pipe deadlock with large
        // stdin/stdout — the stdin pipe buffer fills up and the script waits
        // on the stdout reader, which only starts in with_killing_timeout.
        let stdin_opt = child.stdin.take();
        let stdin_bytes = stdin_json.into_bytes();
        let _write_task = tokio::spawn(async move {
            if let Some(mut s) = stdin_opt {
                use tokio::io::AsyncWriteExt;
                // Swallow rationale (documented POC boundary): a broken
                // pipe (script exits before consuming stdin) is already the
                // observable signal via the script's exit code / stdout
                // parse (surfaces as `invalid_json`/`script_failed`), so the
                // write error needs no separate diagnostic here.
                let _ = s.write_all(&stdin_bytes).await;
                // Dropping closes the pipe → the script sees EOF.
            }
        });

        match with_killing_timeout(child, timeout).await {
            Ok(o) => Some(o),
            Err(KillingTimeoutErr::Elapsed) => {
                emit_script_timeout(sink, reply_target, started).await;
                None
            }
            Err(KillingTimeoutErr::Io(e)) => {
                emit_spawn_error(sink, reply_target, e.to_string()).await;
                None
            }
        }
    }
}

/// Build `(cmd, args)` for `tokio::process::Command::new(cmd).args(args)`.
///
/// `materialised` is the temp file an oversized `script_inline` was written to
/// (GH #349, see [`crate::code::script_file`]); when it is `Some`, the runner is
/// pointed at that path instead of being handed the script in `argv`.
fn build_command(p: &CodeParams, materialised: Option<&std::path::Path>) -> (String, Vec<String>) {
    if let Some(path) = materialised {
        return (p.runner.clone(), vec![path.to_string_lossy().into_owned()]);
    }
    match &p.script {
        Script::Path(path) => (p.runner.clone(), vec![path.clone()]),
        Script::Inline(code) => (p.runner.clone(), vec!["-c".into(), code.clone()]),
    }
}

/// Inject `exit_code`/`duration_ms`/`had_stderr` headers. Per spec
/// (cell-types.md Z.220–225) these OVERRIDE any matching keys the
/// script wrote.
///
/// **Fallible on purpose (W13 hardening).** `parse_stdout_json` accepts any
/// top-level object or array, so the shape checks that used to live here as
/// `.expect()` sat on the far side of a trust boundary: a script printing `[1]`
/// or `{"header": 5}` is foreign input, and it took the cell task down with
/// `parsed not object` / `header not object` instead of answering. Both are now
/// the same class the caller already knows — `invalid_json`, "your stdout is not
/// the wire shape" — which is the only classification that is true of them.
fn inject_standard_headers(
    mut content: Value,
    exit_code: i32,
    duration_ms: i64,
    had_stderr: bool,
) -> Result<Value, String> {
    let obj = content
        .as_object_mut()
        .ok_or("invalid_json: every emitted message must be a JSON object")?;
    let header = obj
        .entry("header".to_string())
        .or_insert(Value::Object(Map::new()));
    let h_obj = header
        .as_object_mut()
        .ok_or("invalid_json: \"header\" must be a JSON object")?;
    h_obj.insert("exit_code".into(), Value::from(exit_code));
    h_obj.insert("duration_ms".into(), Value::from(duration_ms));
    h_obj.insert("had_stderr".into(), Value::Bool(had_stderr));
    Ok(content)
}

/// Byte cap for the stderr content copied into the `log.jsonl` warn line.
///
/// A script's stderr is unbounded foreign content (compiler output, stack
/// traces). The repo has no shared truncation convention for logged foreign
/// content, so `code` caps at 8 KiB — large enough for a full Python traceback,
/// small enough that one noisy run cannot flood `log.jsonl`.
const STDERR_LOG_MAX_BYTES: usize = 8 * 1024;

/// Cap `s` at [`STDERR_LOG_MAX_BYTES`], cutting on a UTF-8 char boundary and
/// appending an explicit marker with the original byte length. Truncation must
/// be visible — a silently shortened diagnostic is worse than a marked one.
fn truncate_stderr_for_log(s: &str) -> String {
    if s.len() <= STDERR_LOG_MAX_BYTES {
        return s.to_string();
    }
    let mut end = STDERR_LOG_MAX_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated, {} bytes total]", &s[..end], s.len())
}

/// Emit an `invalid_input` error body to `target`.
async fn emit_invalid_input(sink: &OutputSink, target: &Path, msg: String) {
    let body = json!({
        "header":{"finish_reason":"error","error_code":"invalid_input"},
        "messages":[{"origin":"tool","type":"tool_result","text":msg,"id":""}]
    });
    let _ = sink
        .push(CellOutput {
            target: target.clone(),
            content: body,
        })
        .await;
}

/// Emit a spawn / I/O error body to `target`.
async fn emit_spawn_error(sink: &OutputSink, target: &Path, msg: String) {
    let body = json!({
        "header":{"finish_reason":"error","error_code":"io_error"},
        "messages":[{"origin":"tool","type":"tool_result","text":msg,"id":""}]
    });
    let _ = sink
        .push(CellOutput {
            target: target.clone(),
            content: body,
        })
        .await;
}

/// Emit a `script_timeout` error body to `target`.
async fn emit_script_timeout(sink: &OutputSink, target: &Path, started: std::time::Instant) {
    let dms = started.elapsed().as_millis() as i64;
    let body = json!({
        "header":{"finish_reason":"error","error_code":"script_timeout","duration_ms":dms},
        "messages":[{"origin":"tool","type":"tool_result","text":"script timed out","id":""}]
    });
    let _ = sink
        .push(CellOutput {
            target: target.clone(),
            content: body,
        })
        .await;
}

/// Emit a `script_failed` error body (non-zero exit code) to `target`.
async fn emit_script_failed(
    sink: &OutputSink,
    target: &Path,
    out: &KillingTimeoutOutput,
    duration_ms: i64,
) {
    // B.1: emit the failing script's output in the shared `bash` sentinel-marker
    // form — stdout followed by a delimited stderr block — so `code` and `bash`
    // produce an identical combined-output shape on the failure path.
    let stdout_s = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr_s = String::from_utf8_lossy(&out.stderr).to_string();
    let text = if out.stderr.is_empty() {
        stdout_s
    } else {
        format!(
            "{stdout_s}\n{}\n{stderr_s}{}",
            crate::bash::STDERR_START,
            crate::bash::STDERR_END
        )
    };
    let body = json!({
        "header":{
            "finish_reason":"error","error_code":"script_failed",
            "exit_code": out.exit_code,
            "had_stderr": !out.stderr.is_empty(),
            "duration_ms": duration_ms,
        },
        "messages":[{"origin":"tool","type":"tool_result","text":text,"id":""}]
    });
    let _ = sink
        .push(CellOutput {
            target: target.clone(),
            content: body,
        })
        .await;
}

/// Emit an `invalid_json` error body to `target`.
async fn emit_invalid_json(
    sink: &OutputSink,
    target: &Path,
    msg: String,
    duration_ms: i64,
    out: &KillingTimeoutOutput,
) {
    let body = json!({
        "header":{
            "finish_reason":"error","error_code":"invalid_json",
            "duration_ms":duration_ms,"exit_code":out.exit_code,
            "had_stderr": !out.stderr.is_empty(),
        },
        "messages":[{"origin":"tool","type":"tool_result","text":msg,"id":""}]
    });
    let _ = sink
        .push(CellOutput {
            target: target.clone(),
            content: body,
        })
        .await;
}

/// Emit a contract-violation error body to `target`. Handles both the
/// multi-send-not-declared case and a `contract.emits` body/header violation.
async fn emit_contract_violation(
    sink: &OutputSink,
    target: &Path,
    code: &'static str,
    duration_ms: i64,
) {
    let detail = match code {
        "multi_send_not_declared" => "script emitted JSON-Array without multi_send_capable",
        "contract_violation" => "script output violates contract.emits",
        _ => "contract violation",
    };
    let body = json!({
        "header":{
            "finish_reason":"error","error_code":code,"duration_ms":duration_ms,
        },
        "messages":[{"origin":"tool","type":"tool_result","text":detail,"id":""}]
    });
    let _ = sink
        .push(CellOutput {
            target: target.clone(),
            content: body,
        })
        .await;
}

#[allow(clippy::manual_async_fn)]
impl StatelessCell for CodeCell {
    /// Handle one message: serialize to stdin-JSON, spawn the configured
    /// runner script as a subprocess, collect stdout, parse output JSON,
    /// inject standard headers (`exit_code`/`duration_ms`/`had_stderr`)
    /// and push to `sink`. Error paths emit structured error bodies
    /// with `finish_reason: "error"`.
    fn handle<'a>(
        &'a self,
        msg: Message,
        sink: &'a OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            let started = std::time::Instant::now();
            // W2b (Ruling A1, mirror of proxy::emit_inbound_error): on a missing
            // `reply_to`, `/colony/dead_letters` is the READ endpoint, NOT a valid
            // delivery target. Emit the error as a normal emission to the input's
            // own `msg.target`; matching no out-edge it ends LOUDLY as `no_route`
            // in the DLQ instead of being silently parked at the read endpoint.
            let reply_target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());

            let stdin_json = match wire::build_stdin_json(&msg, &self.stdin_params) {
                Ok(s) => s,
                Err(e) => {
                    emit_invalid_input(sink, &reply_target, e).await;
                    return;
                }
            };

            let timeout =
                std::time::Duration::from_millis(self.params.external_timeout_ms.unwrap_or(60_000));

            let out = match &self.runner {
                RunnerHandle::Cold => {
                    self.run_cold(
                        stdin_json,
                        timeout,
                        sink,
                        &reply_target,
                        &msg.target,
                        started,
                    )
                    .await
                }
                RunnerHandle::Pooled(pool) => {
                    // Every branch below lands on an error path the cold runner
                    // already owns; the warm runner adds no `error_code`.
                    match pool.run(stdin_json, timeout).await {
                        crate::code::pool::JobOutcome::Ran(out) => Some(out),
                        crate::code::pool::JobOutcome::Timeout => {
                            emit_script_timeout(sink, &reply_target, started).await;
                            None
                        }
                        crate::code::pool::JobOutcome::Io(e) => {
                            emit_spawn_error(sink, &reply_target, e).await;
                            None
                        }
                    }
                }
            };
            let Some(out) = out else { return };

            let duration_ms = started.elapsed().as_millis() as i64;
            let had_stderr = !out.stderr.is_empty();

            if out.exit_code != 0 {
                emit_script_failed(sink, &reply_target, &out, duration_ms).await;
                return;
            }

            // cell-types.md § code: on a successful run (exit 0) stderr is NOT
            // injected into the script output; `header.had_stderr` flags it and
            // the stderr CONTENT lands in `log.jsonl` at warn level. This
            // warn line IS that persistence (the tracing subscriber writes
            // `log.jsonl`). The exit != 0 path carries stderr inside the error
            // message instead (`emit_script_failed`), so it returns above and
            // never reaches this line.
            if had_stderr {
                tracing::warn!(
                    path = %msg.target.as_str(),
                    trace_id = %msg.trace_id,
                    exit_code = out.exit_code,
                    stderr = %truncate_stderr_for_log(&String::from_utf8_lossy(&out.stderr)),
                    "code script wrote to stderr on a successful run"
                );
            }

            let stdout_text = String::from_utf8_lossy(&out.stdout).to_string();
            let parsed = match wire::parse_stdout_json(&stdout_text) {
                Ok(p) => p,
                Err(e) => {
                    emit_invalid_json(sink, &reply_target, e, duration_ms, &out).await;
                    return;
                }
            };

            if parsed.len() > 1 && !self.multi_send_capable {
                emit_contract_violation(
                    sink,
                    &reply_target,
                    "multi_send_not_declared",
                    duration_ms,
                )
                .await;
                return;
            }

            // Auflage A1+A2: validate the FINAL wire-level content (after
            // inject_standard_headers), and do it in two passes so a violation
            // in any message yields exactly ONE contract_violation reply with
            // ZERO regular emits (no partial-emit mix).
            // Pass 1: build with_headers for ALL parsed contents AND validate each.
            let mut wire: Vec<Value> = Vec::with_capacity(parsed.len());
            for content in parsed {
                let with_headers = match inject_standard_headers(
                    content,
                    out.exit_code,
                    duration_ms,
                    had_stderr,
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        emit_invalid_json(sink, &reply_target, e, duration_ms, &out).await;
                        return;
                    }
                };
                if self.validate_emits
                    && let Some(compiled) = &self.emits
                    && let Err(reason) = meclaw_core::validate_emits(&with_headers, compiled)
                {
                    tracing::warn!(error = %reason, "code emit violates contract.emits");
                    emit_contract_violation(sink, &reply_target, "contract_violation", duration_ms)
                        .await;
                    return;
                }
                wire.push(with_headers);
            }
            // Pass 2: push all only now that all are valid.
            for with_headers in wire {
                let _ = sink
                    .push(CellOutput {
                        target: reply_target.clone(),
                        content: with_headers,
                    })
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::{CodeParams, RunnerMode, Script};
    use meclaw_colony::StatelessCell;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tokio::sync::mpsc;

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

    /// Minimal valid params for unit tests (no-op python3 script).
    fn sample_params() -> CodeParams {
        CodeParams {
            runner: "python3".into(),
            script: Script::Inline("pass".into()),
            external_timeout_ms: Some(10_000),
            max_concurrency: None,
            sandbox: None,
            runner_mode: RunnerMode::Cold,
        }
    }

    #[test]
    fn truncate_stderr_for_log_passes_short_input_through() {
        assert_eq!(truncate_stderr_for_log("short stderr"), "short stderr");
        let exact = "x".repeat(STDERR_LOG_MAX_BYTES);
        assert_eq!(truncate_stderr_for_log(&exact), exact);
    }

    #[test]
    fn truncate_stderr_for_log_caps_on_a_char_boundary_with_marker() {
        // Multi-byte chars straddling the cap: cutting at the raw byte index
        // would panic, so the helper walks back to a char boundary.
        let long = "é".repeat(STDERR_LOG_MAX_BYTES); // 2 bytes each
        let out = truncate_stderr_for_log(&long);
        assert!(out.contains("[truncated,"), "cut must be marked: {out}");
        assert!(
            out.contains(&format!("{} bytes total", long.len())),
            "marker must name the original byte length: {out}"
        );
        // Kept prefix stays under the cap and is valid UTF-8 (it is a &str).
        let kept = out.split('…').next().unwrap();
        assert!(kept.len() <= STDERR_LOG_MAX_BYTES);
    }

    /// The runner a cell gets when nobody says otherwise. `cold` is not merely
    /// the documented default — it is the default of the VALUE, so a cell built
    /// by any path other than the factory cannot silently become warm.
    #[test]
    fn a_freshly_built_cell_runs_cold() {
        let cell = CodeCell::new(sample_params(), false, None, false);
        assert!(
            matches!(cell.runner, RunnerHandle::Cold),
            "CodeCell::new must not attach a pool"
        );
    }

    #[test]
    fn code_cell_carries_emits_validator() {
        let emits: meclaw_core::EmitsBlock =
            meclaw_core::serde_json::from_str(r#"{"body":{"messages":{"type":"array"}},"hop":{}}"#)
                .unwrap();
        let compiled = std::sync::Arc::new(meclaw_core::CompiledEmits::compile(&emits).unwrap());
        let cell = CodeCell::new(sample_params(), false, Some(compiled), true);
        assert!(cell.validate_emits);
        assert!(cell.emits.is_some());
    }

    // ---- C2 test scaffolding -------------------------------------------------

    /// Build a `CodeCell` whose script writes `script_stdout` verbatim to
    /// stdout (single Object emission), with a compiled `emits` contract and
    /// the given `validate` flag. `multi_send_capable=false`.
    fn code_cell_with_emits(emits_json: &str, validate: bool, script_stdout: &str) -> CodeCell {
        code_cell_with_emits_inner(emits_json, validate, script_stdout, false)
    }

    /// Like `code_cell_with_emits` but `multi_send_capable=true` — `script_stdout`
    /// is expected to be a JSON array (multi-send).
    fn code_cell_with_emits_multi(
        emits_json: &str,
        validate: bool,
        script_stdout: &str,
    ) -> CodeCell {
        code_cell_with_emits_inner(emits_json, validate, script_stdout, true)
    }

    fn code_cell_with_emits_inner(
        emits_json: &str,
        validate: bool,
        script_stdout: &str,
        multi_send: bool,
    ) -> CodeCell {
        let emits: meclaw_core::EmitsBlock = meclaw_core::serde_json::from_str(emits_json).unwrap();
        let compiled = std::sync::Arc::new(meclaw_core::CompiledEmits::compile(&emits).unwrap());
        // The script echoes a fixed literal — embed it as a python string
        // literal via serde_json so quoting is safe.
        let literal = meclaw_core::serde_json::to_string(script_stdout).unwrap();
        let script = format!("import sys; sys.stdout.write({literal})");
        let params = CodeParams {
            runner: "python3".into(),
            script: Script::Inline(script),
            external_timeout_ms: Some(10_000),
            max_concurrency: None,
            sandbox: None,
            runner_mode: RunnerMode::Cold,
        };
        CodeCell::new(params, multi_send, Some(compiled), validate)
    }

    /// Drive `cell.handle` once and return the single pushed emission.
    async fn run_handle(cell: &CodeCell) -> meclaw_core::CellEmission {
        let mut outs = run_handle_collect(cell).await;
        assert_eq!(outs.len(), 1, "expected exactly one emission");
        outs.remove(0)
    }

    /// Drive `cell.handle` once and collect ALL pushed emissions.
    async fn run_handle_collect(cell: &CodeCell) -> Vec<meclaw_core::CellEmission> {
        let (otx, mut orx) = mpsc::channel(16);
        let sink = make_sink(otx);
        let msg = MessageBuilder::new(Path::new("/code"))
            .body(Body::Inline(json!({"messages":[]})))
            .reply_to(Path::new("/sink"))
            .build();
        cell.handle(msg, &sink).await;
        drop(sink);
        let mut outs = Vec::new();
        while let Some(em) = orx.recv().await {
            outs.push(em);
        }
        outs
    }

    /// Build a cell in `mode` around `script`, pool included when the mode asks
    /// for one.
    fn cell_in_mode(script: &str, mode: RunnerMode, multi: bool) -> CodeCell {
        let params = CodeParams {
            runner: "python3".into(),
            script: Script::Inline(script.into()),
            external_timeout_ms: Some(10_000),
            max_concurrency: None,
            sandbox: None,
            runner_mode: mode,
        };
        let size = params.effective_max_concurrency();
        CodeCell::new(params.clone(), multi, None, false).with_runner_pool(&params, size)
    }

    /// THE claim of this lane: a warm runner changes latency, never semantics.
    /// Same script, same message, same bytes out -- headers included, except
    /// `duration_ms`, which is a measurement and is meant to differ.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn warm_answers_the_same_bytes_as_cold() {
        const SCRIPTS: [(&str, bool); 4] = [
            // a plain body
            (
                r#"import sys,json; sys.stdout.write(json.dumps({"messages":[{"origin":"assistant","type":"text","text":"ok"}]}))"#,
                false,
            ),
            // stderr on a successful run (header.had_stderr, content not injected)
            (
                r#"import sys,json; sys.stderr.write("noise"); sys.stdout.write(json.dumps({"messages":[]}))"#,
                false,
            ),
            // a non-zero exit (script_failed with the bash sentinel form)
            (r#"import sys; sys.stderr.write("bad"); sys.exit(2)"#, false),
            // stdout that is not the wire shape (invalid_json)
            (r#"import sys; sys.stdout.write("not json")"#, false),
        ];
        for (script, multi) in SCRIPTS {
            let cold = run_handle_collect(&cell_in_mode(script, RunnerMode::Cold, multi)).await;
            let warm = run_handle_collect(&cell_in_mode(script, RunnerMode::Warm, multi)).await;
            assert_eq!(
                cold.len(),
                warm.len(),
                "emission count differs for {script}"
            );
            for (c, w) in cold.iter().zip(warm.iter()) {
                let mut c = c.content.clone();
                let mut w = w.content.clone();
                for v in [&mut c, &mut w] {
                    if let Some(h) = v.get_mut("header").and_then(|h| h.as_object_mut()) {
                        h.remove("duration_ms");
                    }
                }
                assert_eq!(c, w, "warm changed the answer for {script}");
            }
        }
    }

    /// A `code` cell whose runner never starts answers `io_error`, the code the
    /// cold path already uses for a failed spawn -- not a new one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_pool_that_cannot_spawn_answers_io_error() {
        let params = CodeParams {
            runner: "python3".into(),
            script: Script::Inline("pass".into()),
            external_timeout_ms: Some(5_000),
            max_concurrency: None,
            sandbox: None,
            runner_mode: RunnerMode::Warm,
        };
        let mut broken = params.clone();
        broken.runner = "definitely-not-a-runner-xyz".into();
        let cell = CodeCell::new(params, false, None, false).with_runner_pool(&broken, 1);
        let out = run_handle(&cell).await;
        assert_eq!(out.header_error_code(), Some("io_error"));
    }

    /// Accessor helpers for an emission's header fields.
    trait EmissionHeader {
        fn header_error_code(&self) -> Option<&str>;
        fn header_finish_reason(&self) -> Option<&str>;
    }
    impl EmissionHeader for meclaw_core::CellEmission {
        fn header_error_code(&self) -> Option<&str> {
            self.content["header"]["error_code"].as_str()
        }
        fn header_finish_reason(&self) -> Option<&str> {
            self.content["header"]["finish_reason"].as_str()
        }
    }

    /// W13 hardening: `[1]` and `{"header": 5}` are foreign input, not bugs.
    ///
    /// Both used to reach `.expect()` inside `inject_standard_headers` and take
    /// the cell task down at a trust boundary. `parse_stdout_json` lets them
    /// through by design (it checks the TOP level only), so the shape check has
    /// to answer rather than panic.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_non_object_emission_is_invalid_json_not_a_panic() {
        let cell = code_cell_with_emits_multi(r#"{"body":{},"hop":{}}"#, false, "[1]");
        let out = run_handle(&cell).await;
        assert_eq!(out.header_error_code(), Some("invalid_json"));
        assert_eq!(out.header_finish_reason(), Some("error"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_non_object_header_is_invalid_json_not_a_panic() {
        let cell = code_cell_with_emits(r#"{"body":{},"hop":{}}"#, false, r#"{"header":5}"#);
        let out = run_handle(&cell).await;
        assert_eq!(out.header_error_code(), Some("invalid_json"));
        assert_eq!(out.header_finish_reason(), Some("error"));
    }

    /// The reject is total, not partial: a bad shape in the SECOND message of a
    /// multi-send emits one `invalid_json` and zero regular messages — the same
    /// two-pass discipline the contract check already follows.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_bad_shape_in_the_second_message_emits_nothing_regular() {
        let cell = code_cell_with_emits_multi(
            r#"{"body":{},"hop":{}}"#,
            false,
            r#"[{"messages":[]}, "not-an-object"]"#,
        );
        let outs = run_handle_collect(&cell).await;
        assert_eq!(outs.len(), 1, "exactly one reply, no partial emit");
        assert_eq!(outs[0].header_error_code(), Some("invalid_json"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn body_violation_yields_contract_violation_when_flag_on() {
        // contract requires body.messages:array required; script writes an
        // object without messages. inject_standard_headers only touches the
        // header, so the body stays violated post-injection.
        let cell = code_cell_with_emits(
            r#"{"body":{"messages":{"type":"array","required":true}},"hop":{}}"#,
            true,
            r#"{"header":{},"foo":"bar"}"#,
        );
        let out = run_handle(&cell).await;
        assert_eq!(out.header_error_code(), Some("contract_violation"));
        assert_eq!(out.header_finish_reason(), Some("error"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn header_violation_yields_contract_violation_when_flag_on() {
        // contract requires header.finish_reason required; script omits it.
        // inject_standard_headers does NOT add finish_reason → still violated.
        let cell = code_cell_with_emits(
            r#"{"body":{},"hop":{"finish_reason":{"type":"string","required":true}}}"#,
            true,
            r#"{"messages":[]}"#,
        );
        let out = run_handle(&cell).await;
        assert_eq!(out.header_error_code(), Some("contract_violation"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flag_off_skips_validation_release_default() {
        // Knob off → violating emission passes through unchanged.
        // (Direct flag injection, since cfg!(debug_assertions) is true in tests.)
        let cell = code_cell_with_emits(
            r#"{"body":{"messages":{"type":"array","required":true}},"hop":{}}"#,
            false,
            r#"{"header":{},"foo":"bar"}"#,
        );
        let out = run_handle(&cell).await;
        assert_ne!(out.header_error_code(), Some("contract_violation"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn multi_send_violation_in_second_message_emits_nothing_regular() {
        // Auflage A2 pin: violation in message 2 → exactly ONE contract_violation,
        // ZERO regular emits (no partial-emit mix).
        let cell = code_cell_with_emits_multi(
            r#"{"body":{"messages":{"type":"array","required":true}},"hop":{}}"#,
            true,
            // Array: [0] valid, [1] violates (no messages)
            r#"[{"messages":[]},{"header":{},"foo":"bar"}]"#,
        );
        let outs = run_handle_collect(&cell).await;
        assert_eq!(outs.len(), 1, "exactly one reply");
        assert_eq!(outs[0].header_error_code(), Some("contract_violation"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn array_with_multi_send_emits_two_messages() {
        let cell = CodeCell::new(
            CodeParams {
                runner: "python3".into(),
                script: Script::Inline(
                    r#"import sys,json; sys.stdout.write(json.dumps([
                        {"messages":[{"origin":"assistant","type":"text","text":"a"}]},
                        {"messages":[{"origin":"assistant","type":"text","text":"b"}]}
                    ]))"#
                        .into(),
                ),
                external_timeout_ms: Some(10_000),
                max_concurrency: None,
                sandbox: None,
                runner_mode: RunnerMode::Cold,
            },
            true,
            None,
            false,
        );
        let (otx, mut orx) = mpsc::channel(8);
        let sink = make_sink(otx);
        let msg = MessageBuilder::new(Path::new("/code"))
            .body(Body::Inline(json!({"messages":[]})))
            .reply_to(Path::new("/sink"))
            .build();
        cell.handle(msg, &sink).await;
        drop(sink);
        let _a = orx.recv().await.unwrap();
        let _b = orx.recv().await.unwrap();
        assert!(orx.recv().await.is_none(), "exactly 2 emissions");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn array_without_multi_send_yields_contract_violation() {
        let cell = CodeCell::new(
            CodeParams {
                runner: "python3".into(),
                script: Script::Inline(
                    r#"import sys,json; sys.stdout.write(json.dumps([{"messages":[]},{"messages":[]}]))"#
                        .into(),
                ),
                external_timeout_ms: Some(10_000),
                max_concurrency: None,
                sandbox: None,
                runner_mode: RunnerMode::Cold,
            },
            false,
            None,
            false,
        );
        let (otx, mut orx) = mpsc::channel(8);
        let sink = make_sink(otx);
        let msg = MessageBuilder::new(Path::new("/code"))
            .body(Body::Inline(json!({"messages":[]})))
            .reply_to(Path::new("/sink"))
            .build();
        cell.handle(msg, &sink).await;
        drop(sink);
        let em = orx.recv().await.unwrap();
        assert_eq!(
            em.content["header"]["error_code"],
            "multi_send_not_declared"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inline_python3_emits_object_body() {
        let cell = CodeCell::new(
            CodeParams {
                runner: "python3".into(),
                script: Script::Inline(
                    r#"import sys,json; sys.stdout.write(json.dumps({"messages":[{"origin":"assistant","type":"text","text":"out"}]}))"#.into(),
                ),
                external_timeout_ms: Some(10_000),
                max_concurrency: None,
                sandbox: None,
                runner_mode: RunnerMode::Cold,
            },
            false,
            None,
            false,
        );
        let (otx, mut orx) = mpsc::channel(8);
        let sink = make_sink(otx);
        let msg = MessageBuilder::new(Path::new("/code"))
            .body(Body::Inline(json!({"messages":[]})))
            .reply_to(Path::new("/sink"))
            .build();
        cell.handle(msg, &sink).await;
        drop(sink);
        let em = orx.recv().await.unwrap();
        let headers = &em.content["header"];
        assert_eq!(headers["exit_code"], 0);
        assert_eq!(headers["had_stderr"], false);
    }
}
