//! The argus, grown into the reference colony (GitHub #155).
//!
//! Two things this file establishes, and the second one is the interesting one:
//!
//! 1. The argus is an ordinary declaration. `grow-argus.json` adds seven
//!    cells to a colony that is already up — no reboot, no special path.
//! 2. **It cannot give itself the power to act.** The edge that lets its
//!    mutations reach `/colony/mutations` is not something a mutation can
//!    create: `/colony/*` is a virtual endpoint, not a registry node, so
//!    `add_edges` refuses it by name. That edge is a boot-time act — a human
//!    editing the seed — and no amount of growing gets around it.
//!
//! That is a stronger property than any rule inside the argus. A control loop
//! that could wire its own mutation lane would be one `add_edges` away from
//! being unbounded, whatever its charter said.

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
        "charter/seed/goals.jsonl",
        "charter/seed/rules.jsonl",
        "meter/config.json",
        "judge/config.json",
        "mutator/config.json",
        "probe/config.json",
        "receipts/config.json",
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

/// The example's own seed, plus the two templates this test grows from. The
/// seed is taken verbatim — the point of growing into the REFERENCE colony is
/// that it is the reference colony.
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
    // A cron the run will never reach: the loop must not tick during the test.
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

/// Boot the seed, then make the templates known — the same two steps the
/// example's own test performs.
async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the seed must boot");
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
    h
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

fn grow_declaration() -> Value {
    let raw = std::fs::read_to_string("../../examples/meclaw-os/grow-argus.json")
        .expect("the declaration ships with the example");
    let mut v: Value = serde_json::from_str(&raw).expect("declaration json");
    // The example resolves ${MODEL_CORE} from .env; here the model is a literal
    // because no provider is ever called.
    v["ctx"]["model"] = json!("test/model");
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_argus_grows_as_an_ordinary_declaration() {
    let Some(_) = shipped_argus() else {
        return;
    };
    let td = tree();
    let h = boot(&td).await;

    let before = registry_paths(&h).await.len();
    let outcome = mutate(&h, grow_declaration()).await;
    assert!(
        committed(&outcome),
        "the argus is an ordinary declaration: {outcome:?}"
    );

    let paths = registry_paths(&h).await;
    for expected in [
        "/argus/charter",
        "/argus/meter",
        "/argus/judge",
        "/argus/mutator",
        "/argus/probe",
        "/argus/receipts",
        "/argus/clock",
    ] {
        assert!(
            paths.iter().any(|p| p == expected),
            "{expected} did not grow: {paths:?}"
        );
    }
    assert_eq!(paths.len(), before + 7, "seven cells, no more: {paths:?}");

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_argus_cannot_wire_its_own_mutation_lane() {
    // The property that bounds this loop from outside itself. `/colony/*` is a
    // virtual endpoint rather than a registry node, so `add_edges` refuses it
    // by name — at any scope, from any cell, including from the argus's own
    // mutator. Granting that lane is a boot-time act: a human edits the seed.
    let Some(_) = shipped_argus() else {
        return;
    };
    let td = tree();
    let h = boot(&td).await;
    mutate(&h, grow_declaration()).await;

    let outcome = mutate(
        &h,
        json!({
            "scope": "/",
            "ctx": {},
            "diff": {"add_edges": [{
                "from": "./argus",
                "to": "/colony/mutations",
                "condition": "has(hop.route) && hop.route == 'mutate'"
            }]}
        }),
    )
    .await;
    assert!(
        !committed(&outcome),
        "a mutation must not be able to mint the lane that carries mutations: {outcome:?}"
    );
    // The endpoint is the HIVE, not a cell inside it (GH #197: the hive has been
    // sealed since `steward@2`, this template's predecessor, and `argus@1.0.0`
    // inherits the seal: `params.ports: []`). That matters for what this test proves: with
    // a deep endpoint the refusal could be the port boundary talking, which
    // would say nothing at all about `/colony/mutations`. Asked at the address
    // the boundary admits, the only thing left to refuse it is the virtual
    // endpoint itself.
    match &outcome {
        meclaw_colony::mutation::MutationOutcome::Rejected { error_code, .. } => assert_ne!(
            error_code, "hive_port_boundary",
            "the refusal has to be about /colony/mutations, not about the seal: {outcome:?}"
        ),
        other => panic!("expected a rejection: {other:?}"),
    }

    h.shutdown().await;
}

/// GH #304 — the shape the mutator used to emit, asked of the real lane.
///
/// `{"swap_nodes": [{"name": …, "params": …}]}` was the decide path's output for
/// the whole life of `steward@2.0.x` -- this template's predecessor, renamed
/// `argus` in GH #462 -- and the validator has required
/// `match.name` + `with` on every entry since long before that: the loop could
/// not commit once, in either direction. The old shape is pinned here as a
/// REFUSAL rather than left to history, because the defect class is a body
/// authored against a validator nobody asked — and the only thing that makes
/// this pin worth anything is that the question goes to `handle_mutation`
/// itself.
///
/// `params` at the entry level is read by nothing, so the entry is refused for
/// the missing `match.name` first; the diff would be inert even with one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_old_swap_nodes_shape_is_refused_by_the_mutation_lane() {
    let Some(_) = shipped_argus() else {
        return;
    };
    let td = tree();
    let h = boot(&td).await;

    let outcome = mutate(
        &h,
        json!({
            "scope": "/",
            "ctx": {},
            "diff": {"swap_nodes": [{"name": "sink", "params": {"model": "test/model"}}]}
        }),
    )
    .await;
    match &outcome {
        meclaw_colony::mutation::MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "schema",
                "the entry-level `params` shape is a schema refusal: {outcome:?}"
            );
        }
        other => panic!("the shape this loop used to emit must not commit: {other:?}"),
    }

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grown_argus_changes_nothing_until_a_goal_is_enabled() {
    // The resting state, proven on a live colony rather than on the seed file:
    // it boots, it grows, and it reaches nothing outside itself. Since GH #462 a
    // TICK is no longer silent -- it leaves an `idle` receipt -- but this test
    // never lets the clock fire, so what it measures is the same thing it always
    // measured: growing the hive costs the colony nothing.
    let Some(_) = shipped_argus() else {
        return;
    };
    let td = tree();
    let h = boot(&td).await;
    mutate(&h, grow_declaration()).await;

    // Nothing arrives anywhere: no dead letters, and the charter's goals are
    // all disabled so a tick would produce no work either.
    let dl = h.drain_dead_letters().await;
    assert!(dl.is_empty(), "a grown argus dead-letters nothing: {dl:?}");

    h.shutdown().await;
}
