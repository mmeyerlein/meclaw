//! GH #572 — a single-cell hive template is refused BY NAME at the mutation
//! door, at both doors that instantiate one.
//!
//! A hive is a scope marker, not an actor: it has no factory, no mailbox and no
//! `cell.db`. Everything that stages a SINGLE cell therefore ends at the same
//! wall — the apply arm looks up a factory for the staged cell's type, finds
//! none for `hive`, and answers `spawn: factory missing for hive`. That is a
//! late, unnamed refusal from the half of the mutation that is supposed to be
//! past deciding, and it is the same wall behind both doors:
//!
//! * `swap_nodes[].with: {"template": …, "name": …}` — the instantiate form
//!   stages the with-side through the single-cell machinery, with no subtree
//!   dispatch at all;
//! * `add_nodes` — the subtree dispatch is there, but a template with no nested
//!   cells is not a subtree, so a hive-rooted single-cell template takes the
//!   single-cell path and hits the same wall.
//!
//! Ruling O-0904-1 refuses it instead of teaching either door to stage a hive:
//! the shape that WORKS is a generation change (`add_nodes` grows a multi-cell
//! subtree, `swap_nodes` names it), a door that sometimes builds and sometimes
//! refuses the same spelling is the worse answer, and a hive that enters alone
//! is a scope marking nothing. ADR 0021.
//!
//! # Why a real colony
//!
//! The claim is not "validation says no" but "validation says no BEFORE
//! anything moved": the verdict is read off the colony's own edge table through
//! `/colony/graph`, and the standing edge is still there afterwards.

use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::{CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::{JsonValue, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use tokio::sync::oneshot;

// ── Harness ──────────────────────────────────────────────────────────────────

const ECHO: &str = r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/dev/null"},
    "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// `hive_only@1.0.0` — a hive at the template ROOT and NOTHING below it.
///
/// The contract is here on purpose: it is the richest thing such a template can
/// say, and the refusal has to come before anything reads it. If the successor
/// ever reached stage 6, this declaration is what it would be believed on.
fn write_hive_only_template(root: &std::path::Path) {
    let tpl = root.join("templates/hive_only");
    write(
        &tpl,
        "template.json",
        r#"{"name":"hive_only","version":"1.0.0"}"#,
    );
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"contract":{"accepts":[
            {"route":"credential_request","at":["./brain"],
             "because":"a brain runs on a grant id, not on a key in its config"}]}}}"#,
    );
}

/// `unit@1.0.0` — the same hive root, with a cell under it. This is what a hive
/// looks like when it can enter the world: the root of a subtree.
fn write_unit_template(root: &std::path::Path) {
    let tpl = root.join("templates/unit");
    write(
        &tpl,
        "template.json",
        r#"{"name":"unit","version":"1.0.0"}"#,
    );
    write(&tpl, "config.json", r#"{"cell":{"type":"hive"}}"#);
    write(&tpl, "brain/config.json", ECHO);
}

/// One caller, one implementation, and the edge between them — the pre-state a
/// generation change starts from, and the thing a refused mutation must leave
/// exactly as it found it.
fn plant(root: &std::path::Path) {
    write(root, "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(root, "main/broker/config.json", ECHO);
    write(root, "main/old/config.json", ECHO);
    write_hive_only_template(root);
    write_unit_template(root);
}

async fn boot(root: &std::path::Path) -> ColonyHandle {
    let h = ColonyHandle::new_with_echo_at(root);
    let mut factories = CellFactoryRegistry::new();
    factories.insert(
        "echo".to_string(),
        std::sync::Arc::new(EchoCellFactory) as std::sync::Arc<dyn meclaw_colony::CellFactory>,
    );
    rescan_templates(&h, root.join("templates")).await;
    bootstrap_from_filesystem(root, &factories, &h.runtime())
        .await
        .expect("the tree boots");
    h
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
    ack_rx.await.unwrap().expect("the template scan succeeds");
}

async fn read_graph(h: &ColonyHandle) -> ReadGraphReply {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// Does the colony's own edge table hold an edge `/broker -> {to}`?
async fn edge_to(h: &ColonyHandle, to: &str) -> bool {
    read_graph(h)
        .await
        .edges
        .iter()
        .any(|e| e.from == "/broker" && e.to == to)
}

/// Draws the edge the swap would swing, and proves it is there.
async fn lay_the_standing_edge(h: &ColonyHandle) {
    let laid = send_mutation(
        h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"./broker","to":"./old"}]}}),
    )
    .await;
    assert!(
        matches!(laid, MutationOutcome::Committed { .. }),
        "the pre-state edge is drawn: {laid:?}"
    );
    assert!(edge_to(h, "/old").await, "and the colony holds it");
}

