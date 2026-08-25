//! GH #163 — a surface can be installed into a colony that is already running.
//!
//! Three separate rules had to hold for that sentence, and each one is a
//! separate defect if it stops holding. They are asserted here together because
//! the feature is the conjunction: fixing any two of them still leaves a canvas
//! that cannot be installed.
//!
//! 1. **The egress door is not a place.** A marked answer leaves from whichever
//!    hive it ran out of graph at, so the lane back out is `-> .` and never
//!    leaves the subtree. Direct-Mode (`EgressPolicy::All`) is unchanged and
//!    still root-only — the negative half of that is pinned in `colony.rs`'s own
//!    unit tests, and the unmarked case is pinned below.
//! 2. **Containment lets exactly two colony endpoints through.** The exemption is
//!    an enumerated list, not a `/colony/*` prefix: a template may declare its own
//!    lane to `/colony/graph` (read-only topology) and to `/colony/ledger`
//!    (counts, never content) — both drawable classes. Everything else, including
//!    `/colony/mutations`, stays out of bounds.
//! 3. **A stray directory never kills a boot.** A hive directory somebody placed
//!    by hand is reported and skipped, not planned — planning its edges made
//!    every endpoint dangle and turned a healthy colony into a restart loop.

use meclaw_colony::{
    BootState, CellFactoryRegistry, ColonyConfig, ColonyDb, ColonyMsg, ColonyRuntime,
    bootstrap_from_filesystem, colony_task, probe_boot_state,
};
use meclaw_core::{Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::echo::EchoCellFactory;
use meclaw_testing::mocks::EchoMockCell;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

const MARK: &str = "surface_reply";

// ──────────────────────────────────────────────────────────────────────────────
// 1. The door
// ──────────────────────────────────────────────────────────────────────────────

/// A colony whose only lane is `/sub/a -> /sub`: a cell inside a hive handing its
/// answer to its OWN hive, which is the shape `./render -> .` has after
/// resolution. `/sub` has no out-edge, so this is a hive dead end one level below
/// the root.
async fn colony_with_a_cell_answering_into_its_own_hive() -> (
    tempfile::TempDir,
    ColonyHandle,
    tokio::sync::mpsc::Receiver<Message>,
) {
    let td = tempfile::TempDir::new().unwrap();
    let (h, rx) = ColonyHandle::new_with_marked_egress_at(&td, vec![], MARK);
    h.spawn(Path::new("/sub/a"), || {
        EchoMockCell::new(Path::new("/sub/a")).emitted_target(Path::new("/sub"))
    })
    .await;
    h.add_hive_scope(Path::new("/")).await;
    h.add_hive_scope(Path::new("/sub")).await;
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/sub/a"),
        Path::new("/sub"),
    )
    .await;
    (td, h, rx)
}

/// **The load-bearing test.** The answer to an HTTP request leaves the colony
/// from a hive that is not the root. Before #163 this dead-lettered, which is why
/// a surface's reply lane had to be `-> /` and why a surface could therefore only
/// be created at a colony's first boot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_marked_answer_leaves_the_colony_from_a_non_root_hive() {
    let (_td, h, mut rx) = colony_with_a_cell_answering_into_its_own_hive().await;

    let mut ctx = meclaw_core::serde_json::Map::new();
    ctx.insert(MARK.to_string(), meclaw_core::serde_json::json!("1"));
    ctx.insert(
        "surface_request".to_string(),
        meclaw_core::serde_json::json!("req-163"),
    );
    h.send(
        MessageBuilder::new(Path::new("/sub/a"))
            .headers(meclaw_core::Headers::from_parts(ctx, Default::default()))
            .build(),
    )
    .await;

    let out = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
        .await
        .expect("a marked message must leave the colony from /sub")
        .expect("egress channel closed");
    assert_eq!(
        out.headers
            .context
            .get("surface_request")
            .and_then(|v| v.as_str()),
        Some("req-163"),
        "and it must still be correlatable to the browser that asked"
    );
    h.shutdown().await;
}

