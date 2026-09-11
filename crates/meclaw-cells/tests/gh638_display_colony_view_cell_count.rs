//! `examples/display-colony-view` — the cell count of the example, measured.
//!
//! `examples/README.md` states one number per example, and for every other
//! colony in that table the number comes from a test that boots the shipped
//! seed and applies the shipped declaration. This one was counted by hand
//! (GH #638), which is how it survived `colony-view@1.1.0` dropping its refresh
//! timer with the table still naming the old total.
//!
//! So the same construction as `meclaw_os_example.rs`: the shipped seed and the
//! shipped `grow.json` verbatim — no inlined copy, no paraphrase — booted,
//! grown, and the registry read back. What is checked in is two cells, and the
//! three templates the declaration names bring six more.
//!
//! There is no model in this colony, so the run needs no provider and spends
//! nothing, and since `web@2.0.0` nothing of the declaration has to be bent
//! either: the screen names a mount inside this colony's own listener rather
//! than a fixed port a second run on the same machine would collide with.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! example or the library is skipped, never judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::Value;
use meclaw_testing::ColonyHandle;
use std::sync::Arc;

/// The two producers the example checks in: the timer and the `code` cell that
/// writes a paragraph. The root hive is a scope marker and is not a cell.
const CELLS_CHECKED_IN: usize = 2;

/// Plus six from the three templates `grow.json` names: three from
/// `display@2.0.1` (the `web` cell, the composer and the view store), two from
/// `colony-view@1.1.0` (the probe and the layout `code` cell — its `refresh`
/// timer left with 1.1.0, GH #553), one from `terminal@1`. MEASURED.
const CELLS_AFTER_GROW: usize = 8;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn example(rel: &str) -> std::path::PathBuf {
    repo("examples/display-colony-view").join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Copy a directory tree verbatim — template seed data travels too.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("web".to_string(), Arc::new(WebCellFactory::default())),
    ]
}

async fn registry_paths(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .expect("read registry");
    let mut v: Vec<String> = ack_rx
        .await
        .expect("registry ack")
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect();
    v.sort();
    v
}

/// The number in `examples/README.md`, measured rather than counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_example_grows_from_two_cells_to_eight() {
    if !example("grow.json").is_file() || !repo("templates/colony-view").is_dir() {
        return;
    }

    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    copy_tree(&example("seed"), root);
    for name in ["display", "colony-view", "terminal", "web"] {
        copy_tree(
            &repo(&format!("templates/{name}")),
            &root.join("templates").join(name),
        );
    }

    // The declaration is run exactly as it ships: since `web@2.0.0` the screen
    // names a MOUNT rather than a port, and a name inside this colony's own
    // listener cannot be taken by a second run on the same machine.
    let grow = read_json(&example("grow.json"));

    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(root, &registry, &h.runtime())
        .await
        .expect("the seed of examples/display-colony-view must boot");

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("GH #440: the rescan must not have aborted");

    let before = registry_paths(&h).await;
    assert_eq!(
        before.len(),
        CELLS_CHECKED_IN,
        "the checked-in half of the example moved: {before:?}"
    );

    for entry in grow["manifest"]
        .as_array()
        .expect("the shipped manifest")
        .clone()
    {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        h.inbox_tx
            .send(ColonyMsg::Mutation {
                payload: entry,
                reply_to: None,
                trace_id: meclaw_core::Uuid::now_v7(),
                parent_message_id: meclaw_core::Uuid::now_v7(),
                ack: ack_tx,
            })
            .await
            .expect("send mutation");
        let outcome = ack_rx.await.expect("mutation ack");
        assert!(
            matches!(
                outcome,
                meclaw_colony::mutation::MutationOutcome::Committed { .. }
            ),
            "precondition: the shipped declaration commits; got {outcome:?}"
        );
    }

    let after = registry_paths(&h).await;
    for expected in [
        "/colony-view/layout",
        "/colony-view/probe",
        "/display/compose",
        "/display/views",
        "/display/web",
        "/scribe",
        "/sink",
        "/tick",
    ] {
        assert!(
            after.iter().any(|p| p == expected),
            "{expected} did not grow: {after:?}"
        );
    }
    assert_eq!(
        after.len(),
        CELLS_AFTER_GROW,
        "two checked-in cells plus six instantiated ones: {after:?}"
    );
}
