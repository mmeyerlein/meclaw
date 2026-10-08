//! Phase-7 BashCell. The shipped surface is documented in `docs/cell-types.md` § `bash`.

use std::time::Duration;

pub struct BashCell {
    pub external_timeout: Duration,
    pub max_concurrency: usize,
    /// Optional process sandbox for the shell (S4, GH #35). `None` means the
    /// legacy unsandboxed behaviour: the shell keeps the daemon's rights.
    pub sandbox: Option<crate::sandbox::SandboxProfile>,
    /// Explicit size cap on the combined output handed to the caller, in
    /// bytes (GH #83). `None` (the default since GH #1085, R-IG-1): the output
    /// (stdout plus the stderr sentinel block) is cut to the tool-result share
    /// of the window the call names (`content_budget::input_soft_of`) and
    /// delivered whole when it names none -- up to the carrier ceiling, above
    /// which it is refused (`too_long`). Every cut carries the mark
    /// `...[cut: <k> of <n> bytes shown; <hint>]`, and `header.bytes` keeps
    /// the full pre-cut size.
    pub max_bytes: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct BashArgs {
    pub command: String,
}

use meclaw_core::JsonValue;

/// `MAX_ARG_STRLEN` on the target platform: `32 * PAGE_SIZE` with the usual
/// 4 KiB page (GH #351). Linux caps a **single** `argv` string at this size,
/// independent of `ARG_MAX` and not raisable. `bash` hands the command to the
/// shell as exactly such a string (`/bin/sh -c <command>`), so this is the hard
/// ceiling on how long a command may be. A platform with larger pages has a
/// HIGHER real cap, so this value only ever refuses earlier than strictly
/// necessary — never too late.
const MAX_ARG_STRLEN: usize = 32 * 4096;

/// Parses tool_call args for BashCell. Returns `Err(human-readable)` with
/// `ERR_INVALID_INPUT` semantics (the caller builds the error body).
///
/// A command at or above [`MAX_ARG_STRLEN`] is refused HERE, before a shell is
/// started (GH #351). Handed to `spawn()` it would fail with
/// `Argument list too long (os error 7)` and surface as `io_error` — a message
/// that reads like an I/O fault of the child while in truth no child ever
/// existed. Refusing it as invalid input names the real reason and the real
/// numbers. The `code` remedy from GH #349 (materialise into a temp file) is
/// deliberately NOT taken here: `sh <file>` is not `sh -c <command>` — `$0`
/// becomes the script path and `$1…` shift by one, so a command that reads
/// positional parameters would change meaning above the cap.
pub(crate) fn parse_bash_args(args: &JsonValue) -> Result<BashArgs, String> {
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "args.command missing or not a string".to_string())?;
    if command.is_empty() {
        return Err("args.command is empty".into());
    }
    if command.len() >= MAX_ARG_STRLEN {
        return Err(format!(
            "args.command is {} bytes; the kernel caps a single argv string at \
             {MAX_ARG_STRLEN} bytes (MAX_ARG_STRLEN = 32 * PAGE_SIZE), and \
             `/bin/sh -c <command>` passes the command as exactly one such \
             string — no shell was started",
            command.len()
        ));
    }
    Ok(BashArgs {
        command: command.to_string(),
    })
}

use crate::process::{KillingTimeoutErr, with_killing_timeout};

// ---- StatelessCell implementation ----

use crate::tool::{
    ERR_INVALID_INPUT, ERR_IO_ERROR, ERR_TIMEOUT, ERR_TOO_LONG, build_error_body,
    build_tool_result_body, parse_tool_call_args,
};
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{CellOutput, Message, OutputSink, Path};

/// Sentinel markers delimiting the stderr block when combined-output bodies
/// inject stderr after stdout. Shared with the `code` cell's `script_failed`
/// path (B.1) so both tool cells emit the identical stderr-delimited form.
pub(crate) const STDERR_START: &str = "##meclaw-stderr-start##";
pub(crate) const STDERR_END: &str = "##meclaw-stderr-end##";