/// The other half, on the same wiring: **unmarked** still dead-letters at the
/// same hive. The door is opened by the marker, not by the geography — a fix that
/// simply stopped checking the hive path would pass the test above and fail this
/// one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unmarked_message_at_the_same_hive_still_dead_letters() {
    let (_td, h, mut rx) = colony_with_a_cell_answering_into_its_own_hive().await;

    h.send(MessageBuilder::new(Path::new("/sub/a")).build())
        .await;

    let mut dlq = Vec::new();
    for _ in 0..300 {
        dlq = h.drain_dead_letters().await;
        if !dlq.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(dlq.len(), 1, "an unmarked hive dead end must dead-letter");
    assert!(
        matches!(dlq[0].reason, meclaw_colony::DeadLetterReason::HiveNoRoute),
        "and with the unchanged reason, got {:?}",
        dlq[0].reason
    );
    assert!(
        rx.try_recv().is_err(),
        "an unmarked message must never reach the egress channel"
    );
    h.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────────
// 2. Containment
// ──────────────────────────────────────────────────────────────────────────────

/// Write a two-cell subtree template whose hive declares one edge to `target`.
fn template_with_absolute_lane(dir: &std::path::Path, target: &str) {
    std::fs::create_dir_all(dir.join("probe")).unwrap();
    std::fs::write(
        dir.join("config.json"),
        meclaw_core::serde_json::to_string(&meclaw_core::serde_json::json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [{"from": "./probe", "to": target}]}}
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("probe/config.json"),
        r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("template.json"),
        r#"{"name":"lane","version":"0.1.0"}"#,
    )
    .unwrap();
}

/// A template's own lane to the colony's read-only topology endpoint resolves at
/// a nested scope. This is the check that used to answer
/// `Schema("subtree edge endpoint /colony/graph escapes subtree root …")`.
#[test]
fn a_template_may_declare_its_own_lane_to_the_colony_graph() {
    let td = tempfile::TempDir::new().unwrap();
    template_with_absolute_lane(td.path(), "/colony/graph");

    let resolved = meclaw_colony::mutation::subtree::resolve_subtree(
        td.path(),
        "/org/acme/member",
        "canvy",
        &meclaw_colony::templates::TemplatesRegistry::default(),
    )
    .expect("a lane to /colony/graph must be in bounds at any scope");
    assert!(
        resolved
            .internal_edges
            .iter()
            .any(|(f, t)| f == "/org/acme/member/canvy/probe" && t == "/colony/graph"),
        "the resolved lane must survive containment, got {:?}",
        resolved.internal_edges
    );
}

/// GH #267 — the ledger endpoint is the second drawable one. It answers counts,
/// never rows, so a template may declare its own lane to it at a nested scope,
/// exactly like the topology endpoint above.
#[test]
fn a_template_may_declare_its_own_lane_to_the_colony_ledger() {
    let td = tempfile::TempDir::new().unwrap();
    template_with_absolute_lane(td.path(), "/colony/ledger");

    let resolved = meclaw_colony::mutation::subtree::resolve_subtree(
        td.path(),
        "/org/acme/member",
        "canvy",
        &meclaw_colony::templates::TemplatesRegistry::default(),
    )
    .expect("a lane to /colony/ledger must be in bounds at any scope");
    assert!(
        resolved
            .internal_edges
            .iter()
            .any(|(f, t)| f == "/org/acme/member/canvy/probe" && t == "/colony/ledger"),
        "the resolved lane must survive containment, got {:?}",
        resolved.internal_edges
    );
}

/// And the exemption is an enumerated list, not a `/colony/*` prefix.
/// `/colony/mutations` is authority transfer; a template that draws it is still
/// rejected.
#[test]
fn a_template_may_not_declare_a_lane_to_the_mutation_endpoint() {
    let td = tempfile::TempDir::new().unwrap();
    template_with_absolute_lane(td.path(), "/colony/mutations");

    let err = meclaw_colony::mutation::subtree::resolve_subtree(
        td.path(),
        "/org/acme/member",
        "canvy",
        &meclaw_colony::templates::TemplatesRegistry::default(),
    )
    .expect_err("/colony/mutations must stay out of bounds for a mutation");
    match err {
        meclaw_colony::mutation::MutationError::Schema(s) => assert!(
            s.contains("/colony/mutations") && s.contains("escapes subtree root"),
            "unexpected reason: {s}"
        ),
        other => panic!("expected Schema(escapes subtree root), got {other:?}"),
    }
}

