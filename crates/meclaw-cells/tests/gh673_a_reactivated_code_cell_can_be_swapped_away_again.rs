//! GH #673 — a `code` cell reactivated by a swap keeps its stop wiring.
//!
//! Measured: a `code` leaf was swapped away (instantiate form, new name),
//! swung back (existing-node form, `with.name` only) and then refused a
//! second swing forward with
//! `disconnect of Awake cell … without live stop-wiring (interim guard)`.
//! The first swap peace-stops the predecessor and parks it `NotYetSpawned`;
//! the swing back reactivates it through the factory's `RespawnFn`. That
//! closure built a bare dispatcher — `stop_tx`/`death_ack_rx` dropped, no
//! `renotify_stop_wiring` — so the reactivated node could not be stopped
//! again until a restart. The stateful factories re-notify; `code` did not
//! (nor did the five other stateless cells — GH #676).
//!
//! Lives in `meclaw-cells/tests/` (like `lr_renotify_stop_wiring.rs`) because
//! the claim needs the real `CodeCellFactory`, and `meclaw-colony` cannot
//! depend on the crate that holds it.
//!
//! Two layers of proof:
//!   1. Full colony: swap forward, swing back, swap forward again — all three
//!      commit in one colony lifetime, no restart. The third mutation is the
//!      discriminator: without the fix it is rejected by the interim guard.
//!   2. Narrow: `spawn_cell`'s `RespawnFn`, invoked the way the reconnect-eager
//!      arm invokes it, puts `ColonyMsg::StopWiringRestored` on the colony
//!      inbox for the cell's own path.

use meclaw_cells::code::CodeCellFactory;
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

/// A `code` script that never has to run: the swap path spawns, stops and
/// respawns the dispatcher; no message reaches the runner.
const SCRIPT: &str =
    "import sys, json; json.load(sys.stdin); sys.stdout.write(json.dumps({\"messages\": []}))";

fn code_config() -> String {
    json!({
        "cell": {"type": "code"},
        "params": {
            "runner": "python3",
            "script_inline": SCRIPT,
            "external_timeout_ms": 10000
        },
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    })
    .to_string()
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// A single-cell `code` template `name` under `<root>/templates`.
fn write_code_template(root: &std::path::Path, name: &str) {
    let tpl = root.join("templates").join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(tpl.join("config.json"), code_config()).unwrap();
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

/// Full-colony proof: a `code` leaf swings forward, back, and forward again in
/// one colony lifetime. The third mutation is the discriminator.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reactivated_code_cell_can_be_swapped_away_again() {
    // Generous death-ack term-timeout: the asserted defect is the interim
    // guard's reject, not a timeout, so this cannot mask the bug under test.
    set_term_timeout_ms_for_test(30_000);

    let td = TempDir::new().unwrap();
    // Root hive carries the single edge /t1 -> /c1; both leaves are `code`.
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/t1","to":"/c1"}
        ]}}}"#,
    );
    write(td.path(), "main/t1/config.json", &code_config());
    write(td.path(), "main/c1/config.json", &code_config());
    // The successor the first swap instantiates.
    write_code_template(td.path(), "c2_code");

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )],
    );
    rescan_templates(&h, td.path().join("templates")).await;

    let mut registry = CellFactoryRegistry::new();
    registry.insert(
        "code".into(),
        Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
    );
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap with t1->c1 succeeds");

    let (active, status) = lifecycle_status(&h, "/c1").await.expect("/c1 registered");
    assert!(
        active && status == "Awake",
        "/c1 boots active + Awake; got {status}"
    );

    // 1. Forward (instantiate form, new name): c1 -> c2. Peace-stops c1.
    let out1 = send_mutation(
        &h,
        swap(
            "c1",
            json!({"template": "c2_code", "name": "c2", "params": {}}),
        ),
    )
    .await;
    assert_committed(&out1, "first swap (c1 -> c2)");
    let (active, status) = lifecycle_status(&h, "/c1").await.expect("/c1 retained");
    assert!(!active, "/c1 inactive after the first swap");
    assert_eq!(status, "NotYetSpawned", "/c1 parked after its peace-stop");
    let (active, _) = lifecycle_status(&h, "/c2").await.expect("/c2 registered");
    assert!(active, "/c2 active after the first swap");

    // 2. Back (existing-node form): c2 -> c1. Reactivates c1 via its RespawnFn.
    let out2 = send_mutation(&h, swap("c2", json!({"name": "c1"}))).await;
    assert_committed(&out2, "swing back (c2 -> c1)");
    let (active, status) = lifecycle_status(&h, "/c1").await.expect("/c1 retained");
    assert!(
        active && status == "Awake",
        "/c1 reactivated + Awake; got {status}"
    );
    let (active, _) = lifecycle_status(&h, "/c2").await.expect("/c2 retained");
    assert!(!active, "/c2 inactive after the swing back");

    // 3. Forward again (existing-node form): c1 -> c2. THE discriminator:
    // the reactivated c1 must carry live stop wiring, or the interim guard
    // rejects the disconnect and the node is stuck until a restart.
    let out3 = send_mutation(&h, swap("c1", json!({"name": "c2"}))).await;
    assert_committed(&out3, "second swap forward (c1 -> c2) after a swing back");
    let (active, status) = lifecycle_status(&h, "/c1").await.expect("/c1 retained");
    assert!(!active, "/c1 inactive after the second swap forward");
    assert_eq!(
        status, "NotYetSpawned",
        "/c1 parked again after its second peace-stop"
    );
    let (active, _) = lifecycle_status(&h, "/c2").await.expect("/c2 retained");
    assert!(active, "/c2 active after the second swap forward");

    h.shutdown().await;
}

/// Narrow proof: `spawn_cell`'s `RespawnFn` re-notifies the colony of the fresh
/// stop pair — the same receipt `lr_renotify_stop_wiring.rs` reads for the
/// long-running factories.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn code_spawn_cell_respawn_renotifies_stop_wiring() {
    let td = TempDir::new().unwrap();
    let (out_tx, _out_rx) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, mut inbox_rx) = mpsc::channel::<ColonyMsg>(8);

    let spawned = Arc::new(CodeCellFactory)
        .spawn_cell(
            Path::new("/c1"),
            json!({"runner": "python3", "script_inline": SCRIPT}),
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
        .expect("a code cell spawns");
    let SpawnedCellKind::Active {
        sender,
        join,
        respawn,
        ..
    } = spawned
    else {
        panic!("a code cell spawns Active")
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
                panic!("no StopWiringRestored from the code respawn closure within 30s")
            })
            .unwrap_or_else(|| panic!("colony inbox closed before StopWiringRestored"));
        if let ColonyMsg::StopWiringRestored { path, .. } = msg {
            break path;
        }
    };
    assert_eq!(
        restored_path.as_str(),
        "/c1",
        "re-notify must carry the cell's own path"
    );

    drop(sender);
    let _ = tokio::time::timeout(Duration::from_secs(30), join).await;
}
