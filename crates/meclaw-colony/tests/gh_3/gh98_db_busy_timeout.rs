//! GH #98: the spawn-time re-open of `colony.db` (and every other database
//! open) must ride out a held write lock via SQLite's busy timeout instead of
//! failing the boot with "database is locked".
//!
//! Root-cause finding (Track N, 2026-08-13): rusqlite installs an IMPLICIT
//! 5000 ms busy timeout on every connection it opens
//! (`rusqlite-0.39.0/src/inner_connection.rs:118`), so the #98 boot failure
//! happened DESPITE a five-second busy wait — the boot-time race partner (the
//! template-scan `ColonyDb`'s writer thread, dropped without a join at the
//! spawn re-open) was starved past that budget under full parallel workspace
//! load. The fix therefore makes the timeout EXPLICIT and owned by meclaw
//! (one constant, `persist::schema::DB_BUSY_TIMEOUT`) and raises it to
//! 30 000 ms — the suite's generous-failure-marker convention: a lock held
//! for moments or seconds is contention and gets waited out; a lock held
//! longer than 30 s is a real wedge and still fails loudly.
//!
//! Deterministic lock inducement: a second connection takes `BEGIN IMMEDIATE`
//! (the WAL writer lock) BEFORE the open under test runs, and commits 8 s
//! later from a helper thread. 8 s is a semantic discriminator, tight by
//! necessity and justified as follows: it sits ABOVE the implicit rusqlite
//! default (5 s) — so the pre-fix code deterministically fails, red — and
//! FAR BELOW the 30 s budget (22 s of margin against cargo-parallel load;
//! a slow-scheduled release only lengthens the wait, it cannot turn the
//! post-fix test red before the 30 s budget is truly exhausted).
//!
//! The write that the open under test performs on an EXISTING database is the
//! idempotent `INSERT OR IGNORE INTO meta …` seed in the setup DDL — any
//! INSERT starts a write transaction regardless of its OR-IGNORE outcome,
//! which is the statement class that hit SQLITE_BUSY in the wild (issue #98:
//! `run_with_hooks Ok: re-open colony.db for spawn: database is locked`).

use meclaw_colony::ColonyDb;
use meclaw_colony::persist::{open_or_create_cell_db, setup_cell_db, setup_colony_db};

/// Held-lock duration: above rusqlite's implicit 5 s default (pre-fix red),
/// far below the 30 s meclaw budget (post-fix green, 22 s margin).
const HOLD: std::time::Duration = std::time::Duration::from_secs(8);

/// Takes the WAL writer lock on `db_path` synchronously (guaranteed held when
/// this returns) and releases it after `hold` from a helper thread.
fn hold_write_lock(
    db_path: &std::path::Path,
    hold: std::time::Duration,
) -> std::thread::JoinHandle<()> {
    let conn = rusqlite::Connection::open(db_path).expect("locker: open");
    conn.execute_batch("BEGIN IMMEDIATE")
        .expect("locker: BEGIN IMMEDIATE must take the write lock");
    std::thread::spawn(move || {
        std::thread::sleep(hold);
        conn.execute_batch("COMMIT")
            .expect("locker: COMMIT must release the write lock");
    })
}

/// The lib.rs boot sequence opens colony.db twice (template scan, then the
/// spawn re-open). The re-open runs against an EXISTING database whose write
/// lock another connection may hold for several seconds under load — that
/// hold must be waited out, not turned into a boot failure.
#[test]
fn colony_db_reopen_for_spawn_survives_a_write_lock_held_beyond_the_rusqlite_default() {
    let td = tempfile::TempDir::new().unwrap();
    let db_path = td.path().join("colony.db");
    // First boot creates the schema (the lib.rs "open colony.db" of the
    // template scan).
    drop(ColonyDb::open(&db_path).expect("first open"));

    let locker = hold_write_lock(&db_path, HOLD);

    // The spawn-time re-open. Pre-#98-fix this dies after rusqlite's implicit
    // 5 s with "database is locked"; with the explicit 30 s budget it waits
    // the 8 s hold out.
    let reopened = ColonyDb::open(&db_path);
    locker.join().expect("locker thread");
    let db = reopened.expect(
        "the spawn re-open must wait out a held write lock within the 30 s \
         busy budget instead of failing the boot (GH #98)",
    );
    drop(db);
}

/// Same busy behaviour for cell.db: a factory open (spawn/respawn/wake) that
/// races another connection's write transaction must wait, not panic the
/// respawn closure into a restart loop.
#[test]
fn cell_db_open_survives_a_write_lock_held_beyond_the_rusqlite_default() {
    let td = tempfile::TempDir::new().unwrap();
    let db_path = td.path().join("cell.db");
    drop(open_or_create_cell_db(&db_path).expect("first open"));

    let locker = hold_write_lock(&db_path, HOLD);

    let reopened = open_or_create_cell_db(&db_path);
    locker.join().expect("locker thread");
    drop(reopened.expect(
        "a cell.db open must wait out a held write lock within the 30 s \
         busy budget instead of failing the spawn (GH #98)",
    ));
}

/// Positive receipt that the central setup functions install the explicit
/// meclaw busy budget (not rusqlite's implicit default): `PRAGMA busy_timeout`
/// reports 30 000 ms on a connection that went through `setup_cell_db` /
/// `setup_colony_db`.
#[test]
fn setup_functions_install_the_explicit_busy_timeout() {
    let td = tempfile::TempDir::new().unwrap();

    let cell = open_or_create_cell_db(&td.path().join("cell.db")).expect("cell open");
    let ms: i64 = cell
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .expect("read cell busy_timeout");
    assert_eq!(
        ms, 30_000,
        "setup_cell_db must install the explicit 30 s busy timeout (GH #98)"
    );

    let colony = rusqlite::Connection::open(td.path().join("colony.db")).expect("colony open");
    setup_colony_db(&colony).expect("setup_colony_db");
    let ms: i64 = colony
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .expect("read colony busy_timeout");
    assert_eq!(
        ms, 30_000,
        "setup_colony_db must install the explicit 30 s busy timeout (GH #98)"
    );

    // Silence the unused-import path when only one branch compiles: both setup
    // fns are exercised above; setup_cell_db is reached via
    // open_or_create_cell_db and re-checked here for the direct-call surface.
    let raw = rusqlite::Connection::open(td.path().join("cell2.db")).expect("raw open");
    setup_cell_db(&raw).expect("setup_cell_db direct");
    let ms: i64 = raw
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .expect("read direct cell busy_timeout");
    assert_eq!(ms, 30_000);
}