/// Widening the list is exactly the moment its edge needs a test: `/colony/trace`
/// hands out other cells' message content and did not move. A template that draws
/// it is still rejected.
#[test]
fn a_template_may_not_declare_a_lane_to_the_trace_endpoint() {
    let td = tempfile::TempDir::new().unwrap();
    template_with_absolute_lane(td.path(), "/colony/trace");

    let err = meclaw_colony::mutation::subtree::resolve_subtree(
        td.path(),
        "/org/acme/member",
        "canvy",
        &meclaw_colony::templates::TemplatesRegistry::default(),
    )
    .expect_err("/colony/trace must stay out of bounds for a mutation");
    match err {
        meclaw_colony::mutation::MutationError::Schema(s) => assert!(
            s.contains("/colony/trace") && s.contains("escapes subtree root"),
            "unexpected reason: {s}"
        ),
        other => panic!("expected Schema(escapes subtree root), got {other:?}"),
    }
}

/// The shipped canvas is the reason this exists, so it is asserted on the shipped
/// canvas: both of its unusual lanes resolve at a deeply nested scope. Guarded
/// like every other template-reading test (GH #49).
#[test]
fn the_shipped_canvy_hive_resolves_at_a_nested_scope() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("templates/canvy");
    if !src.join("config.json").is_file() {
        return;
    }
    let resolved = meclaw_colony::mutation::subtree::resolve_subtree(
        &src,
        "/org/acme/member/alice",
        "canvy",
        &meclaw_colony::templates::TemplatesRegistry::default(),
    )
    .expect("the shipped canvy hive must instantiate at a nested scope");
    let has = |from: &str, to: &str| {
        resolved
            .internal_edges
            .iter()
            .any(|(f, t)| f == from && t == to)
    };
    assert!(
        has(
            "/org/acme/member/alice/canvy/render",
            "/org/acme/member/alice/canvy"
        ),
        "the answer lane must end at the hive itself (`-> .`), got {:?}",
        resolved.internal_edges
    );
    assert!(
        has("/org/acme/member/alice/canvy/probe", "/colony/graph"),
        "the hive must carry its own topology lane, got {:?}",
        resolved.internal_edges
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// 3. The stray directory
// ──────────────────────────────────────────────────────────────────────────────

fn factories() -> CellFactoryRegistry {
    let mut f = CellFactoryRegistry::new();
    f.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn meclaw_colony::CellFactory>,
    );
    f
}

/// One boot, as join handles, so a boot failure is observable instead of fatal.
#[allow(clippy::type_complexity)]
fn boot(
    root: &std::path::Path,
) -> (
    mpsc::Sender<ColonyMsg>,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<Result<meclaw_colony::BootstrapReport, meclaw_colony::BootstrapErrors>>,
) {
    let (inbox_tx, inbox_rx) = mpsc::channel(64);
    let (outputs_tx, outputs_rx) = mpsc::channel(64);
    let db = ColonyDb::open(&root.join("colony.db")).expect("open colony.db");
    let f = factories();
    let colony_join = tokio::spawn(colony_task(meclaw_colony::ColonyTaskConfig::new(
        inbox_tx.clone(),
        inbox_rx,
        outputs_tx.clone(),
        outputs_rx,
        db,
        f.clone(),
        root.to_path_buf(),
        ColonyConfig::default(),
        None,
        None,
    )));
    let runtime = ColonyRuntime {
        inbox_tx: inbox_tx.clone(),
        outputs_tx,
        colony_config: ColonyConfig::default(),
        blob_store: None,
    };
    let root_owned = root.to_path_buf();
    let apply_join =
        tokio::spawn(async move { bootstrap_from_filesystem(&root_owned, &f, &runtime).await });
    (inbox_tx, colony_join, apply_join)
}

