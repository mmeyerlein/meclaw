//! GH #331 — the `store` cell's error paths stamp `hop.operation`.
//!
//! `store/output.rs` writes `operation` on every reply it builds, and the
//! contract of every shipped store template declares `hop.operation` as
//! `required: true`. The three hand-built error emitters in `store/cell.rs`
//! (`emit_invalid_input`, `emit_write_denied`, `emit_query_timeout`) wrote
//! `finish_reason` / `error_code` / `duration_ms` and left `operation` out —
//! so a topology whose return edge reads `hop.operation` (the natural way to
//! recognise which op answered) lost exactly the replies that report a
//! failure, and the declaration promised a field the wire did not carry.
//!
//! Pinned here: every reply the cell can emit carries `header.operation` —
//! the refused op wherever it is parseable, the literal `"error"` where
//! nothing parseable arrived.

use meclaw_cells::store::{StoreCell, StoreParams};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, MessageBuilder, OutputSink, Path, Uuid};
use std::time::Duration;
use tokio::sync::mpsc;

/// The store lives at `/aff/store`, so its owning scope is `/aff` — the same
/// shape `gh132_store_internal_write_surface.rs` drives.
const STORE_PATH: &str = "/aff/store";

/// The `store` cell driven directly, exactly as the other store suites do it:
/// a `tool_call` turn in, one reply out.
struct StoreRig {
    cell: StoreCell,
    db: DbConn,
    sink: OutputSink,
    rx: mpsc::Receiver<CellEmission>,
}

impl StoreRig {
    fn with(conn: rusqlite::Connection, raw_params: Value) -> Self {
        let timeout = raw_params
            .get("query_timeout_ms")
            .and_then(|v| v.as_u64())
            .map(Duration::from_millis);
        let db = DbConn::wrap(conn, timeout);
        let cell = StoreCell::new(StoreParams::parse(&raw_params).expect("params parse"));
        let (otx, rx) = mpsc::channel(16);
        let sink = OutputSink::new(
            otx,
            Path::new(STORE_PATH),
            Uuid::now_v7(),
            Uuid::now_v7(),
            64,
            Headers::new(),
            None,
        );
        Self { cell, db, sink, rx }
    }

    /// One `tool_call` whose `text` goes on the wire VERBATIM — the point of
    /// case (a) is a `text` that is not JSON at all.
    async fn raw_call(&mut self, text: &str, sender: &str) -> Value {
        let msg = MessageBuilder::new(Path::new(STORE_PATH))
            .body(Body::Inline(json!({"messages":[{
                "origin": "assistant", "type": "tool_call",
                "text": text, "id": "call_1"
            }]})))
            .reply_to(Path::new(sender))
            .build();
        self.cell.handle(msg, &self.sink, &mut self.db).await;
        tokio::time::timeout(Duration::from_secs(30), self.rx.recv())
            .await
            .expect("no emission within 30s")
            .expect("channel open")
            .content
    }

    async fn op(&mut self, args: Value) -> Value {
        self.raw_call(&args.to_string(), "/aff/caller").await
    }
}

fn schema_only() -> Value {
    json!({"schema": {"items": {"id": "int", "name": "text"}}})
}

/// (a) nothing parseable arrived — the reply says so with the literal `error`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unparseable_tool_call_answers_with_operation_error() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let mut r = StoreRig::with(conn, schema_only());

    let out = r.raw_call("this is not JSON", "/aff/caller").await;

    assert_eq!(out["header"]["error_code"], "invalid_input");
    assert_eq!(out["header"]["finish_reason"], "error");
    assert_eq!(
        out["header"]["operation"], "error",
        "an unparseable tool_call carries the literal `error`: {out}"
    );
}

/// (b) the op is known, the args are not usable — the reply names the op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rejected_select_answers_with_its_own_operation() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let mut r = StoreRig::with(conn, schema_only());

    // `select` is projection-mandatory: an empty `columns` fails inside
    // dispatch, so the reply travels through `emit_invalid_input`.
    let out = r
        .op(json!({"operation": "select", "table": "nope", "columns": []}))
        .await;

    assert_eq!(out["header"]["error_code"], "invalid_input");
    assert_eq!(
        out["header"]["operation"], "select",
        "the refused op is the one the caller asked for: {out}"
    );
}

/// (c) the query ran and was interrupted — the reply names the op that ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_query_timeout_answers_with_its_own_operation() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // A table large enough that a full-table SELECT outlives a 1 ms timeout
    // (the trigger `paket_7_store_codes_demo.rs` uses).
    conn.execute_batch(
        "CREATE TABLE big (x INTEGER);
         INSERT INTO big(x)
           WITH RECURSIVE r(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM r WHERE x < 400000)
           SELECT x FROM r;",
    )
    .unwrap();
    let mut params = schema_only();
    params["query_timeout_ms"] = json!(1);
    let mut r = StoreRig::with(conn, params);

    let out = r
        .op(json!({"operation": "select", "table": "big", "columns": ["x"]}))
        .await;

    assert_eq!(out["header"]["error_code"], "query_timeout");
    assert_eq!(out["header"]["finish_reason"], "error");
    assert_eq!(
        out["header"]["operation"], "select",
        "the interrupted op is named in the reply: {out}"
    );
}

/// (d) the third emitter on the same surface: a write refused by the write
/// surface (GH #132) names the op it refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_denied_write_answers_with_its_own_operation() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
        .unwrap();
    let mut params = schema_only();
    params["write_surface"] = json!("internal");
    let mut r = StoreRig::with(conn, params);

    // `/outside` is not inside `/aff`, so the write is refused before the
    // database sees it.
    let out = r
        .raw_call(
            &json!({"operation": "insert", "table": "items",
                    "row": {"id": 1, "name": "x"}})
            .to_string(),
            "/outside",
        )
        .await;

    assert_eq!(out["header"]["error_code"], "write_denied");
    assert_eq!(
        out["header"]["operation"], "insert",
        "the refused write names the op: {out}"
    );
}
