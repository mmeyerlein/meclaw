//! GH #276 — a rejected mutation leaves no residue.
//!
//! A live colony answered HTTP 422 for a mutation and kept half of it: the
//! `add_nodes` half was in the registry, `status = active`, while none of the
//! edges from the same diff existed and the `mutation_log` row still said
//! `in_flight`. The caller was told nothing happened; the registry said
//! otherwise. The severity of that depends only on what the diff contained —
//! the first occurrence left a `proxy` holding a bot token the colony already
//! ran a second consumer for.
//!
//! Four reject codes can fire after the node half of a diff has been applied:
//!
//! | code | kind |
//! |---|---|
//! | `required_drain_missing` | post-state validation |
//! | `hive_contract` (lane doors) | post-state validation |
//! | `stop_wiring_unavailable` | runtime condition |
//! | `term_timeout` | runtime condition |
//!
//! Each of the four is pinned here against the same three assertions: the
//! reject names its code, the diff's node is in NO registry (neither the
//! in-memory one the router uses nor the `registry` table an operator reads),
//! and the `mutation_log` row is terminal.
//!
//! A fifth test drives the shape the issue was actually REPORTED in — a diff
//! that instantiates a single-cell template AND a four-node subtree template,
//! not an adoption. That reaches the two registration sites the adoption form
//! never touches (the spawn loop's fresh `StagedDir` and step 9c(1)'s subtree
//! cells) and adds the two claims the adoption form cannot make: the renamed-in
//! directories are gone afterwards, and a cell that had spawned `Awake` finished
//! its teardown BEFORE its directory was swept.

use meclaw_colony::api_dto::{ReadMutationsAuditReply, ReadRegistryReply};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, RespawnFn, SpawnedCellKind,
    bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{CellEmission, JsonValue, Message, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

// ── topology helpers (same shape as the gh147 / gh173 suites) ────────────────

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn echo_cell(td: &std::path::Path, rel: &str, emitted_target: &str) {
    std::fs::create_dir_all(td.join(rel)).unwrap();
    std::fs::write(
        td.join(rel).join("config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"{emitted_target}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

fn hive(td: &std::path::Path, rel: &str, params: &str) {
    std::fs::create_dir_all(td.join(rel)).unwrap();
    std::fs::write(
        td.join(rel).join("config.json"),
        format!(r#"{{"cell":{{"type":"hive"}},"params":{params}}}"#),
    )
    .unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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

async fn registry_paths(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
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
        .unwrap();
    let mut paths: Vec<String> = ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect();
    paths.sort();
    paths
}

async fn mutation_log_statuses(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadMutationsAuditReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadMutationsAudit {
            since: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.status)
        .collect()
}

/// The `registry` table an operator reads after the fact — the durable half of
/// the claim, and the one the issue's `colony.db` artefact was taken from.
fn persisted_registry_paths(db: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare("SELECT path FROM registry ORDER BY path")
        .unwrap();
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows
}

/// The node the diff adds. Created AFTER boot so it is on disk and unregistered
/// — the `adopt` shape of the issue's second reproduction, which needs no
/// template registry and goes through the same `add_nodes` apply half.
fn place_orphan(td: &std::path::Path) {
    echo_cell(td, "main/adopted", "/adopted");
}

fn adopt_entry() -> Value {
    json!({"name": "adopted", "adopt": {"type": "echo"}})
}

/// The three claims of #276, asserted against a live colony and then against
/// `colony.db` once the writer has drained. `forbidden` names the paths the diff
/// would have registered.
async fn assert_no_residue(
    h: ColonyHandle,
    root: &std::path::Path,
    before: &[String],
    forbidden: &[&str],
) {
    let after = registry_paths(&h).await;
    assert_eq!(
        before,
        &after[..],
        "a rejected mutation registers nothing: {before:?} vs {after:?}"
    );
    let statuses = mutation_log_statuses(&h).await;
    assert!(
        !statuses.iter().any(|s| s == "in_flight"),
        "every mutation_log row of a finished mutation is terminal, got {statuses:?}"
    );
    h.shutdown().await;
    let persisted = persisted_registry_paths(&root.join("colony.db"));
    for path in forbidden {
        assert!(
            !persisted.iter().any(|p| p == path),
            "the rejected node {path} reaches no registry row either: {persisted:?}"
        );
    }
}

// ── 1. required_drain_missing ───────────────────────────────────────────────

/// A hive that pairs its `glue` port with a `reject` drain — the declaration
/// the first occurrence of #276 tripped over.
const DRAIN_DECLARED: &str = r#"{"ports":["glue"],
  "required_drains":[{"port":"glue","hop":{"route":"reject"},
    "because":"a refused block leaves on this route and nobody learns of it"}],
  "graph":{"edges":[]}}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_required_drain_reject_registers_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    hive(td.path(), "main", r#"{"graph":{"edges":[]}}"#);
    echo_cell(td.path(), "main/caller", "/caller");
    hive(td.path(), "main/mem", DRAIN_DECLARED);
    echo_cell(td.path(), "main/mem/glue", "/mem/glue");
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    place_orphan(td.path());
    let before = registry_paths(&h).await;

    // One diff: a node to adopt, and the ingress without its drain.
    let outcome = send_mutation(
        &h,
        json!({"diff": {
            "add_nodes": [adopt_entry()],
            "add_edges": [{"from": "./caller", "to": "./mem/glue"}]
        }}),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "required_drain_missing", "{outcome:?}")
        }
        other => panic!("the bare ingress must be refused, got {other:?}"),
    }

    assert_no_residue(h, td.path(), &before, &["/adopted"]).await;
}

// ── 2. hive_contract (lane doors) ───────────────────────────────────────────

/// A hive whose door was moved to another lane while the contract kept naming
/// `in_batch` — the post-state half of the contract check.
const DOORLESS: &str = r#"{
  "ports": [],
  "contract": {
    "accepts": [{"route": "in_batch", "because": "one closed session as a single write batch"}],
    "emits": []
  },
  "graph": {"edges": [
    {"from": ".", "to": "./glue", "condition": "has(hop.route) && hop.route == 'in_other'"}
  ]}}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hive_contract_reject_registers_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    hive(td.path(), "main", r#"{"graph":{"edges":[]}}"#);
    echo_cell(td.path(), "main/caller", "/caller");
    hive(td.path(), "main/mem", DOORLESS);
    echo_cell(td.path(), "main/mem/glue", "/mem/glue");
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");

    place_orphan(td.path());
    let before = registry_paths(&h).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {
            "add_nodes": [adopt_entry()],
            "add_edges": [{"from": "./caller", "to": "./mem",
                           "modifier": {"set_hop": {"route": "'in_batch'"}}}]
        }}),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "hive_contract", "{outcome:?}")
        }
        other => panic!("a lane without a door must be refused, got {other:?}"),
    }

    assert_no_residue(h, td.path(), &before, &["/adopted"]).await;
}

