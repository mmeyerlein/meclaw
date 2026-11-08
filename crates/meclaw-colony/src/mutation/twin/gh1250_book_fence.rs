//! GH #1250 -- the registry book a twin's library seed is read from holds
//! every stamp the colony enqueued before the mutation.
//!
//! The book is the `registry` table of `colony.db`; its provenance columns are
//! written fire-and-forget through the async writer (`SetRegistryProvenance`,
//! whose ack fires on the ENQUEUE, not on the commit), while the mutation reads
//! the book synchronously over the read-only connection. Measured in the
//! release gate of 0.85.1 (run 2 of 2, `export-tree-tests`): a swap sent right
//! after the boot read the book before the writer had committed the original's
//! stamp, found no package, and handed the twin the drifted `seed/` of the
//! original's directory -- `gh1034_the_twin_inherits_the_seed_of_the_originals_
//! library_package` red, green in isolation (20 of 20 at 30 % lane load).
//!
//! The pin makes that window deterministic instead of waiting for a loaded
//! machine: a writer that lags (it holds every `SetRegistryProvenance` until
//! the next op of another kind arrives, then forwards both in their order --
//! the FIFO stays intact) stands between the colony and the real writer. Only a
//! mutation that fences the writer before it reads the book sees the stamp.

use crate::persist::writer::ColonyWriteOp;
use crate::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ColonyRuntime, ContractView, SpawnedCellKind,
};
use meclaw_core::serde_json::{Value as JsonValue, json};
use meclaw_core::{CellEmission, Message, Path};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// The seed the library package `local/judge@1.0.0` shipped.
const PACKAGE_SEED: &str = concat!(
    r#"{"schema": {"slot_path": "text", "value": "json", "updated_at": "int"}}"#,
    "\n",
    r#"{"slot_path": "identity.role", "value": {"text": "You are the judge."}, "updated_at": 0}"#,
    "\n",
);

/// The seed in the original's directory, moved away from its package by hand.
const DRIFTED_SEED: &str = concat!(
    r#"{"schema": {"slot_path": "text", "value": "json", "updated_at": "int"}}"#,
    "\n",
    r#"{"slot_path": "identity.role", "value": {"text": "A hand edit."}, "updated_at": 0}"#,
    "\n",
);

/// An `llm` stand-in that is born dormant and is never woken here: the twin
/// rule asks only for the registered type (meclaw-colony cannot depend on the
/// real `llm` cell of meclaw-cells).
struct DormantLlm;
impl CellFactory for DormantLlm {
    fn validate_params(&self, _: &JsonValue) -> Result<(), String> {
        Ok(())
    }
    fn spawn_cell(
        self: Arc<Self>,
        _: Path,
        _: JsonValue,
        _: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<Arc<crate::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (sender, receiver) = mpsc::channel::<Message>(8);
        let (stop_tx, _stop_rx) = oneshot::channel();
        let (_death_tx, death_ack_rx) = oneshot::channel();
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake: Box::new(|_| unreachable!("no message reaches the dormant stand-in")),
            stop_tx,
            death_ack_rx,
            respawn: Box::new(|| unreachable!("the dormant stand-in never dies")),
        })
    }
}

/// The lagging writer: holds each `SetRegistryProvenance` until an op of
/// another kind (a fence, a durable mutation-log insert, ...) arrives, then
/// forwards the held ops first. The channel closing forwards what it holds.
async fn lagging_writer(mut rx: mpsc::Receiver<ColonyWriteOp>, real: mpsc::Sender<ColonyWriteOp>) {
    let mut held = Vec::new();
    while let Some(op) = rx.recv().await {
        if matches!(op, ColonyWriteOp::SetRegistryProvenance { .. }) {
            held.push(op);
            continue;
        }
        for h in held.drain(..) {
            let _ = real.send(h).await;
        }
        let _ = real.send(op).await;
    }
    for h in held {
        let _ = real.send(h).await;
    }
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_swap_reads_the_registry_book_after_the_writer_committed_the_stamp() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    let llm = r#"{"cell": {"type": "llm"}, "params": {}, "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}}"#;
    write(root, "main/config.json", r#"{"cell": {"type": "hive"}}"#);
    // The original carries no stamp in its file, so the boot indexes none: the
    // one stamp of this colony is the one the test enqueues below.
    write(root, "main/brain/config.json", llm);
    write(root, "main/brain/seed/system.jsonl", DRIFTED_SEED);
    let package = root.join("templates/local/judge@1.0.0");
    write(
        &package,
        "template.json",
        r#"{"name":"judge","version":"1.0.0"}"#,
    );
    write(&package, "config.json", llm);
    write(&package, "seed/system.jsonl", PACKAGE_SEED);
    let twin = root.join("templates/twin");
    write(&twin, "template.json", r#"{"name":"twin"}"#);
    write(&twin, "config.json", llm);

    let mut db = crate::ColonyDb::open_in(root).unwrap();
    let (lag_tx, lag_rx) = mpsc::channel(1000);
    let real = std::mem::replace(&mut db.writer_tx, lag_tx);
    tokio::spawn(lagging_writer(lag_rx, real));

    let mut factories = CellFactoryRegistry::new();
    factories.insert(
        "llm".to_string(),
        Arc::new(DormantLlm) as Arc<dyn CellFactory>,
    );
    let (inbox_tx, inbox_rx) = mpsc::channel(1000);
    let (outputs_tx, outputs_rx) = mpsc::channel(1000);
    let colony_config = crate::ColonyConfig::default();
    let cfg = crate::ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx.clone(),
        outputs_rx,
        db,
        factories.clone(),
        root.to_path_buf(),
        colony_config.clone(),
        None,
        None,
    );
    let join = tokio::spawn(crate::colony_task(cfg));

    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().expect("the rescan must not abort");
    let runtime = ColonyRuntime {
        inbox_tx: inbox_tx.clone(),
        outputs_tx,
        colony_config,
        blob_store: None,
    };
    crate::bootstrap_from_filesystem(root, &factories, &runtime)
        .await
        .expect("bootstrap");

    // The stamp of the original, exactly as the boot's provenance index sends
    // it: acked on the enqueue, held by the lagging writer.
    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::SetRegistryProvenance {
            path: Path::new("/brain"),
            provenance: crate::config::NodeProvenance {
                template: "judge".into(),
                template_version: Some("1.0.0".into()),
                template_chain: None,
                instantiated_at: 0,
            },
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();

    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/", "diff": {"swap_nodes": [
                {"match": {"name": "brain"}, "with": {"template": "twin", "name": "brain-b"}}
            ]}}),
            reply_to: None,
            trace_id: uuid::Uuid::now_v7(),
            parent_message_id: uuid::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let outcome = ack_rx.await.unwrap();
    assert!(
        matches!(outcome, crate::MutationOutcome::Committed { .. }),
        "the swap commits: {outcome:?}"
    );

    let seed = std::fs::read_to_string(root.join("main/brain-b/seed/system.jsonl")).ok();
    assert_eq!(
        seed.as_deref(),
        Some(PACKAGE_SEED),
        "the twin's seed/ is the one of local/judge@1.0.0: the mutation reads \
         the registry book only after the writer committed the stamp enqueued \
         before it"
    );

    let (ack_tx, ack_rx) = oneshot::channel();
    inbox_tx
        .send(ColonyMsg::Shutdown { ack: ack_tx })
        .await
        .unwrap();
    ack_rx.await.unwrap();
    join.await.unwrap();
}
