//! GH #404 — the mutation lane and the boot must agree about what a valid cell is.
//!
//! `add_nodes` validated a dozen things about the node it was about to write —
//! schema, edges, template existence, `.env` tokens, naming collisions, scope
//! bounds, port boundaries, required drains — and never asked the one question
//! the boot asks of every cell: does this `params` block deserialize for the
//! cell type it names? `CellFactory::validate_params` is called for every cell
//! by `plan_bootstrap`, so the two paths that put a cell into a colony
//! disagreed: instantiation accepted what the boot refuses.
//!
//! The timing is the defect, not just the miss. The mutation returned
//! `committed`, the colony kept running with a cell that could never do its
//! job, and the failure surfaced at the *next* process start — a deploy, a
//! crash, a host reboot — as `bootstrap failed`, in front of whoever restarted
//! it rather than whoever grew it. GH #401 was one instance of that class:
//! `templates/argus/clock` shipped a schedule the `timer` type rejects, and
//! `examples/organism` ran until it was restarted and then would not start.
//!
//! WHY THIS FILE IS NOT GH #401'S FILE
//! ==================================
//! `gh401_a_grown_argus_survives_a_reboot.rs` proves a *healthy* declaration
//! reboots. It cannot see this class: with the template repaired, there is
//! nothing left for the guard to catch, and with the template broken it proves
//! only that the second boot dies — which is the symptom. This file deliberately
//! breaks a template again, in exactly the shape #401 shipped, and asserts the
//! refusal happens at the **mutation**, naming the cell and the factory's own
//! reason. The counter-test in the same file keeps the guard from becoming a
//! blanket refusal.

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
/// exception form — this file reads a bare `templates/` path).
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
///
/// `broken` re-introduces the GH #401 defect into the copied argus: the
/// `timer` schema requires `schedule_id`, and a schedule without one is exactly
/// what shipped. The template on disk is untouched — this is a copy.
fn tree(broken: bool) -> tempfile::TempDir {
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
    let clock = templates.join("argus/clock/config.json");
    let raw = std::fs::read_to_string(&clock).unwrap();
    // A cron this run will never reach: the point here is the staging, not a tick.
    let mut cfg: Value =
        meclaw_core::serde_json::from_str(&raw.replace("0 0 */6 * * *", "0 0 0 1 1 *")).unwrap();
    if broken {
        let schedule = cfg["params"]["schedules"][0]
            .as_object_mut()
            .expect("the shipped schedule is an object");
        schedule.remove("schedule_id");
        assert!(
            schedule.get("schedule_id").is_none(),
            "the defect has to be in the copy, or this test proves nothing"
        );
    }
    std::fs::write(
        &clock,
        meclaw_core::serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
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

/// Boot the tree as it stands on disk, returning the outcome as text rather
/// than unwrapping it: whether the tree still starts is an assertion here.
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
async fn a_declaration_the_boot_would_refuse_is_refused_at_the_mutation() {
    let Some(_) = shipped_argus() else {
        return;
    };
    let td = tree(true);

    let (h, first) = boot(&td).await;
    first.expect("the reference seed must boot");
    rescan(&h, &td).await;
    let outcome = mutate(&h, grow_declaration()).await;

    let meclaw_colony::mutation::MutationOutcome::Rejected {
        error_code,
        details,
        ..
    } = &outcome
    else {
        panic!(
            "a template whose params the boot rejects was COMMITTED — the whole \
             defect of GH #404: the colony keeps running, the cell never does \
             its job, and the next process start refuses to boot.\n{outcome:?}"
        );
    };
    assert_eq!(
        error_code, "invalid_params",
        "the refusal names its own reason rather than borrowing `schema`: {details}"
    );
    // The reader has to be able to walk from the refusal to the file. The cell
    // path locates it in the tree; the factory's own words say what is wrong,
    // and they are the SAME words the boot would have printed six hours later.
    assert!(
        details.contains("clock"),
        "the refusal must name the cell it is about: {details}"
    );
    assert!(
        details.contains("schedule_id"),
        "the refusal must carry the factory's own reason: {details}"
    );

    // Pre-destructive: a refused mutation is not a deployment (GH #360/#276).
    // If the guard refused after the rename, the tree it left behind would be
    // the very tree that cannot boot.
    h.shutdown().await;
    let (h2, second) = boot(&td).await;
    second.expect("a refused mutation leaves a tree that still starts");
    h2.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_declaration_with_the_shipped_params_still_commits() {
    let Some(_) = shipped_argus() else {
        return;
    };
    let td = tree(false);

    let (h, first) = boot(&td).await;
    first.expect("the reference seed must boot");
    rescan(&h, &td).await;
    let outcome = mutate(&h, grow_declaration()).await;
    assert!(
        matches!(
            outcome,
            meclaw_colony::mutation::MutationOutcome::Committed { .. }
        ),
        "the guard is a parser, not a wall: the shipped argus must still \
         instantiate unchanged: {outcome:?}"
    );
    h.shutdown().await;

    // And the tree it wrote still starts — the property GH #401 pinned, restated
    // here so that a guard that accepted the wrong thing cannot pass this file.
    let (h2, second) = boot(&td).await;
    second.expect("what the mutation accepted, the boot accepts");
    h2.shutdown().await;
}
