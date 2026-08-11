//! Phase-9 DbConn — wraps rusqlite::Connection so that blocking calls
//! run via tokio::task::spawn_blocking and query timeouts use the
//! rusqlite InterruptHandle (Send+Sync, rusqlite-own cross-thread
//! cancellation mechanism — NOT a Mutex-around-cell-state).

/// Wraps a rusqlite Connection so blocking calls run via
/// `tokio::task::spawn_blocking`. The Connection is single-owned (taken
/// for the duration of each `call`) — no Arc, no Mutex.
pub struct DbConn {
    /// `None` while a call is in flight (the connection lives inside the
    /// blocking closure) and permanently once runtime shutdown cancelled a
    /// blocking task before it ran — that drops the closure and the connection
    /// with it. See [`connection_lost`].
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

    /// Update the query timeout live (β, path C). `DbConn` is single-owned by
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
    ///
    /// A panic inside `f` propagates to the caller. If runtime shutdown
    /// cancels the blocking task before it runs, the call never resolves
    /// instead of panicking — see [`connection_lost`].
    pub async fn call<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut rusqlite::Connection) -> R + Send + 'static,
        R: Send + 'static,
    {
        let Some(mut conn) = self.conn.take() else {
            return connection_lost().await;
        };
        let joined = tokio::task::spawn_blocking(move || {
            let out = f(&mut conn);
            (conn, out)
        })
        .await;
        let Some((conn, out)) = join_blocking(joined) else {
            return connection_lost().await;
        };
        self.conn = Some(conn);
        out
    }
}

/// Join a `spawn_blocking` handle that carried the connection.
///
/// * `Ok` — the closure ran; connection and result come back.
/// * panic — a real panic inside the DB closure keeps propagating unchanged
///   (`resume_unwind`), exactly as the previous `expect` did.
/// * cancelled — the blocking pool dropped the task *before* it ran, which
///   only happens while the runtime is shutting down. The closure owned the
///   connection, so it was dropped with the closure: `None` means the
///   connection is gone for good, and there is nothing to restore.
fn join_blocking<T>(joined: Result<T, tokio::task::JoinError>) -> Option<T> {
    match joined {
        Ok(value) => Some(value),
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(_cancelled) => None,
    }
}

/// The connection was lost to runtime shutdown, so the call can neither run
/// nor produce a result.
///
/// This future never resolves. The only way to reach it is a cancelled
/// blocking task, i.e. a runtime whose blocking pool is shutting down — the
/// task awaiting here is being torn down by that same shutdown and is dropped
/// at this await point. Never resolving is what "this call has no future"
/// means; panicking here is precisely the shutdown crash this guards against,
/// and `call` has no error channel to report into (its result type is the
/// caller's `R`, and changing that signature would touch every call site).
/// A `DbConn` that reached this state stays in it: every later call parks the
/// same way instead of panicking on the missing connection, and the cell's
/// `message_timeout` backstop is what turns a lingering handler into a
/// supervised restart.
async fn connection_lost<R>() -> R {
    std::future::pending().await
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
    ///
    /// Shares `call`'s shutdown contract: a blocking task cancelled by
    /// runtime shutdown parks the call forever rather than panicking (see
    /// [`connection_lost`]). `Interrupted` stays reserved for a real query
    /// timeout, so the cell-side match arms keep their meaning.
    pub async fn call_with_timeout<F, R>(&mut self, f: F) -> Result<R, QueryTimeout>
    where
        F: FnOnce(&mut rusqlite::Connection) -> R + Send + 'static,
        R: Send + 'static,
    {
        let Some(timeout) = self.query_timeout else {
            return Ok(self.call(f).await);
        };
        let Some(mut conn) = self.conn.take() else {
            return connection_lost().await;
        };
        // InterruptHandle: Send. Single handle, moved into the timer task.
        // No Clone needed — once moved, only the timer can call interrupt().
        let interrupt = conn.get_interrupt_handle();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            interrupt.interrupt();
            true // fired
        });
        let joined = tokio::task::spawn_blocking(move || {
            let out = f(&mut conn);
            (conn, out)
        })
        .await;
        let Some((conn, out)) = join_blocking(joined) else {
            timer.abort();
            return connection_lost().await;
        };
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
    use std::future::Future;
    use std::task::Poll;

    /// A handle to a runtime that is already shut down. `spawn_blocking` on
    /// such a handle hands back a `JoinHandle` that resolves to a cancelled
    /// `JoinError` without ever running the closure — deterministically the
    /// same shape as the shutdown race, with no timing assumption.
    fn shut_down_runtime_handle() -> tokio::runtime::Handle {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("build helper runtime");
        let handle = rt.handle().clone();
        rt.shutdown_background();
        handle
    }

    /// Polls `fut` exactly once and reports the `Poll` — no timing assumption,
    /// and a panic inside the polled future surfaces as a test failure.
    macro_rules! poll_once {
        ($fut:expr) => {
            std::future::poll_fn(|cx| Poll::Ready($fut.as_mut().poll(cx))).await
        };
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_does_not_panic_when_blocking_task_is_cancelled() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = DbConn::wrap(conn, None);
        let dead = shut_down_runtime_handle();
        let mut fut = std::pin::pin!(db.call(|_c| ()));
        let first = {
            let _entered = dead.enter();
            poll_once!(fut)
        };
        assert!(
            first.is_pending(),
            "a cancelled blocking task must not panic; the call must never resolve instead, got {first:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_after_cancelled_blocking_task_does_not_panic_on_missing_conn() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = DbConn::wrap(conn, None);
        let dead = shut_down_runtime_handle();
        {
            let mut fut = std::pin::pin!(db.call(|_c| ()));
            let _entered = dead.enter();
            let _ = poll_once!(fut);
        }
        // The connection went down with the cancelled closure. The follow-up
        // access must not panic on the missing connection either.
        let mut fut = std::pin::pin!(db.call(|_c| ()));
        let next = poll_once!(fut);
        assert!(
            next.is_pending(),
            "a call on a connection lost to shutdown must not panic, got {next:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_with_timeout_does_not_panic_when_blocking_task_is_cancelled() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = DbConn::wrap(conn, Some(std::time::Duration::from_secs(5)));
        let dead = shut_down_runtime_handle();
        let mut fut = std::pin::pin!(db.call_with_timeout(|_c| ()));
        let first = {
            let _entered = dead.enter();
            poll_once!(fut)
        };
        assert!(
            first.is_pending(),
            "a cancelled blocking task must not panic in call_with_timeout either, got {first:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_with_timeout_after_cancelled_blocking_task_does_not_panic_on_missing_conn() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = DbConn::wrap(conn, Some(std::time::Duration::from_secs(5)));
        let dead = shut_down_runtime_handle();
        {
            let mut fut = std::pin::pin!(db.call_with_timeout(|_c| ()));
            let _entered = dead.enter();
            let _ = poll_once!(fut);
        }
        let mut fut = std::pin::pin!(db.call_with_timeout(|_c| ()));
        let next = poll_once!(fut);
        assert!(
            next.is_pending(),
            "call_with_timeout on a connection lost to shutdown must not panic, got {next:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_still_propagates_a_panic_from_the_closure() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = DbConn::wrap(conn, None);
        let joined = tokio::spawn(async move {
            db.call(|_c| panic!("closure blew up")).await;
        })
        .await;
        let err = joined.expect_err("a panicking DB closure must not be swallowed");
        assert!(
            err.is_panic(),
            "a real panic in the DB closure must keep propagating, got {err:?}"
        );
    }

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
        // β (path C, immediately live): a runtime params update lowers query_timeout_ms
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
