//! GH #437 — the composition case the issue was actually found in: a colony
//! grown from `ref` markers, whose long-poll consumer must not start when the
//! grown colony first boots (the original is still running and owns the
//! upstream).
//!
//! A FINDING, documented here because it cost the lane a task: unlike the HTTP
//! door, the manifest form and `--apply` — which all funnel the untyped diff
//! into the same `handle_mutation` and therefore inherited `add_nodes[].birth`
//! without a line of code — the `ref` marker does NOT go through `add_nodes` at
//! all. It is planned as a growth (`bootstrap::plan_growth`), enters the
//! mutation machinery only at `subtree::stage_subtree`, and the tree it grows
//! is registered by the BOOT, not by the mutation spawn loop. "All doors
//! inherit it" was true for three of four; the fourth needed its own wiring.
//!
//! The marker declares the state TOP-LEVEL beside `override_params` and not
//! inside the `cell` block, for the reason `docs/config.md` § Spezialfall
//! already gives for `override_params`: the `cell` block describes a CELL and
//! its key list is closed, while a birth state describes an INSTANTIATION
//! ORDER.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, RegistryOverlay, bootstrap_from_filesystem,
    plan_bootstrap,
};
use meclaw_core::serde_json::json;
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

const HIVE: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;
const CELL: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn factory() -> Arc<dyn CellFactory> {
    Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    }) as Arc<dyn CellFactory>
}

/// A two-cell composite template `unit` plus a root tree whose `/os/unit` is a
/// `ref` marker at it. `birth` is written top-level when `birth` is `Some`.
fn setup(root: &std::path::Path, birth: Option<&str>) {
    write(root, "templates/unit/template.json", r#"{"name":"unit"}"#);
    write(root, "templates/unit/config.json", HIVE);
    write(root, "templates/unit/poll/config.json", CELL);
    write(root, "templates/unit/work/config.json", CELL);

    write(root, "main/config.json", HIVE);
    write(root, "main/os/config.json", HIVE);
    let marker = match birth {
        Some(b) => json!({"cell": {"type": "ref", "template": "unit"}, "birth": b}),
        None => json!({"cell": {"type": "ref", "template": "unit"}}),
    };
    write(
        root,
        "main/os/unit/config.json",
        &meclaw_core::serde_json::to_string(&marker).unwrap(),
    );
}

#[allow(clippy::result_large_err)]
async fn try_boot(td: &tempfile::TempDir) -> Result<ColonyHandle, (ColonyHandle, String)> {
    let f = factory();
    let h = ColonyHandle::new_with_factories_at(td, vec![("persist_mock".to_string(), f.clone())]);
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
    let mut reg = CellFactoryRegistry::new();
    reg.insert("persist_mock".into(), f);
    match bootstrap_from_filesystem(td.path(), &reg, &h.runtime()).await {
        Ok(_) => Ok(h),
        Err(e) => Err((h, format!("{e:?}"))),
    }
}

async fn registry_active(h: &ColonyHandle, path: &str) -> Option<bool> {
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

/// What `colony.db` says — the answer the NEXT boot reads.
fn registry_status_in_db(root: &std::path::Path, path: &str) -> String {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("open colony.db");
    conn.query_row("SELECT status FROM registry WHERE path = ?1", [path], |r| {
        r.get::<_, String>(0)
    })
    .unwrap_or_else(|e| panic!("no registry row for {path}: {e}"))
}

/// The tree GREW and it is registered — and it is registered INACTIVE, in RAM
/// and in the row, so the next boot agrees.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ref_marker_declaring_birth_inactive_grows_a_tree_that_does_not_run() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path(), Some("inactive"));
    let h = match try_boot(&td).await {
        Ok(h) => h,
        Err((h, e)) => {
            h.shutdown().await;
            panic!("the boot must succeed: {e}");
        }
    };

    // The marker consumed itself: the referenced template's content stands there.
    assert!(
        td.path().join("main/os/unit/poll/config.json").exists(),
        "the growth must have happened"
    );

    for p in ["/os/unit/poll", "/os/unit/work"] {
        let active = registry_active(&h, p)
            .await
            .unwrap_or_else(|| panic!("{p} must be registered"));
        assert!(!active, "{p} must boot inactive");
    }

    h.shutdown().await;
    for p in ["/os/unit/poll", "/os/unit/work"] {
        assert_eq!(
            registry_status_in_db(td.path(), p),
            "inactive",
            "{p}'s row must say inactive, or the next boot starts it"
        );
    }
}

/// The default is unchanged: a marker that declares nothing grows a tree that
/// behaves exactly as it did before this key existed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ref_marker_without_a_birth_declaration_still_runs() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path(), None);
    let h = match try_boot(&td).await {
        Ok(h) => h,
        Err((h, e)) => {
            h.shutdown().await;
            panic!("the boot must succeed: {e}");
        }
    };
    assert_eq!(
        registry_active(&h, "/os/unit/poll").await,
        Some(true),
        "the shipped default must still boot active"
    );
    h.shutdown().await;
}

/// An unknown value on the marker is refused the same way an unknown value in
/// a diff is — and it is refused at PLAN time, so a boot that cannot fulfil a
/// declaration never starts half a tree.
#[test]
fn a_ref_marker_with_an_unknown_birth_value_refuses_the_boot() {
    let td = tempfile::TempDir::new().unwrap();
    setup(td.path(), Some("asleep"));
    let errs = plan_bootstrap(
        td.path(),
        &CellFactoryRegistry::new(),
        &RegistryOverlay::new(),
    )
    .expect_err("a marker with an unknown birth value must not boot");
    let err = format!("{errs:?}");
    assert!(
        err.contains("birth"),
        "the refusal must name the key: {err}"
    );
    assert!(
        err.contains("asleep"),
        "the refusal must quote the offending value: {err}"
    );
    assert!(
        err.contains("active") && err.contains("inactive"),
        "the refusal must list the values that DO exist: {err}"
    );
}
