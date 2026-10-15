//! GH #676 — the five stateless cells `bash`, `edit`, `file`, `web_fetch`
//! and `web_search` keep their stop wiring when a swap reactivates them.
//!
//! Same class as GH #673: the factory's `RespawnFn` (the closure the
//! reconnect-eager arm invokes when a swap swings back to a parked node)
//! built a bare dispatcher — `colony_inbox = None`, `stop_tx`/`death_ack_rx`
//! dropped, no `renotify_stop_wiring` — so the reactivated node could not be
//! stopped again until a restart, and the second swing forward was refused
//! with `disconnect of Awake cell … without live stop-wiring (interim guard)`.
//! #673 fixed `code`; the same closure stood in these five.
//!
//! Lives in `meclaw-cells/tests/` (like the #673 test) because the claim needs
//! the real factories, and `meclaw-colony` cannot depend on the crate that
//! holds them.
//!
//! Two layers of proof, one `#[test]` per cell type each, so a red case names
//! the type:
//!   1. Full colony: swap forward, swing back, swap forward again — all three
//!      commit in one colony lifetime, no restart. The third mutation is the
//!      discriminator: without the fix it is rejected by the interim guard.
//!   2. Narrow: `spawn_cell`'s `RespawnFn`, invoked the way the reconnect-eager
//!      arm invokes it, puts `ColonyMsg::StopWiringRestored` on the colony
//!      inbox for the cell's own path.
//!
//! None of the five is ever sent a message: `web_fetch`/`web_search` build a
//! client but never call out; `web_search` gets a loopback endpoint and no key.

