//! Phase-9 StoreCell: dispatches store-ops via DbConn (query_timeout
//! via DbConn::call_with_timeout when params.query_timeout_ms is set;
//! else DbConn::call). Emits exactly one tool_result-Turn per message.

use crate::store::{StoreParams, WriteSurface, ops, output};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{Body, CellOutput, Message, OutputSink, Path};

/// Phase-9 StoreCell. Dispatches store ops via DbConn; emits exactly
/// one `tool_result`-Turn per inbound message.
///
/// SQL-Errors (constraint violations, missing tables, etc.) emerge as
/// normal `tool_result`-Turns with an `error_code`-header (per
/// Brainstorm E5). Only query timeouts and parse failures produce
/// `finish_reason: "error"` Error-Messages.
pub struct StoreCell {
    /// Live effective params (β). Held so a runtime `params`-update message can
    /// merge over them ([`crate::params_overlay::apply_update`]). The only
    /// mutable param is `query_timeout_ms` (path C, applied live to the `DbConn`);
    /// `schema` is immutable. When `query_timeout_ms` is set, ops run via
    /// `DbConn::call_with_timeout`; on interrupt → Error-Message with
    /// `finish_reason:"error"`, `error_code:"query_timeout"`.
    pub params: StoreParams,
}

impl StoreCell {
    /// Construct a new StoreCell from its effective params. Implementation
    /// detail — production entry is `StoreCellFactory`.
    #[doc(hidden)]
    pub fn new(params: StoreParams) -> Self {
        Self { params }
    }

    /// The effective query timeout derived from `params.query_timeout_ms`.
    fn query_timeout(&self) -> Option<std::time::Duration> {
        self.params
            .query_timeout_ms
            .map(std::time::Duration::from_millis)
    }
}

/// Parse a `tool_call`-Turn out of the inbound message body. Phase-9
/// scope: only `tool_call`-type turns (Tool-Loop primary use-case).
/// Direct user/system-origin invocation (cell-types.md Z.56) is a
/// Phase-9-Limitation.
fn parse_tool_call(msg: &Message) -> Result<(Value, Option<String>), String> {
    let Body::Inline(v) = &msg.body else {
        return Err("body must be inline (Blob-bodies are Phase-12 deferred)".into());
    };
    let messages = v
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or("missing messages array")?;
    let first = messages.first().ok_or("messages empty")?;
    let ty = first
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or("turn missing type")?;
    if ty != "tool_call" {
        return Err(format!("expected tool_call turn, got {ty}"));
    }
    let text = first
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or("missing text")?;
    let args: Value = meclaw_core::serde_json::from_str(text)
        .map_err(|e| format!("tool_call.text not valid JSON: {e}"))?;
    let id = first
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok((args, id))
}

async fn emit_invalid_input(
    sink: &OutputSink,
    target: &Path,
    msg: String,
    started: std::time::Instant,
) {
    let duration_ms = started.elapsed().as_millis() as i64;
    let body = meclaw_core::serde_json::json!({
        "header": {
            "finish_reason": "error",
            "error_code": "invalid_input",
            "duration_ms": duration_ms,
        },
        "messages": [{"origin":"tool","type":"tool_result","text": msg,"id":""}]
    });
    let _ = sink
        .push(CellOutput {
            target: target.clone(),
            content: body,
        })
        .await;
}

/// GH #132 — the ops that CHANGE the store. Everything not named here is a
/// read (`select`, `search`, `traverse`, `similar`, `alias_candidates`) and is
/// never bounded by the write surface.
///
/// `create_table` counts as a write: it changes the shape of the data, which is
/// the strongest write there is. `canonicalize` counts as a write: it rewrites
/// the derived columns of every row. `params_update` is not a store op at all
/// but the β params slot — it persists into `cell.db` and is therefore a write
/// by the same definition, which is why it travels through the same list.
const WRITE_OPS: &[&str] = &[
    "insert",
    "update",
    "delete",
    "create_table",
    "set_alias",
    "reject_pair",
    "canonicalize",
    "params_update",
];

/// The scope that owns a store: its own parent path (GH #132).
///
/// `/main/affinity/store` is owned by `/main/affinity` — the hive it sits in.
/// A store directly under the colony root gets `/`, which contains every cell,
/// so the declaration is inert there; that is documented rather than special-
/// cased, because at the root there is no enclosing hive a boundary could
/// follow.
fn owner_scope(store_path: &Path) -> String {
    let s = store_path.as_str();
    match s.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => s[..i].to_string(),
    }
}

