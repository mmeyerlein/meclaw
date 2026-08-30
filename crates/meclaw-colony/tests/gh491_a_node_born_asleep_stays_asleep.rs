//! GH #491 — a node born asleep stays asleep until somebody NAMES it.
//!
//! `birth: "inactive"` (GH #437) used to be a starting value and nothing more:
//! the node is fully wired, activity is derived from the edge table, so the
//! next mutation whose recompute REACHED the node derived it active again.
//! Reaching is not addressing — a single `add_edges` one level up pulls a whole
//! subtree into the recompute scope (`connectivity::affected_scope`), and every
//! node in it that happens to be connected woke. Measured in a grown colony: a
//! connector grown asleep, two later mutations that named neither it nor
//! anything under it, and a live long-poller nobody armed.
//!
//! The fix is a DURABLE marker rather than a new operation (no `activate` op —
//! the connectivity model stays): a node born inactive is recorded `dormant` in
//! `colony.db`'s `registry`, and a recompute honours it. The one thing that
//! clears it is a mutation that ADDRESSES the node — one whose `involved` set
//! contains its path (an `add_edges` naming it as an endpoint, a `swap_nodes`
//! at its path). A mutation elsewhere in the tree never wakes it, however far
//! its recompute scope reaches, and a restart carries the marker.
//!
//! Deliberately NOT marked: a node put to sleep with `remove_edges`. Its sleep
//! is durable by construction — it has no edge, so no recompute can derive it
//! active — and the only thing that CAN reconnect it is an edge naming it,
//! which is the same "addressed" test the marker enforces. One rule, two
//! mechanisms, no second marker.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::serde_json::json;
use meclaw_core::{JsonValue, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

const CELL: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "persist_mock".to_string(),
        Arc::new(PersistCellFactory {
            spawn_count: Arc::new(AtomicU32::new(0)),
        }) as Arc<dyn CellFactory>,
    )]
}

fn factory_registry() -> meclaw_colony::CellFactoryRegistry {
    let mut reg = meclaw_colony::CellFactoryRegistry::new();
    for (name, f) in factories() {
        reg.insert(name, f);
    }
    reg
}

async fn send_mutation(h: &ColonyHandle, payload: JsonValue) -> MutationOutcome {
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

async fn rescan(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

/// Whether the registry says the node is active. `None` = not registered.
async fn active(h: &ColonyHandle, path: &str) -> Option<bool> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 500,
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

/// What `colony.db` says — `(status, dormant)`, the answer the next boot reads.
fn row(root: &std::path::Path, path: &str) -> (String, i64) {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("open colony.db");
    conn.query_row(
        "SELECT status, dormant FROM registry WHERE path = ?1",
        [path],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    )
    .unwrap_or_else(|e| panic!("no registry row for {path}: {e}"))
}

/// The world: a root hive, and a composite template `unit` (a hive with two
/// wired occupants) that the mutations below instantiate. The sleeper is grown
/// INSIDE that unit, so a mutation one level up — naming the unit and nothing
/// under it — pulls the whole subtree into the recompute scope. That is the
/// exact shape the defect was measured in.
fn setup(root: &std::path::Path) {
    write(
        root,
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(root, "templates/leaf/template.json", r#"{"name":"leaf"}"#);
    write(root, "templates/leaf/config.json", CELL);
    write(root, "templates/unit/template.json", r#"{"name":"unit"}"#);
    write(
        root,
        "templates/unit/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./front","to":"./keeper"}]},"ports":null}}"#,
    );
    write(root, "templates/unit/front/config.json", CELL);
    write(root, "templates/unit/keeper/config.json", CELL);
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    rescan(&h, td.path().join("templates")).await;
    bootstrap_from_filesystem(td.path(), &factory_registry(), &h.runtime())
        .await
        .expect("bootstrap");
    h
}

/// Grows `/ingress`, `/unit` (wired to the ingress) and, inside the unit, a
/// `sleeper` declared `birth: "inactive"` and fully wired in the same diff.
async fn grow_a_sleeping_node(h: &ColonyHandle) {
    let o = send_mutation(
        h,
        json!({"scope":"/","ctx":{},"diff":{
            "add_nodes":[{"name":"ingress","template":"leaf"},
                         {"name":"unit","template":"unit"}],
            "add_edges":[{"from":"./ingress","to":"./unit"}]
        }}),
    )
    .await;
    assert!(
        matches!(o, MutationOutcome::Committed { .. }),
        "the world must come up: {o:?}"
    );

    let o = send_mutation(
        h,
        json!({"scope":"/unit","ctx":{},"diff":{
            "add_nodes":[{"name":"sleeper","template":"leaf","birth":"inactive"}],
            "add_edges":[{"from":"./front","to":"./sleeper"},
                         {"from":"./sleeper","to":"./keeper"}]
        }}),
    )
    .await;
    assert!(
        matches!(o, MutationOutcome::Committed { .. }),
        "wiring and sleeping in one diff commits: {o:?}"
    );
    assert_eq!(
        active(h, "/unit/sleeper").await,
        Some(false),
        "GH #437: the birth declaration wins over the recompute of its own mutation"
    );
}

