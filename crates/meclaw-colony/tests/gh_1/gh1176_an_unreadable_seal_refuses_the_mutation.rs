//! GH #1176 (re-check M2) — a mutation whose validation cannot read a hive's
//! seal is refused (`hive_config_unreadable`), it does not pass the boundary.
//!
//! The validation reads every hive's `config.json` for its sealed ports. A
//! config that does not parse (or a read that errors, EIO) used to count as
//! "this hive declares nothing", so an `add_edges` from outside to an address
//! inside the hive the seal does not declare passed the port check and was
//! committed -- and the edge stayed when the disk answered again. The slot
//! rebuild already fails closed on the same reader (the sister pin
//! `gh1176_an_unreadable_seal_keeps_the_boundary_closed`); this is the
//! mutation side. A missing `config.json` still declares nothing.
//!
//! Discriminator: with the file whole, the same diff is refused at the port
//! check (`hive_port_boundary`) -- the edge really crosses a seal -- before
//! and after the unreadable phase.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Failure marker (AGENTS.md: 30 s convention).
const MARKER: Duration = Duration::from_secs(30);

/// `/vault` sealed, only its cell `secret` declared as a port.
const VAULT_CONFIG: &str =
    r#"{"cell":{"type":"hive"},"params":{"ports":["secret"],"graph":{"edges":[]}}}"#;

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
    tokio::time::timeout(MARKER, ack_rx)
        .await
        .expect("failure marker: the colony answers the mutation")
        .unwrap()
}

fn refused_with(outcome: &MutationOutcome, code: &str, why: &str) -> String {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, code, "{why}: {outcome:?}");
            details.clone()
        }
        other => panic!("{why}: expected a refusal `{code}`, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unparsable_seal_refuses_an_edge_into_the_hive() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(td.path().join("colony.json"), "{}").unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    echo_cell(td.path(), "main/caller", "/caller");
    std::fs::create_dir_all(td.path().join("main/vault")).unwrap();
    let cfg = td.path().join("main/vault/config.json");
    std::fs::write(&cfg, VAULT_CONFIG).unwrap();
    echo_cell(td.path(), "main/vault/secret", "/vault/secret");
    echo_cell(td.path(), "main/vault/inner", "/vault/inner");

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("boot");

    // From outside into an address the seal does not declare.
    let diff = json!({"diff": {"add_edges": [{"from": "./caller", "to": "./vault/inner"}]}});
    refused_with(
        &send_mutation(&h, diff.clone()).await,
        "hive_port_boundary",
        "with the seal readable the edge crosses it and is refused at the port check",
    );

    // The seal is cut off mid-write and does not parse.
    std::fs::write(&cfg, &VAULT_CONFIG[..30]).unwrap();
    let details = refused_with(
        &send_mutation(&h, diff.clone()).await,
        "hive_config_unreadable",
        "a seal the validation could not read must not let the edge through (fail-closed)",
    );
    assert!(
        details.contains("vault"),
        "the refusal names the hive whose seal could not be read: {details}"
    );

    // The file is whole again: the seal applies again, the refused diff left
    // no edge behind.
    std::fs::write(&cfg, VAULT_CONFIG).unwrap();
    refused_with(
        &send_mutation(&h, diff).await,
        "hive_port_boundary",
        "once the seal reads again the same diff is refused at the port check",
    );
    h.shutdown().await;
}
