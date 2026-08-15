//! Issue #63 — the mirror of #57 on the `store` RESPAWN path: every fallible
//! step of the crash-restart must fail the CELL, never the process.
//!
//! The factory's `RespawnFn` is invoked from `handle_cell_died`, which runs
//! INSIDE the colony task (and inside its await-free restart barrier at that), so
//! the panic class is identical to the wake path: a transient I/O failure during a
//! crash-restart would take the colony task and with it every cell in the process
//! (the panic-free colony hot path invariant, A1′ class). Different trigger strip,
//! same ceiling.
//!
//! | step                                 | class | degraded behaviour               |
//! |--------------------------------------|-------|----------------------------------|
//! | `open_or_create_cell_db_with_status` | hard  | cell answers `sql_error`         |
//! | `params_overlay::restore`            | soft  | falls back to birth params       |
//! | `apply_fts_ddl`                      | soft  | runs without the full-text index |
//! | `apply_schema_ddl`                   | soft  | runs without the declared table  |
//!
//! Construction: every test drives a HEALTHY wake first, peace-stops it (so
//! `cell.db` is closed and the restart starts from a genuine post-mortem state),
//! then calls the real `RespawnFn` synchronously — exactly the way
//! `handle_cell_died` calls `(entry.respawn)()`. A surviving panic therefore
//! unwinds into the test itself. Fabricating a full topology death is deliberately
//! NOT used: a `store` cell only dies by panic or backstop, and both would have to
//! be provoked through a healthy cell that then has to be re-broken between death
//! and restart — a race the direct call removes without weakening the lock (the
//! closure under test is byte-identical either way).
//!
//! The assertion is twofold: reaching the line after `respawn()` at all, plus a
//! POSITIVE receipt that the restarted cell answers.

use meclaw_colony::{CellFactory, RespawnFn, SpawnedCellKind};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, JsonValue, Message, MessageBuilder, Path};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Generic two-column fixture schema — no workshop/-runtime shape is read here.
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

/// A store cell that has lived one healthy wake and is now parked post-mortem:
/// its `cell.db` is closed and its `RespawnFn` is the next thing the colony would
/// call. Every channel end that has to stay alive rides along.
struct Restartable {
    respawn: RespawnFn,
    outputs_rx: mpsc::Receiver<CellEmission>,
    _inbox_rx: mpsc::Receiver<meclaw_colony::ColonyMsg>,
}

/// A restarted store cell plus the channel ends that keep it alive.
struct Restarted {
    sender: mpsc::Sender<Message>,
    outputs_rx: mpsc::Receiver<CellEmission>,
    _join: tokio::task::JoinHandle<()>,
    _peace_rx: tokio::sync::oneshot::Receiver<()>,
    _backstop_rx: tokio::sync::oneshot::Receiver<()>,
    _inbox_rx: mpsc::Receiver<meclaw_colony::ColonyMsg>,
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

/// Send one `insert` into `sender` and wait for the single emission on `rx`.
async fn insert_and_await_emission(
    sender: &mpsc::Sender<Message>,
    rx: &mut mpsc::Receiver<CellEmission>,
) -> CellEmission {
    sender
        .send(insert_msg())
        .await
        .expect("the live cell must own a live mailbox");
    // 30s is a generous failure marker, not a semantic discriminator.
    tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .expect("the live cell must answer within 30s")
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

/// Spawn the dormant cell, drive one HEALTHY wake, prove it answers, then
/// peace-stop it and wait for the `death_ack` (which fires after `cell.db` is
/// closed). What comes back is the parked cell's `RespawnFn` — the exact thing
/// `handle_cell_died` reaches for — plus the first emission, so each test can
/// state what the cell could do BEFORE the restart.
async fn wake_once_then_park(
    cell_dir: &std::path::Path,
    raw_params: JsonValue,
) -> (JsonValue, Restartable) {
    let (otx, mut outputs_rx) = mpsc::channel(16);
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
        respawn,
        ..
    } = spawned
    else {
        unreachable!("the stateful store factory returns Dormant");
    };
    let (stop_tx, death_ack_rx) = wake(receiver);
    let first = insert_and_await_emission(&sender, &mut outputs_rx).await;
    // Peace-stop the woken cell and wait for the ack: it fires after the task
    // closed `cell.db`, so a test may tamper with the file without racing sqlite.
    stop_tx
        .send(())
        .expect("the woken cell accepts a peace-stop");
    tokio::time::timeout(std::time::Duration::from_secs(30), death_ack_rx)
        .await
        .expect("death_ack must fire within 30s")
        .expect("death_ack is sent, not dropped");
    drop(sender);
    (
        first.content,
        Restartable {
            respawn,
            outputs_rx,
            _inbox_rx,
        },
    )
}

/// Call the `RespawnFn` the way `handle_cell_died` does — synchronously, in the
/// caller's task. Reaching the line after this call is assertion one of every
/// test in this file.
fn respawn(parked: Restartable) -> Restarted {
    let (sender, _join, _peace_rx, _backstop_rx) = (parked.respawn)();
    Restarted {
        sender,
        outputs_rx: parked.outputs_rx,
        _join,
        _peace_rx,
        _backstop_rx,
        _inbox_rx: parked._inbox_rx,
    }
}

/// HARD class: `cell.db` cannot be re-opened at the restart.
///
/// Reproducer without any permission games (root-proof): a DIRECTORY takes the
/// place of the file between the death and the restart, so `sqlite3_open` fails
/// with `SQLITE_CANTOPEN`. Pre-fix this was
/// `.expect("respawn: open_or_create_cell_db_with_status failed")` — a panic
/// inside the colony task's restart barrier.
///
/// Degraded contract (mirror of #57): the cell IS restarted, but every message is
/// answered with a named error. No in-memory substitute database is installed —
/// that would make writes look accepted while they vanish on the next restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cell_db_unopenable_at_respawn_restarts_the_cell_degraded() {
    let td = tempfile::TempDir::new().unwrap();
    let (first, parked) = wake_once_then_park(td.path(), store_params()).await;
    assert_eq!(
        first["header"]["rows_affected"], 1,
        "the cell was healthy before the restart: {first:?}",
    );