/// Three mutations that name neither the sleeper nor anything under it. Each
/// one's recompute REACHES it (the unit is an involved endpoint, and
/// `affected_scope` expands an involved path to its whole subtree).
async fn three_foreign_mutations(h: &ColonyHandle) {
    for (i, name) in ["ingress2", "ingress3"].iter().enumerate() {
        let o = send_mutation(
            h,
            json!({"scope":"/","ctx":{},"diff":{
                "add_nodes":[{"name":name,"template":"leaf"}],
                "add_edges":[{"from":format!("./{name}"),"to":"./unit"}]
            }}),
        )
        .await;
        assert!(
            matches!(o, MutationOutcome::Committed { .. }),
            "foreign mutation {i} must commit: {o:?}"
        );
    }
    let o = send_mutation(
        h,
        json!({"scope":"/","ctx":{},"diff":{
            "remove_edges":[{"match":{"from":"ingress3","to":"unit"}}]
        }}),
    )
    .await;
    assert!(
        matches!(o, MutationOutcome::Committed { .. }),
        "the third foreign mutation must commit: {o:?}"
    );
}

/// The defect, and the fix: three mutations elsewhere in the tree leave the
/// sleeper asleep; the one that names it wakes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mutation_that_does_not_name_it_never_wakes_it() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let h = boot(&td).await;
    grow_a_sleeping_node(&h).await;

    // The marker is durable from the moment of birth, not from the first
    // recompute that would have undone it.
    h.shutdown().await;
    assert_eq!(
        row(td.path(), "/unit/sleeper"),
        ("inactive".to_string(), 1),
        "a node born asleep is recorded dormant"
    );

    let h = boot(&td).await;
    three_foreign_mutations(&h).await;
    assert_eq!(
        active(&h, "/unit/sleeper").await,
        Some(false),
        "no mutation that fails to name it may wake it, however far its \
         recompute scope reaches"
    );

    // …and the one that DOES name it wakes it, through the ordinary reconnect.
    let o = send_mutation(
        &h,
        json!({"scope":"/unit","ctx":{},"diff":{
            "add_edges":[{"from":"./keeper","to":"./sleeper"}]
        }}),
    )
    .await;
    assert!(
        matches!(o, MutationOutcome::Committed { .. }),
        "the wake commits: {o:?}"
    );
    assert_eq!(
        active(&h, "/unit/sleeper").await,
        Some(true),
        "an add_edges naming the node IS the wake"
    );

    h.shutdown().await;
    assert_eq!(
        row(td.path(), "/unit/sleeper"),
        ("active".to_string(), 0),
        "waking clears the marker — a woken node is an ordinary node again"
    );
}

/// A restart carries whatever the node was. Asleep before, asleep after — and
/// still un-wakeable by a foreign mutation, which is the half a boot-only
/// receipt cannot see.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_marker_survives_a_restart() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let h = boot(&td).await;
    grow_a_sleeping_node(&h).await;
    h.shutdown().await;

    let h = boot(&td).await;
    assert_eq!(
        active(&h, "/unit/sleeper").await,
        Some(false),
        "the node comes back asleep"
    );
    three_foreign_mutations(&h).await;
    assert_eq!(
        active(&h, "/unit/sleeper").await,
        Some(false),
        "and the marker came back with it — a rehydrated dormant node is not \
         woken by a mutation that does not name it"
    );
    h.shutdown().await;
    assert_eq!(row(td.path(), "/unit/sleeper"), ("inactive".to_string(), 1));
}

/// The other half of the ruling: `remove_edges` sleep gets NO marker, and needs
/// none. The node has no edge, so no recompute can derive it active; the only
/// thing that reconnects it is an edge naming it — the same address test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_slept_by_remove_edges_carries_no_marker() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path());
    let h = boot(&td).await;
    let o = send_mutation(
        &h,
        json!({"scope":"/","ctx":{},"diff":{
            "add_nodes":[{"name":"ingress","template":"leaf"},
                         {"name":"unit","template":"unit"}],
            "add_edges":[{"from":"./ingress","to":"./unit"}]
        }}),
    )
    .await;
    assert!(matches!(o, MutationOutcome::Committed { .. }), "{o:?}");
    assert_eq!(active(&h, "/unit/front").await, Some(true));

    // Sleep the unit: cut the one edge external to it.
    let o = send_mutation(
        &h,
        json!({"scope":"/","ctx":{},"diff":{
            "remove_edges":[{"match":{"from":"ingress","to":"unit"}}]
        }}),
    )
    .await;
    assert!(matches!(o, MutationOutcome::Committed { .. }), "{o:?}");
    assert_eq!(
        active(&h, "/unit/front").await,
        Some(false),
        "cutting the last external edge derives the whole unit inactive"
    );

    h.shutdown().await;
    assert_eq!(
        row(td.path(), "/unit/front"),
        ("inactive".to_string(), 0),
        "sleep-by-edge-removal is durable by construction and takes no marker"
    );
}
