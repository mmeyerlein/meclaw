//! Issue #57 — the remaining fallible steps on the `store` wake path must fail
//! the CELL, never the process.
//!
//! The factory's `WakeFn` runs SYNCHRONOUSLY inside the colony task (both
//! `route_with_log` and `handle_sleep` call it), so a panic there does not kill
//! one cell — it kills the colony task and with it every cell in the process
//! (the panic-free colony hot path invariant, A1′ class). Issue #56 removed the
//! `.expect` around the seed load; this file locks the rest:
//!
//! | step                              | class | degraded behaviour              |
//! |-----------------------------------|-------|---------------------------------|
//! | `open_or_create_cell_db_with_status` | hard  | cell answers `sql_error`     |
//! | `params_overlay::restore`         | soft  | falls back to birth params      |
//! | `apply_fts_ddl`                   | soft  | runs without the full-text index|
//! | `apply_schema_ddl`                | soft  | runs without the declared table |
//!
//! Every test drives the real `WakeFn` the way the colony task does — a direct,
//! synchronous call — so a surviving panic unwinds into the test itself.
//! The assertion is therefore twofold: reaching the line after `wake(...)` at
//! all, plus a POSITIVE receipt that the woken cell answers.

use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, JsonValue, Message, MessageBuilder, Path};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Generic two-column fixture schema — no builder/-runtime shape is read here.
fn store_params() -> JsonValue {
    json!({"schema": {"items": {"id": "int", "name": "text"}}})
}

/// Same schema plus a full-text index declaration on `name`.
fn store_params_with_fts() -> JsonValue {
    json!({
        "schema": {"items": {"id": "int", "name": "text"}},
        "fts": {"items": ["name"]}
    })
}

/// A woken store cell plus every channel end that has to stay alive for it.
struct Woken {
    sender: mpsc::Sender<Message>,
    outputs_rx: mpsc::Receiver<CellEmission>,
    _inbox_rx: mpsc::Receiver<meclaw_colony::ColonyMsg>,
    _stop_tx: tokio::sync::oneshot::Sender<()>,
    _death_ack_rx: tokio::sync::oneshot::Receiver<()>,
}

/// Spawn the dormant cell and invoke its `WakeFn` — exactly what the colony task
/// does on the first delivery. Returns `None` never: a panic in `wake` aborts the
/// test process, which is precisely the regression under lock.
fn spawn_and_wake(cell_dir: &std::path::Path, raw_params: JsonValue) -> Woken {
    let (otx, outputs_rx) = mpsc::channel(16);
    let (itx, _inbox_rx) = mpsc::channel(16);
    let spawned = Arc::new(meclaw_cells::store::StoreCellFactory)
        .spawn_cell(
            Path::new("/notes"),
            raw_params,
            otx,
            cell_dir.to_path_buf(),
            meclaw_colony::ContractView::default(),
            itx,
            None,
            0,
            None,
            None,
            1000,
        )
        .expect("params and seed are well-formed — the spawn must succeed");
    let SpawnedCellKind::Dormant {
        sender,
        receiver,
        wake,
        ..
    } = spawned
    else {
        unreachable!("the stateful store factory returns Dormant");
    };
    let (_stop_tx, _death_ack_rx) = wake(receiver);
    Woken {
        sender,
        outputs_rx,
        _inbox_rx,
        _stop_tx,
        _death_ack_rx,
    }
}

/// A minimal `insert` tool_call against the fixture schema.
fn insert_msg() -> Message {
    MessageBuilder::new(Path::new("/notes"))
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant",
            "type": "tool_call",
            "text": r#"{"operation":"insert","table":"items","row":{"id":1,"name":"a"}}"#,
            "id": "call_1"
        }]})))
        .reply_to(Path::new("/sink"))
        .build()
}

/// Send one `insert` and wait for the cell's single emission.
async fn insert_and_await_emission(woken: &mut Woken) -> CellEmission {
    woken
        .sender
        .send(insert_msg())
        .await
        .expect("the woken cell must own a live mailbox");
    tokio::time::timeout(std::time::Duration::from_secs(30), woken.outputs_rx.recv())
        .await
        .expect("the woken cell must answer within 30s")
        .expect("outputs channel stays open")
}

/// Create `cell.db` up front so a test can plant corruption into it before the
/// first wake. Returns the opened connection (dropped by the caller).
fn precreate_cell_db(cell_dir: &std::path::Path) -> rusqlite::Connection {
    let (conn, _status) = meclaw_colony::persist::cell_db::open_or_create_cell_db_with_status(
        &cell_dir.join("cell.db"),
    )
    .expect("fixture cell.db must open");
    conn
}

