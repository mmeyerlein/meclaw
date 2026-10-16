//! Paket-7 F3 — store error_codes demo (f): the EIGHT SQL-side `store`
//! `error_code` values (docs/cell-types.md § store) are accounted for. Six of
//! them are REAL-triggered through the store cell + ops in this file, and each
//! asserted; `unique_violation` (GH #915) needs a declared `unique` index and
//! is triggered in `gh915_a_store_filters_on_a_json_path.rs`; `store_degraded`
//! (GH #1082) needs a database file SQLite cannot open and is triggered in the
//! unit tests of `src/store/ops.rs` (`an_unopenable_database_file_is_store_degraded`)
//! and `src/store/degraded.rs`, and at wake/restart in `store_wake_failure_modes`
//! and `store_respawn_failure_modes`.
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
//! | `unique_violation`     | INSERT a duplicate into a declared `unique` index    | gh915 test file              |
//! | `store_degraded`       | `cell.db` cannot be opened (SQLITE_CANTOPEN)         | ops.rs / degraded.rs units   |
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
        canonical: Default::default(),
        write_surface: Default::default(),
        indexes: Default::default(),
        evolution: Default::default(),
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
        canonical: Default::default(),
        write_surface: Default::default(),
        indexes: Default::default(),
        evolution: Default::default(),
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
        canonical: Default::default(),
        write_surface: Default::default(),
        indexes: Default::default(),
        evolution: Default::default(),
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
        canonical: Default::default(),
        write_surface: Default::default(),
        indexes: Default::default(),
        evolution: Default::default(),
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
        canonical: Default::default(),
        write_surface: Default::default(),
        indexes: Default::default(),
        evolution: Default::default(),
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
        canonical: Default::default(),
        write_surface: Default::default(),
        indexes: Default::default(),
        evolution: Default::default(),
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
        canonical: Default::default(),
        write_surface: Default::default(),
        indexes: Default::default(),
        evolution: Default::default(),
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

// ── Spec-vs-code coverage table (assert all eight are accounted for) ─────────

/// The repository root this test file lives in (two levels above the crate).
fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The English edition of the cell catalogue under `root`. Privately
/// `docs/cell-types.md` is the German edition and the English one is
/// `docs/cell-types.en.md`; the export (DOCS_MAP) ships the English bytes under
/// the plain name and no `.en.md`. So the `.en.md` is read only where it is a
/// file, else the plain name (gh682 form) -- an unguarded read panicked with
/// NotFound in the public CI, which runs this `_demo` binary (review bb35efd67
/// I2, the 0.26.0 / gh325 class).
fn english_cell_types(root: &std::path::Path) -> std::path::PathBuf {
    let english = root.join("docs/cell-types.en.md");
    if english.is_file() {
        english
    } else {
        root.join("docs/cell-types.md")
    }
}

/// The codes of family 1 ("SQL level gives a regular `tool_result` turn")
/// in the English cell catalogue § store, read from the parenthesis that
/// lists them.
fn documented_family_one_codes(root: &std::path::Path) -> std::collections::BTreeSet<String> {
    let doc_path = english_cell_types(root);
    let doc = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("{} is readable: {e}", doc_path.display()));
    let line = doc
        .lines()
        .find(|l| l.starts_with("1. SQL level gives a regular `tool_result` turn"))
        .unwrap_or_else(|| panic!("{} § store carries the family-1 line", doc_path.display()));
    let open = line.find('(').expect("family-1 line opens its code list");
    let close = line[open..]
        .find(", GH #915")
        .map(|i| open + i)
        .expect("family-1 code list ends before the GH #915 note");
    line[open + 1..close]
        .split('/')
        .map(|c| c.trim().trim_matches('`').trim_matches('"').to_string())
        .collect()
}

#[test]
fn all_eight_store_codes_are_accounted_for() {
    // This is the machine-checkable form of the doc-table at the top: every
    // spec error_code (docs/cell-types.md § store) has a real-trigger test,
    // above, in the GH #915 test file (`unique_violation`), or in the store
    // unit tests and wake/respawn pins (`store_degraded`, GH #1082).
    const SPEC_CODES: [&str; 8] = [
        "sql_error",
        "constraint_violation",
        "query_timeout",
        "type_mismatch",
        "unknown_table",
        "unknown_column",
        "unique_violation",
        "store_degraded",
    ];
    // The table is exhaustive against the doc, not only against itself: the
    // family-1 list of cell-types.en.md plus `query_timeout` (the cell's
    // timeout arm, family 2 in the doc) is exactly this set. GH #1082 added
    // `store_degraded` to the doc while this list still said seven.
    let mut expected = documented_family_one_codes(&repo_root());
    expected.insert("query_timeout".to_string());
    let listed: std::collections::BTreeSet<String> =
        SPEC_CODES.iter().map(|c| c.to_string()).collect();
    assert_eq!(
        listed, expected,
        "the SQL-side store codes of this file are the documented ones"
    );
}

#[test]
fn the_public_clone_reads_the_family_one_list_under_the_plain_name() {
    // The export (DOCS_MAP) ships the English catalogue as docs/cell-types.md
    // and no docs/cell-types.en.md; the public CI runs this `_demo` binary
    // (test-tier.sh t2). Review bb35efd67 I2: an unguarded .en.md read
    // panics there with NotFound (the 0.26.0 / gh325 class). This builds the
    // public layout in a temp dir from the English bytes this tree carries and
    // asserts the same family-1 list comes out of it.
    let public = tempfile::tempdir().expect("temp dir for the public layout");
    std::fs::create_dir_all(public.path().join("docs")).expect("docs/ in the public layout");
    std::fs::copy(
        english_cell_types(&repo_root()),
        public.path().join("docs/cell-types.md"),
    )
    .expect("English catalogue copied under the plain name");
    assert_eq!(
        documented_family_one_codes(public.path()),
        documented_family_one_codes(&repo_root()),
        "the public clone reads the same family-1 list as the private tree"
    );
}

#[test]
fn a_missing_family_one_line_names_the_file_that_was_read() {
    // Delta-check 2843dd16c: the expect message said "cell-types.en.md" while
    // the public clone reads docs/cell-types.md -- a red there pointed at a
    // file the clone does not have. The message names `doc_path` instead.
    let public = tempfile::tempdir().expect("temp dir for the public layout");
    std::fs::create_dir_all(public.path().join("docs")).expect("docs/ in the public layout");
    std::fs::write(
        public.path().join("docs/cell-types.md"),
        "# Cell types\n\nno family-1 line here\n",
    )
    .expect("catalogue without the family-1 line");
    let root = public.path().to_path_buf();
    let panic = std::panic::catch_unwind(move || documented_family_one_codes(&root))
        .expect_err("a catalogue without the family-1 line is a red");
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("the panic carries a message");
    let read = public.path().join("docs/cell-types.md");
    assert!(
        message.contains(&read.display().to_string()),
        "the message names the file that was read ({}): {message}",
        read.display()
    );
    assert!(
        !message.contains("cell-types.en.md"),
        "the message names no file the public clone lacks: {message}"
    );
}
