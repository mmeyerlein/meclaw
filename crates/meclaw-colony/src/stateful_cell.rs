//! Phase-6.5 StatefulCell — the cell variant taking the cell.db connection as a param.
//!
//! DbConn-Ownership lebt im `cell_task_stateful`-Stack-Frame
//! (authority model: cell_task opens → decides resume vs. fresh → reopens on
//! restart; the cell impl is agnostic about DB identity).
//!
//! `+ '_` explicitly sets the future's `&mut self` / `&mut db` capture lifetime
//! (edition 2024 does this by default; the explicit marker is there for clarity of
//! discipline).
//!
//! Lives in `meclaw-colony`, not `meclaw-core` — the same layering logic as
//! `CellFactory` (`meclaw-core` stays DB-agnostic).

#[allow(clippy::manual_async_fn)]
pub trait StatefulCell: Send {
    /// Handle an incoming message with exclusive access to the cell's database connection.
    ///
    /// The `db` parameter is a non-blocking `DbConn` wrapper around the cell's SQLite
    /// connection. All database operations must go through `db.call(...)` to avoid
    /// blocking the async runtime.
    fn handle<'a>(
        &'a mut self,
        msg: meclaw_core::Message,
        sink: &'a meclaw_core::OutputSink,
        db: &'a mut crate::DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a;
}

#[cfg(test)]
mod tests {
    use super::StatefulCell;
    use meclaw_core::Uuid;
    use meclaw_core::{Message, MessageBuilder, OutputSink, Path};
    use tokio::sync::mpsc;

    struct UpdateTwice;
    impl StatefulCell for UpdateTwice {
        #[allow(clippy::manual_async_fn)]
        fn handle<'a>(
            &'a mut self,
            _msg: Message,
            _sink: &'a OutputSink,
            db: &'a mut crate::DbConn,
        ) -> impl std::future::Future<Output = ()> + Send + 'a {
            async move {
                db.call(|c| {
                    c.execute("CREATE TABLE IF NOT EXISTS t (n INTEGER)", [])
                        .unwrap();
                    c.execute("INSERT INTO t VALUES (1)", []).unwrap();
                })
                .await;
                db.call(|c| {
                    c.execute("INSERT INTO t VALUES (2)", []).unwrap();
                })
                .await;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_uses_dbconn_across_await() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut db = crate::DbConn::wrap(conn, None);
        let (tx, _rx) = mpsc::channel(8);
        let sink = OutputSink::new(
            tx,
            Path::new("/x"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/x")).build();
        UpdateTwice.handle(msg, &sink, &mut db).await;
        let cnt: i64 = db
            .call(|c| {
                c.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
                    .unwrap()
            })
            .await;
        assert_eq!(cnt, 2);
    }
}
