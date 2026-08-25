//! GH #390 — what `remove_nodes` really does, in the two places the spec row
//! promised more than the substrate holds.
//!
//! The row in `docs/meclaw-overview.md` § Mutation-Operationen used to say
//! "inkl. Subtree-Kaskade bei Hives" / "including subtree cascade at hives".
//! Both halves of that reading are false, and this file is the retraction's pin:
//!
//! 1. **A hive path is not a `remove_nodes` target.** `remove_nodes[].match.name`
//!    is resolved against the CELL REGISTRY only (`mutation/validate.rs`), while
//!    `swap_nodes` right beside it asks the registry OR the hive scopes. A hive
//!    has no registry row, so the entry is `match_no_hit` — and because
//!    validation is all-or-nothing, the whole mutation is refused, including the
//!    well-formed entries beside it.
//! 2. **Edges do not cascade.** The apply arm removes every edge naming the
//!    matched path ITSELF at one end (exact path equality). An edge between two
//!    DESCENDANTS of the matched node survives, so the disconnected unit stays
//!    whole and re-connectable — the same reasoning `swap_nodes` states for
//!    subtrees (GH #256). What cascades over the subtree is the connectivity
//!    recompute: it flips the nodes below to `active = false` and stops their
//!    tasks.

use meclaw_colony::api_dto::{ReadGraphReply, ReadRegistryReply};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tempfile::TempDir;

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

/// Root hive, one cell `/x`, and a hive `/h` with two cells under it. The
/// `/h/c1 -> /h/c2` edge is INTERNAL to the hive and comes from the hive's own
/// `params.graph` at bootstrap — it is the edge between two descendants that the
/// exact-path rule leaves standing.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main/x")).unwrap();
    std::fs::create_dir_all(td.join("main/h/c1")).unwrap();
    std::fs::create_dir_all(td.join("main/h/c2")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/x/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/x"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/h/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./c1","to":"./c2"}
        ]}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/h/c1/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/h/c1"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/h/c2/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/h/c2"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
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

/// Every `from -> to` pair currently in the edge table, read from the authority.
async fn edge_pairs(h: &ColonyHandle) -> Vec<(String, String)> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .edges
        .into_iter()
        .map(|e| (e.from, e.to))
        .collect()
}

/// `active` flag of the RAM registry entry for `path` (or `None` if absent).
async fn is_active(h: &ColonyHandle, path: &str) -> Option<bool> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .map(|e| e.active)
}

/// Bring the topology up and wire the gating edge `/x -> /h`.
async fn boot(td: &TempDir) -> ColonyHandle {
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"x","to":"h"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the gating edge /x -> /h must commit, got {outcome:?}"
    );
    h
}

/// Half 1 of the retraction: a hive path is `match_no_hit`, and the refusal
/// takes the whole mutation with it — the well-formed `x` entry beside it does
/// not apply either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_nodes_refuses_a_hive_path_and_the_whole_mutation_with_it() {
    let td = TempDir::new().unwrap();
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_nodes":[
            {"match":{"name":"h"}},
            {"match":{"name":"x"}}
        ]}}),
    )
    .await;

    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            violations,
            ..
        } => {
            assert_eq!(
                error_code, "match_no_hit",
                "a hive path is not a remove_nodes target, got {outcome:?}"
            );
            let addressed: Vec<(&str, Option<&str>)> = violations
                .iter()
                .map(|v| (v.code, v.address.as_deref()))
                .collect();
            assert_eq!(
                addressed,
                vec![("match_no_hit", Some("h"))],
                "exactly one violation, and it names the hive: the `x` entry beside it is \
                 well-formed and produces nothing"
            );
        }
        other => panic!("remove_nodes on a hive path must be refused, got {other:?}"),
    }

    // All-or-nothing: `x` was a perfectly good entry and its edge still stands.
    let pairs = edge_pairs(&h).await;
    assert!(
        pairs.contains(&("/x".to_string(), "/h".to_string())),
        "the refused mutation must leave /x -> /h in place, edges: {pairs:?}"
    );
    assert_eq!(
        is_active(&h, "/x").await,
        Some(true),
        "/x stays active — nothing of the refused diff applied"
    );

    h.shutdown().await;
}

/// Half 2 of the retraction: edge removal is exact path incidence. `/x -> /h`
/// goes because `/x` is the matched path; `/h/c1 -> /h/c2` stays because neither
/// end IS the matched path. What cascades over the subtree is the connectivity
/// recompute, which flips the descendants inactive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_nodes_leaves_an_edge_between_two_descendants_standing() {
    let td = TempDir::new().unwrap();
    let h = boot(&td).await;

    let before = edge_pairs(&h).await;
    assert!(
        before.contains(&("/x".to_string(), "/h".to_string())),
        "pre-state: /x -> /h, edges: {before:?}"
    );
    assert!(
        before.contains(&("/h/c1".to_string(), "/h/c2".to_string())),
        "pre-state: the internal /h/c1 -> /h/c2, edges: {before:?}"
    );
    assert_eq!(is_active(&h, "/h/c1").await, Some(true));
    assert_eq!(is_active(&h, "/h/c2").await, Some(true));

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_nodes":[{"match":{"name":"x"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "remove_nodes /x must commit, got {outcome:?}"
    );

    let after = edge_pairs(&h).await;
    assert!(
        !after.contains(&("/x".to_string(), "/h".to_string())),
        "the edge naming the matched path itself goes, edges: {after:?}"
    );
    assert!(
        after.contains(&("/h/c1".to_string(), "/h/c2".to_string())),
        "NO edge cascade: the edge between two descendants of /h survives, edges: {after:?}"
    );

    // The cascade that DOES exist: connectivity. /h lost its only crossing-in
    // edge, so its whole subtree goes inactive — entries and edges stay.
    assert_eq!(
        is_active(&h, "/h/c1").await,
        Some(false),
        "the connectivity recompute cascades over the subtree"
    );
    assert_eq!(is_active(&h, "/h/c2").await, Some(false));

    h.shutdown().await;
}
