//! GH #1084 -- the boot resolves `${VAR}` only in nodes that can run.
//!
//! A parked node (`<name>~<version>`, left behind by a lift) never runs. The
//! boot used to substitute environment placeholders in its `config.json` all
//! the same, so a colony that had dropped an old provider key from its `.env`
//! no longer started: the migration proof of #801 counted 14x
//! `env_var_missing`, every one from a parked node. The rule pinned here:
//!
//! - a parked node keeps its placeholders verbatim (no secret ever reaches a
//!   node that will not run), and a variable missing there is no boot error;
//! - a row UNDER a parked hive (`/screen~1.0.0/inner`) is parked too, the same
//!   reading `validate::is_parked_path` gives every other door;
//! - an active node is unchanged: its token is replaced, and a missing
//!   variable still fails the boot and names the variable;
//! - a parked node is INACTIVE at a first boot (empty `colony.db`): the edge
//!   recompute never reaches a leaf without edges, so the overlay-miss default
//!   is its final word, and it is never spawned with a literal `${VAR}`.

use meclaw_colony::ColonyMsg;
use meclaw_colony::api_dto::{ReadRegistryReply, RegistryEntryDto};
use meclaw_core::serde_json::Value as JsonValue;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

const CONTRACT_TAIL: &str = r#""version":"0.1.0","settings":{},"consumes":{}"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn echo_cell(token: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"/sink","api_key":"${{{token}}}"}},"contract":{{{CONTRACT_TAIL}}}}}"#
    )
}

fn echo_factories() -> meclaw_colony::CellFactoryRegistry {
    let mut m = meclaw_colony::CellFactoryRegistry::new();
    m.insert(
        "echo".to_string(),
        Arc::new(meclaw_testing::factories::EchoCellFactory) as Arc<dyn meclaw_colony::CellFactory>,
    );
    m
}

fn active_of(plan: &meclaw_colony::BootstrapPlan, path: &str) -> bool {
    plan.cells
        .iter()
        .find(|c| c.path.as_str() == path)
        .unwrap_or_else(|| panic!("{path} planned"))
        .active
}

fn api_key_of(plan: &meclaw_colony::BootstrapPlan, path: &str) -> JsonValue {
    plan.cells
        .iter()
        .find(|c| c.path.as_str() == path)
        .unwrap_or_else(|| panic!("{path} planned"))
        .params["api_key"]
        .clone()
}

/// The tree of the migration proof in small: one active node whose variable
/// is present, one parked node and one row under a parked hive whose variable
/// is gone, and one parked node whose variable is still present.
fn tree_with_parked_nodes() -> TempDir {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(td.path(), "main/a/config.json", &echo_cell("VGN_TEST_KEY"));
    write(
        td.path(),
        "main/bump~1.0.0/config.json",
        &echo_cell("VGN_GONE_KEY"),
    );
    write(
        td.path(),
        "main/keep~1.0.0~2/config.json",
        &echo_cell("VGN_TEST_KEY"),
    );
    write(
        td.path(),
        "main/screen~1.0.0/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/screen~1.0.0/inner/config.json",
        &echo_cell("VGN_GONE_KEY"),
    );
    std::fs::write(td.path().join(".env"), "VGN_TEST_KEY=fixture-value\n").unwrap();
    td
}

#[test]
fn a_parked_node_keeps_its_placeholder_and_its_missing_variable_does_not_fail_the_boot() {
    let td = tree_with_parked_nodes();
    let plan = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect("a variable missing only in parked nodes must not fail the boot");

    assert_eq!(
        api_key_of(&plan, "/a"),
        "fixture-value",
        "the active node is resolved as before"
    );
    assert_eq!(
        api_key_of(&plan, "/bump~1.0.0"),
        "${VGN_GONE_KEY}",
        "a parked node keeps its placeholder verbatim"
    );
    assert_eq!(
        api_key_of(&plan, "/screen~1.0.0/inner"),
        "${VGN_GONE_KEY}",
        "a row under a parked hive is parked too"
    );
    assert_eq!(
        api_key_of(&plan, "/keep~1.0.0~2"),
        "${VGN_TEST_KEY}",
        "a secret never reaches a parked node, even when the variable is present"
    );
}