#[allow(clippy::manual_async_fn)]
impl meclaw_colony::StatelessCell for BashCell {
    /// Handle one tool_call message: parse `{command}`, spawn
    /// `/bin/sh -c <command>` via `tokio::process::Command`, run under
    /// `with_killing_timeout`, emit a `tool_result` with stdout (+ stderr
    /// sentinel block) and `exit_code`/`had_stderr`-Headers. Non-zero
    /// exit codes are NORMAL tool_results; only spawn-failure and timeout
    /// produce error messages.
    fn handle<'a>(
        &'a self,
        msg: Message,
        sink: &'a OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            let started = std::time::Instant::now();
            let reply_target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());

            let (args, id) = match parse_tool_call_args(&msg) {
                Ok(v) => v,
                Err(e) => {
                    self.emit_error(sink, reply_target, ERR_INVALID_INPUT, e, None, started)
                        .await;
                    return;
                }
            };
            let parsed = match parse_bash_args(&args) {
                Ok(p) => p,
                Err(e) => {
                    self.emit_error(sink, reply_target, ERR_INVALID_INPUT, e, id, started)
                        .await;
                    return;
                }
            };

            let mut cmd = tokio::process::Command::new("/bin/sh");
            cmd.arg("-c")
                .arg(&parsed.command)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());

            // GH #866: sandbox, `fork` and the journal's `spawned` record are
            // blocking syscalls; under write-back pressure the record's `fsync`
            // alone waited hundreds of milliseconds on a live colony, on a
            // runtime worker that polled nothing else meanwhile. One blocking
            // section, awaited -- the record is durable before this path goes
            // on (ruling B-R2) and is read on the thread that forked.
            let profile = self.sandbox.clone();
            let journal_path = msg.target.as_str().to_string();
            let spawned = tokio::task::spawn_blocking(move || {
                // S4 (GH #35): the sandbox is installed on the command, not
                // around it. A profile that cannot be applied fails HERE,
                // before a child exists — a `restricted` cell never falls back
                // to unsandboxed.
                //
                // GH #85: the returned scope owns the child's cgroup and must
                // stay alive until the child is reaped, so it travels back with
                // the child.
                let scope = match &profile {
                    None => crate::sandbox::SandboxScope::empty(),
                    Some(profile) => crate::sandbox::apply(profile, &mut cmd)
                        .map_err(|e| format!("sandbox not applied: {e}"))?,
                };
                let child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
                // GH #116: crash-durable record of this child. The note retires
                // the entry on EVERY path that still runs code — `retire` below
                // on the ordinary one, `Drop` on task abort and panic unwind. The
                // one path it does not cover is the one it exists for: a
                // `SIGKILL`ed daemon, where the entry survives in the journal and
                // the next boot reaps what it names.
                let note = crate::orphan_journal::note_spawn(child.id(), None, &journal_path);
                Ok::<_, String>((child, scope, note))
            })
            .await;
            let (child, _sandbox_scope, journal_note) = match spawned {
                Ok(Ok(spawned)) => spawned,
                Ok(Err(e)) => {
                    self.emit_error(sink, reply_target, ERR_IO_ERROR, e, id, started)
                        .await;
                    return;
                }
                Err(e) => {
                    self.emit_error(
                        sink,
                        reply_target,
                        ERR_IO_ERROR,
                        format!("spawn did not complete: {e}"),
                        id,
                        started,
                    )
                    .await;
                    return;
                }
            };

            let result = with_killing_timeout(child, self.external_timeout).await;
            // Taken before `retire`: `duration_ms` reports the run, not the
            // journal's `fsync` after it, which under write-back pressure
            // waits hundreds of milliseconds on the ext4 commit (GH #866).
            let duration_ms = started.elapsed().as_millis() as u64;
            // GH #866: the `exited` record off the workers, and BEFORE the
            // answer -- the order of GH #134 (answer first, exit record in the
            // `Drop` after it) turns round: a reader of the answer now finds
            // the journal already settled.
            journal_note.retire().await;

            match result {
                Ok(out) => {
                    let stdout_str = String::from_utf8_lossy(&out.stdout).into_owned();
                    let stderr_str = String::from_utf8_lossy(&out.stderr).into_owned();
                    let had_stderr = !stderr_str.is_empty();
                    let text = if had_stderr {
                        format!("{stdout_str}\n{STDERR_START}\n{stderr_str}{STDERR_END}")
                    } else {
                        stdout_str
                    };
                    // GH #83: `bytes` reports the full combined output, so a
                    // trimmed reply still says how big the run really was.
                    let bytes = text.len() as u64;
                    // GH #1085 (R-IG-1): no fixed cap -- the window's share,
                    // marked, or whole; refused only over the carrier.
                    let (text, truncated) = match crate::content_budget::tool_result_text(
                        text,
                        self.max_bytes,
                        &msg.headers,
                        BASH_CUT_HINT,
                    ) {
                        Ok(v) => v,
                        Err(refusal) => {
                            self.emit_error(
                                sink,
                                reply_target,
                                ERR_TOO_LONG,
                                refusal.detail(),
                                id,
                                started,
                            )
                            .await;
                            return;
                        }
                    };
                    let mut header = Map::new();
                    header.insert("operation".into(), Value::String("bash".into()));
                    header.insert("exit_code".into(), Value::from(out.exit_code));
                    header.insert("had_stderr".into(), Value::from(had_stderr));
                    header.insert("duration_ms".into(), Value::from(duration_ms));
                    header.insert("bytes".into(), Value::from(bytes));
                    if truncated {
                        header.insert("truncated".into(), Value::Bool(true));
                    }
                    let body = build_tool_result_body(text, id, header);
                    tracing::info!(
                        operation = "bash",
                        exit_code = out.exit_code,
                        had_stderr,
                        duration_ms,
                        bytes,
                        truncated,
                        "bash op ok"
                    );
                    let _ = sink
                        .push(CellOutput {
                            target: reply_target,
                            content: body,
                        })
                        .await;
                }
                Err(KillingTimeoutErr::Elapsed) => {
                    let text = format!("command timed out after {:?}", self.external_timeout);
                    let mut header = Map::new();
                    header.insert("operation".into(), Value::String("bash".into()));
                    // B.1: every bash output carries `had_stderr`; error paths set false.
                    header.insert("had_stderr".into(), Value::from(false));
                    // Spec conformance (GH #104 track T): a timed-out child WAS
                    // killed, so the `-1` abnormal-termination convention applies
                    // here too — the timeout error carries `exit_code: -1` next
                    // to `error_code: "timeout"` (cell-types.md § bash). The
                    // pre-spawn error paths stay without exit_code: no child ran.
                    header.insert("exit_code".into(), Value::from(-1));
                    header.insert("duration_ms".into(), Value::from(duration_ms));
                    let body = build_error_body(ERR_TIMEOUT, text, id, header);
                    tracing::info!(
                        operation = "bash",
                        error_code = ERR_TIMEOUT,
                        duration_ms,
                        "bash op timed out"
                    );
                    let _ = sink
                        .push(CellOutput {
                            target: reply_target,
                            content: body,
                        })
                        .await;
                }
                Err(KillingTimeoutErr::Io(e)) => {
                    let mut header = Map::new();
                    header.insert("operation".into(), Value::String("bash".into()));
                    // B.1: every bash output carries `had_stderr`; error paths set false.
                    header.insert("had_stderr".into(), Value::from(false));
                    header.insert("duration_ms".into(), Value::from(duration_ms));
                    let body = build_error_body(ERR_IO_ERROR, e.to_string(), id, header);
                    tracing::info!(
                        operation = "bash",
                        error_code = ERR_IO_ERROR,
                        duration_ms,
                        "bash op io err"
                    );
                    let _ = sink
                        .push(CellOutput {
                            target: reply_target,
                            content: body,
                        })
                        .await;
                }
            }
        }
    }
}

