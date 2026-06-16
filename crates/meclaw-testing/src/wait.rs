//! Test-Harness-Poll-Helpers für Phase-5-Quieszenz.
//!
//! Drei Helpers: message_log-COUNT-Polling (Cascade-Barrier),
//! cell.db-Value-Polling (Restore-Barrier), Arc<AtomicU32>-Polling (Restart-Counter).
//!
//! Alle: 10ms-Interval, Fail-Timeout mit Count-Dump im panic.

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Pollt cell.db system.value für `slot_path` bis == `expected` oder timeout.
/// Read-only-Connection; öffnet & schließt sie pro Poll. Letzten gesehenen Wert
/// im Timeout-panic dumpen.
pub async fn wait_for_cell_db_value(
    cell_dir: &std::path::Path,
    slot_path: &str,
    expected: &str,
    timeout: std::time::Duration,
) {
    let db_path = cell_dir.join("cell.db");
    let start = std::time::Instant::now();
    let mut last_seen: String = String::from("<none>");
    loop {
        // Phase-13-K-2: cell.db may not exist yet — the stateful factory now
        // returns `Dormant` (Mailbox-Paar only), and the DB-Open lives in the
        // WakeFn closure. First message → Wake-Pre-Send → DB-Open. Tolerate
        // pre-Wake polls by treating Open-failure as "not-yet-existing".
        match rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(conn) => {
                let v: Result<String, _> = conn.query_row(
                    "SELECT value FROM system WHERE slot_path = ?",
                    [slot_path],
                    |r| r.get(0),
                );
                drop(conn);
                match v {
                    Ok(s) => {
                        if s == expected {
                            return;
                        }
                        last_seen = s;
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => {}
                    // Phase-13-K-2: between `Connection::open` and
                    // `setup_cell_db`, the file exists but tables don't yet.
                    // Poll until setup finishes.
                    Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
                        if msg.contains("no such table") => {}
                    Err(e) => panic!("wait_for_cell_db_value: SQL error: {e:?}"),
                }
            }
            Err(_) => {
                // Pre-Wake: cell.db does not exist yet. Keep polling.
            }
        }
        if start.elapsed() > timeout {
            panic!(
                "wait_for_cell_db_value: timeout after {:?}, last_seen={last_seen}, expected={expected} for slot_path={slot_path}",
                start.elapsed()
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Pollt einen `Arc<AtomicU32>`-Counter bis >= `expected` oder timeout.
/// KEINE DB-Operation; direkter Atomic-Read.
pub async fn wait_for_spawn_count(
    counter: &std::sync::Arc<std::sync::atomic::AtomicU32>,
    expected: u32,
    timeout: std::time::Duration,
) {
    let start = std::time::Instant::now();
    loop {
        let actual = counter.load(std::sync::atomic::Ordering::Relaxed);
        if actual >= expected {
            return;
        }
        if start.elapsed() > timeout {
            panic!(
                "wait_for_spawn_count: timeout after {:?}, got {actual}, expected >= {expected}",
                start.elapsed()
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Pollt message_log COUNT für `trace_id` bis >= `expected` oder timeout.
/// Read-only-Connection; öffnet & schließt sie pro Poll.
pub async fn wait_for_message_log_count(
    db_path: &std::path::Path,
    trace_id: &str,
    expected: i64,
    timeout: std::time::Duration,
) {
    let start = std::time::Instant::now();
    loop {
        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("open colony.db read-only");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_log WHERE trace_id = ?",
                [trace_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        drop(conn);
        if count >= expected {
            return;
        }
        if start.elapsed() > timeout {
            panic!(
                "wait_for_message_log_count: timeout after {:?}, got {count}, expected >= {expected} for trace_id={trace_id}",
                start.elapsed()
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Phase-6 T23: poll `mutation_log.status` until it matches `expected_status`.
/// Used by T26's crash-recovery demo to sync on durable state transitions.
pub async fn wait_for_mutation_status(
    colony_db_path: &std::path::Path,
    mutation_id: &str,
    expected_status: &str,
    timeout: std::time::Duration,
) {
    let start = std::time::Instant::now();
    loop {
        if let Ok(conn) = rusqlite::Connection::open_with_flags(
            colony_db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            let status: Result<String, _> = conn.query_row(
                "SELECT status FROM mutation_log WHERE id=?",
                [mutation_id],
                |r| r.get(0),
            );
            if let Ok(s) = status
                && s == expected_status
            {
                return;
            }
        }
        if start.elapsed() > timeout {
            panic!(
                "wait_for_mutation_status: timeout after {timeout:?} for id={mutation_id} expected={expected_status}"
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Phase-6 T23: poll `mutation_log` for any row with `status='in_flight'`.
/// Used by T26's crash-recovery demo — server-side mutation_id is unknown at
/// send-time, so the count-based barrier replaces a specific-id lookup.
pub async fn wait_for_any_in_flight(
    colony_db_path: &std::path::Path,
    timeout: std::time::Duration,
) {
    let start = std::time::Instant::now();
    loop {
        if let Ok(conn) = rusqlite::Connection::open_with_flags(
            colony_db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            let cnt: Result<i64, _> = conn.query_row(
                "SELECT COUNT(*) FROM mutation_log WHERE status='in_flight'",
                [],
                |r| r.get(0),
            );
            if let Ok(c) = cnt
                && c >= 1
            {
                return;
            }
        }
        if start.elapsed() > timeout {
            panic!("wait_for_any_in_flight: timeout after {timeout:?}, no in_flight row appeared");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn wait_for_message_log_count_returns_when_expected_reached() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        meclaw_colony::persist::setup_colony_db(&conn).unwrap();
        for i in 0..3 {
            conn.execute(
                "INSERT INTO message_log (id, trace_id, parent_message_id, correlation_id, ttl,
                 from_path, to_path, reply_to, headers, body_kind, body_payload, created_at)
                 VALUES (?, 'T1', NULL, NULL, 64, '@external', '/a', NULL, '{}', 'inline', 'null', 0)",
                rusqlite::params![format!("id-{i}")],
            ).unwrap();
        }
        drop(conn);
        wait_for_message_log_count(&db_path, "T1", 3, std::time::Duration::from_secs(2)).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[should_panic(expected = "wait_for_message_log_count")]
    async fn wait_for_message_log_count_panics_on_timeout() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("colony.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        meclaw_colony::persist::setup_colony_db(&conn).unwrap();
        drop(conn);
        wait_for_message_log_count(&db_path, "T1", 1, std::time::Duration::from_millis(100)).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn wait_for_cell_db_value_returns_when_expected_value_matches() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        meclaw_colony::persist::setup_cell_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO system (slot_path, value, updated_at) VALUES ('counter', '42', 0)",
            [],
        )
        .unwrap();
        drop(conn);
        wait_for_cell_db_value(
            &cell_dir,
            "counter",
            "42",
            std::time::Duration::from_secs(2),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[should_panic(expected = "wait_for_cell_db_value")]
    async fn wait_for_cell_db_value_panics_on_timeout() {
        let td = tempfile::TempDir::new().unwrap();
        let cell_dir = td.path().to_path_buf();
        let conn = rusqlite::Connection::open(cell_dir.join("cell.db")).unwrap();
        meclaw_colony::persist::setup_cell_db(&conn).unwrap();
        drop(conn);
        wait_for_cell_db_value(
            &cell_dir,
            "counter",
            "99",
            std::time::Duration::from_millis(100),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn wait_for_spawn_count_returns_when_expected_reached() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicU32;
        let counter = Arc::new(AtomicU32::new(3));
        wait_for_spawn_count(&counter, 3, std::time::Duration::from_secs(2)).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[should_panic(expected = "wait_for_spawn_count")]
    async fn wait_for_spawn_count_panics_on_timeout() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicU32;
        let counter = Arc::new(AtomicU32::new(0));
        wait_for_spawn_count(&counter, 5, std::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn wait_for_mutation_status_returns_when_status_matches() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        meclaw_colony::persist::setup_colony_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO mutation_log (id, scope, payload_json, status, created_at) VALUES ('X', '/', '{}', 'committed', 100)",
            [],
        ).unwrap();
        drop(conn);
        wait_for_mutation_status(
            &db_path,
            "X",
            "committed",
            std::time::Duration::from_secs(2),
        )
        .await;
    }

    #[tokio::test]
    #[should_panic(expected = "wait_for_mutation_status")]
    async fn wait_for_mutation_status_panics_on_timeout() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        meclaw_colony::persist::setup_colony_db(&conn).unwrap();
        drop(conn);
        wait_for_mutation_status(
            &db_path,
            "absent",
            "committed",
            std::time::Duration::from_millis(100),
        )
        .await;
    }

    #[tokio::test]
    async fn wait_for_any_in_flight_returns_when_row_present() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        meclaw_colony::persist::setup_colony_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO mutation_log (id, scope, payload_json, status, created_at) VALUES ('Y', '/', '{}', 'in_flight', 100)",
            [],
        ).unwrap();
        drop(conn);
        wait_for_any_in_flight(&db_path, std::time::Duration::from_secs(2)).await;
    }

    #[tokio::test]
    #[should_panic(expected = "wait_for_any_in_flight")]
    async fn wait_for_any_in_flight_panics_on_timeout() {
        let td = tempfile::TempDir::new().unwrap();
        let db_path = td.path().join("c.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        meclaw_colony::persist::setup_colony_db(&conn).unwrap();
        drop(conn);
        wait_for_any_in_flight(&db_path, std::time::Duration::from_millis(100)).await;
    }
}