/// True iff `sender` lies inside `scope` (equal to it or below it).
fn within_scope(scope: &str, sender: &Path) -> bool {
    let s = sender.as_str();
    if scope == "/" {
        return true;
    }
    s == scope || s.starts_with(&format!("{scope}/"))
}

/// GH #132 — refuse a write from outside the owning hive scope.
///
/// Returns `Some(detail)` when the op must be refused. Fail-closed on an unknown
/// sender: a message without `reply_to` is a source message (an HTTP ingress,
/// an event) that no edge inside the hive produced, so it is outside by
/// definition.
fn write_surface_violation(
    params: &StoreParams,
    store_path: &Path,
    sender: Option<&Path>,
    operation: &str,
) -> Option<String> {
    if params.write_surface != WriteSurface::Internal {
        return None;
    }
    if !WRITE_OPS.contains(&operation) {
        return None;
    }
    let scope = owner_scope(store_path);
    match sender {
        Some(p) if within_scope(&scope, p) => None,
        Some(p) => Some(format!(
            "operation '{operation}' refused: this store declares \
             write_surface \"internal\", so only senders inside '{scope}' may write — \
             '{p}' is outside it. Reads are unaffected.",
            p = p.as_str()
        )),
        None => Some(format!(
            "operation '{operation}' refused: this store declares \
             write_surface \"internal\", so only senders inside '{scope}' may write — this \
             message carries no sender. Reads are unaffected."
        )),
    }
}

async fn emit_write_denied(
    sink: &OutputSink,
    target: &Path,
    call_id: Option<String>,
    detail: String,
    started: std::time::Instant,
) {
    let duration_ms = started.elapsed().as_millis() as i64;
    let body = meclaw_core::serde_json::json!({
        "header": {
            "finish_reason": "error",
            "error_code": "write_denied",
            "duration_ms": duration_ms,
        },
        "messages": [{"origin":"tool","type":"tool_result","text": detail,
                      "id": call_id.unwrap_or_default()}]
    });
    let _ = sink
        .push(CellOutput {
            target: target.clone(),
            content: body,
        })
        .await;
}

async fn emit_query_timeout(
    sink: &OutputSink,
    target: &Path,
    call_id: Option<String>,
    started: std::time::Instant,
) {
    let duration_ms = started.elapsed().as_millis() as i64;
    let body = meclaw_core::serde_json::json!({
        "header": {
            "finish_reason": "error",
            "error_code": "query_timeout",
            "duration_ms": duration_ms,
        },
        "messages": [{"origin":"tool","type":"tool_result",
                      "text":"query exceeded query_timeout_ms",
                      "id": call_id.unwrap_or_default()}]
    });
    let _ = sink
        .push(CellOutput {
            target: target.clone(),
            content: body,
        })
        .await;
}

#[allow(clippy::manual_async_fn)]
impl StatefulCell for StoreCell {
    /// Handle one store-op message: parse the inbound `tool_call`-Turn,
    /// dispatch the operation via `DbConn` (with optional `query_timeout`
    /// via InterruptHandle when `params.query_timeout_ms` is set), and
    /// emit exactly one `tool_result`-Turn. SQL-Errors produce a
    /// `tool_result` with `error_code`-Header (brainstorm E5: NOT
    /// `finish_reason: "error"`). Only parse failures (`invalid_input`)
    /// and query timeouts (`query_timeout`) produce Error-Messages with
    /// `finish_reason: "error"`.
    fn handle<'a>(
        &'a mut self,
        msg: Message,
        sink: &'a OutputSink,
        db: &'a mut DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            let started = std::time::Instant::now();
            let reply_target = msg.reply_to.clone().unwrap_or_else(|| msg.target.clone());
            // GH #132: the sender the COLONY stamped (`reply_to`), never a body
            // field — an identity a caller could write into the body is not an
            // identity. `sink.sender_path()` is this cell's own address, set by
            // `cell_task` from the registry, so the owning scope is derived from
            // substrate truth on both sides.
            let store_path = sink.sender_path().clone();
            let sender = msg.reply_to.clone();

