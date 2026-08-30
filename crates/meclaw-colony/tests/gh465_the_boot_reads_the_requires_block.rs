//! GH #465 — the first boot owes a `requires` block the same answer the
//! mutation door gives, and it owes it BEFORE it writes.
//!
//! WHAT THIS FILE IS
//! =================
//! A template's `requires.ctx` / `requires.env` block is a contract (GH #292):
//! a mutation that names a template without supplying a declared key is refused
//! `requirement_missing`, pre-staging, so nothing half-lands and the refusal
//! names what to go and fix.
//!
//! GH #424 gave the boot a second instantiating path — a `cell.type: "ref"`
//! marker the first start fulfils — and that path did not read the block. Two
//! consequences, and neither of them was visible:
//!
//! * a marker naming a template whose values substitute a **plain** `${VAR}`
//!   was staged, and the refusal arrived from the middle of the substitution as
//!   `env_var_missing`, from a tree already built under `.staging/`;
//! * a marker naming a template that DECLARES a key without substituting it
//!   itself — the roll-up case, where the values live one ref deeper — was
//!   grown clean, and the omission surfaced at the first turn, or never.
//!
//! So `grow_one` now runs `validate_requires`, the door's own stage 3, against
//! the marker read as the one-entry diff it is. Same function, same walk over
//! the ref chain, same wording. What is measured here is the boundary: the
//! refusal has the door's code, it quotes the declaration's `because`, and it
//! happens before the first byte.
//!
//! The end-to-end case on the SHIPPED shell is
//! `crates/meclaw-cells/tests/gh465_one_declaration_boots_the_os.rs`; this file
//! plants its own templates so the mechanism is provable without a library.

use meclaw_colony::ColonyMsg;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

const HIVE: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

/// Write a `config.json` (or any file) under `{root}/{rel}`.
fn write(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn ref_marker(reference: &str) -> String {
    format!(r#"{{"cell":{{"type":"ref","template":"{reference}"}}}}"#)
}

fn factory() -> Arc<dyn CellFactory> {
    Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    })
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

async fn boot_error(td: &tempfile::TempDir) -> String {
    match try_boot(td).await {
        Ok(h) => {
            h.shutdown().await;
            panic!("this tree must not boot")
        }
        Err((h, e)) => {
            h.shutdown().await;
            e
        }
    }
}

async fn boots(td: &tempfile::TempDir) {
    match try_boot(td).await {
        Ok(h) => h.shutdown().await,
        Err((h, e)) => {
            h.shutdown().await;
            panic!("the tree must boot: {e}")
        }
    }
}

/// A colony root: `colony.json`, an empty root hive, and a marker at `/x`.
fn plant(td: &tempfile::TempDir, template: &str, reference: &str, env: &str) {
    let root = td.path();
    write(root, "colony.json", r#"{"schema_version":1}"#);
    write(root, "main/config.json", HIVE);
    write(root, "main/x/config.json", &ref_marker(reference));
    write(root, "templates/needy/template.json", template);
    write(
        root,
        "templates/needy/config.json",
        r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    std::fs::write(root.join(".env"), env).unwrap();
}

/// The declaration this file leans on: one required env key with a `because`.
const NEEDY: &str = r#"{"name":"needy","version":"1.0.0","requires":{"env":{"NEEDED_KEY":{"type":"string","required":true,"because":"the reason a reader is owed"}}}}"#;

// ──────────────────────────────────────────────────────────────────────────────

/// The boot refuses a marker whose declaration the colony cannot satisfy —
/// with the door's own code and the declaration's own sentence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declared_env_key_the_colony_does_not_hold_refuses_the_boot() {
    let td = tempfile::TempDir::new().unwrap();
    plant(&td, NEEDY, "needy@1.0.0", "SOMETHING_ELSE=1\n");
    let err = boot_error(&td).await;
    assert!(
        err.contains("requirement_missing"),
        "the boot must answer with the mutation door's code: {err}"
    );
    assert!(
        err.contains("NEEDED_KEY"),
        "the refusal must name the key: {err}"
    );
    assert!(
        err.contains("the reason a reader is owed"),
        "the refusal must quote the declaration's `because` — the sentence is the whole \
         difference between a name and an explanation: {err}"
    );
}

/// And it refuses BEFORE it writes: the marker is still a marker.
///
/// The `.staging/` half matters as much as the tree: a refusal that leaves a
/// staged subtree behind is a refusal the next boot may resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_refusal_leaves_the_marker_and_writes_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    plant(&td, NEEDY, "needy@1.0.0", "");
    let _ = boot_error(&td).await;

    let marker_dir = td.path().join("main").join("x");
    let cfg: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(marker_dir.join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        cfg["cell"]["type"], "ref",
        "the marker must be untouched — the growth was refused, not half-applied"
    );
    let children: Vec<String> = std::fs::read_dir(&marker_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(children, vec!["config.json".to_string()]);

    let staging = td.path().join(".staging");
    let staged: Vec<String> = std::fs::read_dir(&staging)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        staged.is_empty(),
        "the check runs before `stage_subtree`, so nothing may have been staged: {staged:?}"
    );
}

/// With the key present the same tree grows, unchanged.
///
/// Anti-vacuity: without this, a check that refused every growth would satisfy
/// both tests above.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_tree_grows_once_the_key_is_there() {
    let td = tempfile::TempDir::new().unwrap();
    plant(&td, NEEDY, "needy@1.0.0", "NEEDED_KEY=a-value\n");
    boots(&td).await;
    let cfg: meclaw_core::serde_json::Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(td.path().join("main/x/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        cfg["cell"]["type"], "persist_mock",
        "the marker must have been fulfilled"
    );
}

/// A template that declares nothing is untouched by the check — the boot of
/// every tree that worked before GH #465 works after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_template_without_a_requires_block_grows_as_it_always_did() {
    let td = tempfile::TempDir::new().unwrap();
    plant(
        &td,
        r#"{"name":"needy","version":"1.0.0"}"#,
        "needy@1.0.0",
        "",
    );
    boots(&td).await;
}