impl BashCell {
    /// Emit a UBF error-body to `reply_target` with `operation: "bash"`
    /// and the given `code`/`text`.
    async fn emit_error(
        &self,
        sink: &OutputSink,
        reply_target: Path,
        code: &str,
        text: String,
        id: Option<String>,
        started: std::time::Instant,
    ) {
        let duration_ms = started.elapsed().as_millis() as u64;
        let mut header = Map::new();
        header.insert("operation".into(), Value::String("bash".into()));
        // B.1: every bash output carries `had_stderr`; error paths set false.
        header.insert("had_stderr".into(), Value::from(false));
        header.insert("duration_ms".into(), Value::from(duration_ms));
        let body = build_error_body(code, text, id, header);
        tracing::info!(
            operation = "bash",
            error_code = code,
            duration_ms,
            "bash pre-op err"
        );
        let _ = sink
            .push(CellOutput {
                target: reply_target,
                content: body,
            })
            .await;
    }
}

// ---- BashCellFactory ----

use meclaw_colony::{CellFactory, RespawnFn, SpawnedCellKind, build_stateless_task};

/// Factory for `BashCell`. Unit struct — stateless, config lives in params.
pub struct BashCellFactory;

/// GH #1099: how many commands run at once by default. Each one is a child
/// process with its own CPU and memory, so the ceiling is sized like a small
/// box's cores rather than like the traffic; a turn's parallel tool round
/// rarely asks for more than four shells at once.
/// Over the ceiling a message waits for a free slot in the stateless
/// dispatcher; since GH #1099 that wait is never silent -- the dispatcher logs
/// it under `meclaw::concurrency` with the trace id and `waited_ms`.
const DEFAULT_BASH_MAX_CONCURRENCY: usize = 4;
const DEFAULT_BASH_EXTERNAL_TIMEOUT_MS: u64 = 60_000;
/// What the cut mark tells the model to do for the rest of a cut output
/// (GH #1085): run the command again on a narrower slice.
const BASH_CUT_HINT: &str = "narrow the command's output, e.g. with head, tail or grep";

