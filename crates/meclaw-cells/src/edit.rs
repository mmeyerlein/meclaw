//! Phase-7 EditCell. Detail in Step 3 dieses Tasks.

use std::path::PathBuf;

pub struct EditCell {
    pub base_path: PathBuf,
    pub max_concurrency: usize,
}

pub(crate) enum EditOp {
    FindReplace {
        path: String,
        find: String,
        replace: String,
    },
    InsertAtLine {
        path: String,
        line: u32,
        content: String,
    },
}

pub(crate) fn parse_edit_args(args: &meclaw_core::JsonValue) -> Result<EditOp, String> {
    let op = args
        .get("op")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "args.op missing or not a string".to_string())?;
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "args.path missing or not a string".to_string())?;
    if path.is_empty() {
        return Err("args.path is empty".into());
    }
    match op {
        "find_replace" => {
            let find = args.get("find").and_then(|v| v.as_str()).ok_or_else(|| {
                "args.find missing or not a string (required for find_replace)".to_string()
            })?;
            if find.is_empty() {
                return Err("args.find is empty".into());
            }
            let replace = args
                .get("replace")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    "args.replace missing or not a string (required for find_replace)".to_string()
                })?;
            Ok(EditOp::FindReplace {
                path: path.to_string(),
                find: find.to_string(),
                replace: replace.to_string(),
            })
        }
        "insert_at_line" => {
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "args.line missing or not a positive integer".to_string())?;
            if line < 1 {
                return Err("args.line must be >= 1 (1-based)".into());
            }
            if line > u32::MAX as u64 {
                return Err("args.line exceeds u32::MAX".into());
            }
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "args.content missing or not a string".to_string())?;
            Ok(EditOp::InsertAtLine {
                path: path.to_string(),
                line: line as u32,
                content: content.to_string(),
            })
        }
        other => Err(format!("unknown edit op: {other}")),
    }
}

// ---- StatelessCell impl ----

use crate::boundary::{self, resolve_error_code};
use crate::tool::{
    ERR_INVALID_INPUT, ERR_IO_ERROR, ERR_NOT_A_FILE, ERR_NOT_FOUND, ERR_PATTERN_NOT_FOUND,
    build_error_body, build_tool_result_body, parse_tool_call_args,
};
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{CellOutput, Message, OutputSink, Path};

