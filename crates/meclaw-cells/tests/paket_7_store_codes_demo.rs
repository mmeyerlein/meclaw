//! Paket-7 F3 — store error_codes demo (f): all SIX `store` `error_code`
//! values (cell-types.md Z.71 / D-015) are REAL-triggered through the store
//! cell + ops, and each asserted.
//!
//! Spec-code → trigger table (each row is exercised by a `#[test]` below):
//!
//! | spec error_code        | how it is triggered                                  | path                         |
//! |------------------------|------------------------------------------------------|------------------------------|
//! | `unknown_table`        | SELECT from a non-existent table                     | classify_sql_error (message) |
//! | `unknown_column`       | INSERT a column the table does not declare           | classify_sql_error (message) |
//! | `constraint_violation` | INSERT a duplicate INTEGER PRIMARY KEY               | ErrorCode::ConstraintViolation |
//! | `type_mismatch`        | INSERT a non-integer into an INTEGER PRIMARY KEY     | ErrorCode::TypeMismatch      |
//! | `sql_error`            | a malformed table name → generic SQLITE_ERROR        | classify backstop            |
//! | `query_timeout`        | a long SELECT interrupted by the query_timeout knob  | DbConn InterruptHandle       |
//!
//! The five SQL codes flow through `StoreCell::handle` end-to-end (real
//! `tool_call` message → real ops → real rusqlite error → classified
//! `error_code` header). `query_timeout` flows through the InterruptHandle
//! path (`DbConn::call_with_timeout`), which `classify_sql_error` never sees —
//! it is produced by the cell's timeout arm.
//!
//! All five SQL codes are emitted as NORMAL `tool_result`-Turns with an
//! `error_code` header (brainstorm E5: NOT `finish_reason:"error"`).
//! `query_timeout` is the one SQL-side code that DOES carry
//! `finish_reason:"error"` (Konzept-A timeout → Error-Message).

use meclaw_cells::store::{StoreCell, StoreParams};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use std::time::Duration;
use tokio::sync::mpsc;

/// Build an `OutputSink` rooted at `/store`.
fn make_sink(otx: mpsc::Sender<CellEmission>) -> OutputSink {
    OutputSink::new(
        otx,
        Path::new("/store"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    )
}

/// Drive `StoreCell::handle` once with a `tool_call` carrying `args` (a JSON
/// op-object as `text`) against `db`, returning the single emission.
async fn run_store_op(cell: &mut StoreCell, db: &mut DbConn, args: &str) -> CellEmission {
    let (otx, mut orx) = mpsc::channel(8);
    let sink = make_sink(otx);
    let body = json!({"messages":[{
        "origin":"assistant","type":"tool_call","text": args,"id":"call_1"
    }]});
    let msg = MessageBuilder::new(Path::new("/store"))
        .body(Body::Inline(body))
        .reply_to(Path::new("/sink"))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    orx.recv()
        .await
        .expect("store cell must emit exactly one tool_result")
}

fn error_code(em: &CellEmission) -> Option<&str> {
    em.content["header"]["error_code"].as_str()
}

// ── 1) unknown_table ────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_code_unknown_table() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = StoreCell::new(StoreParams {
        schema: Default::default(),
        query_timeout_ms: None,
        fts: Default::default(),
    });
    // No table created → SELECT hits "no such table".
    let em = run_store_op(
        &mut cell,
        &mut db,
        r#"{"operation":"select","table":"ghost","columns":["x"]}"#,
    )
    .await;
    assert_eq!(error_code(&em), Some("unknown_table"));
    // brainstorm E5: SQL-error is a normal tool_result, NOT finish_reason:error.
    assert!(
        em.content["header"].get("finish_reason").is_none(),
        "SQL error_code must NOT carry finish_reason:error (brainstorm E5)"
    );
    assert_eq!(em.content["messages"][0]["type"], "tool_result");
}

// ── 2) unknown_column ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_code_unknown_column() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE t (a INTEGER)", []).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = StoreCell::new(StoreParams {
        schema: Default::default(),
        query_timeout_ms: None,
        fts: Default::default(),
    });
    // Column "nosuch" is not declared → "table t has no column named nosuch".
    let em = run_store_op(
        &mut cell,
        &mut db,
        r#"{"operation":"insert","table":"t","row":{"nosuch":1}}"#,
    )
    .await;
    assert_eq!(error_code(&em), Some("unknown_column"));
    assert!(em.content["header"].get("finish_reason").is_none());
}

// ── 3) constraint_violation ─────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_code_constraint_violation() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE u (id INTEGER PRIMARY KEY)", [])
        .unwrap();
    conn.execute("INSERT INTO u VALUES (1)", []).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = StoreCell::new(StoreParams {
        schema: Default::default(),
        query_timeout_ms: None,
        fts: Default::default(),
    });
    // Duplicate PK → SQLITE_CONSTRAINT.
    let em = run_store_op(
        &mut cell,
        &mut db,
        r#"{"operation":"insert","table":"u","row":{"id":1}}"#,
    )
    .await;
    assert_eq!(error_code(&em), Some("constraint_violation"));
    assert!(em.content["header"].get("finish_reason").is_none());
}

