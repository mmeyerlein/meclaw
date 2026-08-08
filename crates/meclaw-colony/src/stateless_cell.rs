//! Phase-7 StatelessCell — the cell variant without state and without a cell.db.
//!
//! `handle(&self, msg, sink)` is reentrant: the dispatcher
//! (`cell_task::stateless_dispatcher`) spawns a short-lived worker per message
//! that holds `cell.clone()` (an Arc) plus a per-message `OutputSink`.
//! Implementors hold **no mutable state** in the cell struct — every cell field is
//! read-only configuration (e.g. `base_path` for FileCell).
//!
//! `+ '_` explicitly sets the future's `&self` / `&sink` capture lifetime
//! (edition 2024 does this by default; the explicit marker is there for clarity,
//! analogous to `Cell`/`StatefulCell`).
//!
//! Lives in `meclaw-colony`, not `meclaw-core` — the same layering logic
//! as `CellFactory` and `StatefulCell`. `meclaw-core` stays I/O-agnostic.
//!
//! Object safety: not object-safe (RPITIT). The dispatcher is generic over
//! `<F: StatelessCell + 'static>`, monomorphized per cell type.

#[allow(clippy::manual_async_fn)]
pub trait StatelessCell: Send + Sync {
    /// Handle one message. `&self` is shared-immutable (read-only config);
    /// `sink` is per-message-ephemeral. Both borrows share lifetime `'a` so
    /// the returned Future can capture either reference across `.await` points.
    fn handle<'a>(
        &'a self,
        msg: meclaw_core::Message,
        sink: &'a meclaw_core::OutputSink,
    ) -> impl std::future::Future<Output = ()> + Send + 'a;
}

#[cfg(test)]
mod tests {
    use super::StatelessCell;
    use meclaw_core::{
        CellEmission, CellOutput, MessageBuilder, OutputSink, Path, Uuid, serde_json::json,
    };
    use tokio::sync::mpsc;

    struct PushTwice;
    #[allow(clippy::manual_async_fn)]
    impl StatelessCell for PushTwice {
        fn handle<'a>(
            &'a self,
            _msg: meclaw_core::Message,
            sink: &'a OutputSink,
        ) -> impl std::future::Future<Output = ()> + Send + 'a {
            async move {
                let _ = sink
                    .push(CellOutput {
                        target: Path::new("/x"),
                        content: json!({"i": 1}),
                    })
                    .await;
                tokio::task::yield_now().await;
                let _ = sink
                    .push(CellOutput {
                        target: Path::new("/x"),
                        content: json!({"i": 2}),
                    })
                    .await;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_captures_self_and_sink_across_await() {
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(8);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/c"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            meclaw_core::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/c")).build();
        PushTwice.handle(msg, &sink).await;
        let e1 = out_rx.recv().await.unwrap();
        let e2 = out_rx.recv().await.unwrap();
        assert_eq!(e1.content, json!({"i": 1}));
        assert_eq!(e2.content, json!({"i": 2}));
    }
}