            // β: params-update slot (config.md § Access l.20), handled FIRST and
            // strictly. The `params` block is merged into `self.params` + persisted
            // to cell.db, THEN any tool_call runs with the updated params. A
            // params-only message persists and returns silently. All-or-nothing:
            // an immutable (`schema`) / unknown / malformed update is a loud
            // `invalid_input` reject with NO partial apply.
            let mut has_params = false;
            if let Body::Inline(v) = &msg.body
                && let Some(params_val) = v.get("params")
            {
                has_params = true;
                // GH #132: a params update writes to `cell.db`, so it is a write
                // and the same boundary applies. Checked BEFORE the merge, so an
                // outside update leaves nothing behind — not even a persisted
                // overlay.
                if let Some(detail) = write_surface_violation(
                    &self.params,
                    &store_path,
                    sender.as_ref(),
                    "params_update",
                ) {
                    emit_write_denied(sink, &reply_target, None, detail, started).await;
                    return;
                }
                let update_obj = match params_val.as_object() {
                    Some(o) => o.clone(),
                    None => {
                        emit_invalid_input(
                            sink,
                            &reply_target,
                            "invalid params slot: not a JSON object".to_string(),
                            started,
                        )
                        .await;
                        return;
                    }
                };
                match crate::params_overlay::apply_update(&self.params, &update_obj) {
                    Ok((new_params, overlay)) => {
                        let now = crate::params_overlay::now_unix_seconds();
                        let persist = db
                            .call(move |c| {
                                crate::params_overlay::persist_params_overlay(c, &overlay, now)
                            })
                            .await;
                        if let Err(e) = persist {
                            emit_invalid_input(
                                sink,
                                &reply_target,
                                format!("cell.db params write failed: {e}"),
                                started,
                            )
                            .await;
                            return;
                        }
                        // Live apply: self.params + DbConn query timeout (path C,
                        // sofort-live — the next op already uses the new value).
                        self.params = new_params;
                        db.set_query_timeout(self.query_timeout());
                    }
                    Err(e) => {
                        emit_invalid_input(sink, &reply_target, e.detail(), started).await;
                        return;
                    }
                }
            }

            // params-only message (no tool_call to follow): persist + stay silent.
            let has_messages = matches!(&msg.body, Body::Inline(v)
                if v.get("messages").and_then(|m| m.as_array()).is_some_and(|a| !a.is_empty()));
            if has_params && !has_messages {
                return;
            }

            let (args, call_id) = match parse_tool_call(&msg) {
                Ok(p) => p,
                Err(e) => {
                    emit_invalid_input(sink, &reply_target, e, started).await;
                    return;
                }
            };
            // GH #132: the writer boundary, checked between parse and dispatch —
            // after the args are known (so the op name is real) and before the DB
            // task starts (so a refused write touches nothing). Reads pass
            // straight through; an open write surface never reaches this branch.
            if let Some(detail) = write_surface_violation(
                &self.params,
                &store_path,
                sender.as_ref(),
                args.get("operation").and_then(|v| v.as_str()).unwrap_or(""),
            ) {
                emit_write_denied(sink, &reply_target, call_id, detail, started).await;
                return;
            }
            // The canonical bindings ride along into the DB task: they are what
            // makes the derived column store-owned on every write (0.2.0 P2).
            let canonical = self.params.canonical.clone();
            let outcome = if self.query_timeout().is_some() {
                match db
                    .call_with_timeout(move |c| ops::dispatch_with(c, &args, &canonical))
                    .await
                {
                    Ok(Ok(o)) => o,
                    Ok(Err(e)) => {
                        emit_invalid_input(sink, &reply_target, e, started).await;
                        return;
                    }
                    Err(meclaw_colony::QueryTimeout::Interrupted) => {
                        emit_query_timeout(sink, &reply_target, call_id, started).await;
                        return;
                    }
                }
            } else {
                match db
                    .call(move |c| ops::dispatch_with(c, &args, &canonical))
                    .await
                {
                    Ok(o) => o,
                    Err(e) => {
                        emit_invalid_input(sink, &reply_target, e, started).await;
                        return;
                    }
                }
            };
            let duration_ms = started.elapsed().as_millis() as i64;
            let (body, headers) =
                output::build_tool_result(&outcome, call_id.unwrap_or_default(), duration_ms);
            let mut content = Map::new();
            content.insert("header".into(), Value::Object(headers));
            if let Value::Object(o) = body {
                content.extend(o);
            }
            let _ = sink
                .push(CellOutput {
                    target: reply_target.clone(),
                    content: Value::Object(content),
                })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_colony::DbConn;
    use meclaw_colony::stateful_cell::StatefulCell;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
    use tokio::sync::mpsc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn insert_emits_tool_result_with_operation_header() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
            .unwrap();
        let mut db = DbConn::wrap(conn, None);

