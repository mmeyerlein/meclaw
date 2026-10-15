//! Phase-13.5 Lifecycle-3a Resume/Reconnect (G4): `add_nodes` at an already
//! existing path is a Reconnect/Resume, not an instantiation.
//!
//! Spec § Authority "Instantiation and cell_id stability" (overview Z.170-180):
//! instantiation happens **iff** no cell-directory exists at the target path
//! (fresh cell_id). If the directory exists → Reconnect/Resume: NO new cell_id,
//! `config.json` untouched, `cell.db` resumed (M1).
//!
//! Requirements:
//! - A1: `config.json` is NOT rewritten on Reconnect (config-rewrite is
//!   `swap_nodes`-exclusive = Slice 4). Reconnect = no staging output for the
//!   existing node (directory completely untouched).
//! - A2: existing path with status Awake (running task) → mutation rejected with
//!   error_code `resume_requires_stopped_cell`; registry + `cell.db` + FS stay
//!   untouched.

use meclaw_colony::CellFactoryRegistry;
use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::Uuid;
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::{EchoCellFactory, PersistCellFactory};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
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

/// Writes a root hive only + an `echo` template. The echo cell (stateless →
/// permanently Awake) is brought in by the FIRST `add_nodes` mutation under
/// scope `/`; spec overview Z.331 anchors logical `/a` under the root cell dir
/// `main`, so it lands on disk at `{root}/main/a` — the same anchored FS path
/// the second (Resume) mutation consults via `path_truth`.
fn write_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    let tpl_dir = td.join("templates").join("echo");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"echo"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/a"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

async fn send_mutation(
    h: &ColonyHandle,
    payload: meclaw_core::serde_json::Value,
) -> MutationOutcome {
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

async fn read_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
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
    let reply = ack_rx.await.unwrap();
    reply.entries.into_iter().find(|e| e.path == path)
}

/// A2: `add_nodes` at an existing path whose registry status is `Awake` →
/// `Rejected { error_code: "resume_requires_stopped_cell" }`. The registry
/// entry (cell_id, status) and the live `config.json` stay byte-identical.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_at_awake_path_rejects_resume_requires_stopped_cell() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap must succeed");
    rescan_templates(&h, td.path().join("templates")).await;

    // First add_nodes: instantiate /a fresh (echo is stateless → Awake).
    let first = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "a", "template": "echo"}]}
        }),
    )
    .await;
    assert!(
        matches!(first, MutationOutcome::Committed { .. }),
        "first add_nodes must instantiate /a; got {first:?}"
    );

    let before = read_entry(&h, "/a").await.expect("/a must be registered");
    assert_eq!(before.lifecycle_status, "Awake", "echo cell must be Awake");
    let cell_id_before = before.cell_id.clone();
    // Spec overview Z.331: logical `/a` anchors under the root cell dir `main`
    // → on-disk `{root}/main/a` (root-cell-dir name stripped from logical paths).
    let config_before = std::fs::read_to_string(td.path().join("main/a/config.json")).unwrap();

    // Second add_nodes(name="a", scope="/") targets the EXISTING /a path.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "a", "template": "echo"}]}
        }),
    )
    .await;

    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "resume_requires_stopped_cell",
                "Awake path must reject with resume_requires_stopped_cell"
            );
        }
        other => panic!("expected Rejected, got {other:?}"),
    }

    // Registry entry untouched: same cell_id, still Awake.
    let after = read_entry(&h, "/a")
        .await
        .expect("/a must still be registered after reject");
    assert_eq!(after.cell_id, cell_id_before, "cell_id must be unchanged");
    assert_eq!(
        after.lifecycle_status, "Awake",
        "status must still be Awake"
    );

    // config.json untouched (on-disk `{root}/main/a`, spec overview Z.331).
    let config_after = std::fs::read_to_string(td.path().join("main/a/config.json")).unwrap();
    assert_eq!(
        config_after, config_before,
        "config.json must be byte-identical after reject"
    );

    h.shutdown().await;
}

