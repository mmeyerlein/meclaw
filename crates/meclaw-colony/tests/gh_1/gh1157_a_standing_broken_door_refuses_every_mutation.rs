//! GH #1157 — a hive contract that stands broken refuses every mutation,
//! however far from the hive that mutation lands.
//!
//! The lane-door check judges what STANDS, not only what a diff touches. A
//! colony can carry a broken contract: boot only warns about one, and a hive
//! born by a mutation is not door-checked in its own birth. From then on every
//! mutation is refused until the hive is repaired. #1157 made the check local
//! per contract (cheaper), and this pin holds that it did not make it narrower:
//! an edge between two cells nowhere near the hive is still refused for it.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn echo_cell(td: &std::path::Path, rel: &str, emitted_target: &str) {
    std::fs::create_dir_all(td.join(rel)).unwrap();
    std::fs::write(
        td.join(rel).join("config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"{emitted_target}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

fn hive(td: &std::path::Path, rel: &str, params: &str) {
    std::fs::create_dir_all(td.join(rel)).unwrap();
    std::fs::write(
        td.join(rel).join("config.json"),
        format!(r#"{{"cell":{{"type":"hive"}},"params":{params}}}"#),
    )
    .unwrap();
}

/// `/mem` promises `in_batch`, but its only door opens for `in_other`. The
/// door edge itself wires the hive, so the contract is live, and broken.
const DOORLESS: &str = r#"{
  "ports": [],
  "contract": {
    "accepts": [{"route": "in_batch", "because": "one closed session as a single write batch"}],
    "emits": []
  },
  "graph": {"edges": [
    {"from": ".", "to": "./glue", "condition": "has(hop.route) && hop.route == 'in_other'"}
  ]}}"#;

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_far_away_mutation_is_refused_for_a_standing_broken_door() {
    let td = tempfile::TempDir::new().unwrap();
    hive(td.path(), "main", r#"{"graph":{"edges":[]}}"#);
    echo_cell(td.path(), "main/caller", "/caller");
    echo_cell(td.path(), "main/sink", "/sink");
    echo_cell(td.path(), "main/other", "/other");
    hive(td.path(), "main/mem", DOORLESS);
    echo_cell(td.path(), "main/mem/glue", "/mem/glue");

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot only warns about a broken contract");

    // Two mutations, neither near /mem: each is refused for /mem's door.
    for (from, to) in [("./caller", "./sink"), ("./sink", "./other")] {
        let outcome = send_mutation(
            &h,
            json!({"diff": {"add_edges": [{"from": from, "to": to}]}}),
        )
        .await;
        match &outcome {
            MutationOutcome::Rejected {
                error_code,
                details,
                ..
            } => {
                assert_eq!(error_code, "hive_contract", "{outcome:?}");
                assert!(
                    details.contains("hive '/mem' declares it accepts the lane 'in_batch'"),
                    "the refusal names the standing broken door: {details}"
                );
            }
            other => panic!(
                "{from} -> {to}: a standing broken door refuses every mutation, got {other:?}"
            ),
        }
    }
    h.shutdown().await;
}