#[test]
fn a_parked_node_is_inactive_at_a_first_boot() {
    let td = tree_with_parked_nodes();
    let plan = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect("plan");
    assert!(active_of(&plan, "/a"), "an active node stays active");
    for parked in ["/bump~1.0.0", "/keep~1.0.0~2", "/screen~1.0.0/inner"] {
        assert!(
            !active_of(&plan, parked),
            "{parked} is parked and must be inactive at a first boot"
        );
    }
}

async fn registry_entries(h: &meclaw_testing::ColonyHandle) -> Vec<RegistryEntryDto> {
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
    ack_rx.await.unwrap().entries
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_node_is_registered_but_never_spawned_at_a_first_boot() {
    let td = tree_with_parked_nodes();
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo".to_string(),
            Arc::new(meclaw_testing::factories::EchoCellFactory)
                as Arc<dyn meclaw_colony::CellFactory>,
        )],
    );
    meclaw_colony::bootstrap_from_filesystem(td.path(), &echo_factories(), &h.runtime())
        .await
        .expect("a first boot with parked nodes succeeds");
    let entries = registry_entries(&h).await;
    let entry = |path: &str| {
        entries
            .iter()
            .find(|e| e.path == path)
            .unwrap_or_else(|| panic!("{path} registered: {entries:?}"))
            .clone()
    };
    let a = entry("/a");
    assert!(a.active, "the active node is active: {a:?}");
    assert_ne!(
        a.lifecycle_status, "NotYetSpawned",
        "the active eager node is spawned: {a:?}"
    );
    for parked in ["/bump~1.0.0", "/keep~1.0.0~2", "/screen~1.0.0/inner"] {
        let e = entry(parked);
        assert!(!e.active, "{parked} is inactive: {e:?}");
        assert_eq!(
            e.lifecycle_status, "NotYetSpawned",
            "{parked} is registered but never spawned: {e:?}"
        );
    }
    h.shutdown().await;
}

async fn send_mutation(
    h: &meclaw_testing::ColonyHandle,
    payload: JsonValue,
) -> meclaw_colony::MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// No regression for the way back: a parked node that a `move_nodes` takes to
/// an unparked address and an edge wires becomes active there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_node_moved_to_an_unparked_address_and_wired_becomes_active() {
    let td = tree_with_parked_nodes();
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![(
            "echo".to_string(),
            Arc::new(meclaw_testing::factories::EchoCellFactory)
                as Arc<dyn meclaw_colony::CellFactory>,
        )],
    );
    meclaw_colony::bootstrap_from_filesystem(td.path(), &echo_factories(), &h.runtime())
        .await
        .expect("first boot");
    let moved = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"move_nodes": [{"match": {"name": "keep~1.0.0~2"}, "to": "keep"}]}
        }),
    )
    .await;
    assert!(
        matches!(moved, meclaw_colony::MutationOutcome::Committed { .. }),
        "the parked node moves to an unparked address: {moved:?}"
    );
    let wired = send_mutation(
        &h,
        meclaw_core::serde_json::json!({
            "scope": "/",
            "diff": {"add_edges": [{"from": "./a", "to": "./keep"}]}
        }),
    )
    .await;
    assert!(
        matches!(wired, meclaw_colony::MutationOutcome::Committed { .. }),
        "the unparked node takes an edge: {wired:?}"
    );
    let entries = registry_entries(&h).await;
    let keep = entries
        .iter()
        .find(|e| e.path == "/keep")
        .unwrap_or_else(|| panic!("/keep registered: {entries:?}"));
    assert!(keep.active, "the unparked, wired node is active: {keep:?}");
    assert!(
        !entries.iter().any(|e| e.path == "/keep~1.0.0~2"),
        "the parked address is gone: {entries:?}"
    );
    h.shutdown().await;
}

#[test]
fn an_active_node_with_a_missing_variable_still_fails_the_boot() {
    let td = tree_with_parked_nodes();
    write(td.path(), "main/b/config.json", &echo_cell("VGN_GONE_KEY"));
    let errs = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect_err("an active node with a missing variable fails the boot");
    let rendered = format!("{errs:?}");
    assert!(
        rendered.contains("env_var_missing") && rendered.contains("VGN_GONE_KEY"),
        "the boot error carries the token and names the variable: {rendered}"
    );
    assert_eq!(
        rendered.matches("env_var_missing").count(),
        1,
        "only the active node is reported, never the parked ones: {rendered}"
    );
}
