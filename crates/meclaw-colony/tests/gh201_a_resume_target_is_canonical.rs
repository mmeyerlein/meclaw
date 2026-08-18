//! GH #201 — an `add_nodes[].name` written `./fetch` resumes the node `fetch`.
//!
//! The last member of the family closed by #166/#179/#189/#193/#194/#195/#198/
//! #199, and the only one that never lived in the validator: `colony.rs` pushed
//! `add_nodes[].name` AS WRITTEN into `resume_names`, and that list is
//! subtracted from `registry_names`, which hold CANONICAL registry short names.
//! `"./fetch" != "fetch"`, so a resume target spelled the canonical way never
//! cancelled its own registry entry, and an `add_nodes` at an existing path —
//! which is a Reconnect/Resume (overview Z.170-180), the same node keeping its
//! identity — was refused as a duplicate:
//!
//! | scope | `add_nodes[].name` | existing path | before |
//! |---|---|---|---|
//! | `/`     | `fetch`      | `/fetch`  | commits |
//! | `/`     | `./fetch`    | `/fetch`  | `naming_collision "./fetch"` |
//! | `/unit` | `q`          | `/unit/q` | commits |
//! | `/unit` | `./q`        | `/unit/q` | `naming_collision "./q"` |
//! | `/`     | `unit/q`     | `/unit/q` | commits |
//! | `/`     | `./unit/q`   | `/unit/q` | commits |
//!
//! Short-name-only, and depth-invisible — the deep half is answered from
//! `deep_registry_paths`, which is filtered by `resume_targets`, and those were
//! pushed RESOLVED from the start. The same asymmetry #199 found one file over:
//! the resolved twin was always right, and only the raw spelling was wrong.
//!
//! Lenient-opposite — a legitimate Resume refused, nothing committed wrong —
//! which is presumably why it outlived its siblings.
//!
//! So the test pins the rule, not the two red cells: every spelling of a resume
//! target, at the scope root and at depth, resumes the node it names. Beside it
//! the two properties that must survive. A Resume is a Resume, not a fresh
//! instantiation: `cell_id` is not re-minted and `cell.db` is not re-seeded —
//! canonicalising the name must not turn a Resume into an overwrite. And the
//! collision check is not disarmed by it: an `add_nodes` at a path that is NOT
//! a resume target is still refused, in either spelling, and a second entry of
//! the same diff aiming at the resume target is still refused too — which is
//! what keeps the widened cancel (`./fetch` now removes `fetch` from
//! `registry_names`) from becoming a free pass on that name.
//!
//! Identity assertions read `colony.db`: the persisted `registry` row is what
//! survives a restart, and therefore the only proof that the node came through
//! the Resume as itself rather than merely that validation stopped objecting.

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

/// The two ways of writing one name. Every case runs over both, because "these
/// two spellings decide the same way" is the whole claim and the defect was
/// exactly one of them.
const SPELLINGS: [&str; 2] = ["", "./"];

/// What a cell that has already run leaves behind. Resume must hand it back
/// untouched (M1, no re-seed); a fresh instantiation would replace it.
const DB_MARKER: &[u8] = b"gh201-resume-cell-db-marker-state";

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

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
    ack_rx.await.unwrap();
}

/// The live registry's `cell_id` for `path`, read BEFORE the mutation — the
/// value the persisted row is compared against afterwards.
async fn live_cell_id(h: &ColonyHandle, path: &str) -> String {
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
        .unwrap_or_else(|| panic!("{path} must be registered"))
        .cell_id
}

/// The `cell_id` the persisted `registry` row carries — read from `colony.db`,
/// which is what a restart reads back, and therefore what "the same node" means
/// beyond the lifetime of this process.
fn persisted_cell_id(root: &std::path::Path, path: &str) -> Option<String> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    conn.query_row(
        "SELECT cell_id FROM registry WHERE path = ?1",
        [path],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Root cell `main` (logical `/`), a `/fetch` to resume at the scope root, and
/// a hive `/unit` with a `/unit/q` in it for the same thing at depth. Both
/// cells are stateful, so they boot `NotYetSpawned` — not running, which is what
/// makes them resumable at all (an `Awake` target is `resume_requires_stopped_cell`).
/// Each gets a populated `cell.db` so a re-seed would be visible.
async fn bootstrapped_colony() -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().unwrap();
    write(td.path(), "main/config.json", HIVE_CONFIG);
    write(td.path(), "main/fetch/config.json", CELL_CONFIG);
    write(td.path(), "main/spare/config.json", CELL_CONFIG);
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
    // AFTER bootstrap: boot runs an integrity `quick_check` over every on-disk
    // `cell.db`, so the marker cannot be there yet. Nothing opens these files
    // afterwards — both cells are `NotYetSpawned` — so a re-seed on Resume would
    // be the only thing that could change them.
    std::fs::write(td.path().join("main/fetch/cell.db"), DB_MARKER).unwrap();
    std::fs::write(td.path().join("main/unit/q/cell.db"), DB_MARKER).unwrap();
    (td, h)
}