        let mut cell = StoreCell::new(
            StoreParams::parse(&json!({
                "schema": {"items": {"id": "int", "name": "text"}}
            }))
            .unwrap(),
        );
        let (otx, mut orx) = mpsc::channel(8);
        let sink = OutputSink::new(
            otx,
            Path::new("/store"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            None,
        );
        let body = json!({"messages":[{
            "origin":"assistant","type":"tool_call",
            "text": r#"{"operation":"insert","table":"items","row":{"id":1,"name":"x"}}"#,
            "id":"call_1"
        }]});
        let msg = MessageBuilder::new(Path::new("/store"))
            .body(Body::Inline(body))
            .reply_to(Path::new("/sink"))
            .build();
        cell.handle(msg, &sink, &mut db).await;
        drop(sink);

        let em = orx.recv().await.unwrap();
        let headers = &em.content["header"];
        assert_eq!(headers["operation"], "insert");
        assert_eq!(headers["rows_affected"], 1);
    }

    // ---- β2: runtime params-overlay (store) ----

    fn store_cell() -> StoreCell {
        StoreCell::new(StoreParams::parse(&json!({"schema": {"items": {"id": "int"}}})).unwrap())
    }

    fn sink_pair() -> (OutputSink, mpsc::Receiver<meclaw_core::CellEmission>) {
        let (otx, orx) = mpsc::channel(8);
        let sink = OutputSink::new(
            otx,
            Path::new("/store"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            None,
        );
        (sink, orx)
    }

    fn params_msg(body: meclaw_core::serde_json::Value) -> Message {
        MessageBuilder::new(Path::new("/store"))
            .body(Body::Inline(body))
            .reply_to(Path::new("/sink"))
            .build()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn params_update_query_timeout_persisted_and_live() {
        // Path C, immediately live: a params update sets query_timeout_ms; it is persisted
        // to cell.db AND immediately applied to the live DbConn (positive receipt:
        // db.query_timeout() reflects the new value without any wake/respawn — and
        // db_conn.rs::set_query_timeout_takes_effect_on_next_call proves that live
        // value enforces the timeout on the next query).
        let td = tempfile::TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = DbConn::wrap(conn, None);
        let mut cell = store_cell();
        let (sink, mut orx) = sink_pair();

        cell.handle(
            params_msg(json!({"params": {"query_timeout_ms": 1234}})),
            &sink,
            &mut db,
        )
        .await;

        // live wiring + self.params
        assert_eq!(
            db.query_timeout(),
            Some(std::time::Duration::from_millis(1234)),
            "DbConn must carry the new timeout live (path C)"
        );
        assert_eq!(cell.params.query_timeout_ms, Some(1234));
        // persisted to cell.db params table
        let persisted: String = db
            .call(|c| {
                c.query_row(
                    "SELECT value FROM params WHERE key='query_timeout_ms'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
            })
            .await;
        assert_eq!(persisted, "1234");
        // params-only message stays silent (no emission).
        drop(sink);
        assert!(orx.recv().await.is_none(), "params-only must not emit");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn params_update_immutable_schema_rejected() {
        let td = tempfile::TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = DbConn::wrap(conn, None);
        let mut cell = store_cell();
        let (sink, mut orx) = sink_pair();

        cell.handle(
            params_msg(json!({"params": {"schema": {"x": {"y": "int"}}}})),
            &sink,
            &mut db,
        )
        .await;
        drop(sink);

        let em = orx.recv().await.expect("immutable reject must emit");
        assert_eq!(em.content["header"]["error_code"], "invalid_input");
        // schema unchanged (no partial apply): the original single `items` table.
        assert_eq!(cell.params.schema.len(), 1);
        assert!(cell.params.schema.contains_key("items"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn params_update_unknown_key_rejected() {
        let td = tempfile::TempDir::new().unwrap();
        let conn =
            meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
        let mut db = DbConn::wrap(conn, None);
        let mut cell = store_cell();
        let (sink, mut orx) = sink_pair();

        cell.handle(
            params_msg(json!({"params": {"query_timeut_ms": 10}})),
            &sink,
            &mut db,
        )
        .await;
        drop(sink);

        let em = orx.recv().await.expect("unknown-key reject must emit");
        assert_eq!(em.content["header"]["error_code"], "invalid_input");
    }
}