/// Root hive only + a `persist_mock` template. The stateful cell is brought in
/// by the FIRST `add_nodes` mutation (so it lands at the Mutation-staging root,
/// the same FS-root the second Resume-mutation will consult — consistent FS
/// convention, unlike mixing a `main/`-prefixed bootstrap cell with a
/// prefix-less mutation cell).
fn write_template_only(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    let tpl_dir = td.join("templates").join("persist_mock");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(tpl_dir.join("template.json"), r#"{"name":"persist_mock"}"#).unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// Apply resume (step C): a second `add_nodes` at an existing path whose cell
/// is NOT running (NotYetSpawned) with a populated `cell.db` → mutation
/// **committed**, `cell.db`-state survives (Resume), `cell_id` unchanged,
/// `config.json` byte-identical. The existing registry entry stays; no rename
/// (no ENOTEMPTY), no `registry.remove`, no config rewrite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_at_not_running_path_resumes_committed_db_survives() {
    let td = tempfile::TempDir::new().unwrap();
    write_template_only(td.path());

    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory)],
    );

    let mut reg = CellFactoryRegistry::new();
    reg.insert(
        "persist_mock".into(),
        Arc::new(PersistCellFactory {
            spawn_count: spawn_count.clone(),
        }) as Arc<dyn CellFactory>,
    );
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap must succeed");
    rescan_templates(&h, td.path().join("templates")).await;

    // ── First add_nodes: instantiate /q fresh (NotYetSpawned, never run). ──────
    let first = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "q", "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(first, MutationOutcome::Committed { .. }),
        "first add_nodes must instantiate /q; got {first:?}"
    );

    let before = read_entry(&h, "/q").await.expect("/q must be registered");
    assert_eq!(
        before.lifecycle_status, "NotYetSpawned",
        "stateful cell must be NotYetSpawned (never run) after instantiation"
    );
    let cell_id_before = before.cell_id.clone();

    // Spec overview Z.331: logical `/q` anchors under the root cell dir `main`
    // → on-disk `{root}/main/q` (root-cell-dir name stripped from logical paths).
    let cell_dir = td.path().join("main/q");
    let config_before = std::fs::read_to_string(cell_dir.join("config.json")).unwrap();

    // Simulate a cell that has already run: write a populated cell.db with a
    // known byte-pattern. Resume must NOT touch it (M1 resume, no re-seed).
    let db_path = cell_dir.join("cell.db");
    let db_marker = b"phase-13.5-resume-cell-db-marker-state".to_vec();
    std::fs::write(&db_path, &db_marker).unwrap();

    // ── Second add_nodes at the EXISTING /q path → Reconnect/Resume. ───────────
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "q", "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "Resume at existing non-running path must be Committed; got {outcome:?}"
    );

    // cell_id unchanged (no re-mint).
    let after = read_entry(&h, "/q")
        .await
        .expect("/q must still be registered after resume");
    assert_eq!(
        after.cell_id, cell_id_before,
        "cell_id must be unchanged on Resume"
    );
    assert_eq!(
        after.lifecycle_status, "NotYetSpawned",
        "Resume must not alter the (non-running) lifecycle status in 3a"
    );

    // cell.db survived byte-identically (M1 Resume, no re-seed, no rewrite).
    let db_after = std::fs::read(&db_path).unwrap();
    assert_eq!(
        db_after, db_marker,
        "cell.db must survive Resume byte-identically"
    );

    // config.json byte-identical (A1: no config rewrite on Resume).
    let config_after = std::fs::read_to_string(cell_dir.join("config.json")).unwrap();
    assert_eq!(
        config_after, config_before,
        "config.json must be byte-identical on Resume"
    );

    h.shutdown().await;
}

/// Root hive + two templates of DIFFERENT cell-types (`persist_mock`, `other`).
/// The first instantiates `/q` as a `persist_mock` cell; the second is used to
/// attempt a type-changing Resume at that same path.
fn write_two_type_templates(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    let pm = td.join("templates").join("persist_mock");
    std::fs::create_dir_all(&pm).unwrap();
    std::fs::write(pm.join("template.json"), r#"{"name":"persist_mock"}"#).unwrap();
    std::fs::write(
        pm.join("config.json"),
        r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // A second template whose `cell.type` differs from `persist_mock`. The on-disk
    // `cell.type` is what the F2 resume-type-compat check reads; the factory the
    // type maps to is irrelevant — the mismatch is rejected pre-destructively,
    // before any spawn.
    let other = td.join("templates").join("other_mock");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("template.json"), r#"{"name":"other_mock"}"#).unwrap();
    std::fs::write(
        other.join("config.json"),
        r#"{"cell":{"type":"echo","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// F2-Ruling (Paket-5 T11): single-cell Resume at an existing path whose on-disk
/// `cell.type` (`persist_mock`) differs from the resuming template's `cell.type`
/// (`echo`) → `Rejected { error_code: "resume_type_mismatch" }`. The reject is
/// pre-destructive: registry entry (cell_id, status), `cell.db`, and `config.json`
/// stay byte-identical.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_nodes_at_path_with_other_type_rejects_resume_type_mismatch() {
    let td = tempfile::TempDir::new().unwrap();
    write_two_type_templates(td.path());

    let spawn_count = Arc::new(AtomicU32::new(0));
    let persist_factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: spawn_count.clone(),
    });
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), persist_factory)],
    );

    let mut reg = CellFactoryRegistry::new();
    reg.insert(
        "persist_mock".into(),
        Arc::new(PersistCellFactory {
            spawn_count: spawn_count.clone(),
        }) as Arc<dyn CellFactory>,
    );
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap must succeed");
    rescan_templates(&h, td.path().join("templates")).await;

    // First add_nodes: instantiate /q fresh as a persist_mock cell.
    let first = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "q", "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(first, MutationOutcome::Committed { .. }),
        "first add_nodes must instantiate /q; got {first:?}"
    );

    let before = read_entry(&h, "/q").await.expect("/q must be registered");
    let cell_id_before = before.cell_id.clone();
    let status_before = before.lifecycle_status.clone();

    let cell_dir = td.path().join("main/q");
    let config_before = std::fs::read_to_string(cell_dir.join("config.json")).unwrap();
    let db_path = cell_dir.join("cell.db");
    let db_marker = b"resume-type-mismatch-cell-db-marker".to_vec();
    std::fs::write(&db_path, &db_marker).unwrap();

    // Second add_nodes at the EXISTING /q path, but with the `other_mock`
    // template (cell.type "echo" != on-disk "persist_mock") → type-mismatch.
    let outcome = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "q", "template": "other_mock"}]}
        }),
    )
    .await;

    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "resume_type_mismatch",
                "type-changing Resume must reject with resume_type_mismatch"
            );
        }
        other => panic!("expected Rejected, got {other:?}"),
    }

    // Nothing changed: registry entry, cell.db, config.json all untouched.
    let after = read_entry(&h, "/q")
        .await
        .expect("/q must still be registered after reject");
    assert_eq!(after.cell_id, cell_id_before, "cell_id must be unchanged");
    assert_eq!(
        after.lifecycle_status, status_before,
        "lifecycle_status must be unchanged after reject"
    );
    assert_eq!(
        std::fs::read(&db_path).unwrap(),
        db_marker,
        "cell.db must be byte-identical after reject"
    );
    assert_eq!(
        std::fs::read_to_string(cell_dir.join("config.json")).unwrap(),
        config_before,
        "config.json must be byte-identical after reject"
    );

    h.shutdown().await;
}