/// One resume, end to end: the mutation commits, the persisted `registry` row
/// still carries the `cell_id` the node had before, and its `cell.db` is
/// byte-identical. `cell_dir` is the on-disk directory of the logical `path`
/// (spec overview Z.331 anchors logical paths under the root cell dir `main`).
async fn assert_resumes(scope: &str, name: &str, path: &str, cell_dir: &str) {
    let (td, h) = bootstrapped_colony().await;
    let cell_id_before = live_cell_id(&h, path).await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": scope,
            "diff": {"add_nodes": [{"name": name, "template": "persist_mock"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "scope '{scope}' + add_nodes '{name}' names the existing {path} and must \
         resume it; got {outcome:?}"
    );

    h.shutdown().await;
    assert_eq!(
        persisted_cell_id(td.path(), path).as_deref(),
        Some(cell_id_before.as_str()),
        "the persisted registry row for {path} keeps its cell_id — a Resume is \
         the same node, not a re-mint (scope '{scope}', name '{name}')"
    );
    assert_eq!(
        std::fs::read(td.path().join(cell_dir).join("cell.db")).unwrap(),
        DB_MARKER,
        "and its cell.db survives byte-identically (scope '{scope}', name '{name}')"
    );
}

// ── one node, every spelling of the name that resumes it ────────────────────

/// The reported pair at the scope root: `./fetch` was the red cell, `fetch` was
/// green, and both name `/fetch`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_spellings_of_a_resume_target_meet_at_the_scope_root() {
    for prefix in SPELLINGS {
        assert_resumes("/", &format!("{prefix}fetch"), "/fetch", "main/fetch").await;
    }
}

/// The same pair one scope down. Nothing about the defect was root-specific —
/// `registry_names` is scope-filtered, and the raw spelling missed it in every
/// scope alike.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_two_spellings_of_a_resume_target_meet_inside_a_nested_scope() {
    for prefix in SPELLINGS {
        assert_resumes("/unit", &format!("{prefix}q"), "/unit/q", "main/unit/q").await;
    }
}

/// And at depth, where the name selects the absolute-path namespace instead.
/// This half has worked all along — `resume_targets` was pushed through
/// `resolve_scoped_path`, so `deep_registry_paths` was always filtered
/// correctly — and canonicalising the short half must not disturb it. It is
/// also the reason a deep name has no business in `resume_names`: nothing ever
/// queries that list with a path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_name_still_resumes_the_node_it_addresses() {
    for prefix in SPELLINGS {
        assert_resumes("/", &format!("{prefix}unit/q"), "/unit/q", "main/unit/q").await;
    }
}

// ── and what must survive the cancel ────────────────────────────────────────

/// The resume cancel removes a name from the collision set, so it has to remove
/// exactly one. A path that no node occupies is no resume target, and two
/// entries claiming it are still a `naming_collision` — in either spelling,
/// since the claims are compared resolved (#195).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_path_that_is_no_resume_target_still_collides() {
    for prefix in SPELLINGS {
        let (_td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {"add_nodes": [
                    {"name": "fresh", "template": "persist_mock"},
                    {"name": format!("{prefix}fresh"), "template": "persist_mock"}
                ]}
            }),
        )
        .await;
        assert!(
            matches!(
                outcome,
                MutationOutcome::Rejected { ref error_code, .. } if error_code == "naming_collision"
            ),
            "two add_nodes claiming the fresh /fresh must stay a naming_collision \
             for '{prefix}fresh'; got {outcome:?}"
        );
        h.shutdown().await;
    }
}

/// A Resume is this diff saying "that path is mine", not an amnesty on the
/// name. Now that `./fetch` cancels `fetch` in `registry_names`, a second entry
/// of the same diff aiming at `/fetch` could have slipped through the pre-state
/// check — the duplicate-claim guard is what still refuses it, and it refuses
/// it however either side is spelled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_entry_aiming_at_the_resume_target_is_still_refused() {
    for prefix in SPELLINGS {
        let (_td, h) = bootstrapped_colony().await;
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {
                    "add_nodes": [{"name": "./fetch", "template": "persist_mock"}],
                    "swap_nodes": [{
                        "match": {"name": "spare"},
                        "with": {"template": "persist_mock", "name": format!("{prefix}fetch")}
                    }]
                }
            }),
        )
        .await;
        assert!(
            matches!(
                outcome,
                MutationOutcome::Rejected { ref error_code, .. } if error_code == "naming_collision"
            ),
            "a with-side instantiate at the resume target must stay a \
             naming_collision for '{prefix}fetch'; got {outcome:?}"
        );
        h.shutdown().await;
    }
}
