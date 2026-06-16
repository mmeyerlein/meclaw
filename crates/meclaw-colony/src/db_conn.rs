//! Phase-9 DbConn — wraps rusqlite::Connection so that blocking calls
//! run via tokio::task::spawn_blocking and query timeouts use the
//! rusqlite InterruptHandle (Send+Sync, rusqlite-own cross-thread
//! cancellation mechanism — NOT a Mutex-around-cell-state).

/// Wraps a rusqlite Connection so blocking calls run via
/// `tokio::task::spawn_blocking`. The Connection is single-owned (taken
/// for the duration of each `call`) — no Arc, no Mutex.
pub struct DbConn {
    conn: Option<rusqlite::Connection>,
    /// Timeout applied by `call_with_timeout`; ignored by `call`.
    query_timeout: Option<std::time::Duration>,
}

impl DbConn {
    /// Wrap a connection. `query_timeout` is consulted by
    /// `call_with_timeout`; `call` itself is unbounded.
    pub fn wrap(conn: rusqlite::Connection, query_timeout: Option<std::time::Duration>) -> Self {
        Self {
            conn: Some(conn),
            query_timeout,
        }
    }

    /// Update the query-timeout live (β, Weg C). `DbConn` is single-owned by
    /// its cell's handler task (no Arc/Mutex), so a `&mut self` setter is
    /// concurrency-safe — the new value is consulted by the NEXT
    /// `call_with_timeout`. A runtime params-update (`query_timeout_ms`) calls
    /// this so the change takes effect immediately, without a wake/respawn.
    pub fn set_query_timeout(&mut self, query_timeout: Option<std::time::Duration>) {
        self.query_timeout = query_timeout;
    }

    /// The currently-effective query timeout (consulted by `call_with_timeout`).
    pub fn query_timeout(&self) -> Option<std::time::Duration> {
        self.query_timeout
    }

    /// Run `f` on the wrapped connection via `spawn_blocking`.
    /// `f` is `Send + 'static`; output is `Send + 'static`. Cell-Code
    /// materializes owned inputs/outputs across this boundary.
    pub async fn call<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut rusqlite::Connection) -> R + Send + 'static,
        R: Send + 'static,
    {
        let mut conn = self.conn.take().expect("DbConn: conn already taken");
        let (conn, out) = tokio::task::spawn_blocking(move || {
            let out = f(&mut conn);
            (conn, out)
        })
        .await
        .expect("DbConn: spawn_blocking task panicked");
        self.conn = Some(conn);
        out
    }
}

/// Error from `call_with_timeout` when the timer fires before the
/// closure returns. The query is cancelled via
/// `rusqlite::Connection::interrupt`; rusqlite returns
/// `Error::SqliteFailure(SQLITE_INTERRUPT, _)` which the closure may
/// see as its own result — the wrapper still reports `Interrupted`.
#[derive(Debug, thiserror::Error)]
pub enum QueryTimeout {
    /// The configured `query_timeout` elapsed and the query was interrupted.
    #[error("query timed out (interrupted via SQLite interrupt handle)")]
    Interrupted,
}

impl DbConn {
    /// Like `call`, but cancels the running query via SQLite's
    /// `interrupt` mechanism if `query_timeout` elapses first.
    ///
    /// If `query_timeout` is `None`, behaves like `call` and returns
    /// `Ok(f(...))`. Otherwise: races the blocking call against a
    /// `tokio::time::sleep(query_timeout)` task that calls
    /// `interrupt()` on elapse. Returns `Err(Interrupted)` if the
    /// timer fired.
    pub async fn call_with_timeout<F, R>(&mut self, f: F) -> Result<R, QueryTimeout>
    where
        F: FnOnce(&mut rusqlite::Connection) -> R + Send + 'static,
        R: Send + 'static,
    {
        let Some(timeout) = self.query_timeout else {
            return Ok(self.call(f).await);
        };
        let mut conn = self.conn.take().expect("DbConn: conn already taken");
        // InterruptHandle: Send. Single handle, moved into the timer task.
        // No Clone needed — once moved, only the timer can call interrupt().
        let interrupt = conn.get_interrupt_handle();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            interrupt.interrupt();
            true // fired
        });
        let (conn, out) = tokio::task::spawn_blocking(move || {
            let out = f(&mut conn);
            (conn, out)
        })
        .await
        .expect("DbConn: spawn_blocking task panicked");
        self.conn = Some(conn);
        // Race-window (benign, accepted): the query may finish a few
        // nanoseconds before the timer fires `interrupt()`. We detect
        // that via timer.abort() before .await — if the timer already
        // returned its `true`, we honor it as a timeout; if it was
        // aborted before firing, JoinError::is_cancelled → not fired.
        timer.abort();
        let fired = matches!(timer.await, Ok(true));
        if fired {
            Err(QueryTimeout::Interrupted)
        } else {
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_executes_closure_on_connection() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = DbConn::wrap(conn, None);
        let n: i64 = db
            .call(|c| {
                c.execute("CREATE TABLE t (n INTEGER)", []).unwrap();
                c.execute("INSERT INTO t VALUES (42)", []).unwrap();
                c.query_row("SELECT n FROM t", [], |r| r.get(0)).unwrap()
            })
            .await;
        assert_eq!(n, 42);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_with_timeout_interrupts_long_query() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = DbConn::wrap(conn, Some(std::time::Duration::from_millis(50)));
        let res = db
            .call_with_timeout(|c| {
                // Recursive CTE produces ~1M rows — interruptible inside SQLite.
                c.query_row(
                    "WITH RECURSIVE r(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM r WHERE x < 1000000)
                     SELECT COUNT(*) FROM r",
                    [],
                    |r| r.get::<_, i64>(0),
                )
            })
            .await;
        match res {
            Err(QueryTimeout::Interrupted) => {}
            other => panic!("expected Interrupted, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_query_timeout_takes_effect_on_next_call() {
        // β (Weg C, sofort-live): a runtime params-update lowers query_timeout_ms
        // live via `set_query_timeout`; the NEXT `call_with_timeout` enforces it
        // (no respawn needed). Positive live receipt.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = DbConn::wrap(conn, None); // start unbounded
        assert_eq!(db.query_timeout(), None);
        db.set_query_timeout(Some(std::time::Duration::from_millis(50)));
        assert_eq!(
            db.query_timeout(),
            Some(std::time::Duration::from_millis(50))
        );
        let res = db
            .call_with_timeout(|c| {
                c.query_row(
                    "WITH RECURSIVE r(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM r WHERE x < 1000000)
                     SELECT COUNT(*) FROM r",
                    [],
                    |r| r.get::<_, i64>(0),
                )
            })
            .await;
        assert!(
            matches!(res, Err(QueryTimeout::Interrupted)),
            "the live-set timeout must apply to the next call, got {res:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_with_timeout_passes_through_fast_query() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = DbConn::wrap(conn, Some(std::time::Duration::from_secs(5)));
        let res = db
            .call_with_timeout(|c| c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0)))
            .await;
        assert!(matches!(res, Ok(Ok(1))));
    }
}