async fn shutdown(inbox_tx: mpsc::Sender<ColonyMsg>, colony_join: tokio::task::JoinHandle<()>) {
    let (ack_tx, ack_rx) = oneshot::channel();
    let _ = inbox_tx.send(ColonyMsg::Shutdown { ack: ack_tx }).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), ack_rx).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), colony_join).await;
}

/// A hive directory placed by hand into a colony that already booted must not be
/// able to kill it. Verified on the exact shape that did: a hive whose
/// `params.graph` wires its own children, none of which is adopted.
///
/// The receipt is positive on both sides — the colony comes up AND the stray tree
/// is reported as unregistered, so "it boots" cannot be achieved by silently
/// adopting the directory (which would be the other, worse, way to make this test
/// green).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hand_placed_hive_directory_does_not_break_the_next_boot() {
    let td = tempfile::TempDir::new().unwrap();
    let main = td.path().join("main");
    std::fs::create_dir_all(main.join("solo")).unwrap();
    std::fs::write(main.join("config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
    std::fs::write(
        main.join("solo/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/solo"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // --- Boot 1: healthy colony. ---
    let (inbox1, colony1, apply1) = boot(td.path());
    apply1
        .await
        .expect("boot-1 apply task must not panic")
        .expect("boot 1 must succeed");
    shutdown(inbox1, colony1).await;
    assert_eq!(
        probe_boot_state(&td.path().join("colony.db")).expect("probe"),
        BootState::Reboot,
        "boot 2 must be classified as a Reboot for this test to mean anything"
    );

    // --- Somebody copies a hive tree in by hand (the #163 recovery scenario). ---
    let stray = main.join("stray");
    std::fs::create_dir_all(stray.join("inner")).unwrap();
    std::fs::write(
        stray.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./inner","to":"."}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        stray.join("inner/config.json"),
        r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // --- Boot 2: MUST come up. Before the fix: BootstrapErrors{DanglingEndpoint}. ---
    let (inbox2, colony2, apply2) = boot(td.path());
    let report = apply2
        .await
        .expect("boot-2 apply task must not panic")
        .expect("a stray directory must never turn a healthy colony into a boot failure");
    shutdown(inbox2, colony2).await;

    // The registry is untouched — the stray tree was reported, never adopted.
    let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
    let stray_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM registry WHERE path LIKE '%stray%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let stray_hives: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM hive_scopes WHERE path LIKE '%stray%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stray_rows, 0, "the stray cell must not be registered");
    assert_eq!(stray_hives, 0, "the stray hive must not be registered");
    let _ = report;
}

/// The plan-level half of the same rule, asserted where the decision is made:
/// the stray hive lands in `unregistered_nodes` and contributes **no** edges.
#[test]
fn a_stray_hive_is_reported_and_contributes_no_edges() {
    let td = tempfile::TempDir::new().unwrap();
    let main = td.path().join("main");
    std::fs::create_dir_all(main.join("stray/inner")).unwrap();
    std::fs::write(main.join("config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
    std::fs::write(
        main.join("stray/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":"./inner","to":"."}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        main.join("stray/inner/config.json"),
        r#"{"cell":{"type":"echo"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // An overlay that knows only the root: the reboot's view of a tree whose
    // stray subtree was never instantiated.
    let mut overlay = meclaw_colony::persist::colony_db::RegistryOverlay::new();
    overlay.insert(
        meclaw_core::Path::new("/"),
        (meclaw_core::Uuid::now_v7(), "active".to_string()),
    );
    let plan = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &factories(),
        &overlay,
        BootState::Reboot,
        None,
    )
    .expect("planning must not fail on a stray subtree");

    assert!(
        plan.unregistered_nodes
            .iter()
            .any(|p| p.as_str() == "/stray"),
        "the stray hive must be reported, got {:?}",
        plan.unregistered_nodes
    );
    assert!(
        !plan
            .edges
            .iter()
            .any(|e| e.from.as_str().contains("stray") || e.to.as_str().contains("stray")),
        "a stray hive must contribute no edges — that is what used to dangle"
    );
    assert!(
        !plan.hives.iter().any(|h| h.path.as_str() == "/stray"),
        "and it must not be planned as a hive scope either"
    );
}