#[allow(clippy::manual_async_fn)]
impl meclaw_colony::StatelessCell for EditCell {
    /// Handle one tool_call message: parse args, dispatch the edit op
    /// (find_replace / insert_at_line) via `spawn_blocking`, emit a
    /// `tool_result` with `matches_changed`-Header or an error message.
    /// 0 matches for find_replace → `ERR_PATTERN_NOT_FOUND`.
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
                    self.emit_error(
                        sink,
                        reply_target,
                        ERR_INVALID_INPUT,
                        e,
                        None,
                        "unknown",
                        started,
                    )
                    .await;
                    return;
                }
            };
            let op = match parse_edit_args(&args) {
                Ok(o) => o,
                Err(e) => {
                    self.emit_error(
                        sink,
                        reply_target,
                        ERR_INVALID_INPUT,
                        e,
                        id,
                        "unknown",
                        started,
                    )
                    .await;
                    return;
                }
            };

            let base = self.base_path.clone();
            let op_label = describe_edit_op(&op);
            let outcome = tokio::task::spawn_blocking(move || run_edit_op(&base, op))
                .await
                .unwrap_or_else(|e| OpOutcome::Err {
                    code: ERR_IO_ERROR,
                    text: format!("spawn_blocking join error: {e}"),
                });

            let duration_ms = started.elapsed().as_millis() as u64;
            match outcome {
                OpOutcome::Ok {
                    text,
                    bytes,
                    matches_changed,
                } => {
                    let mut header = Map::new();
                    header.insert("operation".into(), Value::String(op_label.clone()));
                    header.insert("matches_changed".into(), Value::from(matches_changed));
                    header.insert("duration_ms".into(), Value::from(duration_ms));
                    header.insert("bytes".into(), Value::from(bytes));
                    let body = build_tool_result_body(text, id, header);
                    tracing::info!(operation = %op_label, matches_changed, duration_ms, "edit op ok");
                    let _ = sink
                        .push(CellOutput {
                            target: reply_target,
                            content: body,
                        })
                        .await;
                }
                OpOutcome::Err { code, text } => {
                    let mut header = Map::new();
                    header.insert("operation".into(), Value::String(op_label.clone()));
                    // B.1: error path applied no edits — report it explicitly so every
                    // edit output carries `matches_changed` (parity with the ok path).
                    header.insert("matches_changed".into(), Value::from(0u64));
                    header.insert("duration_ms".into(), Value::from(duration_ms));
                    let body = build_error_body(code, text, id, header);
                    tracing::info!(operation = %op_label, error_code = code, duration_ms, "edit op err");
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

impl EditCell {
    /// Emit a UBF error-body to `reply_target` with the given `code`,
    /// `text`, and `operation` label.
    #[allow(clippy::too_many_arguments)]
    async fn emit_error(
        &self,
        sink: &OutputSink,
        reply_target: Path,
        code: &str,
        text: String,
        id: Option<String>,
        operation: &str,
        started: std::time::Instant,
    ) {
        let duration_ms = started.elapsed().as_millis() as u64;
        let mut header = Map::new();
        header.insert("operation".into(), Value::String(operation.to_string()));
        header.insert("duration_ms".into(), Value::from(duration_ms));
        let body = build_error_body(code, text, id, header);
        tracing::info!(operation, error_code = code, duration_ms, "edit pre-op err");
        let _ = sink
            .push(CellOutput {
                target: reply_target,
                content: body,
            })
            .await;
    }
}

fn describe_edit_op(op: &EditOp) -> String {
    match op {
        EditOp::FindReplace { .. } => "find_replace".into(),
        EditOp::InsertAtLine { .. } => "insert_at_line".into(),
    }
}

enum OpOutcome {
    Ok {
        text: String,
        bytes: u64,
        matches_changed: u64,
    },
    Err {
        code: &'static str,
        text: String,
    },
}

fn run_edit_op(base: &std::path::Path, op: EditOp) -> OpOutcome {
    match op {
        EditOp::FindReplace {
            path,
            find,
            replace,
        } => {
            let resolved = match boundary::resolve_existing(base, &path) {
                Ok(p) => p,
                Err(e) => {
                    return OpOutcome::Err {
                        code: resolve_error_code(&e),
                        text: e.to_string(),
                    };
                }
            };
            let meta = match std::fs::metadata(&resolved) {
                Ok(m) => m,
                Err(e) => return map_io_err(e),
            };
            if meta.is_dir() {
                return OpOutcome::Err {
                    code: ERR_NOT_A_FILE,
                    text: format!("{path:?} is a directory"),
                };
            }
            let content = match std::fs::read_to_string(&resolved) {
                Ok(s) => s,
                Err(e) => return map_io_err(e),
            };
            let matches = content.matches(find.as_str()).count() as u64;
            if matches == 0 {
                return OpOutcome::Err {
                    code: ERR_PATTERN_NOT_FOUND,
                    text: format!("pattern {find:?} not found in {path:?}"),
                };
            }
            let new_content = content.replace(find.as_str(), replace.as_str());
            if let Err(e) = std::fs::write(&resolved, new_content.as_bytes()) {
                return map_io_err(e);
            }
            let status_text = format!("{{\"matches_changed\":{matches}}}");
            let bytes = status_text.len() as u64;
            OpOutcome::Ok {
                text: status_text,
                bytes,
                matches_changed: matches,
            }
        }
        EditOp::InsertAtLine {
            path,
            line,
            content: insert_content,
        } => {
            let resolved = match boundary::resolve_existing(base, &path) {
                Ok(p) => p,
                Err(e) => {
                    return OpOutcome::Err {
                        code: resolve_error_code(&e),
                        text: e.to_string(),
                    };
                }
            };
            let meta = match std::fs::metadata(&resolved) {
                Ok(m) => m,
                Err(e) => return map_io_err(e),
            };
            if meta.is_dir() {
                return OpOutcome::Err {
                    code: ERR_NOT_A_FILE,
                    text: format!("{path:?} is a directory"),
                };
            }
            let file_content = match std::fs::read_to_string(&resolved) {
                Ok(s) => s,
                Err(e) => return map_io_err(e),
            };
            // split_inclusive behält Newlines; insert_content owned für Borrow-Safety
            let mut lines: Vec<&str> = file_content.split_inclusive('\n').collect();
            let line_idx = (line as usize).saturating_sub(1);
            if line_idx > lines.len() {
                return OpOutcome::Err {
                    code: ERR_INVALID_INPUT,
                    text: format!("line {line} out of range (file has {} lines)", lines.len()),
                };
            }
            lines.insert(line_idx, insert_content.as_str());
            let new_content: String = lines.concat();
            if let Err(e) = std::fs::write(&resolved, new_content.as_bytes()) {
                return map_io_err(e);
            }
            let status_text = "{\"inserted\":1}".to_string();
            let bytes = status_text.len() as u64;
            OpOutcome::Ok {
                text: status_text,
                bytes,
                matches_changed: 1,
            }
        }
    }
}

fn map_io_err(e: std::io::Error) -> OpOutcome {
    let code = if e.kind() == std::io::ErrorKind::NotFound {
        ERR_NOT_FOUND
    } else {
        ERR_IO_ERROR
    };
    OpOutcome::Err {
        code,
        text: e.to_string(),
    }
}

// ---- EditCellFactory ----

use meclaw_colony::{CellFactory, RespawnFn, SpawnedCellKind, build_stateless_task};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Factory for EditCell. Validates params (base_path absolute, max_concurrency ≥1).
/// spawn_cell canonicalises base_path and checks it is a directory.
pub struct EditCellFactory;

const DEFAULT_EDIT_MAX_CONCURRENCY: usize = 8;

struct ParsedEditParams {
    base_path: PathBuf,
    max_concurrency: usize,
}

fn parse_params_pure(raw: &meclaw_core::JsonValue) -> Result<ParsedEditParams, String> {
    let bp = raw
        .get("base_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "params.base_path missing or not a string".to_string())?;
    let bp_path = std::path::Path::new(bp);
    if !bp_path.is_absolute() {
        return Err(format!("params.base_path must be absolute, got: {bp}"));
    }
    let mc = match raw.get("max_concurrency") {
        None => DEFAULT_EDIT_MAX_CONCURRENCY,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.max_concurrency must be a positive integer".to_string())?
            as usize,
    };
    if mc == 0 {
        return Err("params.max_concurrency must be >= 1".into());
    }
    Ok(ParsedEditParams {
        base_path: bp_path.to_path_buf(),
        max_concurrency: mc,
    })
}

/// FS-side validation: canonicalize `base_path` + assert it is a directory.
/// Returns the canonicalized path so `spawn_cell` can reuse it. U11: this runs
/// in BOTH `validate_params` and `spawn_cell` so the parser-invariant holds —
/// otherwise validate accepts a `base_path` that spawn_cell rejects, and the
/// reject surfaces as a Boot-PANIC via `bootstrap_apply.rs`
/// `.expect("validated in plan-phase")` instead of a clean validate error.
fn validate_base_path_fs(parsed: &ParsedEditParams) -> Result<PathBuf, String> {
    let canon = parsed.base_path.canonicalize().map_err(|e| {
        format!(
            "base_path canonicalize failed ({:?}): {e}",
            parsed.base_path
        )
    })?;
    if !canon.is_dir() {
        return Err(format!("base_path is not a directory: {canon:?}"));
    }
    Ok(canon)
}

impl CellFactory for EditCellFactory {
    fn validate_params(&self, params: &meclaw_core::JsonValue) -> Result<(), String> {
        let parsed = parse_params_pure(params)?;
        validate_base_path_fs(&parsed).map(|_| ())
    }

    /// Stateless cell — `cell_dir` and the three Phase-13-G-1 substrate params
    /// (`colony_inbox_tx`, `idle_timeout`, `cell_timeout`) are unused.
    fn spawn_cell(
        self: Arc<Self>,
        path: meclaw_core::Path,
        params: meclaw_core::JsonValue,
        outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let parsed = parse_params_pure(&params)?;
        // FS-Validierung — SAME path as validate_params (U11).
        let canon = validate_base_path_fs(&parsed)?;
        let max_concurrency = parsed.max_concurrency;

        let cell = Arc::new(EditCell {
            base_path: canon.clone(),
            max_concurrency,
        });
        let (tx, rx) = mpsc::channel::<meclaw_core::Message>(mailbox_capacity);
        // Phase-13.5 Lifecycle-3b Task 3 + P3-A4 funnel: initial dispatcher via
        // `build_stateless_task` (owns the peace-keep-alive; stateless → no
        // cell.db → death_ack on dispatcher task-end). RespawnFn passes
        // `colony_inbox = None`.
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
        let respawn_canon = canon.clone();
        let respawn_outputs_tx = outputs_tx.clone();
        let respawn_blob = blob_store.clone();
        let respawn_mailbox_capacity = mailbox_capacity;
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
        let respawn: RespawnFn = Box::new(move || {
            let cell = Arc::new(EditCell {
                base_path: respawn_canon.clone(),
                max_concurrency,
            });
            let (tx, rx) = mpsc::channel::<meclaw_core::Message>(respawn_mailbox_capacity);
            let p = respawn_path.clone();
            let o = respawn_outputs_tx.clone();
            let b = respawn_blob.clone();
            // Stateless respawn is intentionally bare (no renotify, colony_inbox
            // = None). Dropping stop_tx/death_ack_rx is behaviorally identical to
            // the old bare `None,None,None` spawn (stop-fut parks, death_ack
            // unobserved). Peace-keep-alive lives in the helper.
            let (join, peace_rx, _stop_tx, _death_ack_rx, backstop_rx) = build_stateless_task(
                p,
                rx,
                o,
                cell,
                max_concurrency,
                message_timeout,
                None,
                b,
                respawn_consumes.clone(),
            );
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
    /// Builds the SAME `Arc<EditCell>` as `spawn_cell` (incl. the canonicalize +
    /// is_dir FS-validation) and routes it through the
    /// `build_stateless_boot_inactive_respawn` funnel (I1). Returns `None` when
    /// params no longer parse OR the base_path no longer resolves to a directory.
    #[allow(clippy::too_many_arguments)]
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: meclaw_core::Path,
        params: meclaw_core::JsonValue,
        outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        contract: meclaw_colony::ContractView,
        colony_inbox_tx: mpsc::Sender<meclaw_colony::ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        message_timeout: Option<std::time::Duration>,
        blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let parsed = parse_params_pure(&params).ok()?;
        let canon = parsed.base_path.canonicalize().ok()?;
        if !canon.is_dir() {
            return None;
        }
        let max_concurrency = parsed.max_concurrency;
        let cell = Arc::new(EditCell {
            base_path: canon,
            max_concurrency,
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

    // ---- T9: Factory tests ----

    #[test]
    fn factory_validate_params_rejects_missing_base_path() {
        use meclaw_colony::CellFactory;
        assert!(
            EditCellFactory
                .validate_params(&meclaw_core::serde_json::json!({}))
                .is_err()
        );
    }

    #[test]
    fn factory_validate_params_rejects_relative_base_path() {
        use meclaw_colony::CellFactory;
        assert!(
            EditCellFactory
                .validate_params(&meclaw_core::serde_json::json!({"base_path": "rel/dir"}))
                .is_err()
        );
    }

    #[test]
    fn factory_validate_params_accepts_absolute_base_path() {
        use meclaw_colony::CellFactory;
        assert!(
            EditCellFactory
                .validate_params(&meclaw_core::serde_json::json!({"base_path": "/tmp"}))
                .is_ok()
        );
    }

    // ---- U11: validate ≡ spawn parse path (Existenz/is_dir auch in validate) ----

    #[test]
    fn factory_validate_params_rejects_nonexistent_base_path() {
        use meclaw_colony::CellFactory;
        let err = EditCellFactory
            .validate_params(&json!({"base_path": "/nonexistent/definitely/not/here"}))
            .unwrap_err();
        assert!(
            err.contains("base_path"),
            "validate must reject a non-existent base_path (U11), got: {err}"
        );
    }

    #[test]
    fn factory_validate_params_rejects_base_path_that_is_a_file() {
        use meclaw_colony::CellFactory;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let err = EditCellFactory
            .validate_params(&json!({"base_path": tmp.path().to_str().unwrap()}))
            .unwrap_err();
        assert!(
            err.contains("not a directory") || err.contains("base_path"),
            "validate must reject a base_path that is a file (U11), got: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn factory_spawn_cell_routes_message_to_tool_result() {
        use meclaw_colony::CellFactory;
        use meclaw_core::{Body, CellEmission, MessageBuilder, Path, serde_json::json};
        use std::sync::Arc;
        use tokio::sync::mpsc;

        let td = tempfile::TempDir::new().unwrap();
        std::fs::write(td.path().join("a.txt"), b"foo").unwrap();
        let factory: Arc<dyn CellFactory> = Arc::new(EditCellFactory);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = factory
            .spawn_cell(
                Path::new("/edit"),
                json!({"base_path": td.path().to_str().unwrap(), "max_concurrency": 4}),
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

        let msg = MessageBuilder::new(Path::new("/edit"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"op":"find_replace","path":"a.txt","find":"foo","replace":"BAR"}"#,
                    "id": "call-1"
                }]
            })))
            .build();
        let (sender, join) = match spawned {
            SpawnedCellKind::Active { sender, join, .. } => (sender, join),
            SpawnedCellKind::Dormant { .. } => unreachable!("Phase-13-G-2: only Active"),
        };
        sender.send(msg).await.unwrap();

        // Deterministisches Rendezvous: recv().await returnt sobald der Worker
        // die Emission in out_tx schreibt. Kein zeitbasierter Failure-Marker —
        // Channel-Close (None) würde den Test mit unwrap() explodieren lassen,
        // was ein echter Failure wäre, kein Flake.
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["header"]["operation"], "find_replace");
        assert_eq!(em.content["header"]["matches_changed"], 1);

        drop(sender);
        join.await.unwrap();
    }

    #[test]
    fn factory_spawn_cell_rejects_nonexistent_base_path() {
        use meclaw_colony::CellFactory;
        use std::sync::Arc;
        use tokio::sync::mpsc;
        let factory: Arc<dyn CellFactory> = Arc::new(EditCellFactory);
        let (out_tx, _out_rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let r = factory.spawn_cell(
            meclaw_core::Path::new("/edit"),
            meclaw_core::serde_json::json!({"base_path": "/this/does/not/exist/xyzzy-edit"}),
            out_tx,
            std::path::PathBuf::new(),
            meclaw_colony::ContractView::default(),
            inbox_tx,
            None,
            0,
            None,
            None,
            1000,
        );
        assert!(r.is_err());
    }

    // ---- T7/T8 tests ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_find_replace_insert_emit_tool_result() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
            validate_ubf_body,
        };
        use tokio::sync::mpsc;

        let td = tempfile::TempDir::new().unwrap();
        std::fs::write(td.path().join("a.txt"), b"foo bar foo\nbaz\n").unwrap();
        let cell = EditCell {
            base_path: td.path().to_path_buf(),
            max_concurrency: 8,
        };

        async fn invoke(cell: &EditCell, args: meclaw_core::serde_json::Value) -> CellEmission {
            let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
            let sink = OutputSink::new(
                out_tx,
                Path::new("/edit"),
                Uuid::now_v7(),
                Uuid::now_v7(),
                10,
                meclaw_core::Headers::new(),
                None,
            );
            let body = json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": args.to_string(), "id": "call-x"
                }]
            });
            let msg = MessageBuilder::new(Path::new("/edit"))
                .reply_to(Path::new("/caller"))
                .body(Body::Inline(body))
                .build();
            cell.handle(msg, &sink).await;
            out_rx.recv().await.expect("emission")
        }

        // FIND_REPLACE: 2 matches
        let em = invoke(
            &cell,
            json!({"op":"find_replace","path":"a.txt","find":"foo","replace":"qux"}),
        )
        .await;
        validate_ubf_body(&em.content).unwrap();
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["header"]["operation"], "find_replace");
        assert_eq!(em.content["header"]["matches_changed"], 2);
        let written = std::fs::read_to_string(td.path().join("a.txt")).unwrap();
        assert_eq!(written, "qux bar qux\nbaz\n");

        // FIND_REPLACE: 0 matches → ERR_PATTERN_NOT_FOUND
        let em = invoke(
            &cell,
            json!({"op":"find_replace","path":"a.txt","find":"NOPE","replace":"x"}),
        )
        .await;
        assert_eq!(em.content["header"]["error_code"], "pattern_not_found");

        // INSERT_AT_LINE: line=1 (am Anfang)
        std::fs::write(td.path().join("b.txt"), b"line2\n").unwrap();
        let em = invoke(
            &cell,
            json!({"op":"insert_at_line","path":"b.txt","line":1,"content":"line1\n"}),
        )
        .await;
        assert_eq!(em.content["header"]["operation"], "insert_at_line");
        assert_eq!(em.content["header"]["matches_changed"], 1);
        let written = std::fs::read_to_string(td.path().join("b.txt")).unwrap();
        assert_eq!(written, "line1\nline2\n");

        // INSERT_AT_LINE: line > file_lines+1 → ERR_INVALID_INPUT
        let em = invoke(
            &cell,
            json!({"op":"insert_at_line","path":"b.txt","line":100,"content":"x\n"}),
        )
        .await;
        assert_eq!(em.content["header"]["error_code"], "invalid_input");

        // FIND_REPLACE: nicht-existent → ERR_NOT_FOUND
        let em = invoke(
            &cell,
            json!({"op":"find_replace","path":"nope.txt","find":"a","replace":"b"}),
        )
        .await;
        assert_eq!(em.content["header"]["error_code"], "not_found");

        // FIND_REPLACE: absolute path → ERR_INVALID_INPUT
        let em = invoke(
            &cell,
            json!({"op":"find_replace","path":"/etc/passwd","find":"a","replace":"b"}),
        )
        .await;
        assert_eq!(em.content["header"]["error_code"], "invalid_input");
    }

    #[test]
    fn parse_edit_args_find_replace() {
        let op = parse_edit_args(&json!({
            "op": "find_replace", "path": "a.txt", "find": "foo", "replace": "bar"
        }))
        .unwrap();
        assert!(matches!(op,
            EditOp::FindReplace { ref path, ref find, ref replace }
            if path == "a.txt" && find == "foo" && replace == "bar"));
    }

    #[test]
    fn parse_edit_args_insert_at_line() {
        let op = parse_edit_args(&json!({
            "op": "insert_at_line", "path": "a.txt", "line": 3, "content": "new\n"
        }))
        .unwrap();
        assert!(matches!(op,
            EditOp::InsertAtLine { ref path, line, ref content }
            if path == "a.txt" && line == 3 && content == "new\n"));
    }

    #[test]
    fn parse_edit_args_find_replace_requires_find_and_replace() {
        assert!(parse_edit_args(&json!({"op": "find_replace", "path": "a.txt"})).is_err());
        assert!(
            parse_edit_args(&json!({"op": "find_replace", "path": "a.txt", "find": "x"})).is_err()
        );
    }

    #[test]
    fn parse_edit_args_rejects_zero_line() {
        let r = parse_edit_args(&json!({
            "op": "insert_at_line", "path": "a.txt", "line": 0, "content": "x"
        }));
        assert!(r.is_err(), "line=0 must be rejected (1-based)");
    }

    #[test]
    fn parse_edit_args_rejects_unknown_op() {
        assert!(parse_edit_args(&json!({"op": "patch", "path": "a.txt"})).is_err());
    }

    #[test]
    fn parse_edit_args_rejects_missing_path() {
        assert!(
            parse_edit_args(&json!({"op": "find_replace", "find": "x", "replace": "y"})).is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn edit_offers_boot_inactive_respawn() {
        use meclaw_colony::CellFactory;
        let td = tempfile::TempDir::new().unwrap();
        let factory = Arc::new(EditCellFactory);
        let (out_tx, _orx) = tokio::sync::mpsc::channel(8);
        let (itx, _irx) = tokio::sync::mpsc::channel(8);
        let hook = factory.build_boot_inactive_respawn(
            meclaw_core::Path::new("/c"),
            json!({"base_path": td.path().to_str().unwrap(), "max_concurrency": 4}),
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
            "stateless edit factory MUST offer a real boot-inactive respawn (eager reconnect)"
        );
    }
}