// ── GH #292: a Resume does not have to repeat the template's contract ────────

/// Root hive + a `persist_mock` template that DECLARES a required `ctx` key and
/// uses it (`requires.ctx.model`, `${ctx.model}` in its params). The stateful
/// type is what makes the second `add_nodes` a clean Resume: the cell is
/// `NotYetSpawned` after instantiation, so the Awake guard (A2) does not fire.
fn write_requiring_template(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    let tpl_dir = td.join("templates").join("persist_mock");
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(
        tpl_dir.join("template.json"),
        r#"{"name":"persist_mock","requires":{"ctx":{"model":{"type":"string","required":true,
             "because":"the brain this cell infers with"}}}}"#,
    )
    .unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true,"model":"${ctx.model}"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

/// GH #292: the `requires` check refuses an INSTANTIATION that omits a declared
/// key — but a Reconnect/Resume at an existing path instantiates nothing. It
/// stages nothing, substitutes no `${ctx.X}` and rewrites no `config.json` (A1),
/// so it consumes none of the declared keys and must not be refused for them.
///
/// The third mutation is the counter-proof in the same colony: a FRESH name
/// with the same template and the same empty `ctx` is still refused. The
/// exemption belongs to the Resume, not to the check.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_does_not_have_to_repeat_the_templates_required_ctx_keys() {
    let td = tempfile::TempDir::new().unwrap();
    write_requiring_template(td.path());

    let spawn_count = Arc::new(AtomicU32::new(0));
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "persist_mock".to_string(),
            Arc::new(PersistCellFactory {
                spawn_count: spawn_count.clone(),
            }) as Arc<dyn CellFactory>,
        )],
    );
    let mut reg = CellFactoryRegistry::new();
    reg.insert(
        "persist_mock".into(),
        Arc::new(PersistCellFactory {
            spawn_count: spawn_count.clone(),
        }) as Arc<dyn CellFactory>,
    );
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap must succeed");
    rescan_templates(&h, td.path().join("templates")).await;

    // 1. The instantiation supplies the declared key and commits.
    let first = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "q", "template": "persist_mock"}]},
            "ctx": {"model": "some-model"}
        }),
    )
    .await;
    assert!(
        matches!(first, MutationOutcome::Committed { .. }),
        "the instantiation supplies ctx.model and must commit; got {first:?}"
    );
    let before = read_entry(&h, "/q").await.expect("/q must be registered");
    let cell_id_before = before.cell_id.clone();
    let config_before = std::fs::read_to_string(td.path().join("main/q/config.json")).unwrap();

    // 2. The Resume at the same path, with NO ctx at all.
    let resume = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "q", "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(resume, MutationOutcome::Committed { .. }),
        "a Resume consumes none of the declared keys and must commit; got {resume:?}"
    );
    let after = read_entry(&h, "/q")
        .await
        .expect("/q must still be registered after the resume");
    assert_eq!(
        after.cell_id, cell_id_before,
        "the Resume keeps the identity (no re-mint)"
    );
    assert_eq!(
        std::fs::read_to_string(td.path().join("main/q/config.json")).unwrap(),
        config_before,
        "the Resume rewrites no config.json — which is why it needs no ctx"
    );

    // 3. Counter-proof: a fresh instantiation with the same empty ctx is refused.
    let fresh = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "fresh", "template": "persist_mock"}]}
        }),
    )
    .await;
    match &fresh {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "requirement_missing",
            "the exemption belongs to the Resume, not to the check; got {fresh:?}"
        ),
        other => panic!("a fresh instantiation without ctx.model must be refused, got {other:?}"),
    }

    h.shutdown().await;
}
