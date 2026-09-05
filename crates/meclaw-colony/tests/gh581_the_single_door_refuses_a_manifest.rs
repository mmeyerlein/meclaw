//! GH #581 — the single-declaration door refuses a manifest body BY NAME.
//!
//! WHAT WAS MEASURED
//! =================
//! `ColonyMsg::Mutation` is the single-declaration door. Handed a body of the
//! MANIFEST form — `{"manifest": [ … ]}` — it answered
//! `MutationOutcome::Committed { id }` and applied nothing: no node, no edge,
//! no log row for the declarations it was handed. The manifest door
//! (`ColonyMsg::MutationDoor`) applies the very same body correctly.
//!
//! The mechanism was one `unwrap_or`: `handle_mutation` reads its work from
//! `payload["diff"]` and treats an absent key as the EMPTY diff. A manifest
//! body carries no `diff`, so every step below it had nothing to do and the
//! door reported success for a colony that never changed. The vocabulary check
//! that already refuses a key nobody reads (`refuse_unknown_diff_keys`) looks
//! INSIDE the diff and therefore never saw the top-level `manifest`.
//!
//! "Committed" for "did nothing" is the one answer a door must never give
//! (development-rules § 2c: a refusal is named, a commit is real).
//!
//! WHAT IS PINNED HERE
//! ===================
//! 1. the manifest form at the SINGLE door is `Rejected`, with the existing
//!    `error_code` `schema`, and the refusal is SPURLESS — no id, no
//!    mutation-log row, no node in the registry;
//! 2. the control: the identical body at the MANIFEST door commits and the
//!    node arrives;
//! 3. the control: an ordinary single declaration at the single door still
//!    commits.
//!
//! WHY `schema` AND NOT A NEW CODE
//! ===============================
//! `error_code` is a documented public contract surface (README § Stability),
//! and a body form the door will not apply is precisely what `schema` already
//! means. `ManifestError::error_code` says the same thing from the other side
//! of the same wall: a manifest that cannot be read is not a new class of
//! failure, it is the oldest one.

use meclaw_colony::api_dto::{ReadMutationsAuditReply, ReadRegistryReply};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ManifestOutcome, MutationDoorOutcome,
    MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::{Uuid, serde_json::Value, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

const CELL_CONFIG: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;
const HIVE_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The one declaration both doors are handed, so the two verdicts are
/// comparable: it adds `/grown` from the fixture's only template.
fn one_declaration() -> Value {
    json!({
        "scope": "/",
        "diff": {"add_nodes": [{"name": "grown", "template": "persist_mock"}]}
    })
}

/// The same declaration, wrapped in the manifest form.
fn as_manifest() -> Value {
    json!({"manifest": [one_declaration()]})
}

async fn send_single(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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
    ack_rx.await.expect("the single door must answer")
}

async fn send_door(h: &ColonyHandle, payload: Value) -> MutationDoorOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::MutationDoor {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.expect("the manifest door must answer")
}

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().expect("rescan must not abort");
}

/// Every path the registry holds, so "nothing was applied" is read off a list
/// rather than guessed.
async fn registry_paths(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 200,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let mut out: Vec<String> = ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect();
    out.sort();
    out
}

/// The mutation log, in full — the spurlessness of a refusal is a claim about
/// this table.
async fn mutation_log_ids(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadMutationsAuditReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadMutationsAudit {
            since: None,
            limit: 200,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.id)
        .collect()
}

/// Root cell `main` (logical `/`) and one template to grow `/grown` from.
async fn bootstrapped_colony() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", HIVE_CONFIG);
    write(
        td.path(),
        "templates/persist_mock/template.json",
        r#"{"name":"persist_mock"}"#,
    );
    write(td.path(), "templates/persist_mock/config.json", CELL_CONFIG);

    let factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), factory.clone())],
    );
    let mut reg = CellFactoryRegistry::new();
    reg.insert("persist_mock".into(), factory);
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap must succeed");
    rescan_templates(&h, td.path().join("templates")).await;
    (td, h)
}