/// HARD class: `cell.db` cannot be opened at all.
///
/// Reproducer without any permission games (root-proof): a DIRECTORY sits where
/// the file belongs, so `sqlite3_open` fails with `SQLITE_CANTOPEN`. Pre-fix this
/// was `.expect("wake: open_or_create_cell_db_with_status failed")` — a panic in
/// the colony task.
///
/// Degraded contract: the cell IS started, but every message is answered with a
/// named error. No in-memory substitute database is installed — that would make
/// writes look accepted while they vanish on restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_db_unopenable_at_wake_starts_the_cell_degraded() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("cell.db")).unwrap();

    // Reaching the next line at all is assertion one.
    let mut woken = spawn_and_wake(td.path(), store_params());

    let em = insert_and_await_emission(&mut woken).await;
    assert_eq!(
        em.content["header"]["finish_reason"], "error",
        "a degraded store cell must answer with an error, not with a fake success: {:?}",
        em.content
    );
    assert_eq!(
        em.content["header"]["error_code"], "sql_error",
        "the error_code stays inside the closed store set (cell-types.md § store): {:?}",
        em.content
    );
    let text = em.content["messages"][0]["text"]
        .as_str()
        .expect("the error turn carries text");
    assert!(
        text.contains("cell.db"),
        "the error text must name the unusable cell.db; got: {text}"
    );
    assert_eq!(
        em.content["messages"][0]["id"], "call_1",
        "the tool_call id is echoed so a tool loop can correlate the failure"
    );
}

/// SOFT class: the `cell.db` params overlay cannot be replayed.
///
/// Reproducer: a `params` row whose value is not JSON at all — the shape a
/// half-written or hand-edited `cell.db` produces. Pre-fix this killed the
/// process twice over: the `.expect` inside `read_params_overlay` and, for a
/// merely type-invalid value, the `.expect` around `restore` in the WakeFn.
///
/// Degraded contract: loud log, then the cell runs on its BIRTH params
/// (`config.json`) — the overlay is lost, the cell is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broken_params_overlay_at_wake_falls_back_to_birth_params() {
    let td = tempfile::TempDir::new().unwrap();
    {
        let conn = precreate_cell_db(td.path());
        conn.execute(
            "INSERT INTO params (key, value, updated_at) VALUES ('query_timeout_ms', ?1, 0)",
            ["this is not json"],
        )
        .expect("plant a corrupt overlay row");
    }

    let mut woken = spawn_and_wake(td.path(), store_params());

    let em = insert_and_await_emission(&mut woken).await;
    assert_eq!(
        em.content["header"]["operation"], "insert",
        "the cell must run on its birth params: {:?}",
        em.content
    );
    assert_eq!(
        em.content["header"]["rows_affected"], 1,
        "the birth schema is still applied, so the insert lands: {:?}",
        em.content
    );
}

/// SOFT class: the FTS DDL is refused.
///
/// Reproducer: an existing `items_fts` index whose column list is NOT a prefix
/// extension of the declaration, which `apply_fts_ddl` refuses loudly (a
/// non-additive drift must never be silently rebuilt). Pre-fix:
/// `.expect("wake: apply_fts_ddl")` inside the colony task.
///
/// Degraded contract: the cell starts WITHOUT the full-text index; every
/// non-search op is unaffected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fts_ddl_failure_at_wake_starts_the_cell_without_the_index() {
    let td = tempfile::TempDir::new().unwrap();
    {
        let conn = precreate_cell_db(td.path());
        conn.execute_batch(
            "CREATE TABLE items (id INTEGER, name TEXT);
             CREATE VIRTUAL TABLE items_fts USING fts5(other_column);",
        )
        .expect("plant an index with a drifted column list");
    }

    let mut woken = spawn_and_wake(td.path(), store_params_with_fts());

    let em = insert_and_await_emission(&mut woken).await;
    assert_eq!(
        em.content["header"]["operation"], "insert",
        "the base table still works without the index: {:?}",
        em.content
    );
    assert_eq!(
        em.content["header"]["rows_affected"], 1,
        "the insert lands even though the index was refused: {:?}",
        em.content
    );
}

/// SOFT class: the schema DDL is refused.
///
/// Reproducer: an INDEX occupies the name of a declared table. `IF NOT EXISTS`
/// only tolerates an existing table or view of that name, so the statement fails
/// with "there is already an index named items" — deterministic and independent
/// of file permissions (a chmod-based read-only database would be a no-op for
/// root). Pre-fix: `.expect("wake: apply_schema_ddl")` inside the colony task.
///
/// Degraded contract: the cell starts, the missing table surfaces per message as
/// a normal store error — the colony survives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn schema_ddl_failure_at_wake_does_not_panic_the_wake() {
    let td = tempfile::TempDir::new().unwrap();
    {
        let conn = precreate_cell_db(td.path());
        conn.execute_batch(
            "CREATE TABLE base (x TEXT);
             CREATE INDEX items ON base(x);",
        )
        .expect("plant a name collision on a declared table");
    }

    // Reaching the next line at all is the assertion; the emission below proves
    // the cell is live rather than merely un-panicked.
    let mut woken = spawn_and_wake(td.path(), store_params());

    let em = insert_and_await_emission(&mut woken).await;
    assert_eq!(
        em.content["header"]["error_code"], "unknown_table",
        "the declared table could not be created, so the op reports it per message \
         instead of taking the colony down: {:?}",
        em.content
    );
}
