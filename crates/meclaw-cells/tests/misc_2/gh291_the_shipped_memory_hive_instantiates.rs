//! GH #291, acceptance bullet 2 — **the shipped stack still instantiates**.
//!
//! Task 17 turned `params.contract.accepts[].context` from a note for a reader
//! into a checked requirement, collected as the fourth check of stage 6 in the
//! mutation pipeline. A rule that refuses is only half a rule: the other half
//! is that everything the library ships must still go in. `memory-hive` is the
//! sharpest case in the tree — six accepted lanes, and `in_query` alone names
//! seven `context` keys — so if the new check can refuse a correct install, it
//! refuses this one.
//!
//! It is asked of the **real mutation path with the real cell factories**: the
//! template is the shipped directory, read at runtime and not a fixture; the
//! four cell types it contains (`code`, `store`, `timer`, `llm`) are the
//! substrate's own factories, the same set `meclaw_os_example` boots with; and
//! the verdict is the `MutationOutcome` the colony returns. Nothing here stands
//! in for the thing under test.
//!
//! Why an instantiation is the right question for THIS check: a fresh
//! composite is exactly the shape the lane rule could most easily get wrong.
//! Its inner hives are wired by the template's own `params.graph`, so their
//! caller side is a hive path that — at the moment of instantiation — nothing
//! outside addresses yet. That is the dormancy rule (`hive_contract`'s island
//! reading) doing its work: a requirement nothing can deliver to is dormant,
//! not broken. Without it a contracted hive could not be installed at all, and
//! this test is what would fail first.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and a template that did not travel is skipped
//! rather than judged.

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use tokio::sync::oneshot;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The four cell types the shipped hive is built from — the substrate's own,
/// not stand-ins. A stand-in would answer a different question: whether a
/// synthetic tree instantiates.
fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
    ]
}

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factories() {
        r.insert(name, f);
    }
    r
}

/// Copy a directory tree verbatim — the template must arrive as it ships.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Every `${VAR}` the tree references WITHOUT a default, bound to a dummy.
///
/// Collected from the template rather than listed here (same approach as
/// `gh144_fresh_hive_keeps_its_egress`): a missing binding is `EnvVarMissing`,
/// which would turn "one more env key was added to the hive" into a red that
/// looks like a contract failure. The values are never used — nothing in this
/// test sends a message.
fn dummy_env(source: &std::path::Path) -> Vec<(String, String)> {
    let mut names = std::collections::BTreeSet::new();
    let mut stack = vec![source.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&p) else {
                continue;
            };
            let mut rest = raw.as_str();
            while let Some(start) = rest.find("${") {
                rest = &rest[start + 2..];
                let Some(end) = rest.find('}') else { break };
                let name = &rest[..end];
                if !name.contains(":-")
                    && !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                {
                    names.insert(name.to_string());
                }
                rest = &rest[end + 1..];
            }
        }
    }
    names
        .into_iter()
        .map(|n| {
            let v = format!("dummy-{n}");
            (n, v)
        })
        .collect()
}

/// The cells the shipped template carries, by name, sorted — read out of the
/// template directory rather than written down here.
///
/// A hard-coded twelve would be a second copy of the template's shape, and the
/// library ships in two sizes (the public export carries a subset), so a number
/// that is honest in one tree can be wrong in the other. The floor that a
/// derived list still has to clear is asserted at the call site: a template
/// that yields NO cells is a wrong root, not a small hive.
///
/// Hives are excluded, and that is the same rule the assertion below rests on:
/// a hive is a scope marker, so it never becomes a graph node.
fn shipped_cell_names(template_root: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(template_root)
        .unwrap_or_else(|e| panic!("{}: {e}", template_root.display()))
        .filter_map(|entry| {
            let p = entry.unwrap().path();
            let raw = std::fs::read_to_string(p.join("config.json")).ok()?;
            let v: Value = meclaw_core::serde_json::from_str(&raw).ok()?;
            let is_hive = v["cell"]["type"].as_str() == Some("hive");
            (!is_hive).then(|| p.file_name()?.to_str().map(str::to_string))?
        })
        .collect();
    names.sort();
    assert!(
        !names.is_empty(),
        "no cell under {} at all — wrong root",
        template_root.display()
    );
    names
}

async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_memory_hive_still_instantiates_through_the_mutation_path() {
    let source = repo("templates/memory-hive");
    if !source.join("config.json").is_file() {
        // The public export ships a subset; a template that did not travel is
        // not a defect.
        return;
    }

    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();

    // The smallest colony that can receive a mutation: a root hive with an
    // empty graph. What is under test is what the mutation ADDS.
    std::fs::create_dir_all(root.join("main")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    // The template library, as it ships.
    copy_tree(&source, &root.join("templates/memory-hive"));

    // `<root>/.env` is the substitution source the mutation path reads (the
    // same one the boot reads).
    let env: String = dummy_env(&source)
        .into_iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect();
    std::fs::write(root.join(".env"), env).unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, factories());
    bootstrap_from_filesystem(root, &registry(), &h.runtime())
        .await
        .expect("the empty root must boot");

    let (ack_tx, ack_rx) = oneshot::channel();
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

    let outcome = mutate(
        &h,
        json!({"scope": "/", "diff": {
            "add_nodes": [{"name": "memory", "template": "memory-hive"}]
        }}),
    )
    .await;

    match &outcome {
        MutationOutcome::Committed { .. } => {}
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => panic!(
            "the shipped memory-hive must still install: {error_code} — {details}\n\
             (if this says `hive_contract` and names a context key, the GH #291 lane \
             check has started refusing a correct instantiation)"
        ),
    }

    // `Committed` on an empty diff would be a vacuous green, so the tree the
    // mutation built is read back: the hive path itself, and the cells that
    // make its declared lanes reachable. Counted rather than enumerated — the
    // template's inside is free to change, which is the entire point of a
    // contract stated in lanes.
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: meclaw_core::Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let graph = ack_rx.await.unwrap();
    let paths: Vec<&str> = graph.nodes.iter().map(|n| n.path.as_str()).collect();
    // A hive is a scope marker and not an actor, so the hive PATH carries no
    // registry row and must NOT appear among the nodes. Asserted rather than
    // only commented: this is the discipline that makes a contract a statement
    // about a path, and an instantiation that registered `/memory` as a cell
    // would have broken it silently.
    assert!(
        !paths.contains(&"/memory"),
        "a hive is a scope marker, not a node: {paths:?}"
    );

    // The cells the template ships, counted exactly. `>= 5` would let seven of
    // the twelve go missing without a word — and a composite that arrives
    // half-built is the failure this test is for.
    let mut inside: Vec<&str> = paths
        .iter()
        .filter_map(|p| p.strip_prefix("/memory/"))
        .collect();
    inside.sort_unstable();
    let expected = shipped_cell_names(&source);
    assert_eq!(
        inside, expected,
        "the composite must arrive whole: every cell `templates/memory-hive` carries, and no \
         other. Graph: {paths:?}"
    );

    h.shutdown().await;
}