// ── The two doors ────────────────────────────────────────────────────────────

/// Door 1: `swap_nodes[].with` in its INSTANTIATE form.
///
/// The with-side is staged through the single-cell machinery — `parse_subtree`
/// is never consulted there — so before this refusal the mutation validated
/// clean, staged a `StagedDir` for a cell type nothing can spawn, and answered
/// `spawn: factory missing for hive` from the apply arm. Now it is
/// `hive_template_single_cell`, at stage 4, with nothing staged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hive_successor_is_refused_by_name() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;
    lay_the_standing_edge(&h).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"swap_nodes":[
            {"match":{"name":"old"},"with":{"template":"hive_only@1.0.0","name":"new"}}
        ]}}),
    )
    .await;
    let MutationOutcome::Rejected {
        error_code,
        details,
        ..
    } = &outcome
    else {
        panic!("a hive cannot be instantiated as a swap successor: {outcome:?}");
    };
    assert_eq!(
        error_code, "hive_template_single_cell",
        "the door refuses by name, not with the apply arm's `spawn`: {details}"
    );
    assert!(
        details.contains("add_nodes"),
        "and the refusal names the shape that works: {details}"
    );

    assert!(
        edge_to(&h, "/old").await,
        "the refusal is pre-destructive: the standing edge is untouched"
    );
    assert!(
        !edge_to(&h, "/new").await,
        "and nothing was swung onto a successor that was never staged"
    );

    h.shutdown().await;
}

/// Door 2: `add_nodes`.
///
/// This door DOES dispatch on the subtree, but a template with no nested cells
/// is not one — so a hive-rooted single-cell template falls through to the same
/// single-cell staging and the same missing factory. One predicate answers both
/// doors, because it is one question: may a hive enter the world alone?
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hive_only_template_is_refused_at_add_nodes_too() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[
            {"name":"fresh","template":"hive_only@1.0.0"}
        ]}}),
    )
    .await;
    let MutationOutcome::Rejected {
        error_code,
        details,
        ..
    } = &outcome
    else {
        panic!("a hive cannot be grown alone: {outcome:?}");
    };
    assert_eq!(
        error_code, "hive_template_single_cell",
        "the same code at the other door: {details}"
    );

    assert!(
        read_graph(&h)
            .await
            .nodes
            .iter()
            .all(|c| c.path != "/fresh"),
        "and nothing was registered at the refused address"
    );

    h.shutdown().await;
}

// ── The shape that works ─────────────────────────────────────────────────────

/// The control: the same intent written the way GH #256 says a generation
/// change is written — `add_nodes` grows a MULTI-CELL unit whose root is a
/// hive, and `swap_nodes` names it in the existence form.
///
/// This is the half that makes the refusal above honest. A predicate that took
/// this with it would not be refusing "a hive alone", it would be refusing
/// hives; the subtree door stages the unit and registers its hive scope, and
/// that is the only way a hive has ever entered the world.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generation_change_into_a_subtree_still_commits() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;
    lay_the_standing_edge(&h).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"gen2","template":"unit@1.0.0"}],
            "swap_nodes":[{"match":{"name":"old"},"with":{"name":"gen2"}}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a hive enters the world as the root of a subtree: {outcome:?}"
    );

    assert!(
        edge_to(&h, "/gen2").await,
        "and the caller's edge is swung onto the new generation"
    );

    h.shutdown().await;
}