// ── the two runtime rejects ─────────────────────────────────────────────────

/// A cell whose FIRST task honours the peace-stop and whose RESPAWNED task
/// carries no stop wiring — the long-running survivor class the stop-wiring
/// guard is the permanent backstop for. `wedged` instead makes even the first
/// task ignore the stop, so the death-ack never fires and the disconnect runs
/// into the term-timeout.
///
/// `torn_down` counts task teardowns and is bumped BEFORE the death ack is sent,
/// mirroring a real cell (close `cell.db`, then answer). A test can therefore
/// read the counter right after a mutation returned and know whether the colony
/// waited for the teardown or ran ahead of it.
struct SurvivorFactory {
    wedged: bool,
    torn_down: Arc<AtomicUsize>,
}

fn build_task(
    stop_rx: Option<oneshot::Receiver<()>>,
    death_ack: Option<oneshot::Sender<()>>,
    wedged: bool,
    torn_down: Arc<AtomicUsize>,
) -> (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
) {
    let (tx, _rx) = mpsc::channel::<Message>(16);
    let (peace_tx, peace_rx) = oneshot::channel::<()>();
    let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        // Keep the backstop sender alive for as long as the task is: a dropped
        // sender is how the watcher learns of a backstop death.
        let _keep = _backstop_tx;
        match stop_rx {
            Some(rx) if !wedged => {
                let _ = rx.await;
                let _ = peace_tx.send(());
                // A teardown that TAKES time — a real cell closes `cell.db` here.
                // This is a semantic timing discriminator, not a failure marker:
                // it has to outlast any incidental scheduling between the reject
                // and the assertion, and stay far below the 5 s term-timeout the
                // rollback waits with. 150 ms sits between the two by an order of
                // magnitude on each side.
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                torn_down.fetch_add(1, Ordering::SeqCst);
                if let Some(ack) = death_ack {
                    let _ = ack.send(());
                }
            }
            // A wedged cell, or a respawn without stop wiring: the task simply
            // stays up. Nothing but the test's end takes it down.
            _ => {
                let _keep_ack = death_ack;
                let _keep_peace = peace_tx;
                std::future::pending::<()>().await;
            }
        }
    });
    (tx, join, peace_rx, backstop_rx)
}