struct ParsedBashParams {
    external_timeout: Duration,
    max_concurrency: usize,
    sandbox: Option<crate::sandbox::SandboxProfile>,
    max_bytes: Option<usize>,
}

fn parse_params_pure(raw: &meclaw_core::JsonValue) -> Result<ParsedBashParams, String> {
    let mc = match raw.get("max_concurrency") {
        None => DEFAULT_BASH_MAX_CONCURRENCY,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.max_concurrency must be a positive integer".to_string())?
            as usize,
    };
    if mc == 0 {
        return Err("params.max_concurrency must be >= 1".into());
    }
    let ms = match raw.get("external_timeout_ms") {
        None => DEFAULT_BASH_EXTERNAL_TIMEOUT_MS,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.external_timeout_ms must be a positive integer".to_string())?,
    };
    if ms == 0 {
        return Err("params.external_timeout_ms must be >= 1".into());
    }
    // GH #1085 (R-IG-1): no fixed default -- absent, the window decides.
    let mb = match raw.get("max_bytes") {
        None => None,
        Some(v) => Some(
            v.as_u64()
                .ok_or_else(|| "params.max_bytes must be a positive integer".to_string())?
                as usize,
        ),
    };
    if mb == Some(0) {
        return Err("params.max_bytes must be >= 1".into());
    }
    let sandbox = crate::sandbox::SandboxProfile::parse(raw)?;
    Ok(ParsedBashParams {
        external_timeout: Duration::from_millis(ms),
        max_concurrency: mc,
        sandbox,
        max_bytes: mb,
    })
}

impl CellFactory for BashCellFactory {
    fn validate_params(&self, params: &meclaw_core::JsonValue) -> Result<(), String> {
        parse_params_pure(params).map(|_| ())
    }

