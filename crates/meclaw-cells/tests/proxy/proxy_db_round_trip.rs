//! Phase-10-C T2: Cursor-CRUD-Round-Trip. Created-Default 0 (W9), Resumed
//! reads the persisted value.

use meclaw_cells::proxy::db::{load_offset, save_offset, setup_proxy_schema};

#[test]
fn load_offset_on_fresh_schema_returns_zero() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    let off = load_offset(&conn).unwrap();
    assert_eq!(
        off, 0,
        "fresh schema must default offset to 0 (Created path)"
    );
}

#[test]
fn save_then_load_round_trips_offset() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    setup_proxy_schema(&conn).unwrap();
    save_offset(&conn, 42).unwrap();
    assert_eq!(load_offset(&conn).unwrap(), 42);
    save_offset(&conn, 4711).unwrap();
    assert_eq!(
        load_offset(&conn).unwrap(),
        4711,
        "second save must overwrite"
    );
}
