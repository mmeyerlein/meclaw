//! Phase-7 FileCell — stateless file-access tool cell.
//!
//! Security boundary: all relative paths are resolved under `base_path` via
//! `canonicalize`. Absolute paths and symlink-escapes are rejected.
//! Disk I/O runs in `tokio::task::spawn_blocking` (Constraint 5).

use crate::boundary::{self, ResolveErr, resolve_error_code};
use crate::tool::{
    ERR_INVALID_INPUT, ERR_IO_ERROR, ERR_NOT_A_DIRECTORY, ERR_NOT_A_FILE, ERR_NOT_FOUND,
    build_error_body, build_tool_result_body, parse_tool_call_args,
};
use meclaw_core::serde_json::{self, Map, Value};
use meclaw_core::{CellOutput, JsonValue, Message, OutputSink, Path};
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;

// ---- FileCell struct -------------------------------------------------------

/// Stateless file-access cell. Read-only configuration fields.
pub struct FileCell {
    /// Base directory — all relative paths are resolved under this boundary.
    pub base_path: PathBuf,
    /// Maximum number of concurrently running workers for this cell.
    pub max_concurrency: usize,
}

// ---- FileOp ----------------------------------------------------------------

pub(crate) enum FileOp {
    Read {
        path: String,
        /// GH #106: how the bytes reach the caller. `None` ⇒ the historic
        /// UTF-8 text contract.
        mode: ReadMode,
        /// GH #106: byte range. BYTE semantics — it knows nothing about
        /// characters (that is what `ReadMode::Base64` is for).
        range: ByteRange,
    },
    Write {
        path: String,
        content: String,
    },
    List {
        path: String,
    },
    Stat {
        path: String,
    },
}

/// GH #106: the two shapes a `read` payload can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadMode {
    /// Default. `text` is the file content; non-UTF-8 is a typed `io_error`.
    Text,
    /// `text` is the standard-alphabet, padded base64 of the raw bytes, and
    /// the emission carries `encoding: "base64"`.
    Base64,
}

/// GH #106: an optional byte window into the file. `Default` is the whole file
/// and takes the historic `read_to_string` path unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ByteRange {
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

impl ByteRange {
    /// True when the caller asked for the whole file (the pinned default path).
    fn is_whole_file(&self) -> bool {
        self.offset.is_none() && self.limit.is_none()
    }

    /// Applies the window to `bytes`, clamping at EOF. An offset at or past the
    /// end yields an empty slice — the "you are at the end" paging signal.
    fn slice<'a>(&self, bytes: &'a [u8]) -> &'a [u8] {
        let start = self.offset.unwrap_or(0).min(bytes.len() as u64) as usize;
        let rest = &bytes[start..];
        match self.limit {
            Some(n) => &rest[..(n.min(rest.len() as u64) as usize)],
            None => rest,
        }
    }
}

/// Standard base64 (RFC 4648 §4, padded). Hand-rolled: the encoder is twenty
/// lines and the tech-stack allow-list is closed — a dependency for this would
/// be a spec conflict, not a convenience.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// ---- parse_file_op ---------------------------------------------------------

/// GH #106: the arguments that only `read` understands.
const READ_ONLY_ARGS: [&str; 3] = ["mode", "offset", "limit"];

/// GH #106: `mode` is optional; absent, `null` and `"text"` are the same
/// (historic) contract.
fn parse_read_mode(args: &JsonValue) -> Result<ReadMode, String> {
    match args.get("mode") {
        None => Ok(ReadMode::Text),
        Some(v) if v.is_null() => Ok(ReadMode::Text),
        Some(v) => match v.as_str() {
            Some("text") => Ok(ReadMode::Text),
            Some("base64") => Ok(ReadMode::Base64),
            _ => Err("args.mode must be \"text\" or \"base64\"".to_string()),
        },
    }
}

/// GH #106: `offset` (>= 0) and `limit` (>= 1), both in BYTES. A zero `limit`
/// is rejected — a read of nothing is not a read, it is a mistyped argument.
fn parse_byte_range(args: &JsonValue) -> Result<ByteRange, String> {
    fn num(args: &JsonValue, key: &str) -> Result<Option<u64>, String> {
        match args.get(key) {
            None => Ok(None),
            Some(v) if v.is_null() => Ok(None),
            Some(v) => Ok(Some(v.as_u64().ok_or_else(|| {
                format!("args.{key} must be a non-negative integer (bytes)")
            })?)),
        }
    }
    let offset = num(args, "offset")?;
    let limit = num(args, "limit")?;
    if limit == Some(0) {
        return Err("args.limit must be >= 1 (bytes)".into());
    }
    Ok(ByteRange { offset, limit })
}

