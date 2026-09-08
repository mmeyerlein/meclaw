//! GH #612 (the mutation half) — an `add_edges` endpoint is answered in the
//! namespace the APPLY will use.
//!
//! Endpoints are scope-relative: a bare short name is resolved against the
//! mutation's own guard scope (`mutation::resolve_scoped_path`). The
//! endpoint-existence check collected hive short names colony-globally, so a
//! name that names a hive ANYWHERE passed — and then the apply put the edge on
//! `<this scope>/<name>`, which is a different node, or no node at all.
//!
//! Measured before the fix: with a hive at `/a/phone` and nothing under
//! `/b/phone`, `{"scope":"/b","diff":{"add_edges":[{"from":"./x","to":"phone"}]}}`
//! committed and left the edge `/b/x -> /b/phone` in the table. Validation and
//! apply had named two different nodes and nothing said so.
//!
//! A cross-scope reference to a hive keeps the spelling it always had — a
//! relative path, resolved absolutely — and that half is pinned here too, so
//! the fix cannot be mistaken for "hives are no longer reachable from
//! elsewhere".

use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Path, Uuid};
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

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let dir = root.join(rel);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), body).unwrap();
}

fn hive(root: &std::path::Path, rel: &str) {
    write(
        root,
        rel,
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
}

fn echo_cell(root: &std::path::Path, rel: &str) {
    write(
        root,
        rel,
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/nowhere"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
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
    ack_rx.await.unwrap()
}

async fn read_graph(h: &ColonyHandle) -> ReadGraphReply {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

fn edges(reply: &ReadGraphReply) -> Vec<(String, String)> {
    let mut e: Vec<(String, String)> = reply
        .edges
        .iter()
        .map(|x| (x.from.clone(), x.to.clone()))
        .collect();
    e.sort();
    e
}

/// `/a` holds the hive `/a/phone` and a cell `c`; `/b` holds two cells and no
/// `phone` at all.
async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    hive(td.path(), "main");
    hive(td.path(), "main/a");
    hive(td.path(), "main/a/phone");
    echo_cell(td.path(), "main/a/phone/inner");
    echo_cell(td.path(), "main/a/c");
    hive(td.path(), "main/b");
    echo_cell(td.path(), "main/b/x");
    echo_cell(td.path(), "main/b/y");

    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the topology must boot");
    h
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_short_name_that_names_a_hive_elsewhere_is_not_an_endpoint_here() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;
    let before = edges(&read_graph(&h).await);

    match send_mutation(
        &h,
        json!({"scope":"/b","diff":{"add_edges":[{"from":"./x","to":"phone"}]}}),
    )
    .await
    {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "edge_schema", "got {details}");
            assert!(
                details.contains("phone") && details.contains("/b"),
                "the refusal names the endpoint AND the scope it was resolved against: {details}"
            );
        }
        other => {
            panic!("a short name that hits nothing in this scope must be refused, got {other:?}")
        }
    }

    assert_eq!(
        before,
        edges(&read_graph(&h).await),
        "a pre-destructive reject leaves the edge table untouched"
    );
    h.shutdown().await;
}

/// The positive control on the same colony: from the scope that really holds
/// the hive, the short name is an endpoint and always was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_short_name_commits_from_the_scope_that_holds_the_hive() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    match send_mutation(
        &h,
        json!({"scope":"/a","diff":{"add_edges":[{"from":"./c","to":"phone"}]}}),
    )
    .await
    {
        MutationOutcome::Committed { .. } => {}
        other => panic!("the hive's own scope must still wire it by name, got {other:?}"),
    }
    assert!(
        edges(&read_graph(&h).await).contains(&("/a/c".to_string(), "/a/phone".to_string())),
        "the edge lands on the hive the name was checked against"
    );
    h.shutdown().await;
}

/// And the spelling a cross-scope reference always had — a relative path —
/// still commits: what the fix removes is a bare name borrowed from a foreign
/// scope, not the reachability of a hive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relative_path_across_scopes_still_commits() {
    let td = tempfile::TempDir::new().unwrap();
    let h = boot(&td).await;

    match send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"./b/x","to":"./a/phone"}]}}),
    )
    .await
    {
        MutationOutcome::Committed { .. } => {}
        other => panic!("a resolved path endpoint is unaffected, got {other:?}"),
    }
    assert!(
        edges(&read_graph(&h).await).contains(&("/b/x".to_string(), "/a/phone".to_string())),
        "the cross-scope reference is the relative path, and it is answered absolutely"
    );
    h.shutdown().await;
}
