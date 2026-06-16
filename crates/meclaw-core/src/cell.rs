//! Uniform cell trait per spec § Output-Pfad.
//!
//! Cells push `CellOutput` via `OutputSink`. The sink is constructed by
//! `cell_task` per consumed message and carries the parent-context
//! (`sender_path`, `parent_message_id`, `trace_id`) that the cell does
//! not (and per spec must not) know.
//!
//! Returns `impl Future + Send` (not AFIT) so generic `cell_task<C: Cell>`
//! can `tokio::spawn` the future onto a multi-thread runtime.

use std::future::Future;

use crate::message::Message;
use crate::output::OutputSink;

#[allow(clippy::manual_async_fn)]
pub trait Cell: Send {
    fn handle(&mut self, msg: Message, sink: &OutputSink) -> impl Future<Output = ()> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_builder::MessageBuilder;
    use crate::output::{CellEmission, CellOutput};
    use crate::path::Path;
    use serde_json::json;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    struct EchoOnce;

    impl Cell for EchoOnce {
        #[allow(clippy::manual_async_fn)]
        fn handle(&mut self, _msg: Message, sink: &OutputSink) -> impl Future<Output = ()> + Send {
            async move {
                let _ = sink
                    .push(CellOutput {
                        target: Path::new("/dst"),
                        content: json!({"ok": true}),
                    })
                    .await;
            }
        }
    }

    #[tokio::test]
    async fn cell_trait_dispatches_via_sink() {
        let (out_tx, mut out_rx) = mpsc::channel::<CellEmission>(4);
        let sink = OutputSink::new(
            out_tx,
            Path::new("/echo"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            10,
            crate::Headers::new(),
            None,
        );
        let msg = MessageBuilder::new(Path::new("/echo")).build();
        let mut cell = EchoOnce;
        cell.handle(msg, &sink).await;
        let em = out_rx.recv().await.unwrap();
        assert_eq!(em.sender_path.as_str(), "/echo");
        assert_eq!(em.target.as_str(), "/dst");
        assert_eq!(em.content, json!({"ok": true}));
    }
}