use meclaw_cells::{
    BashCellFactory, EditCellFactory, FileCellFactory, WebFetchCellFactory, WebSearchCellFactory,
};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ContractView, MutationOutcome, SpawnedCellKind,
    bootstrap_from_filesystem, set_term_timeout_ms_for_test,
};
use meclaw_core::{CellEmission, JsonValue, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

/// The spawn params of one of the five, given a directory the `file`/`edit`
/// factories may use as their (existing, absolute) `base_path`.
fn params_for(cell_type: &str, base_dir: &std::path::Path) -> JsonValue {
    match cell_type {
        "bash" => json!({}),
        "edit" | "file" => json!({"base_path": base_dir.to_str().unwrap()}),
        "web_fetch" => json!({}),
        // Never called: the swap path spawns, stops and respawns the
        // dispatcher; no message reaches the cell. A loopback endpoint keeps
        // the params honest without any network.
        "web_search" => json!({"endpoint": "http://127.0.0.1:9/search"}),
        other => panic!("no params for cell type {other}"),
    }
}

fn factory_for(cell_type: &str) -> Arc<dyn CellFactory> {
    match cell_type {
        "bash" => Arc::new(BashCellFactory),
        "edit" => Arc::new(EditCellFactory),
        "file" => Arc::new(FileCellFactory),
        "web_fetch" => Arc::new(WebFetchCellFactory),
        "web_search" => Arc::new(WebSearchCellFactory),
        other => panic!("no factory for cell type {other}"),
    }
}

fn leaf_config(cell_type: &str, base_dir: &std::path::Path) -> String {
    json!({
        "cell": {"type": cell_type},
        "params": params_for(cell_type, base_dir),
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    })
    .to_string()
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// A single-cell template `name` of `cell_type` under `<root>/templates`.
fn write_leaf_template(
    root: &std::path::Path,
    name: &str,
    cell_type: &str,
    base_dir: &std::path::Path,
) {
    let tpl = root.join("templates").join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(tpl.join("config.json"), leaf_config(cell_type, base_dir)).unwrap();
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

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
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

async fn lifecycle_status(h: &ColonyHandle, path: &str) -> Option<(bool, String)> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadRegistryReply>();
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
        .map(|e| (e.active, e.lifecycle_status))
}

fn swap(matched: &str, with: JsonValue) -> JsonValue {
    json!({
        "scope": "/",
        "diff": {"swap_nodes": [
            {"match": {"name": matched}, "with": with}
        ]}
    })
}

fn assert_committed(outcome: &MutationOutcome, what: &str) {
    match outcome {
        MutationOutcome::Committed { .. } => {}
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            panic!("{what} must commit; got Rejected({error_code}): {details}")
        }
    }
}

/// Full-colony proof for one cell type: a leaf swings forward, back, and
/// forward again in one colony lifetime. The third mutation is the
/// discriminator.
async fn a_reactivated_cell_can_be_swapped_away_again(cell_type: &str) {
    // Generous death-ack term-timeout: the asserted defect is the interim
    // guard's reject, not a timeout, so this cannot mask the bug under test.
    set_term_timeout_ms_for_test(30_000);

    let td = TempDir::new().unwrap();
    // `file`/`edit` need an existing absolute base_path; one shared work dir.
    let work = td.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    // Root hive carries the single edge /t1 -> /c1; both leaves are `cell_type`.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/t1","to":"/c1"}
        ]}}}"#,
    );
    write(
        td.path(),
        "main/t1/config.json",
        &leaf_config(cell_type, &work),
    );
    write(
        td.path(),
        "main/c1/config.json",
        &leaf_config(cell_type, &work),
    );
    // The successor the first swap instantiates.
    let successor = format!("c2_{cell_type}");
    write_leaf_template(td.path(), &successor, cell_type, &work);

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(cell_type.to_string(), factory_for(cell_type))],
    );
    rescan_templates(&h, td.path().join("templates")).await;

    let mut registry = CellFactoryRegistry::new();
    registry.insert(cell_type.into(), factory_for(cell_type));
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap with t1->c1 succeeds");

    let (active, status) = lifecycle_status(&h, "/c1").await.expect("/c1 registered");
    assert!(
        active && status == "Awake",
        "[{cell_type}] /c1 boots active + Awake; got {status}"
    );

    // 1. Forward (instantiate form, new name): c1 -> c2. Peace-stops c1.
    let out1 = send_mutation(
        &h,
        swap(
            "c1",
            json!({"template": successor, "name": "c2", "params": {}}),
        ),
    )
    .await;
    assert_committed(&out1, &format!("[{cell_type}] first swap (c1 -> c2)"));
    let (active, status) = lifecycle_status(&h, "/c1").await.expect("/c1 retained");
    assert!(!active, "[{cell_type}] /c1 inactive after the first swap");
    assert_eq!(
        status, "NotYetSpawned",
        "[{cell_type}] /c1 parked after its peace-stop"
    );
    let (active, _) = lifecycle_status(&h, "/c2").await.expect("/c2 registered");
    assert!(active, "[{cell_type}] /c2 active after the first swap");

    // 2. Back (existing-node form): c2 -> c1. Reactivates c1 via its RespawnFn.
    let out2 = send_mutation(&h, swap("c2", json!({"name": "c1"}))).await;
    assert_committed(&out2, &format!("[{cell_type}] swing back (c2 -> c1)"));
    let (active, status) = lifecycle_status(&h, "/c1").await.expect("/c1 retained");
    assert!(
        active && status == "Awake",
        "[{cell_type}] /c1 reactivated + Awake; got {status}"
    );
    let (active, _) = lifecycle_status(&h, "/c2").await.expect("/c2 retained");
    assert!(!active, "[{cell_type}] /c2 inactive after the swing back");

    // 3. Forward again (existing-node form): c1 -> c2. THE discriminator:
    // the reactivated c1 must carry live stop wiring, or the interim guard
    // rejects the disconnect and the node is stuck until a restart.
    let out3 = send_mutation(&h, swap("c1", json!({"name": "c2"}))).await;
    assert_committed(
        &out3,
        &format!("[{cell_type}] second swap forward (c1 -> c2) after a swing back"),
    );
    let (active, status) = lifecycle_status(&h, "/c1").await.expect("/c1 retained");
    assert!(
        !active,
        "[{cell_type}] /c1 inactive after the second swap forward"
    );
    assert_eq!(
        status, "NotYetSpawned",
        "[{cell_type}] /c1 parked again after its second peace-stop"
    );
    let (active, _) = lifecycle_status(&h, "/c2").await.expect("/c2 retained");
    assert!(
        active,
        "[{cell_type}] /c2 active after the second swap forward"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reactivated_bash_cell_can_be_swapped_away_again() {
    a_reactivated_cell_can_be_swapped_away_again("bash").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reactivated_edit_cell_can_be_swapped_away_again() {
    a_reactivated_cell_can_be_swapped_away_again("edit").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reactivated_file_cell_can_be_swapped_away_again() {
    a_reactivated_cell_can_be_swapped_away_again("file").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reactivated_web_fetch_cell_can_be_swapped_away_again() {
    a_reactivated_cell_can_be_swapped_away_again("web_fetch").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reactivated_web_search_cell_can_be_swapped_away_again() {
    a_reactivated_cell_can_be_swapped_away_again("web_search").await;
}

/// Narrow proof for one factory: `spawn_cell`'s `RespawnFn` re-notifies the
/// colony of the fresh stop pair — the same receipt `lr_renotify_stop_wiring.rs`
/// reads for the long-running factories.
async fn spawn_cell_respawn_renotifies_stop_wiring(cell_type: &str) {
    let td = TempDir::new().unwrap();
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, mut inbox_rx) = mpsc::channel::<ColonyMsg>(8);

    let spawned = factory_for(cell_type)
        .spawn_cell(
            Path::new("/c1"),
            params_for(cell_type, td.path()),
            out_tx,
            td.path().to_path_buf(),
            ContractView::default(),
            inbox_tx,
            None,
            -1,
            None,
            None,
            1000,
        )
        .unwrap_or_else(|e| panic!("a {cell_type} cell spawns: {e}"));
    let SpawnedCellKind::Active {
        sender,
        join,
        respawn,
        ..
    } = spawned
    else {
        panic!("a {cell_type} cell spawns Active")
    };
    // Retire the initial dispatcher; the respawn is what is under test.
    drop(sender);
    let _ = tokio::time::timeout(Duration::from_secs(30), join).await;

    // Invoke the respawn closure (exactly what the reconnect-eager arm does).
    let (sender, join, _peace_rx, _backstop_rx) = respawn();

    // The re-notify is not ordered against the task the closure just spawned,
    // so drain until the positive receipt shows up (GH #128 discipline).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let restored_path = loop {
        let msg = tokio::time::timeout_at(deadline, inbox_rx.recv())
            .await
            .unwrap_or_else(|_| {
                panic!("no StopWiringRestored from the {cell_type} respawn closure within 30s")
            })
            .unwrap_or_else(|| {
                panic!("colony inbox closed before StopWiringRestored ({cell_type})")
            });
        if let ColonyMsg::StopWiringRestored { path, .. } = msg {
            break path;
        }
    };
    assert_eq!(
        restored_path.as_str(),
        "/c1",
        "[{cell_type}] re-notify must carry the cell's own path"
    );

    drop(sender);
    let _ = tokio::time::timeout(Duration::from_secs(30), join).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bash_spawn_cell_respawn_renotifies_stop_wiring() {
    spawn_cell_respawn_renotifies_stop_wiring("bash").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edit_spawn_cell_respawn_renotifies_stop_wiring() {
    spawn_cell_respawn_renotifies_stop_wiring("edit").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_spawn_cell_respawn_renotifies_stop_wiring() {
    spawn_cell_respawn_renotifies_stop_wiring("file").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web_fetch_spawn_cell_respawn_renotifies_stop_wiring() {
    spawn_cell_respawn_renotifies_stop_wiring("web_fetch").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web_search_spawn_cell_respawn_renotifies_stop_wiring() {
    spawn_cell_respawn_renotifies_stop_wiring("web_search").await;
}