// ── 1. the refusal ──────────────────────────────────────────────────────────

/// The manifest form at the single door is refused BY NAME, before anything is
/// minted, staged or logged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_body_at_the_single_door_is_refused_by_name() {
    let (_td, h) = bootstrapped_colony().await;
    let before = registry_paths(&h).await;

    let outcome = send_single(&h, as_manifest()).await;

    let (id, error_code, details) = match outcome {
        MutationOutcome::Rejected {
            id,
            error_code,
            details,
            ..
        } => (id, error_code, details),
        other => panic!(
            "the single door must REFUSE a manifest body, not report success \
             for a colony it never changed; got {other:?}"
        ),
    };
    assert_eq!(
        error_code, "schema",
        "the refusal carries the existing form-error code; got {error_code}"
    );
    assert!(
        details.contains("manifest"),
        "the refusal names the key that made it a refusal, so the caller can \
         find the other door; got {details}"
    );
    assert_eq!(
        id, None,
        "the refusal is spurless: it comes BEFORE the mutation id is minted"
    );
    assert!(
        mutation_log_ids(&h).await.is_empty(),
        "and therefore leaves no row in the mutation log"
    );
    assert_eq!(
        registry_paths(&h).await,
        before,
        "and the graph is the one the door found"
    );
    h.shutdown().await;
}

// ── 2. the control at the other door ────────────────────────────────────────

/// The identical body at the MANIFEST door still commits and still applies.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_manifest_body_commits_at_the_manifest_door() {
    let (_td, h) = bootstrapped_colony().await;

    let outcome = send_door(&h, as_manifest()).await;

    match outcome {
        MutationDoorOutcome::Manifest(ManifestOutcome::Committed { ids }) => {
            assert_eq!(ids.len(), 1, "one entry, one id; got {ids:?}");
        }
        other => panic!("the manifest door must commit this body; got {other:?}"),
    }
    assert!(
        registry_paths(&h).await.iter().any(|p| p == "/grown"),
        "and the declaration it rolled off is in the registry"
    );
    h.shutdown().await;
}

// ── 3. the control at the door under repair ─────────────────────────────────

/// An ordinary single declaration at the single door commits, unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_single_declaration_still_commits_at_the_single_door() {
    let (_td, h) = bootstrapped_colony().await;

    let outcome = send_single(&h, one_declaration()).await;

    match outcome {
        MutationOutcome::Committed { .. } => {}
        other => panic!("the single form must be untouched by the refusal; got {other:?}"),
    }
    assert!(
        registry_paths(&h).await.iter().any(|p| p == "/grown"),
        "and the node it declared is in the registry"
    );
    h.shutdown().await;
}

// ── 4. the ambiguous body ───────────────────────────────────────────────────

/// A body that carries BOTH forms is refused too — and for the reason the
/// manifest door already gives it: an author who wrote two intentions into one
/// document, where guessing which one wins is how a mutation lands somewhere
/// nobody asked for. The single door used to apply the `diff` half and drop the
/// `manifest` half without a word.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_carrying_both_forms_is_refused_at_the_single_door() {
    let (_td, h) = bootstrapped_colony().await;
    let before = registry_paths(&h).await;

    let mut body = one_declaration();
    body.as_object_mut()
        .unwrap()
        .insert("manifest".into(), json!([one_declaration()]));
    let outcome = send_single(&h, body).await;

    match outcome {
        MutationOutcome::Rejected { id, error_code, .. } => {
            assert_eq!(error_code, "schema");
            assert_eq!(id, None, "spurless, like every other pre-id refusal");
        }
        other => panic!("a body that is both forms at once must be refused; got {other:?}"),
    }
    assert_eq!(
        registry_paths(&h).await,
        before,
        "and NOT half-applied: the `diff` half must not land either"
    );
    h.shutdown().await;
}
