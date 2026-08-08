//! Deep-Audit F2 (a) — mid-rename strict-fail end-to-end.
//!
//! Befund (verifiziert): a `rename(2)` failing AFTER an earlier one already
//! landed used to surface as `MutationError::Schema` → the call-site mislabelled
//! it as a clean pre-destructive `Rejected{live tree untouched}`, while renames
//! 1..i already stood in the live tree. Now the rename path returns
//! `LiveTreeMutated` and `colony::handle_mutation` PANICS (strict-fail) instead of
//! lying — the half-state surfaces on the next boot as orphan dirs (see
//! `substrate_fix_mid_rename_boot_orphan.rs`), never silently adopted.
//!
//! Isolated in its own test binary: the `FAIL_RENAME_AT` static from
//! `mutation::hook` is binary-local — a hook set in this binary must not hit
//! foreign `handle_mutation` calls (same argument as
//! `phase_6_crash_recovery.rs`). Only active under `feature = "test-hooks"`.

#![cfg(feature = "test-hooks")]

use meclaw_colony::{ColonyMsg, mutation::hook};
use meclaw_core::{Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use tokio::sync::oneshot;

fn write_tpl(ws: &std::path::Path, name: &str, cell_type: &str) {
    let tpl = ws.join("templates").join(name);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        format!(
            r#"{{"cell":{{"type":"{cell_type}"}},"params":{{"echo_to":"/sink"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
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
    ack_rx.await.unwrap();
}

/// A two-`add_nodes` mutation whose SECOND rename is injected to fail after the
/// first already committed → `colony_task` strict-fails (panics), and node `a`
/// stands partially in the live tree (audit-model, no rollback).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mid_rename_failure_panics_and_leaves_partial_live_tree() {
    let td = tempfile::TempDir::new().unwrap();
    write_tpl(td.path(), "echo-tpl", "echo");
    let h = ColonyHandle::new_with_echo_at(td.path());
    rescan_templates(&h, td.path().join("templates")).await;

    // Inject: fail the rename loop when its committed counter reaches 1 (i.e. the
    // SECOND node), after the first node already landed.
    hook::set_fail_rename_at(1);

    // Send the mutation. Do NOT await the ack — the colony panics before replying.
    let (ack_tx, _ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [
                    {"name": "a", "template": "echo-tpl"},
                    {"name": "b", "template": "echo-tpl"}
                ]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();

    let root = td.path().to_path_buf();
    let join = tokio::time::timeout(std::time::Duration::from_secs(30), h.join_result())
        .await
        .expect("colony_task must finish (panic), not hang");

    // 1. The colony PANICKED (strict-fail) — not a clean return.
    let err = join.expect_err("mid-rename failure must PANIC the colony_task, not return cleanly");
    assert!(
        err.is_panic(),
        "colony_task must die by panic (strict-fail), got {err:?}"
    );

    // 2. Audit-model: the first node's rename already landed in the live tree.
    assert!(
        root.join("a").join("config.json").exists(),
        "node `a` (committed before the injected failure) must stand in the live tree"
    );
    // The second node never renamed in.
    assert!(
        !root.join("b").exists(),
        "node `b` (the failing rename) must NOT be in the live tree"
    );

    hook::clear_fail_rename();
}
