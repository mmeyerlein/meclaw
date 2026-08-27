//! GH #195 — one path holds one node, and two entries of the same diff may not
//! both claim it.
//!
//! The naming checks compare a diff's names against `registry_names` /
//! `deep_registry_paths`, which arrive RESUME-FILTERED: an `add_nodes` at an
//! existing path is a Reconnect/Resume, deliberately not a collision. Beside
//! that sat `seen_in_diff`, which tracked `add_nodes` entries among themselves
//! and nothing else. Between the two, a claim from any other entry of the same
//! diff was invisible — the registry check had been told to ignore the path, and
//! the in-diff check was not looking at that side.
//!
//! What that let through, measured rather than read:
//!
//! - `add_nodes` at a resume target + a `swap_nodes[].with.name` at the same
//!   path. #188's staging guard refuses the destructive outcome now, so what was
//!   left is the diagnosis: the operator got the generic occupied-path message,
//!   which advises clearing the directory or adopting it deliberately — advice
//!   for a leftover tree, and wrong when the truth is that two entries of the
//!   operator's own diff want one path.
//! - `add_nodes` at a FRESH name + a `swap_nodes[].with.name` at the same name,
//!   two `swap_nodes[].with` at one name, and a `swap_nodes[].with` against a
//!   `move_nodes[].to`. None of these is protected by a directory that already
//!   exists, so nothing refused them: they staged two trees onto one path and
//!   the second apply failed halfway. That is `LiveTreeMutated`, which
//!   strict-fails the whole colony task — an ordinary diff took the colony down.
//!
//! So the in-diff claim set has to span every entry that CLAIMS a path:
//! `add_nodes` (resume targets included — a resume is not a collision against
//! the registry, but it is still this diff claiming that path), the instantiate
//! form of `swap_nodes[].with`, and `move_nodes[].to`. The existing-node form of
//! `swap_nodes[].with` is deliberately not in it: it references a node, it does
//! not create one.
//!
//! One assertion is on the message, deliberately — naming the real problem is
//! what this issue is about, the destructive half having been closed by #188.

use meclaw_colony::CellFactoryRegistry;
use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::{Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tokio::sync::oneshot;

const CELL_CONFIG: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;
const HIVE_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The ack is `expect`ed rather than `unwrap`ed on purpose: before the fix these
/// diffs did not reject, they strict-failed the colony task mid-apply, and the
/// dropped ack channel is how that shows up here.
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
    ack_rx
        .await
        .expect("the colony must answer the mutation, not die applying it")
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

async fn registry_entry(
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
            limit: 200,
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
}

/// Root cell `main` (logical `/`), a cell `/anchor` to swap away from, a cell
/// `/fetch` that a `add_nodes` resumes and a move can carry, a hive `/unit` to
/// aim deep names into, and `/unit/q` inside it.
async fn bootstrapped_colony() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", HIVE_CONFIG);
    write(td.path(), "main/anchor/config.json", CELL_CONFIG);
    write(td.path(), "main/fetch/config.json", CELL_CONFIG);
    write(td.path(), "main/unit/config.json", HIVE_CONFIG);
    write(td.path(), "main/unit/q/config.json", CELL_CONFIG);
    write(
        td.path(),
        "templates/persist_mock/template.json",
        r#"{"name":"persist_mock"}"#,
    );
    write(td.path(), "templates/persist_mock/config.json", CELL_CONFIG);

    let factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
        spawn_count: Arc::new(AtomicU32::new(0)),
    });
    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![("persist_mock".to_string(), factory.clone())],
    );
    let mut reg = CellFactoryRegistry::new();
    reg.insert("persist_mock".into(), factory);
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap must succeed");
    rescan_templates(&h, td.path().join("templates")).await;
    (td, h)
}

fn assert_naming_collision(outcome: &MutationOutcome, what: &str) -> String {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } if error_code == "naming_collision" => details.clone(),
        other => panic!("{what} must reject as naming_collision; got {other:?}"),
    }
}

// ── two claims, one path ────────────────────────────────────────────────────

/// The case as reported: `add_nodes` at a path that already exists is a Resume
/// and is deliberately taken out of the collision sets, so a
/// `swap_nodes[].with.name` at the same path had nothing left to be compared
/// against. #188 stops the overwrite; this is about the diff being told what is
/// actually wrong with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_add_nodes_resume_target_and_a_swap_target_are_two_claims() {
    let (_td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "fetch", "template": "persist_mock"}],
                "swap_nodes": [{
                    "match": {"name": "anchor"},
                    "with": {"name": "fetch", "template": "persist_mock"}
                }]
            }
        }),
    )
    .await;
    let details = assert_naming_collision(&outcome, "a resume target claimed twice");
    assert!(
        details.contains("add_nodes[0].name") && details.contains("swap_nodes[0].with.name"),
        "the reject must name BOTH entries of the diff that claim the path, by \
         position, so the operator can find them; got {details}"
    );
    assert!(
        details.contains("/fetch"),
        "and the path they both claim; got {details}"
    );
    // The occupied-path message #188 raises at staging is right for a leftover
    // tree and wrong here: following it would have the operator clear a
    // directory that is not the problem. Two entries of their own diff are.
    assert!(
        !details.contains("Clear the directory"),
        "and must NOT hand out the leftover-directory advice, which is wrong \
         when the path is claimed twice by the diff itself; got {details}"
    );
    h.shutdown().await;
}