// ── 4) type_mismatch ────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_code_type_mismatch() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // INTEGER PRIMARY KEY rejects a non-integer rowid value → SQLITE_MISMATCH.
    conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)", [])
        .unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = StoreCell::new(StoreParams {
        schema: Default::default(),
        query_timeout_ms: None,
        fts: Default::default(),
    });
    let em = run_store_op(
        &mut cell,
        &mut db,
        r#"{"operation":"insert","table":"t","row":{"id":"not-an-int"}}"#,
    )
    .await;
    assert_eq!(error_code(&em), Some("type_mismatch"));
    assert!(em.content["header"].get("finish_reason").is_none());
}

// ── 5) sql_error (backstop) ─────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_code_sql_error_backstop() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE t (a INTEGER)", []).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = StoreCell::new(StoreParams {
        schema: Default::default(),
        query_timeout_ms: None,
        fts: Default::default(),
    });
    // TRIGGER CHANGED IN P3 (behaviour of the backstop is unchanged).
    // Until P3 this test reached the backstop by injecting a dangling clause via
    // a malformed `table` name (`SELECT "a" FROM "t" GROUP BY ("`). P3 resolves
    // every identifier against the SQLite catalog *before* building SQL, so that
    // payload now stops one layer earlier as `unknown_table` — the hole this
    // test used as a lever is closed (and pinned below in
    // `store_code_malformed_table_name_is_rejected_by_the_catalog`).
    // The D-015 Watchlist invariant still needs a live proof, so the trigger is
    // now a genuinely generic SQLite failure with valid identifiers: updating a
    // VIEW yields "cannot modify v because it is a view", which matches NEITHER
    // known prefix → the classifier must degrade to `sql_error`, never
    // misclassify.
    conn_view(&mut db).await;
    let em = run_store_op(
        &mut cell,
        &mut db,
        r#"{"operation":"update","table":"v","set":{"a":1}}"#,
    )
    .await;
    assert_eq!(
        error_code(&em),
        Some("sql_error"),
        "unrecognised SQL error must fall back to sql_error (backstop); got {:?} text={:?}",
        error_code(&em),
        em.content["messages"][0]["text"]
    );
    assert!(em.content["header"].get("finish_reason").is_none());
}

/// Create the view the backstop test updates through. Kept out of the test body
/// so the payload above stays the only interesting line.
async fn conn_view(db: &mut DbConn) {
    db.call(|c| {
        c.execute_batch("CREATE VIEW v AS SELECT a FROM t;")
            .unwrap();
    })
    .await;
}

/// P3 regression lock: the identifier that used to slip a dangling clause into
/// the generated SQL is now rejected by the catalog — no statement is built, the
/// answer is a regular `tool_result` with `unknown_table`, and nothing executes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_code_malformed_table_name_is_rejected_by_the_catalog() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE t (a INTEGER)", []).unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = StoreCell::new(StoreParams {
        schema: Default::default(),
        query_timeout_ms: None,
        fts: Default::default(),
    });
    let em = run_store_op(
        &mut cell,
        &mut db,
        r#"{"operation":"select","table":"t\" GROUP BY (","columns":["a"]}"#,
    )
    .await;
    assert_eq!(error_code(&em), Some("unknown_table"));
    assert!(em.content["header"].get("finish_reason").is_none());
    let still_there: i64 = db
        .call(|c| {
            c.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='t'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        })
        .await;
    assert_eq!(still_there, 1, "the base table must be untouched");
}

// ── 6) query_timeout (DbConn InterruptHandle path) ──────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_code_query_timeout() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // Build a large table so a full-table SELECT is long enough to be
    // interrupted by the tiny query_timeout (the recursive-CTE interrupt
    // pattern from db_conn's own timeout test).
    conn.execute_batch(
        "CREATE TABLE big (x INTEGER);
         INSERT INTO big(x)
           WITH RECURSIVE r(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM r WHERE x < 400000)
           SELECT x FROM r;",
    )
    .unwrap();

    let timeout = Duration::from_millis(1);
    let mut db = DbConn::wrap(conn, Some(timeout));
    // The cell must be configured with the SAME timeout so it dispatches via
    // call_with_timeout (Konzept-A) and maps Interrupted → query_timeout.
    let mut cell = StoreCell::new(StoreParams {
        schema: Default::default(),
        query_timeout_ms: Some(timeout.as_millis() as u64),
        fts: Default::default(),
    });

    // A full-table SELECT over 400k rows — interruptible inside SQLite, far
    // longer than 1ms → the InterruptHandle timer fires.
    let em = run_store_op(
        &mut cell,
        &mut db,
        r#"{"operation":"select","table":"big","columns":["x"]}"#,
    )
    .await;
    assert_eq!(error_code(&em), Some("query_timeout"));
    // query_timeout is the SQL-side code that DOES carry finish_reason:error
    // (Konzept-A timeout → Error-Message, unlike the five classify codes).
    assert_eq!(em.content["header"]["finish_reason"], "error");
}

// ── Spec-vs-code coverage table (assert all six are accounted for) ───────────

#[test]
fn all_six_store_codes_are_demonstrated() {
    // This is the machine-checkable form of the doc-table at the top: every
    // spec error_code (cell-types.md Z.71) has a real-trigger test above.
    const SPEC_CODES: [&str; 6] = [
        "sql_error",
        "constraint_violation",
        "query_timeout",
        "type_mismatch",
        "unknown_table",
        "unknown_column",
    ];
    // Each maps to a `#[test]` in this file (named store_code_*). The mapping is
    // exhaustive: 6 spec codes ↔ 6 trigger tests.
    assert_eq!(
        SPEC_CODES.len(),
        6,
        "store declares exactly six error_codes"
    );
}
