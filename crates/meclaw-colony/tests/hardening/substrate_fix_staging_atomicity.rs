//! Substrate-fix findings 7+8 — staging atomicity / spurless reject.
//!
//! Spec § Validation (overview Z.279): "the entire diff is rejected, no partial
//! commit, the live tree untouched". A reject must leave NO filesystem residue.
//!
//! Befund 7: two `add_nodes` with the SAME name in one diff used to die as a
//! `schema` (rename-IO) token AND strand the first node's directory — now it
//! rejects in VALIDATE as `naming_collision`, before any FS touch.
//!
//! Befund 8: a spawn reject left the renamed cell directory (and the
//! `.staging/<id>` dir) in `{root}`; the identical follow-up mutation then saw
//! the stranded directory as a resume and committed without spawn/registry.
//! Now a spawn reject sweeps the unregistered staged directories + staging.

use meclaw_colony::{CellFactory, ColonyMsg, ContractView, MutationOutcome, SpawnedCellKind};
use meclaw_core::{CellEmission, JsonValue, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// A factory whose `spawn_cell` always fails — drives the post-rename spawn
/// reject path. `validate_params` passes (the reject must happen at spawn, not
/// validation).
struct FailSpawnFactory;
impl CellFactory for FailSpawnFactory {
    fn validate_params(&self, _: &JsonValue) -> Result<(), String> {
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        _: Path,
        _: JsonValue,
        _: mpsc::Sender<CellEmission>,
        _: std::path::PathBuf,
        _: ContractView,
        _: mpsc::Sender<ColonyMsg>,
        _: Option<std::time::Duration>,
        _: i64,
        _: Option<std::time::Duration>,
        _: Option<Arc<meclaw_colony::DiskBlobStore>>,
        _: usize,
    ) -> Result<SpawnedCellKind, String> {
        Err("spawn always fails (test)".into())
    }
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

fn write_tpl(ws: &std::path::Path, name: &str, cell_type: &str) {
    let tpl = ws.join("templates").join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        format!(
            r#"{{"cell":{{"type":"{cell_type}"}},"params":{{"emitted_target":"/sink"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

fn staging_dir_count(ws: &std::path::Path) -> usize {
    let staging = ws.join(".staging");
    match std::fs::read_dir(&staging) {
        Ok(rd) => rd.flatten().count(),
        Err(_) => 0,
    }
}

/// Befund 7: in-diff duplicate add_nodes → `naming_collision` (not `schema`),
/// rejected in validate before staging, leaving NO residue.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn in_diff_duplicate_rejects_naming_collision_with_no_residue() {
    let td = tempfile::TempDir::new().unwrap();
    write_tpl(td.path(), "echo-tpl", "echo");
    let h = ColonyHandle::new_with_echo_at(td.path());
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [
                {"name": "dup", "template": "echo-tpl"},
                {"name": "dup", "template": "echo-tpl"}
            ]}
        }),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "naming_collision",
            "in-diff duplicate must reject as naming_collision, got {error_code}"
        ),
        other => panic!("expected Rejected{{naming_collision}}, got {other:?}"),
    }

    assert!(
        !td.path().join("dup").exists(),
        "no `dup` directory may be stranded in {{root}} on reject"
    );
    assert_eq!(
        staging_dir_count(td.path()),
        0,
        "no .staging/<id> residue may survive a reject"
    );

    h.shutdown().await;
}

/// Befund 8: a spawn reject must be spurless — NO stranded cell directory, NO
/// `.staging/<id>` residue — so the identical follow-up mutation behaves
/// consistently (rejects again, not commit-as-resume).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawn_reject_is_spurless_and_followup_is_consistent() {
    let td = tempfile::TempDir::new().unwrap();
    write_tpl(td.path(), "fail-tpl", "failer");
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "failer".to_string(),
            Arc::new(FailSpawnFactory) as Arc<dyn CellFactory>,
        )],
    );
    rescan_templates(&h, td.path().join("templates")).await;

    let m = json!({
        "scope": "/",
        "diff": {"add_nodes": [{"name": "f1", "template": "fail-tpl"}]}
    });

    let first = send_mutation(&h, m.clone()).await;
    match &first {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "spawn",
                "spawn failure → spawn reject, got {error_code}"
            )
        }
        other => panic!("expected spawn Rejected, got {other:?}"),
    }
    // Spurless: no stranded final dir, no staging residue.
    assert!(
        !td.path().join("f1").exists(),
        "spawn reject must not strand `f1` in {{root}}"
    );
    assert_eq!(
        staging_dir_count(td.path()),
        0,
        "spawn reject must not leave .staging residue"
    );

    // Follow-up consistency: the identical mutation must NOT commit-as-resume
    // off a stranded directory — it rejects again (spawn still fails).
    let second = send_mutation(&h, m).await;
    assert!(
        matches!(second, MutationOutcome::Rejected { .. }),
        "identical follow-up must reject consistently (no commit-as-resume off a \
         stranded dir), got {second:?}"
    );
    assert!(
        !td.path().join("f1").exists(),
        "still no stranded `f1` after the follow-up reject"
    );
    assert_eq!(staging_dir_count(td.path()), 0, "still no staging residue");

    h.shutdown().await;
}