    // The crash-restart finds a `cell.db` that is no longer openable.
    std::fs::remove_file(td.path().join("cell.db")).unwrap();
    std::fs::create_dir_all(td.path().join("cell.db")).unwrap();

    // Reaching the next line at all is assertion one.
    let mut restarted = respawn(parked);

    let em = insert_and_await_emission(&restarted.sender, &mut restarted.outputs_rx).await;
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

/// SOFT class: the FTS DDL is refused at the restart.
///
/// Reproducer: an existing `items_fts` index whose column list is NOT a prefix
/// extension of the declaration, which `apply_fts_ddl` refuses loudly (a
/// non-additive drift must never be silently rebuilt). The wake already survives
/// this (#57); pre-fix the RESTART did not — `.expect("respawn: apply_fts_ddl
/// failed")`.
///
/// Degraded contract: the cell restarts WITHOUT the full-text index; every
/// non-search op is unaffected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fts_ddl_failure_at_respawn_restarts_the_cell_without_the_index() {
    let td = tempfile::TempDir::new().unwrap();
    {
        let conn = precreate_cell_db(td.path());
        conn.execute_batch(
            "CREATE TABLE items (id INTEGER, name TEXT);
             CREATE VIRTUAL TABLE items_fts USING fts5(other_column);",
        )
        .expect("plant an index with a drifted column list");
    }

    let (first, parked) = wake_once_then_park(td.path(), store_params_with_fts()).await;
    assert_eq!(
        first["header"]["rows_affected"], 1,
        "the base table worked before the restart: {first:?}",
    );

    let mut restarted = respawn(parked);

    let em = insert_and_await_emission(&restarted.sender, &mut restarted.outputs_rx).await;
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

/// SOFT class: the `cell.db` params overlay cannot be replayed at the restart.
///
/// Reproducer: a `params` row whose value is not JSON at all — the shape a
/// half-written or hand-edited `cell.db` produces. Pre-fix:
/// `.expect("respawn: restore params from cell.db overlay")`.
///
/// Degraded contract: loud log, then the restarted cell runs on its BIRTH params
/// (`config.json`) — the overlay is lost, the cell is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broken_params_overlay_at_respawn_falls_back_to_birth_params() {
    let td = tempfile::TempDir::new().unwrap();
    {
        let conn = precreate_cell_db(td.path());
        conn.execute(
            "INSERT INTO params (key, value, updated_at) VALUES ('query_timeout_ms', ?1, 0)",
            ["this is not json"],
        )
        .expect("plant a corrupt overlay row");
    }

    let (_first, parked) = wake_once_then_park(td.path(), store_params()).await;

    let mut restarted = respawn(parked);

    let em = insert_and_await_emission(&restarted.sender, &mut restarted.outputs_rx).await;
    assert_eq!(
        em.content["header"]["operation"], "insert",
        "the restarted cell must run on its birth params: {:?}",
        em.content
    );
    assert_eq!(
        em.content["header"]["rows_affected"], 1,
        "the birth schema is still applied, so the insert lands: {:?}",
        em.content
    );
}

/// SOFT class: the schema DDL is refused at the restart.
///
/// Reproducer: an INDEX occupies the name of a declared table. `IF NOT EXISTS`
/// only tolerates an existing table or view of that name, so the statement fails
/// with "there is already an index named items" — deterministic and independent
/// of file permissions (a chmod-based read-only database would be a no-op for
/// root). Pre-fix: `.expect("respawn: apply_schema_ddl failed")`.
///
/// Degraded contract: the cell restarts, the missing table surfaces per message
/// as a normal store error — the colony survives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn schema_ddl_failure_at_respawn_does_not_panic_the_respawn() {
    let td = tempfile::TempDir::new().unwrap();
    {
        let conn = precreate_cell_db(td.path());
        conn.execute_batch(
            "CREATE TABLE base (x TEXT);
             CREATE INDEX items ON base(x);",
        )
        .expect("plant a name collision on a declared table");
    }

    let (first, parked) = wake_once_then_park(td.path(), store_params()).await;
    assert_eq!(
        first["header"]["error_code"], "unknown_table",
        "the declared table could not be created at wake either: {first:?}",
    );

    let mut restarted = respawn(parked);

    let em = insert_and_await_emission(&restarted.sender, &mut restarted.outputs_rx).await;
    assert_eq!(
        em.content["header"]["error_code"], "unknown_table",
        "the declared table could not be created, so the op reports it per message \
         instead of taking the colony down: {:?}",
        em.content
    );
}
