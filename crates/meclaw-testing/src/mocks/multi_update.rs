//! Phase-6.5 demo cell. Writes 2 rows to `multi_update_log` with an
//! `await`-emit between them. Existence + both rows present proves
//! interleaved emit+write over an await boundary works in cell_task_stateful.

use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::json;
use meclaw_core::{CellOutput, Message, OutputSink, Path};

/// Phase-6.5 test cell: emits + writes alternately across an await boundary.
///
/// Does NOT own a `Connection` field — the connection is passed in per
/// `handle()` call from `cell_task_stateful`'s stack frame.
pub struct MultiUpdateMockCell {
    sink_target: Path,
}

impl MultiUpdateMockCell {
    /// Construct a fresh cell that emits to `sink_target`.
    pub fn new(sink_target: Path) -> Self {
        Self { sink_target }
    }
}

impl StatefulCell for MultiUpdateMockCell {
    #[allow(clippy::manual_async_fn)]
    fn handle<'a>(
        &'a mut self,
        _msg: Message,
        sink: &'a OutputSink,
        db: &'a mut meclaw_colony::DbConn,
    ) -> impl std::future::Future<Output = ()> + Send + 'a {
        async move {
            let _ = sink
                .push(CellOutput {
                    target: self.sink_target.clone(),
                    content: json!({"messages": [], "header": {"step": 1}}),
                })
                .await;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_secs() as i64;
            db.call(move |c| {
                c.execute(
                    "INSERT INTO multi_update_log (step, inserted_at) VALUES (1, ?)",
                    [now],
                )
                .expect("insert step 1");
            })
            .await;
            let _ = sink
                .push(CellOutput {
                    target: self.sink_target.clone(),
                    content: json!({"messages": [], "header": {"step": 2}}),
                })
                .await;
            db.call(move |c| {
                c.execute(
                    "INSERT INTO multi_update_log (step, inserted_at) VALUES (2, ?)",
                    [now],
                )
                .expect("insert step 2");
            })
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::{MessageBuilder, Uuid};
    use tokio::sync::mpsc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cell_writes_both_log_rows_across_await() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE multi_update_log (step INTEGER, inserted_at INTEGER)",
            [],
        )
        .unwrap();
        let mut db = meclaw_colony::DbConn::wrap(conn, None);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/m"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/m")).build();
        let mut cell = MultiUpdateMockCell::new(Path::new("/sink"));
        cell.handle(msg, &sink, &mut db).await;
        let rows: Vec<i64> = db
            .call(|c| {
                c.prepare("SELECT step FROM multi_update_log ORDER BY step")
                    .unwrap()
                    .query_map([], |r| r.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap()
            })
            .await;
        assert_eq!(rows, vec![1, 2]);
        let mut emits = 0;
        while out_rx.try_recv().is_ok() {
            emits += 1;
        }
        assert_eq!(emits, 2);
    }
}