    /// Stateless cell — `cell_dir` and the three Phase-13-G-1 substrate params
    /// (`colony_inbox_tx`, `idle_timeout`, `cell_timeout`) are unused; idle/wake
    /// is a stateful concept.
    fn spawn_cell(
        self: std::sync::Arc<Self>,
        path: meclaw_core::Path,
        params: meclaw_core::JsonValue,
        outputs_tx: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: tokio::sync::mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let parsed = parse_params_pure(&params)?;
        let external_timeout = parsed.external_timeout;
        let max_concurrency = parsed.max_concurrency;
        let sandbox = parsed.sandbox;
        let max_bytes = parsed.max_bytes;

        let cell = std::sync::Arc::new(BashCell {
            external_timeout,
            max_concurrency,
            sandbox: sandbox.clone(),
            max_bytes,
        });
        let (tx, rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(mailbox_capacity);
        // Phase-13.5 Lifecycle-3b Task 3 + P3-A4 funnel: the initial dispatcher
        // is spawned via `build_stateless_task`, which owns the peace-keep-alive
        // (`peace_tx` lives in the task → drops on task-end → watcher sees Err →
        // CellDied; stateless → no cell.db → death_ack fires on dispatcher
        // task-end). The live stop pair goes back on the `Active` tuple; the
        // respawn below re-notifies its own.
        let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build_stateless_task(
            path.clone(),
            rx,
            outputs_tx.clone(),
            cell,
            max_concurrency,
            message_timeout,
            Some(colony_inbox_tx.clone()),
            blob_store.clone(),
            contract.consumes.clone(),
        );

        let respawn_path = path.clone();
        let respawn_outputs_tx = outputs_tx.clone();
        let respawn_blob = blob_store.clone();
        let respawn_inbox = colony_inbox_tx.clone();
        let respawn_mailbox_capacity = mailbox_capacity;
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
        let respawn_sandbox = sandbox;
        let respawn: RespawnFn = Box::new(move || {
            let cell = std::sync::Arc::new(BashCell {
                external_timeout,
                max_concurrency,
                // The boundary survives a restart: a respawned shell is as
                // restricted as the one it replaces.
                sandbox: respawn_sandbox.clone(),
                max_bytes,
            });
            let (tx, rx) =
                tokio::sync::mpsc::channel::<meclaw_core::Message>(respawn_mailbox_capacity);
            let p = respawn_path.clone();
            let o = respawn_outputs_tx.clone();
            let b = respawn_blob.clone();
            // GH #676 (class of #673): a respawn (crash-restart /
            // reconnect-eager, e.g. a swap swinging back to this node) mints a
            // FRESH live stop pair that the frozen `RespawnFn` 4-tuple cannot
            // return. Dropping it left the reactivated cell un-stoppable until
            // a restart — every later disconnect tripped the interim guard. So
            // the closure hands the pair back via `renotify_stop_wiring` (sync
            // try_send, never await — this runs inside the await-free respawn
            // corridor), exactly like the other factories.
            let (join, peace_rx, stop_tx, death_ack_rx, backstop_rx) = build_stateless_task(
                p.clone(),
                rx,
                o,
                cell,
                max_concurrency,
                message_timeout,
                Some(respawn_inbox.clone()),
                b,
                respawn_consumes.clone(),
            );
            meclaw_colony::renotify_stop_wiring(&respawn_inbox, p, stop_tx, death_ack_rx);
            (tx, join, peace_rx, backstop_rx)
        });

        Ok(SpawnedCellKind::Active {
            sender: tx,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }

    /// Paket-8: boot-inactive eager respawn (No-Delete reconnect-after-reboot).
    /// Builds the SAME `Arc<BashCell>` as `spawn_cell` and routes it through the
    /// `build_stateless_boot_inactive_respawn` funnel (I1). Returns `None` only if
    /// params no longer parse (defensive; validated at boot).
    #[allow(clippy::too_many_arguments)]
    fn build_boot_inactive_respawn(
        self: std::sync::Arc<Self>,
        path: meclaw_core::Path,
        params: meclaw_core::JsonValue,
        outputs_tx: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: tokio::sync::mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let parsed = parse_params_pure(&params).ok()?;
        let max_concurrency = parsed.max_concurrency;
        let cell = std::sync::Arc::new(BashCell {
            external_timeout: parsed.external_timeout,
            max_concurrency,
            sandbox: parsed.sandbox,
            max_bytes: parsed.max_bytes,
        });
        Some(meclaw_colony::build_stateless_boot_inactive_respawn(
            path,
            outputs_tx,
            cell,
            max_concurrency,
            message_timeout,
            colony_inbox_tx,
            blob_store,
            mailbox_capacity,
            contract.consumes.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    // ── Task-2 tests ────────────────────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn killing_timeout_returns_output_for_fast_command() {
        use std::process::Stdio;
        use tokio::process::Command;

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("echo hello; echo err 1>&2")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = cmd.spawn().unwrap();
        let out = with_killing_timeout(child, std::time::Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "hello\n");
        assert_eq!(String::from_utf8(out.stderr).unwrap(), "err\n");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn killing_timeout_kills_slow_command_and_proves_dead() {
        use std::process::Stdio;
        use tokio::process::Command;

        let td = tempfile::TempDir::new().unwrap();
        let marker = td.path().join("MARKER");
        let marker_str = marker.to_str().unwrap();
        let cmd_str = format!("sleep 30; touch {marker_str}");

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(&cmd_str)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = cmd.spawn().unwrap();

        let started = std::time::Instant::now();
        let r = with_killing_timeout(child, std::time::Duration::from_millis(100)).await;
        let elapsed = started.elapsed();

        assert!(matches!(r, Err(KillingTimeoutErr::Elapsed)));
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "killing-timeout must return promptly after elapsed, took {elapsed:?}"
        );
        assert!(
            !marker.exists(),
            "marker must NOT exist — proves sleep was killed before touch ran"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn killing_timeout_returns_nonzero_exit_for_failing_command() {
        use std::process::Stdio;
        use tokio::process::Command;

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("exit 7")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = cmd.spawn().unwrap();
        let out = with_killing_timeout(child, std::time::Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(out.exit_code, 7);
    }

    // ── Task-3 tests ────────────────────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_echo_command_emits_tool_result() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
            validate_ubf_body,
        };
        use tokio::sync::mpsc;

        let cell = BashCell {
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            sandbox: None,
            max_bytes: None,
        };

        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/bash"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let body = json!({
            "messages": [{
                "origin": "assistant", "type": "tool_call",
                "text": r#"{"command": "echo hello"}"#, "id": "call-1"
            }]
        });
        let msg = MessageBuilder::new(Path::new("/bash"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(body))
            .build();
        cell.handle(msg, &sink).await;

        let em = out_rx.recv().await.expect("emission");
        validate_ubf_body(&em.content).expect("valid UBF");
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["messages"][0]["origin"], "tool");
        assert_eq!(em.content["messages"][0]["type"], "tool_result");
        assert_eq!(em.content["messages"][0]["text"], "hello\n");
        assert_eq!(em.content["header"]["operation"], "bash");
        assert_eq!(em.content["header"]["exit_code"], 0);
        assert_eq!(em.content["header"]["had_stderr"], false);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_stderr_command_includes_sentinel_block() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use tokio::sync::mpsc;

        let cell = BashCell {
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            sandbox: None,
            max_bytes: None,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/bash"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/bash"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"command": "echo out; echo err 1>&2"}"#, "id": "call-2"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        let text = em.content["messages"][0]["text"].as_str().unwrap();
        assert!(text.contains("out\n"));
        assert!(text.contains("##meclaw-stderr-start##\nerr\n##meclaw-stderr-end##"));
        assert_eq!(em.content["header"]["had_stderr"], true);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_nonzero_exit_is_normal_tool_result() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use tokio::sync::mpsc;

        let cell = BashCell {
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            sandbox: None,
            max_bytes: None,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/bash"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/bash"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"command": "exit 3"}"#, "id": "call-3"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["exit_code"], 3);
        // NO finish_reason: "error" — a non-zero exit is NORMAL (decision 1.1).
        assert!(
            em.content["header"].get("finish_reason").is_none()
                || em.content["header"]["finish_reason"] != "error"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_timeout_emits_error_with_err_timeout() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use tokio::sync::mpsc;

        let cell = BashCell {
            external_timeout: std::time::Duration::from_millis(100),
            max_concurrency: 4,
            sandbox: None,
            max_bytes: None,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/bash"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/bash"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"command": "sleep 30"}"#, "id": "call-4"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.content["header"]["finish_reason"], "error");
        assert_eq!(em.content["header"]["error_code"], "timeout");
    }

    // ── Task-4 tests ────────────────────────────────────────────────────────

    #[test]
    fn factory_validate_params_accepts_empty_object() {
        use meclaw_colony::CellFactory;
        let f = BashCellFactory;
        assert!(
            f.validate_params(&meclaw_core::serde_json::json!({}))
                .is_ok()
        );
    }

    #[test]
    fn factory_validate_params_rejects_max_concurrency_zero() {
        use meclaw_colony::CellFactory;
        let f = BashCellFactory;
        let r = f.validate_params(&meclaw_core::serde_json::json!({"max_concurrency": 0}));
        assert!(r.is_err());
    }

    #[test]
    fn factory_validate_params_rejects_external_timeout_zero() {
        use meclaw_colony::CellFactory;
        let f = BashCellFactory;
        let r = f.validate_params(&meclaw_core::serde_json::json!({"external_timeout_ms": 0}));
        assert!(r.is_err());
    }

    #[test]
    fn factory_validate_params_accepts_valid_overrides() {
        use meclaw_colony::CellFactory;
        let f = BashCellFactory;
        let r = f.validate_params(&meclaw_core::serde_json::json!({
            "max_concurrency": 2,
            "external_timeout_ms": 10000
        }));
        assert!(r.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn factory_spawn_cell_routes_message_to_tool_result() {
        use meclaw_colony::CellFactory;
        use meclaw_core::{Body, CellEmission, MessageBuilder, Path, serde_json::json};
        use std::sync::Arc;
        use tokio::sync::mpsc;

        let factory: Arc<dyn CellFactory> = Arc::new(BashCellFactory);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = factory
            .spawn_cell(
                Path::new("/bash"),
                json!({"max_concurrency": 2, "external_timeout_ms": 5000}),
                out_tx,
                std::path::PathBuf::new(),
                meclaw_colony::ContractView::default(),
                inbox_tx,
                None,
                0,
                None,
                None,
                1000,
            )
            .expect("spawn");

        let msg = MessageBuilder::new(Path::new("/bash"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"command": "echo factory"}"#, "id": "call-f"
                }]
            })))
            .build();
        let (sender, join) = match spawned {
            meclaw_colony::SpawnedCellKind::Active { sender, join, .. } => (sender, join),
            meclaw_colony::SpawnedCellKind::Dormant { .. } => {
                unreachable!("Phase-13-G-2: only Active")
            }
        };
        sender.send(msg).await.unwrap();

        // Deterministic rendezvous: recv().await returns as soon as the worker
        // writes the emission into out_tx. No time-based failure marker — a
        // channel close (None) would blow the test up on unwrap(), which would be
        // a real failure, not a flake.
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["messages"][0]["text"], "factory\n");
        assert_eq!(em.content["header"]["operation"], "bash");

        drop(sender);
        join.await.unwrap();
    }

    // ── Task-1 tests ────────────────────────────────────────────────────────

    #[test]
    fn parse_bash_args_happy_path() {
        let args = parse_bash_args(&json!({"command": "echo hi"})).unwrap();
        assert_eq!(args.command, "echo hi");
    }

    #[test]
    fn parse_bash_args_rejects_missing_command() {
        assert!(parse_bash_args(&json!({})).is_err());
    }

    #[test]
    fn parse_bash_args_rejects_non_string_command() {
        assert!(parse_bash_args(&json!({"command": 42})).is_err());
    }

    #[test]
    fn parse_bash_args_rejects_empty_command() {
        assert!(parse_bash_args(&json!({"command": ""})).is_err());
    }

    // ───── GH #351: the argv cap ─────

    #[test]
    fn parse_bash_args_rejects_a_command_at_the_argv_cap() {
        let cmd = "x".repeat(MAX_ARG_STRLEN);
        let err = parse_bash_args(&json!({ "command": cmd })).expect_err("must refuse");
        assert!(err.contains("131072"), "the limit is spelled out: {err}");
    }

    #[test]
    fn the_refusal_names_the_actual_size_next_to_the_limit() {
        // AT the cap both numbers coincide, so a refusal that dropped the
        // caller's own size would still read correctly. 137 bytes above the cap
        // forces them apart: 131 209 arrived, 131 072 is the wall.
        const OVER: usize = MAX_ARG_STRLEN + 137;
        assert_eq!(OVER, 131_209, "the two numbers must differ to be a pin");
        let err =
            parse_bash_args(&json!({ "command": "x".repeat(OVER) })).expect_err("must refuse");
        assert!(
            err.contains("131209"),
            "the refusal names the ACTUAL size, not just the wall: {err}"
        );
        assert!(err.contains("131072"), "the limit is spelled out: {err}");
    }

    #[test]
    fn parse_bash_args_accepts_the_largest_command_that_still_spawns() {
        // Measured under GH #349: the kernel counts the terminating NUL, so
        // 131 071 bytes still spawn and 131 072 do not.
        let cmd = "x".repeat(MAX_ARG_STRLEN - 1);
        let args = parse_bash_args(&json!({ "command": cmd })).expect("one byte below the cap");
        assert_eq!(args.command.len(), MAX_ARG_STRLEN - 1);
    }

    // ───── GH #83: the size cap ─────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_runaway_stdout_arrives_trimmed_and_marked() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use tokio::sync::mpsc;

        let cell = BashCell {
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            sandbox: None,
            max_bytes: Some(1000),
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/bash"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        // 20,000 bytes of stdout: 2,000 lines of "xxxxxxxxx" (9+1 bytes each).
        let msg = MessageBuilder::new(Path::new("/bash"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"command": "yes xxxxxxxxx | head -n 2000"}"#, "id": "call-cap"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        let text = em.content["messages"][0]["text"].as_str().unwrap();
        assert!(
            text.len() < 20_000,
            "the cap must bite: {} bytes arrived",
            text.len()
        );
        assert!(
            text.ends_with(&format!(
                "...[cut: 1000 of 20000 bytes shown; {BASH_CUT_HINT}]"
            )),
            "cut marked, full size named: ...{}",
            &text[text.len().saturating_sub(90)..]
        );
        assert_eq!(
            em.content["header"]["truncated"], true,
            "the declared `truncated` header finally has a producer"
        );
        assert_eq!(
            em.content["header"]["bytes"], 20_000,
            "`bytes` reports the full output size, not what survived the cap"
        );
        assert_eq!(em.content["header"]["exit_code"], 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_output_under_the_cap_is_untouched_and_unmarked() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use tokio::sync::mpsc;

        let cell = BashCell {
            external_timeout: std::time::Duration::from_secs(5),
            max_concurrency: 4,
            sandbox: None,
            max_bytes: None,
        };
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/bash"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/bash"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"command": "echo small"}"#, "id": "call-s"
                }]
            })))
            .build();
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.content["messages"][0]["text"], "small\n");
        assert!(
            em.content["header"].get("truncated").is_none(),
            "no cut, no marker"
        );
        assert_eq!(em.content["header"]["bytes"], 6);
    }

    #[test]
    fn factory_validate_params_rejects_max_bytes_zero() {
        use meclaw_colony::CellFactory;
        assert!(
            BashCellFactory
                .validate_params(&meclaw_core::serde_json::json!({"max_bytes": 0}))
                .is_err()
        );
    }

    #[test]
    fn factory_defaults_max_bytes_to_the_window_budget() {
        let p = parse_params_pure(&meclaw_core::serde_json::json!({})).unwrap();
        assert_eq!(p.max_bytes, None, "no fixed default (GH #1085)");
    }

    /// GH #1085 lock: the old 256 KiB default is gone -- an output over it
    /// arrives whole when the call names no window, and cut to the window's
    /// tool-result share (marked, total named) when it does.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_output_over_the_old_cap_is_whole_without_a_budget_and_marked_with_one() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
        };
        use tokio::sync::mpsc;

        // 300 KiB of stdout: 30 720 lines of "xxxxxxxxx\n".
        let run = |hop: Value| async move {
            let cell = BashCell {
                external_timeout: std::time::Duration::from_secs(30),
                max_concurrency: 4,
                sandbox: None,
                max_bytes: None,
            };
            let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
            let sink = OutputSink::new(
                out_tx,
                Path::new("/bash"),
                Uuid::now_v7(),
                Uuid::now_v7(),
                10,
                meclaw_core::Headers::new(),
                None,
            );
            let msg = MessageBuilder::new(Path::new("/bash"))
                .reply_to(Path::new("/caller"))
                .hop(hop.as_object().cloned().unwrap_or_default())
                .body(Body::Inline(json!({
                    "messages": [{
                        "origin": "assistant", "type": "tool_call",
                        "text": r#"{"command": "yes xxxxxxxxx | head -n 30720"}"#, "id": "c"
                    }]
                })))
                .build();
            cell.handle(msg, &sink).await;
            out_rx.recv().await.unwrap().content
        };
        let whole = run(json!({})).await;
        assert_eq!(whole["header"]["exit_code"], 0, "{}", whole["header"]);
        assert_eq!(
            whole["messages"][0]["text"].as_str().unwrap().len(),
            300 * 1024,
            "whole"
        );
        assert!(whole["header"].get("truncated").is_none());

        // input_soft 10 000 tokens -> 10 % at 3 bytes a token = 3 000 bytes.
        let cut = run(json!({"input_soft": 10_000})).await;
        let text = cut["messages"][0]["text"].as_str().unwrap();
        assert!(
            text.ends_with(&format!(
                "...[cut: 3000 of {} bytes shown; {BASH_CUT_HINT}]",
                300 * 1024
            )),
            "...{}",
            &text[text.len().saturating_sub(90)..]
        );
        assert_eq!(cut["header"]["truncated"], true);
        assert_eq!(cut["header"]["bytes"], 300 * 1024);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bash_offers_boot_inactive_respawn() {
        use meclaw_colony::CellFactory;
        let factory = std::sync::Arc::new(BashCellFactory);
        let (out_tx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let hook = factory.build_boot_inactive_respawn(
            meclaw_core::Path::new("/c"),
            json!({"max_concurrency": 2, "external_timeout_ms": 5000}),
            out_tx,
            std::path::PathBuf::new(),
            meclaw_colony::ContractView::default(),
            itx,
            None, // idle_timeout
            0,    // cell_timeout
            None, // message_timeout
            None, // blob_store
            1000, // mailbox_capacity
        );
        assert!(
            hook.is_some(),
            "stateless bash factory MUST offer a real boot-inactive respawn (eager reconnect)"
        );
    }
}