/// Parses tool_call args for FileCell. Returns `Err(human-readable)` with
/// `ERR_INVALID_INPUT` semantics (caller builds the error body).
pub(crate) fn parse_file_op(args: &JsonValue) -> Result<FileOp, String> {
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
    // GH #106: `mode`/`offset`/`limit` are READ arguments. Ignoring them on the
    // other ops would let a caller believe in a partial write or a paged list
    // that never happened — the same class of lie as a guard that never runs.
    if op != "read" {
        for key in READ_ONLY_ARGS {
            if args.get(key).is_some_and(|v| !v.is_null()) {
                return Err(format!("args.{key} is only valid for op \"read\""));
            }
        }
    }
    match op {
        "read" => Ok(FileOp::Read {
            path: path.to_string(),
            mode: parse_read_mode(args)?,
            range: parse_byte_range(args)?,
        }),
        "write" => {
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    "args.content missing or not a string (required for write)".to_string()
                })?;
            Ok(FileOp::Write {
                path: path.to_string(),
                content: content.to_string(),
            })
        }
        "list" => Ok(FileOp::List {
            path: path.to_string(),
        }),
        "stat" => Ok(FileOp::Stat {
            path: path.to_string(),
        }),
        other => Err(format!("unknown op: {other}")),
    }
}

// ---- FileCell resolve helpers ----------------------------------------------

impl FileCell {
    /// Resolve `rel` for read/list/stat: must exist, canonicalize follows
    /// symlinks; final canonical path must be under `base_path.canonicalize()`.
    pub(crate) fn resolve_existing(&self, rel: &str) -> Result<PathBuf, ResolveErr> {
        boundary::resolve_existing(&self.base_path, rel)
    }

    /// Resolve `rel` for write: parent MUST exist (Entscheidung 1.1, no
    /// auto-`create_dir_all`); file itself may be new. Parent is canonicalized
    /// (symlink-safe); final path = canon_parent.join(file_name).
    pub(crate) fn resolve_write_parent(&self, rel: &str) -> Result<PathBuf, ResolveErr> {
        boundary::resolve_write_parent(&self.base_path, rel)
    }
}

// ---- StatelessCell impl ----------------------------------------------------