impl CellFactory for SurvivorFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn spawn_cell(
        self: Arc<Self>,
        _path: Path,
        _params: JsonValue,
        _outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let wedged = self.wedged;
        let torn_down = self.torn_down.clone();
        let (sender, join, peace_rx, backstop_rx) =
            build_task(Some(stop_rx), Some(death_ack_tx), wedged, torn_down.clone());
        // No stop wiring on the respawn, and no `renotify_stop_wiring`: the
        // survivor the guard exists for.
        let respawn: RespawnFn =
            Box::new(move || build_task(None, None, wedged, torn_down.clone()));
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }
}

/// Boot a colony holding one survivor cell `/lr` and one echo peer `/peer`,
/// with the survivor already wired to the peer (so it is active). Returns the
/// handle and the shared teardown counter of the `lr` factory.
async fn boot_with_survivor(
    td: &tempfile::TempDir,
    wedged: bool,
) -> (ColonyHandle, Arc<AtomicUsize>) {
    let torn_down = Arc::new(AtomicUsize::new(0));
    hive(td.path(), "main", r#"{"graph":{"edges":[]}}"#);
    echo_cell(td.path(), "main/peer", "/peer");
    std::fs::create_dir_all(td.path().join("main/lr")).unwrap();
    std::fs::write(
        td.path().join("main/lr/config.json"),
        r#"{"cell":{"type":"lr","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    let factories: Vec<(String, Arc<dyn CellFactory>)> = vec![
        (
            "echo".to_string(),
            Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
        ),
        (
            "lr".to_string(),
            Arc::new(SurvivorFactory {
                wedged,
                torn_down: torn_down.clone(),
            }) as Arc<dyn CellFactory>,
        ),
    ];
    let h = ColonyHandle::new_with_factories_at(td, factories.clone());
    let mut reg = CellFactoryRegistry::new();
    for (name, f) in factories {
        reg.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("bootstrap");
    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_edges": [{"from": "./lr", "to": "./peer"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "wiring the survivor must commit, got {outcome:?}"
    );
    (h, torn_down)
}

// ── 3. stop_wiring_unavailable ──────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_wiring_reject_registers_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, _torn_down) = boot_with_survivor(&td, false).await;

    // Disconnect once: the first task has live stop wiring and comes down.
    let outcome = send_mutation(
        &h,
        json!({"diff": {"remove_edges": [{"match": {"from": "./lr", "to": "./peer"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the first disconnect commits, got {outcome:?}"
    );
    // Reconnect: eager respawn, Awake again — but without stop wiring.
    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_edges": [{"from": "./lr", "to": "./peer"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the reconnect commits, got {outcome:?}"
    );

    place_orphan(td.path());
    let before = registry_paths(&h).await;

    // The second disconnect, in a diff that also adds a node.
    let outcome = send_mutation(
        &h,
        json!({"diff": {
            "add_nodes": [adopt_entry()],
            "remove_edges": [{"match": {"from": "./lr", "to": "./peer"}}]
        }}),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "stop_wiring_unavailable", "{outcome:?}")
        }
        other => panic!("the unwired disconnect must be refused, got {other:?}"),
    }

    assert_no_residue(h, td.path(), &before, &["/adopted"]).await;
}

// ── 3b. the shape the issue was reported in: a multi-cell TEMPLATE ──────────

/// The names under `{root}/.staging`. A committed mutation leaves its own empty
/// `.staging/<id>` behind (pre-existing, unrelated to #276), so the claim here is
/// a DIFFERENCE: the refused mutation adds nothing that was not there before.
fn staging_entries(root: &std::path::Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root.join(".staging"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
}

/// One single-cell template and one four-node subtree template — the two shapes
/// `add_nodes` can instantiate, and the two the rollback has to undo in
/// different places (`StagedDir` in step 9, `StagedRenameRoot` in step 9c(1)).
fn write_templates(root: &std::path::Path) {
    let solo = root.join("templates").join("solo");
    write(&solo, "template.json", r#"{"name":"solo"}"#);
    // `lr`, not `echo`: the single-cell half of the diff spawns Awake with real
    // stop wiring, so the rollback's peace-stop and its wait are observable.
    write(
        &solo,
        "config.json",
        r#"{"cell":{"type":"lr","timeout":-1},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let pipe = root.join("templates").join("pipe");
    write(&pipe, "template.json", r#"{"name":"pipe"}"#);
    write(
        &pipe,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./x","to":"./y"},
            {"from":"./y","to":"./z"}
        ]}}}"#,
    );
    for name in ["x", "y", "z"] {
        write(
            &pipe,
            &format!("{name}/config.json"),
            &format!(
                r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"/pipe/{name}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
            ),
        );
    }
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

/// The first occurrence in #276 was a hive TEMPLATE with thirteen cells, not an
/// adoption — a shape whose directories are renamed in by `apply_mutation` and
/// whose registration runs through step 9c(1) rather than the single-cell spawn
/// loop. Both halves are in this diff, so the rollback has to reach both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_template_instantiation_leaves_no_registry_row_and_no_directory() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, torn_down) = boot_with_survivor(&td, false).await;
    write_templates(td.path());
    rescan_templates(&h, td.path().join("templates")).await;

    // Bring `/lr` into the unwired-Awake state the guard is the backstop for.
    let outcome = send_mutation(
        &h,
        json!({"diff": {"remove_edges": [{"match": {"from": "./lr", "to": "./peer"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{outcome:?}"
    );
    let outcome = send_mutation(
        &h,
        json!({"diff": {"add_edges": [{"from": "./lr", "to": "./peer"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{outcome:?}"
    );

    let before = registry_paths(&h).await;
    let staging_before = staging_entries(td.path());
    let torn_down_before = torn_down.load(Ordering::SeqCst);

    let outcome = send_mutation(
        &h,
        json!({"diff": {
            "add_nodes": [
                {"name": "solo", "template": "solo"},
                {"name": "pipe", "template": "pipe"}
            ],
            "remove_edges": [{"match": {"from": "./lr", "to": "./peer"}}]
        }}),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "stop_wiring_unavailable", "{outcome:?}")
        }
        other => panic!("the unwired disconnect must be refused, got {other:?}"),
    }

    // The rolled-back `/solo` was spawned Awake. Its task had finished tearing
    // itself down BEFORE `handle_mutation` returned — which is what stands
    // between the sweep below and a `remove_dir_all` walking a directory whose
    // `cell.db` is still open.
    assert_eq!(
        torn_down.load(Ordering::SeqCst),
        torn_down_before + 1,
        "the rollback waits for the peace-stopped cell to come down before sweeping"
    );

    // The directories: neither the single-cell one nor the subtree's rename-root
    // may survive, and the staging tree must be gone too.
    for rel in ["main/solo", "main/pipe"] {
        assert!(
            !td.path().join(rel).exists(),
            "{rel} is the residue of a refused mutation and must be swept"
        );
    }
    assert_eq!(
        staging_before,
        staging_entries(td.path()),
        "the staging tree the refused mutation built must be gone again"
    );

    assert_no_residue(
        h,
        td.path(),
        &before,
        &["/solo", "/pipe", "/pipe/x", "/pipe/y", "/pipe/z"],
    )
    .await;
}

// ── 4. term_timeout ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_term_timeout_reject_registers_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    // Wedged: the task never answers the peace-stop, so the inline death-ack
    // wait runs into the 5 s default budget. The budget is deliberately NOT
    // overridden — `set_term_timeout_ms_for_test` is process-global and would
    // reach the other tests in this binary.
    let (h, _torn_down) = boot_with_survivor(&td, true).await;

    place_orphan(td.path());
    let before = registry_paths(&h).await;

    let outcome = send_mutation(
        &h,
        json!({"diff": {
            "add_nodes": [adopt_entry()],
            "remove_edges": [{"match": {"from": "./lr", "to": "./peer"}}]
        }}),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(error_code, "term_timeout", "{outcome:?}")
        }
        other => panic!("the wedged disconnect must time out, got {other:?}"),
    }

    assert_no_residue(h, td.path(), &before, &["/adopted"]).await;
}
