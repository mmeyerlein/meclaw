//! GH #401 — a colony grown from templates has to be able to start again.
//!
//! `templates/argus/clock` shipped a `params.schedules[0]` the `timer` cell
//! type rejects: no `schedule_id`, `name` where the schema says `schedule_name`,
//! and no `emit_to`. Instantiation does **not** deserialize a cell's params, so
//! the mutation committed, the colony kept running, and the defect surfaced at
//! the next boot as `InvalidParams` — the worst possible moment, and a long way
//! from the declaration that caused it.
//!
//! `meclaw-os@1.0.0` refs the argus, so this reached every tree grown from the
//! shell, `examples/organism` included: five declarations produced a colony that
//! ran until it was restarted and then refused to start.
//!
//! WHY THE TEST IS A REBOOT AND NOT AN ASSERTION ON THE FILE
//! ========================================================
//! Reading the shipped JSON and checking it has three keys would pin the
//! symptom, not the property. The property is that a grown tree boots, and only
//! a second `bootstrap_from_filesystem` over the filesystem the mutation wrote
//! can state it. That is also the only shape that would have caught this: every
//! test that grew the argus stopped at the mutation, which is exactly where
//! the defect is invisible.
//!
//! The other half is `crates/meclaw-cells/tests/gh401_shipped_timer_schedules_deserialize.rs`
//! — this file proves one grown colony restarts, that one proves no other
//! template carries the same defect.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::vault::VaultCellFactory;
use meclaw_cells::{LlmCellFactory, TimerCellFactory};
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// The argus template, or `None` where it does not ship (the documented R2b
/// exception form).
fn shipped_argus() -> Option<std::path::PathBuf> {
    let root = templates_root().join("argus");
    for rel in [
        "config.json",
        "template.json",
        "charter/config.json",
        "meter/config.json",
        "clock/config.json",
    ] {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The reference colony's own seed plus the templates this test grows from.
fn tree() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    let example =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/meclaw-os");
    copy_tree(&example.join("seed"), root);
    let templates = root.join("templates");
    copy_tree(
        &shipped_argus().expect("checked by the caller"),
        &templates.join("argus"),
    );
    copy_tree(
        &templates_root().join("terminal"),
        &templates.join("terminal"),
    );
    copy_tree(&templates_root().join("door"), &templates.join("door"));
    // A cron this run will never reach: the point here is the boot, not a tick.
    let clock = templates.join("argus/clock/config.json");
    let raw = std::fs::read_to_string(&clock).unwrap();
    std::fs::write(&clock, raw.replace("0 0 */6 * * *", "0 0 0 1 1 *")).unwrap();
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\nMODEL_CORE=gpt-4o-mock\n",
    )
    .unwrap();
    td
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
        ("vault".to_string(), Arc::new(VaultCellFactory)),
    ]
}

/// Boot the tree as it stands on disk. Returns the bootstrap outcome rather
/// than unwrapping it — the second call is the assertion of this file — and
/// renders a failure as its own text, because the error type is crate-private
/// and what this test needs from it is the report a reader gets.
async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Result<(), String>) {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    let outcome = bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .map(|_| ())
        .map_err(|e| format!("{e:?}"));
    (h, outcome)
}

async fn rescan(h: &ColonyHandle, td: &tempfile::TempDir) {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("GH #440: the rescan must not have aborted");
}

async fn mutate(h: &ColonyHandle, payload: Value) -> meclaw_colony::mutation::MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

fn committed(o: &meclaw_colony::mutation::MutationOutcome) -> bool {
    matches!(
        o,
        meclaw_colony::mutation::MutationOutcome::Committed { .. }
    )
}

/// The shipped declaration, with the model token resolved the way the example's
/// own `.env` resolves it.
fn grow_declaration() -> Value {
    let raw = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/meclaw-os/grow-argus.json"),
    )
    .expect("the shipped declaration");
    let mut v: Value = meclaw_core::serde_json::from_str(&raw).expect("it parses");
    v["ctx"]["model"] = json!("test/model");
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_colony_that_grew_an_argus_starts_again() {
    let Some(_) = shipped_argus() else {
        return;
    };
    let td = tree();

    // ── first boot: the seed, then the declaration
    let (h, first) = boot(&td).await;
    first.expect("the reference seed must boot");
    rescan(&h, &td).await;
    let outcome = mutate(&h, grow_declaration()).await;
    assert!(
        committed(&outcome),
        "the argus is an ordinary declaration: {outcome:?}"
    );
    h.shutdown().await;

    // ── second boot: the same filesystem, nothing edited in between
    //
    // This is the whole test. A mutation does not deserialize the params it
    // writes, so a template defect rides out the session that created it and
    // lands on whoever restarts the process.
    let (h2, second) = boot(&td).await;
    if let Err(errors) = second {
        panic!(
            "a colony that grew an argus refused to start again — the defect \
             class of GH #401: a template whose params the boot rejects commits \
             happily and kills the NEXT boot.\n{errors}"
        );
    }
    h2.shutdown().await;
}