impl meclaw_colony::StatelessCell for FileCell {
    #[allow(clippy::manual_async_fn)]
    /// Handle one tool_call message: parse args, dispatch the file op
    /// (read/write/list/stat) via `spawn_blocking`, emit a `tool_result`
    /// or error message to `msg.reply_to` (fallback `msg.target` (W2d: its own path, not the READ endpoint)).
    fn handle<'a>(
        &'a self,
        msg: Message,
        sink: &'a OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            let started = std::time::Instant::now();
            let reply_target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());

            // Parse tool_call
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
            let op = match parse_file_op(&args) {
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

            // Run the op in spawn_blocking
            let base = self.base_path.clone();
            let op_label = describe_op(&op);
            let result = tokio::task::spawn_blocking({
                let base = base.clone();
                move || run_op(&base, op)
            })
            .await;

            let outcome = match result {
                Ok(o) => o,
                Err(e) => OpOutcome::Err {
                    code: ERR_IO_ERROR,
                    text: format!("spawn_blocking join error: {e}"),
                },
            };

            let duration_ms = started.elapsed().as_millis() as u64;
            match outcome {
                OpOutcome::Ok {
                    text,
                    bytes,
                    encoding,
                } => {
                    let mut header = Map::new();
                    header.insert("operation".into(), Value::String(op_label.clone()));
                    header.insert("bytes".into(), Value::from(bytes));
                    header.insert("duration_ms".into(), Value::from(duration_ms));
                    // GH #106: only a non-text payload announces itself.
                    if let Some(enc) = encoding {
                        header.insert("encoding".into(), Value::String(enc.to_string()));
                    }
                    let body = build_tool_result_body(text, id, header);
                    tracing::info!(operation = %op_label, bytes, duration_ms, "file op ok");
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
                    header.insert("duration_ms".into(), Value::from(duration_ms));
                    let body = build_error_body(code, text, id, header);
                    tracing::info!(operation = %op_label, error_code = code, duration_ms, "file op err");
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

impl FileCell {
    /// Emit a UBF error-body to `reply_target` with the given `code` and
    /// human-readable `text`. Sets `operation`/`duration_ms`-Headers.
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
        tracing::info!(operation, error_code = code, duration_ms, "file pre-op err");
        let _ = sink
            .push(CellOutput {
                target: reply_target,
                content: body,
            })
            .await;
    }
}

// ---- Op helpers ------------------------------------------------------------

fn describe_op(op: &FileOp) -> String {
    match op {
        FileOp::Read { .. } => "read".into(),
        FileOp::Write { .. } => "write".into(),
        FileOp::List { .. } => "list".into(),
        FileOp::Stat { .. } => "stat".into(),
    }
}

enum OpOutcome {
    Ok {
        text: String,
        bytes: u64,
        /// GH #106: `Some("base64")` marks a payload that is NOT the file's
        /// text. Absent on every historic path.
        encoding: Option<&'static str>,
    },
    Err {
        code: &'static str,
        text: String,
    },
}

/// GH #106: turns the resolved file into the payload the caller asked for.
/// The default (text, whole file) keeps the historic `read_to_string` path
/// byte-for-byte — including its UTF-8 error text.
fn read_payload(p: &StdPath, mode: ReadMode, range: ByteRange) -> OpOutcome {
    if mode == ReadMode::Text && range.is_whole_file() {
        return match std::fs::read_to_string(p) {
            Err(e) => map_io_to_outcome(e),
            Ok(s) => {
                let bytes = s.len() as u64;
                OpOutcome::Ok {
                    text: s,
                    bytes,
                    encoding: None,
                }
            }
        };
    }
    let raw = match std::fs::read(p) {
        Err(e) => return map_io_to_outcome(e),
        Ok(b) => b,
    };
    let slice = range.slice(&raw);
    let bytes = slice.len() as u64;
    match mode {
        ReadMode::Base64 => OpOutcome::Ok {
            text: base64_encode(slice),
            bytes,
            encoding: Some("base64"),
        },
        // The range is BYTE semantics, so it can land mid-character. Same code
        // as any other non-UTF-8 read; the text names the way out.
        ReadMode::Text => match std::str::from_utf8(slice) {
            Ok(s) => OpOutcome::Ok {
                text: s.to_string(),
                bytes,
                encoding: None,
            },
            Err(e) => OpOutcome::Err {
                code: ERR_IO_ERROR,
                text: format!(
                    "byte range is not valid UTF-8 ({e}); \
                     offset/limit count BYTES — read it with mode \"base64\""
                ),
            },
        },
    }
}

/// Runs a `FileOp` synchronously. Called from `spawn_blocking`.
///
/// Builds a temporary `FileCell` for resolve-method access (verify note 3).
fn run_op(base: &StdPath, op: FileOp) -> OpOutcome {
    let cell = FileCell {
        base_path: base.to_path_buf(),
        max_concurrency: 0, // unused in resolve helpers
    };
    match op {
        FileOp::Read { path, mode, range } => match cell.resolve_existing(&path) {
            Err(e) => OpOutcome::Err {
                code: resolve_error_code(&e),
                text: e.to_string(),
            },
            Ok(p) => match std::fs::metadata(&p) {
                Err(e) => map_io_to_outcome(e),
                Ok(m) if m.is_dir() => OpOutcome::Err {
                    code: ERR_NOT_A_FILE,
                    text: format!("{path:?} is a directory"),
                },
                Ok(_) => read_payload(&p, mode, range),
            },
        },
        FileOp::Write { path, content } => match cell.resolve_write_parent(&path) {
            Err(e) => OpOutcome::Err {
                code: resolve_error_code(&e),
                text: e.to_string(),
            },
            Ok(p) => match std::fs::write(&p, &content) {
                Err(e) => map_write_io_to_outcome(e),
                Ok(()) => OpOutcome::Ok {
                    text: format!("{{\"written\":{}}}", content.len()),
                    bytes: content.len() as u64,
                    encoding: None,
                },
            },
        },
        FileOp::List { path } => match cell.resolve_existing(&path) {
            Err(e) => OpOutcome::Err {
                code: resolve_error_code(&e),
                text: e.to_string(),
            },
            Ok(p) => match std::fs::metadata(&p) {
                Err(e) => map_io_to_outcome(e),
                Ok(m) if !m.is_dir() => OpOutcome::Err {
                    code: ERR_NOT_A_DIRECTORY,
                    text: format!("{path:?} is not a directory"),
                },
                Ok(_) => list_dir(&p),
            },
        },
        FileOp::Stat { path } => match cell.resolve_existing(&path) {
            Err(e) => OpOutcome::Err {
                code: resolve_error_code(&e),
                text: e.to_string(),
            },
            Ok(p) => stat_path(&p),
        },
    }
}

fn list_dir(p: &StdPath) -> OpOutcome {
    let mut entries: Vec<(String, Value)> = Vec::new();
    match std::fs::read_dir(p) {
        Err(e) => return map_io_to_outcome(e),
        Ok(rd) => {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let ft = match entry.file_type() {
                    Ok(t) => t,
                    // Skip note (documented POC boundary): a broken symlink or a
                    // permission race during read_dir makes file_type() err; the
                    // entry is dropped markerless, so a caller cannot tell
                    // "absent" from "untypeable". Lowest-severity silent skip —
                    // registered here rather than surfaced.
                    Err(_) => continue,
                };
                let kind = if ft.is_symlink() {
                    "symlink"
                } else if ft.is_dir() {
                    "dir"
                } else if ft.is_file() {
                    "file"
                } else {
                    "other"
                };
                let mut obj = Map::new();
                obj.insert("name".into(), Value::String(name.clone()));
                obj.insert("kind".into(), Value::String(kind.into()));
                if ft.is_file()
                    && let Ok(meta) = entry.metadata()
                {
                    obj.insert("size".into(), Value::from(meta.len()));
                }
                entries.push((name, Value::Object(obj)));
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let arr: Vec<Value> = entries.into_iter().map(|(_, v)| v).collect();
    let text = serde_json::to_string(&arr).unwrap();
    let bytes = text.len() as u64;
    OpOutcome::Ok {
        text,
        bytes,
        encoding: None,
    }
}

fn stat_path(p: &StdPath) -> OpOutcome {
    // symlink_metadata: for symlinks we report the link itself (kind=symlink),
    // not the target (brainstorm § 1.3).
    let meta = match std::fs::symlink_metadata(p) {
        Err(e) => return map_io_to_outcome(e),
        Ok(m) => m,
    };
    let ft = meta.file_type();
    let kind = if ft.is_symlink() {
        "symlink"
    } else if ft.is_dir() {
        "dir"
    } else if ft.is_file() {
        "file"
    } else {
        "other"
    };
    let mut obj = Map::new();
    obj.insert("kind".into(), Value::String(kind.into()));
    if ft.is_file() {
        obj.insert("size".into(), Value::from(meta.len()));
    } else {
        obj.insert("size".into(), Value::Null);
    }
    if let Ok(mtime) = meta.modified()
        && let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH)
    {
        obj.insert("modified".into(), Value::from(d.as_secs()));
    }
    let text = serde_json::to_string(&Value::Object(obj)).unwrap();
    let bytes = text.len() as u64;
    OpOutcome::Ok {
        text,
        bytes,
        encoding: None,
    }
}

fn map_io_to_outcome(e: std::io::Error) -> OpOutcome {
    let code = match e.kind() {
        std::io::ErrorKind::NotFound => ERR_NOT_FOUND,
        _ => ERR_IO_ERROR,
    };
    OpOutcome::Err {
        code,
        text: e.to_string(),
    }
}

/// GH #79: the write stage, after the parent resolved. A bare
/// `Permission denied (os error 13)` does not say at which stage it happened
/// nor which of the write path's several `io_error` causes it is — both are
/// repairs an agent picks differently. Classification is unchanged
/// (`map_io_to_outcome`'s taxonomy), only the text is named.
fn map_write_io_to_outcome(e: std::io::Error) -> OpOutcome {
    use std::io::ErrorKind;
    let code = match e.kind() {
        ErrorKind::NotFound => ERR_NOT_FOUND,
        _ => ERR_IO_ERROR,
    };
    let reason = match e.kind() {
        ErrorKind::PermissionDenied => Some("permission denied"),
        ErrorKind::ReadOnlyFilesystem => Some("read-only filesystem"),
        ErrorKind::IsADirectory => Some("target path is a directory"),
        ErrorKind::NotADirectory => Some("a path component is not a directory"),
        ErrorKind::StorageFull => Some("no space left on device"),
        // The parent was resolved a moment ago, so this is a race, not a typo.
        ErrorKind::NotFound => Some("path vanished between resolve and write"),
        _ => None,
    };
    OpOutcome::Err {
        code,
        text: match reason {
            Some(r) => format!("write failed: {r} ({e})"),
            None => format!("write failed: {e}"),
        },
    }
}

// ---- FileCellFactory -------------------------------------------------------

use meclaw_colony::{CellFactory, RespawnFn, SpawnedCellKind, build_stateless_task};

/// Unit-Struct factory for `FileCell`. Registered under cell type `"file"`.
pub struct FileCellFactory;

const DEFAULT_FILE_MAX_CONCURRENCY: usize = 8;

struct ParsedFileParams {
    base_path: PathBuf,
    max_concurrency: usize,
}

/// Pure validation: checks string + `is_absolute`. No filesystem access.
fn parse_params_pure(raw: &JsonValue) -> Result<ParsedFileParams, String> {
    let bp = raw
        .get("base_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "params.base_path missing or not a string".to_string())?;
    let bp_path = StdPath::new(bp);
    if !bp_path.is_absolute() {
        return Err(format!("params.base_path must be absolute, got: {bp}"));
    }
    let mc = match raw.get("max_concurrency") {
        None => DEFAULT_FILE_MAX_CONCURRENCY,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| "params.max_concurrency must be a positive integer".to_string())?
            as usize,
    };
    if mc == 0 {
        return Err("params.max_concurrency must be >= 1".into());
    }
    Ok(ParsedFileParams {
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
fn validate_base_path_fs(parsed: &ParsedFileParams) -> Result<PathBuf, String> {
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

impl CellFactory for FileCellFactory {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        let parsed = parse_params_pure(params)?;
        validate_base_path_fs(&parsed).map(|_| ())
    }

    /// Stateless cell — `cell_dir` and the three Phase-13-G-1 substrate params
    /// (`colony_inbox_tx`, `idle_timeout`, `cell_timeout`) are unused.
    fn spawn_cell(
        self: Arc<Self>,
        path: meclaw_core::Path,
        params: JsonValue,
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
        // FS-Validierung (Entscheidung 2.7) — SAME path as validate_params (U11).
        let canon = validate_base_path_fs(&parsed)?;
        let max_concurrency = parsed.max_concurrency;

        let cell = Arc::new(FileCell {
            base_path: canon.clone(),
            max_concurrency,
        });
        let (tx, rx) = tokio::sync::mpsc::channel::<meclaw_core::Message>(mailbox_capacity);
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

        // RespawnFn: builds FileCell + dispatcher fresh (stateless, no state).
        let respawn_path = path.clone();
        let respawn_canon = canon.clone();
        let respawn_outputs_tx = outputs_tx.clone();
        let respawn_blob = blob_store.clone();
        let respawn_mailbox_capacity = mailbox_capacity;
        // Slice 2: the cell's OWN pre-compiled consumes views (Arc-clone).
        let respawn_consumes = contract.consumes.clone();
        let respawn: RespawnFn = Box::new(move || {
            let cell = Arc::new(FileCell {
                base_path: respawn_canon.clone(),
                max_concurrency,
            });
            let (tx, rx) =
                tokio::sync::mpsc::channel::<meclaw_core::Message>(respawn_mailbox_capacity);
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
    /// Builds the SAME `Arc<FileCell>` as `spawn_cell` (incl. the canonicalize +
    /// is_dir FS-validation) and routes it through the
    /// `build_stateless_boot_inactive_respawn` funnel (I1). Returns `None` when
    /// params no longer parse OR the base_path no longer resolves to a directory.
    #[allow(clippy::too_many_arguments)]
    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: meclaw_core::Path,
        params: JsonValue,
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
        let canon = parsed.base_path.canonicalize().ok()?;
        if !canon.is_dir() {
            return None;
        }
        let max_concurrency = parsed.max_concurrency;
        let cell = Arc::new(FileCell {
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

// ---- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    // ---- Task-6 Factory Tests -----------------------------------------------

    #[test]
    fn factory_validate_params_rejects_missing_base_path() {
        use meclaw_colony::CellFactory;
        let f = FileCellFactory;
        assert!(
            f.validate_params(&meclaw_core::serde_json::json!({}))
                .is_err()
        );
    }

    #[test]
    fn factory_validate_params_rejects_relative_base_path() {
        use meclaw_colony::CellFactory;
        let f = FileCellFactory;
        assert!(
            f.validate_params(&meclaw_core::serde_json::json!({"base_path": "rel/dir"}))
                .is_err()
        );
    }

    #[test]
    fn factory_validate_params_rejects_max_concurrency_zero() {
        use meclaw_colony::CellFactory;
        let f = FileCellFactory;
        let r = f.validate_params(&meclaw_core::serde_json::json!({
            "base_path": "/tmp",
            "max_concurrency": 0
        }));
        assert!(r.is_err());
    }

    #[test]
    fn factory_validate_params_accepts_absolute_base_path() {
        use meclaw_colony::CellFactory;
        let f = FileCellFactory;
        assert!(
            f.validate_params(&meclaw_core::serde_json::json!({"base_path": "/tmp"}))
                .is_ok()
        );
    }

    // ---- U11: validate ≡ spawn parse path (existence/is_dir checked in validate too) ----

    #[test]
    fn factory_validate_params_rejects_nonexistent_base_path() {
        use meclaw_colony::CellFactory;
        let f = FileCellFactory;
        let err = f
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
        let f = FileCellFactory;
        let err = f
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
        std::fs::write(td.path().join("hello.txt"), b"world").unwrap();
        let factory: Arc<dyn CellFactory> = Arc::new(FileCellFactory);
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let spawned = factory
            .spawn_cell(
                Path::new("/file"),
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

        let msg = MessageBuilder::new(Path::new("/file"))
            .reply_to(Path::new("/caller"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "assistant", "type": "tool_call",
                    "text": r#"{"op":"read","path":"hello.txt"}"#,
                    "id": "call-1"
                }]
            })))
            .build();
        let (sender, join) = match spawned {
            SpawnedCellKind::Active { sender, join, .. } => (sender, join),
            SpawnedCellKind::Dormant { .. } => unreachable!("Phase-13-G-2: only Active"),
        };
        sender.send(msg).await.unwrap();

        // Deterministic rendezvous: recv().await returns as soon as the worker
        // writes the emission into out_tx. No time-based failure marker — a
        // channel close (None) would blow the test up on unwrap(), which would be
        // a real failure, not a flake.
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["messages"][0]["text"], "world");
        assert_eq!(em.content["header"]["operation"], "read");

        drop(sender);
        join.await.unwrap();
    }

    #[test]
    fn factory_spawn_cell_rejects_nonexistent_base_path() {
        use meclaw_colony::CellFactory;
        use std::sync::Arc;
        use tokio::sync::mpsc;
        let factory: Arc<dyn CellFactory> = Arc::new(FileCellFactory);
        let (out_tx, _out_rx) = mpsc::channel::<meclaw_core::CellEmission>(8);
        let (inbox_tx, _inbox_rx) = mpsc::channel(8);
        let r = factory.spawn_cell(
            meclaw_core::Path::new("/file"),
            meclaw_core::serde_json::json!({"base_path": "/this/does/not/exist/xyzzy-phase7"}),
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
        assert!(r.is_err(), "spawn must reject nonexistent base_path");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_read_write_list_stat_emit_tool_result() {
        use meclaw_colony::StatelessCell;
        use meclaw_core::{
            Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
            validate_ubf_body,
        };
        use tokio::sync::mpsc;

        let td = tempfile::TempDir::new().unwrap();
        std::fs::write(td.path().join("hello.txt"), b"world").unwrap();
        let cell = FileCell {
            base_path: td.path().to_path_buf(),
            max_concurrency: 8,
        };

        async fn invoke(cell: &FileCell, args: meclaw_core::serde_json::Value) -> CellEmission {
            let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
            let sink = OutputSink::new(
                out_tx,
                Path::new("/file"),
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
            let msg = MessageBuilder::new(Path::new("/file"))
                .reply_to(Path::new("/caller"))
                .body(Body::Inline(body))
                .build();
            cell.handle(msg, &sink).await;
            out_rx.recv().await.expect("emission")
        }

        // READ
        let em = invoke(&cell, json!({"op":"read","path":"hello.txt"})).await;
        validate_ubf_body(&em.content).unwrap();
        assert_eq!(em.target, Path::new("/caller"));
        assert_eq!(em.content["messages"][0]["text"], "world");
        assert_eq!(em.content["header"]["operation"], "read");
        assert_eq!(em.content["header"]["bytes"], 5);

        // WRITE (Parent existiert)
        let em = invoke(
            &cell,
            json!({"op":"write","path":"out.txt","content":"hello"}),
        )
        .await;
        validate_ubf_body(&em.content).unwrap();
        assert_eq!(em.content["header"]["operation"], "write");
        assert_eq!(em.content["header"]["bytes"], 5);
        let written = std::fs::read_to_string(td.path().join("out.txt")).unwrap();
        assert_eq!(written, "hello");

        // WRITE into a non-existent parent → io_error (decision 1.1)
        let em = invoke(
            &cell,
            json!({"op":"write","path":"subdir/x.txt","content":"y"}),
        )
        .await;
        validate_ubf_body(&em.content).unwrap();
        assert_eq!(em.content["header"]["error_code"], "io_error");

        // LIST
        let em = invoke(&cell, json!({"op":"list","path":"."})).await;
        validate_ubf_body(&em.content).unwrap();
        let text = em.content["messages"][0]["text"].as_str().unwrap();
        let entries: meclaw_core::serde_json::Value =
            meclaw_core::serde_json::from_str(text).unwrap();
        let entries = entries.as_array().unwrap();
        assert!(entries.iter().any(|e| e["name"] == "hello.txt"));
        assert!(entries.iter().any(|e| e["name"] == "out.txt"));

        // STAT
        let em = invoke(&cell, json!({"op":"stat","path":"hello.txt"})).await;
        validate_ubf_body(&em.content).unwrap();
        let text = em.content["messages"][0]["text"].as_str().unwrap();
        let stat: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(text).unwrap();
        assert_eq!(stat["kind"], "file");
        assert_eq!(stat["size"], 5);

        // not_found
        let em = invoke(&cell, json!({"op":"read","path":"nope.txt"})).await;
        assert_eq!(em.content["header"]["error_code"], "not_found");

        // path_outside_boundary (absolute → invalid_input)
        let em = invoke(&cell, json!({"op":"read","path":"/etc/passwd"})).await;
        assert_eq!(em.content["header"]["error_code"], "invalid_input");
    }

    #[test]
    fn parse_file_op_read() {
        let op = parse_file_op(&json!({"op": "read", "path": "a.txt"})).unwrap();
        assert!(matches!(op, FileOp::Read { ref path, mode, range }
            if path == "a.txt" && mode == ReadMode::Text && range == ByteRange::default()));
    }

    /// GH #106: RFC 4648 §4 test vectors — the encoder is hand-rolled, so its
    /// padding behaviour is pinned against the standard, not against itself.
    #[test]
    fn base64_encode_matches_rfc4648_vectors() {
        for (raw, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(raw.as_bytes()), want, "vector {raw:?}");
        }
        // Bytes no text mode could ever carry.
        assert_eq!(base64_encode(&[0xFF, 0xFE, 0x00]), "//4A");
    }

    #[test]
    fn parse_file_op_read_mode_and_range() {
        let op = parse_file_op(
            &json!({"op": "read", "path": "a.bin", "mode": "base64", "offset": 2, "limit": 8}),
        )
        .unwrap();
        assert!(matches!(op, FileOp::Read { mode, range, .. }
            if mode == ReadMode::Base64
                && range == ByteRange { offset: Some(2), limit: Some(8) }));

        // null reads as absent on every one of the three.
        let op = parse_file_op(
            &json!({"op": "read", "path": "a.bin", "mode": null, "offset": null, "limit": null}),
        )
        .unwrap();
        assert!(matches!(op, FileOp::Read { mode, range, .. }
            if mode == ReadMode::Text && range == ByteRange::default()));

        for bad in [
            json!({"op": "read", "path": "a", "mode": "binary"}),
            json!({"op": "read", "path": "a", "mode": 1}),
            json!({"op": "read", "path": "a", "limit": 0}),
            json!({"op": "read", "path": "a", "offset": -1}),
            json!({"op": "write", "path": "a", "content": "x", "limit": 1}),
            json!({"op": "list", "path": ".", "mode": "base64"}),
            json!({"op": "stat", "path": "a", "offset": 1}),
        ] {
            assert!(parse_file_op(&bad).is_err(), "must reject {bad}");
        }
    }

    #[test]
    fn byte_range_clamps_at_both_ends() {
        let data = b"0123456789";
        assert_eq!(ByteRange::default().slice(data), data);
        assert_eq!(
            ByteRange {
                offset: Some(3),
                limit: Some(4)
            }
            .slice(data),
            b"3456"
        );
        // Past EOF is empty, never a panic and never an error.
        assert_eq!(
            ByteRange {
                offset: Some(99),
                limit: None
            }
            .slice(data),
            b""
        );
        assert_eq!(
            ByteRange {
                offset: Some(8),
                limit: Some(999)
            }
            .slice(data),
            b"89"
        );
    }

    #[test]
    fn parse_file_op_write_requires_content() {
        assert!(parse_file_op(&json!({"op": "write", "path": "a.txt"})).is_err());
        let op = parse_file_op(&json!({"op": "write", "path": "a.txt", "content": "x"})).unwrap();
        assert!(
            matches!(op, FileOp::Write { ref path, ref content } if path == "a.txt" && content == "x")
        );
    }

    #[test]
    fn parse_file_op_rejects_unknown_op() {
        assert!(parse_file_op(&json!({"op": "delete", "path": "a.txt"})).is_err());
    }

    #[test]
    fn parse_file_op_rejects_missing_path() {
        assert!(parse_file_op(&json!({"op": "read"})).is_err());
    }

    #[test]
    fn resolve_under_boundary_rejects_absolute_rel() {
        let td = tempfile::TempDir::new().unwrap();
        let cell = FileCell {
            base_path: td.path().to_path_buf(),
            max_concurrency: 8,
        };
        assert!(cell.resolve_existing("/etc/passwd").is_err());
    }

    #[test]
    fn resolve_under_boundary_rejects_traversal() {
        let td = tempfile::TempDir::new().unwrap();
        let cell = FileCell {
            base_path: td.path().to_path_buf(),
            max_concurrency: 8,
        };
        std::fs::write(td.path().join("a.txt"), b"hi").unwrap();
        assert!(cell.resolve_existing("../../../etc/passwd").is_err());
    }

    #[test]
    fn resolve_under_boundary_accepts_inside() {
        let td = tempfile::TempDir::new().unwrap();
        let cell = FileCell {
            base_path: td.path().to_path_buf(),
            max_concurrency: 8,
        };
        std::fs::write(td.path().join("a.txt"), b"hi").unwrap();
        let resolved = cell.resolve_existing("a.txt").unwrap();
        assert!(resolved.ends_with("a.txt"));
        assert!(resolved.starts_with(td.path().canonicalize().unwrap()));
    }

    #[test]
    fn resolve_write_parent_must_exist() {
        let td = tempfile::TempDir::new().unwrap();
        let cell = FileCell {
            base_path: td.path().to_path_buf(),
            max_concurrency: 8,
        };
        let r = cell.resolve_write_parent("new.txt");
        assert!(r.is_ok());
        let r = cell.resolve_write_parent("subdir/new.txt");
        assert!(r.is_err(), "parent 'subdir' must exist; no auto-mkdir");
    }

    #[test]
    fn resolve_symlink_escape_rejected() {
        let td = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), td.path().join("escape"))
            .unwrap();
        #[cfg(not(unix))]
        return;
        let cell = FileCell {
            base_path: td.path().to_path_buf(),
            max_concurrency: 8,
        };
        assert!(cell.resolve_existing("escape").is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn file_offers_boot_inactive_respawn() {
        use meclaw_colony::CellFactory;
        use meclaw_core::serde_json::json;
        let td = tempfile::TempDir::new().unwrap();
        let factory = Arc::new(FileCellFactory);
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
            "stateless file factory MUST offer a real boot-inactive respawn (eager reconnect)"
        );
    }
}