/// The same two entries at a name nothing occupies yet. No directory stands in
/// the way here, so #188's staging guard never sees it: both entries staged and
/// the second apply failed halfway, which strict-fails the colony task.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_add_nodes_name_and_a_swap_target_are_two_claims() {
    let (_td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "brandnew", "template": "persist_mock"}],
                "swap_nodes": [{
                    "match": {"name": "anchor"},
                    "with": {"name": "brandnew", "template": "persist_mock"}
                }]
            }
        }),
    )
    .await;
    assert_naming_collision(&outcome, "a fresh name claimed by an add and a swap");
    h.shutdown().await;
}

/// Two `swap_nodes[].with` at one name — the same hole with `add_nodes` out of
/// the picture entirely, which is what shows that the claim set has to span the
/// with-side against itself and not merely against `add_nodes`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_swap_targets_at_one_name_are_two_claims() {
    let (_td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"swap_nodes": [
                {"match": {"name": "anchor"},
                 "with": {"name": "brandnew", "template": "persist_mock"}},
                {"match": {"name": "fetch"},
                 "with": {"name": "brandnew", "template": "persist_mock"}}
            ]}
        }),
    )
    .await;
    assert_naming_collision(&outcome, "one name claimed by two swap targets");
    h.shutdown().await;
}

/// And the third claimant: a `move_nodes[].to` aiming where a swap target is
/// going. `validate_move_nodes` knew about `add_nodes` claims and not about
/// with-side ones, so the move's `rename(2)` landed on the directory the swap
/// had just created.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_target_and_a_swap_target_are_two_claims() {
    let (_td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "swap_nodes": [{
                    "match": {"name": "anchor"},
                    "with": {"name": "unit/brandnew", "template": "persist_mock"}
                }],
                "move_nodes": [{"match": {"name": "fetch"}, "to": "unit/brandnew"}]
            }
        }),
    )
    .await;
    assert_naming_collision(&outcome, "one path claimed by a swap target and a move");
    h.shutdown().await;
}

/// Claims are compared as PATHS, so the two spellings of one deep name are one
/// claim — the same rule `scoped_name` decides everywhere else on this surface.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_spellings_of_one_claim_are_one_claim() {
    for (added, target) in [
        ("unit/fresh", "./unit/fresh"),
        ("./unit/fresh", "unit/fresh"),
    ] {
        let (_td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {
                    "add_nodes": [{"name": added, "template": "persist_mock"}],
                    "swap_nodes": [{
                        "match": {"name": "anchor"},
                        "with": {"name": target, "template": "persist_mock"}
                    }]
                }
            }),
        )
        .await;
        assert_naming_collision(
            &outcome,
            &format!("add '{added}' + swap target '{target}' name one path and"),
        );
        h.shutdown().await;
    }
}

// ── and what must keep working ──────────────────────────────────────────────

/// The leniency the claim set must not eat: an `add_nodes` at an existing path
/// is a Resume, the same node keeping its identity, and stays committable. It
/// claims that path — but it is the only claimant, and one claim is no
/// collision.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lone_resume_still_commits() {
    let (_td, h) = bootstrapped_colony().await;
    let before = registry_entry(&h, "/fetch")
        .await
        .expect("precondition: /fetch is registered");

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "fetch", "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "an add_nodes at an existing path is a Resume and must commit; got {outcome:?}"
    );

    let after = registry_entry(&h, "/fetch")
        .await
        .expect("and the node is still registered at its path");
    assert_eq!(
        after.cell_id, before.cell_id,
        "a Resume keeps the identity — it is the same node, not a second one"
    );
    h.shutdown().await;
}

/// The existing-node form of `swap_nodes[].with` REFERENCES a node, it does not
/// create one, so it is not a claim — and a `with.name` forward-referencing an
/// `add_nodes` of the same diff has to keep resolving. Reading that as a second
/// claim on the path would break the one composite diff the with-side was given
/// forward-reference resolution for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_with_name_forward_referencing_an_add_nodes_is_no_second_claim() {
    let (_td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "successor", "template": "persist_mock"}],
                "swap_nodes": [{"match": {"name": "fetch"}, "with": {"name": "successor"}}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "an existing-node with.name pointing at this diff's own add_nodes must \
         still commit; got {outcome:?}"
    );
    assert!(
        registry_entry(&h, "/successor").await.is_some(),
        "and the node the swap swung onto is registered"
    );
    h.shutdown().await;
}

/// Entries claiming DIFFERENT paths are not each other's problem — the check
/// must not turn a composite diff into a collision just for having several
/// creating entries in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn distinct_claims_in_one_diff_still_commit() {
    let (_td, h) = bootstrapped_colony().await;
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {
                "add_nodes": [{"name": "one", "template": "persist_mock"}],
                "swap_nodes": [{
                    "match": {"name": "anchor"},
                    "with": {"name": "two", "template": "persist_mock"}
                }],
                "move_nodes": [{"match": {"name": "fetch"}, "to": "unit/three"}]
            }
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "three entries claiming three paths must commit; got {outcome:?}"
    );
    for p in ["/one", "/two", "/unit/three"] {
        assert!(
            registry_entry(&h, p).await.is_some(),
            "and {p} is registered"
        );
    }
    h.shutdown().await;
}
