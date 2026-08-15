//! Phase-13.5 a5-subtree T9: the canonical SUBTREE slice-demos.
//!
//! These are the not-yet-covered proofs of the A5 subtree slice. Demos (d), (e)
//! and (f) are ALREADY proven by existing tests and are NOT duplicated here:
//!
//! - (d) activity before/after `add_edges` →
//!   `tests/phase_13_5_t8b_subtree_apply.rs::add_edges_to_subtree_root_activates_whole_subtree`
//!   plus the inactive-on-instantiation assertion in
//!   `add_nodes_subtree_registers_all_cells_inactive_with_hive_scope_and_internal_edges`.
//! - (e) existing-path per-node resume (Paket-5 T12 F4-closure; was the A5
//!   `subtree_resume_unsupported` reject) →
//!   `tests/phase_13_5_t8b2_subtree_validation.rs::add_nodes_subtree_at_existing_root_path_resumes_per_node`.
//! - (f) validation reject / no `.staging` leak →
//!   `tests/phase_13_5_t8b2_subtree_validation.rs::add_nodes_subtree_with_internal_cycle_rejected_before_staging`
//!   (+ `add_nodes_subtree_with_edge_escaping_root_rejected`).
//!
//! The demos written HERE:
//!
//! (a) CORE E2E — a probe message flows through the WHOLE subtree including an
//!     INNER hive-transit hop, terminating at a recording cell INSIDE the subtree.
//! (b) cell_ids are fresh + pairwise-disjoint across two instantiations.
//! (c) the whole subtree survives a colony reboot.
//!
//! ── Topology for demo (a) (fully self-contained under the subtree root) ──
//!
//! All edges are real EdgeTable entries declared in each hive's `params.graph`,
//! resolved relative to the OWNING hive (per `stage_subtree`/`resolve_subtree`):
//!
//! ```text
//!   /p1               root hive       params.graph.edges:
//!                                        ./ingest -> ./mid      (/p1/ingest -> /p1/mid)
//!                                      (only a descendant↔descendant edge — the
//!                                       root hive is DISCONNECTED until anchor->p1)
//!   /p1/ingest        echo_sub cell   (forwarding cell; emit target is
//!                                       overridden by the ingest->mid edge)
//!   /p1/mid           INNER hive      params.graph.edges:
//!                                        . -> ./sink            (/p1/mid -> /p1/mid/sink)
//!   /p1/mid/sink      echo_sub cell   terminal proof cell INSIDE the subtree;
//!                                       echo_to=/capture (external CaptureCell)
//! ```
//!
//! Activation: ONE `add_edges` from a live anchor cell to the subtree-root hive
//! `/p1` connects the root hive; activity is edge-derived, so the whole edge-wired
//! chain (`/p1` -> `/p1/ingest` -> `/p1/mid` -> `/p1/mid/sink`) becomes active.
//! Before that single edge the whole subtree is inactive (the root hive's only
//! edge is internal descendant↔descendant wiring, which does not self-connect it).
//!
//! Flow for a probe addressed at the entry cell `/p1/ingest`:
//!   1. `/p1/ingest` echo_sub cell receives + emits; the `ingest -> mid` edge
//!      overrides the emit target → `/p1/mid`.
//!   2. `/p1/mid`    INNER-HIVE TRANSIT hop → out-edge `. -> ./sink` → `/p1/mid/sink`.
//!   3. `/p1/mid/sink` terminal inner cell receives (POSITIVE receipt) and
//!      forwards to the external `/capture` CaptureCell.
//!
//! How the inner-hive-transit traversal is PROVEN (positive + deep):
//!   - POSITIVE receipt: the `/capture` CaptureCell receives the probe. A receipt
//!     ⟺ `/p1/mid/sink` is live AND was routed to — which can only happen via
//!     `/p1/mid`'s out-edge (the sink's only inbound edge is `mid -> mid/sink`).
//!   - DEEP: a fresh rusqlite probe asserts the message_log carries a hop with
//!     `to_path == /p1/mid` — the inner hive WAS a real transit node on the path.

use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ContractView, MutationOutcome, RespawnFn,
    SpawnedCellKind, bootstrap_from_filesystem, cell_task,
};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

// ──────────────────────────────────────────────────────────────────────────────
// Test-local `echo_sub` factory that supports boot-inactive eager reconnect.
//
// The shared `meclaw_testing::factories::EchoCellFactory` does NOT implement
// `CellFactory::build_boot_inactive_respawn` → a subtree cell instantiated
// inactive via `add_nodes` would be flipped `active==true` on the activating
// `add_edges`, but its task would NOT actually be (re)spawned until a reboot
// (the inert respawn fallback). For demo (a)'s LIVE message flow we need a real
// respawn, exactly like a production eager (stateless/long-running) factory. This
// test-local factory mirrors `phase_13_5_slice_4_boot_inactive_eager_reconnect`'s
// `BootInactiveEagerFactory`, but spawns a real `EchoMockCell` (receive + forward
// to `params.echo_to`) so the subtree can carry traffic end-to-end.
// ──────────────────────────────────────────────────────────────────────────────

/// A stateless echo factory whose `spawn_cell` + `build_boot_inactive_respawn`
/// share the same task-construction closure (so a boot-inactive eager cell gets a
/// REAL respawn and runs immediately on `add_edges`-reconnect).
struct SubtreeEchoFactory;

/// Parse the mandatory `params.echo_to` path.
fn parse_echo_to(params: &JsonValue) -> Result<Path, String> {
    params
        .get("echo_to")
        .and_then(|v| v.as_str())
        .map(Path::new)
        .ok_or_else(|| "params.echo_to missing or not a string".to_string())
}

/// Build the closure that constructs a fresh `EchoMockCell` task. Shared verbatim
/// by `spawn_cell` (eager initial spawn) and `build_boot_inactive_respawn`.
fn make_echo_build(
    path: Path,
    echo_to: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
) -> impl Fn() -> (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
) {
    move || {
        let (tx, rx) = mpsc::channel::<Message>(1000);
        let (peace_tx, peace_rx) = oneshot::channel();
        let (_backstop_tx, backstop_rx) = oneshot::channel();
        let cell = EchoMockCell::new(path.clone()).echo_to(echo_to.clone());
        let p = path.clone();
        let o = outputs_tx.clone();
        let join = tokio::spawn(async move {
            let _keep_peace = peace_tx;
            cell_task(p, rx, o, cell, None, None).await;
        });
        (tx, join, peace_rx, backstop_rx)
    }
}

impl CellFactory for SubtreeEchoFactory {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        parse_echo_to(params).map(|_| ())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let echo_to = parse_echo_to(&params)?;
        let build = make_echo_build(path, echo_to, outputs_tx);
        let (sender, join, peace_rx, backstop_rx) = build();
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        let respawn: RespawnFn = Box::new(build);
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

    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<std::sync::Arc<meclaw_colony::DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        // Same wiring as `spawn_cell`'s respawn, but DO NOT call `build()` here →
        // no task spawned at boot/instantiation (boot-gating). The reconnect arm
        // calls this closure to spawn the live task immediately.
        let echo_to = parse_echo_to(&params).ok()?;
        let build = make_echo_build(path, echo_to, outputs_tx);
        Some(Box::new(build))
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Shared helpers (mirrors the t8b/t8b2 + hive_transit harness)
// ──────────────────────────────────────────────────────────────────────────────

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

async fn all_registry(h: &ColonyHandle) -> Vec<meclaw_colony::api_dto::RegistryEntryDto> {
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

async fn registry_entry(
    h: &ColonyHandle,
    path: &str,
) -> Option<meclaw_colony::api_dto::RegistryEntryDto> {
    all_registry(h).await.into_iter().find(|e| e.path == path)
}

fn echo_factory() -> Arc<dyn CellFactory> {
    Arc::new(SubtreeEchoFactory)
}

/// Build a `CellFactoryRegistry` with `echo_sub` for `bootstrap_from_filesystem`
/// (the reboot path rehydrates the registry from the FS + colony.db).
fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert("echo_sub".into(), echo_factory());
    r
}

/// Write the self-contained `pipeline_sub` SUBTREE template under
/// `<root>/templates/pipeline_sub`.
///
/// `subtree_abs` is the absolute logical path the subtree will live at once
/// instantiated (e.g. `/p1`) — used to wire the `ingest` cell's `echo_to` at the
/// inner hive `<subtree_abs>/mid`. The `mid/sink` terminal cell forwards to the
/// external `/capture` sink.
fn write_pipeline_sub_template(root: &std::path::Path, ingest_echo_to: &str) {
    let tpl = root.join("templates").join("pipeline_sub");
    write(&tpl, "template.json", r#"{"name":"pipeline_sub"}"#);
    // Root hive: a single INTERNAL edge ./ingest → ./mid (inner hive). Both
    // endpoints are descendants of the root hive — so per the connectivity rules
    // (`connectivity.rs`: internal wiring does NOT count for the hive's own
    // connectivity, only an edge that uses the hive's own path as an endpoint),
    // the root hive `/p1` is DISCONNECTED until the activating anchor->p1 edge.
    // That keeps the whole subtree inactive on instantiation (no `. -> ...` edge
    // that would self-connect the hive).
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./ingest","to":"./mid"}
        ]}}}"#,
    );
    // ingest: forwarding echo cell. Its emit target is overridden by the
    // ingest->mid edge; echo_to is a harmless self-target default.
    write(
        &tpl,
        "ingest/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"{ingest_echo_to}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    );
    // mid: INNER hive (transit). Out-edge `. -> ./sink`.
    write(
        &tpl,
        "mid/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./sink"}
        ]}}}"#,
    );
    // mid/sink: terminal proof cell INSIDE the subtree. Forwards to /capture.
    write(
        &tpl,
        "mid/sink/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/capture"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// Fresh-rusqlite probe: does a hive_scope row exist for `path`?
fn hive_scope_persisted(db_dir: &std::path::Path, path: &str) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM hive_scopes WHERE path = ?1",
            [path],
            |r| r.get(0),
        )
        .unwrap();
    n > 0
}

/// Fresh-rusqlite probe: does an edge `from -> to` exist?
fn edge_persisted(db_dir: &std::path::Path, from: &str, to: &str) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE from_path = ?1 AND to_path = ?2",
            [from, to],
            |r| r.get(0),
        )
        .unwrap();
    n > 0
}

/// Fresh-rusqlite probe: does the message_log carry a hop with `to_path == path`?
/// This is the DEEP evidence that the inner hive was a real transit node.
fn message_log_has_to_path(db_dir: &std::path::Path, path: &str) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log WHERE to_path = ?1",
            [path],
            |r| r.get(0),
        )
        .unwrap();
    n > 0
}

/// Build a probe addressed at the subtree root hive carrying an origin-format UBF
/// body (Phase-6 lesson: never role/content).
fn probe_to(target: &str) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"ping"}]}),
        ))
        .ttl(8)
        .build()
}

/// Bounded wait for a receipt (30s failure-marker convention).
async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

// ──────────────────────────────────────────────────────────────────────────────
// Demo (a): CORE E2E — message flows through the whole subtree incl. inner hive.
// ──────────────────────────────────────────────────────────────────────────────

/// A probe addressed at the subtree-root hive `/p1` transits the root hive to
/// `/p1/ingest`, whose emission is edge-routed to the INNER hive `/p1/mid`, which
/// transits to the terminal inner cell `/p1/mid/sink`. The sink forwards to the
/// external `/capture` CaptureCell — a POSITIVE receipt there proves the message
/// traversed the inner-hive-transit hop, and a fresh message_log probe asserts the
/// `/p1/mid` transit hop is recorded (deep proof).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_flows_through_subtree_including_inner_hive_transit() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // Subtree lands at /p1; ingest forwards to the inner hive /p1/mid.
    write_pipeline_sub_template(td.path(), "/p1/mid");

    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;

    // Anti-Cascade: register the external /capture sink BEFORE anything routes,
    // plus a live /anchor cell to be the activating edge's `from` endpoint.
    let (cap_tx, mut cap_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/capture"), move || {
        CaptureCell::new(cap_tx.clone())
    })
    .await;
    h.spawn(Path::new("/anchor"), || {
        EchoMockCell::new(Path::new("/anchor"))
    })
    .await;

    // Instantiate the parentless subtree → all subtree cells inactive.
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"p1","template":"pipeline_sub"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes(pipeline_sub) must commit; got {outcome:?}"
    );
    assert!(
        !registry_entry(&h, "/p1/ingest").await.unwrap().active,
        "/p1/ingest must be inactive before the activating edge"
    );
    assert!(
        !registry_entry(&h, "/p1/mid/sink").await.unwrap().active,
        "/p1/mid/sink must be inactive before the activating edge"
    );

    // ONE activating edge: live /anchor → subtree-root hive /p1.
    // W2b (Ruling A1): the terminal /p1/mid/sink echoes to /capture — that hop
    // needs a wired catch-all out-edge now (identity-fallback gone). /p1/mid/sink
    // is a live deep node (R12 deep resolution), /capture is live. Folded in.
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_edges": [{"from":"anchor","to":"p1"},{"from":"p1/mid/sink","to":"capture"}]}
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_edges anchor->p1 must commit; got {outcome:?}"
    );
    // Whole edge-wired chain is now active (edge-derived activity).
    assert!(
        registry_entry(&h, "/p1/ingest").await.unwrap().active,
        "/p1/ingest must be active after anchor->p1"
    );
    assert!(
        registry_entry(&h, "/p1/mid/sink").await.unwrap().active,
        "/p1/mid/sink must be active after anchor->p1"
    );

    // Probe at the subtree entry cell /p1/ingest. Its emission is edge-routed by
    // `ingest -> mid` to the INNER hive /p1/mid, which transits to /p1/mid/sink.
    h.send(probe_to("/p1/ingest")).await;

    // POSITIVE receipt: the terminal inner cell /p1/mid/sink ran and forwarded
    // to /capture — only reachable via the inner-hive /p1/mid transit hop.
    let got = recv_bounded(&mut cap_rx).await;
    assert!(
        got.is_some(),
        "probe at /p1/ingest must traverse the subtree (ingest → INNER hive /p1/mid → mid/sink) \
         and reach the /capture sink"
    );

    h.shutdown().await; // BARRIER — flush the writer before the message_log probe.

    // DEEP proof: the inner hive /p1/mid was a real transit node on the path.
    let db_dir = td.path().to_path_buf();
    assert!(
        message_log_has_to_path(&db_dir, "/p1/mid"),
        "message_log must record a transit hop to the INNER hive /p1/mid"
    );
    assert!(
        message_log_has_to_path(&db_dir, "/p1/mid/sink"),
        "message_log must record the terminal hop to /p1/mid/sink"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Demo (b): cell_ids fresh + pairwise-disjoint across two instantiations.
// ──────────────────────────────────────────────────────────────────────────────

/// `add_nodes pipeline_sub` as `p1` then again as `p2`. Every subtree cell in p1
/// and p2 has a distinct, valid UUID-v7 `cell_id` (recursive fresh-UUID minting,
/// no cross-instance collision).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_instantiations_have_fresh_disjoint_cell_ids() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // No message flow here → ingest echo_to is irrelevant; a harmless self-target.
    write_pipeline_sub_template(td.path(), "/p1/mid");

    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;

    for name in ["p1", "p2"] {
        let outcome = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": name, "template":"pipeline_sub"}]}
            }),
        )
        .await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "add_nodes(pipeline_sub) as {name} must commit; got {outcome:?}"
        );
    }

    // Collect the cell_ids of every spawnable subtree cell across BOTH instances.
    let paths = ["/p1/ingest", "/p1/mid/sink", "/p2/ingest", "/p2/mid/sink"];
    let mut ids: Vec<String> = Vec::new();
    for p in paths {
        let e = registry_entry(&h, p)
            .await
            .unwrap_or_else(|| panic!("{p} must be registered"));
        // Valid UUID v7.
        let u: Uuid = e
            .cell_id
            .parse()
            .unwrap_or_else(|_| panic!("{p} cell_id must parse"));
        assert_eq!(u.get_version_num(), 7, "{p} cell_id must be UUID v7");
        ids.push(e.cell_id);
    }

    // Pairwise-disjoint across p1 + p2.
    let set: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(
        set.len(),
        ids.len(),
        "all subtree cell_ids must be pairwise-disjoint across the two instances: {ids:?}"
    );

    h.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────────
// Demo (c): reboot survival.
// ──────────────────────────────────────────────────────────────────────────────

/// `add_nodes pipeline_sub` + `add_edges` to activate, then a colony reboot (drop
/// the colony, re-bootstrap from the same root + same colony.db). After reboot:
/// all subtree cells are present with the SAME cell_ids, the subtree-root hive
/// scope + an inner hive scope are still present (fresh rusqlite probe on
/// `hive_scopes`), the internal edges are still present (`edges` table), and the
/// active status survived (edge-derived, recomputed on boot).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subtree_survives_reboot_with_same_ids_scopes_edges_and_status() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_pipeline_sub_template(td.path(), "/p1/mid");

    // ── Boot 1 ──
    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;
    h.spawn(Path::new("/anchor"), || {
        EchoMockCell::new(Path::new("/anchor"))
    })
    .await;

    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name":"p1","template":"pipeline_sub"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    let outcome = send_mutation(
        &h,
        json!({
            "scope": "/",
            "diff": {"add_edges": [{"from":"anchor","to":"p1"}]}
        }),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // Capture pre-reboot cell_ids + active status.
    let pre_ingest = registry_entry(&h, "/p1/ingest").await.unwrap();
    let pre_sink = registry_entry(&h, "/p1/mid/sink").await.unwrap();
    assert!(
        pre_ingest.active && pre_sink.active,
        "subtree active pre-reboot"
    );

    // ── REBOOT: shutdown colony1, boot a FRESH colony on the SAME {root}. ──
    h.shutdown().await;

    let h2 =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    // Fresh bootstrap from the SAME filesystem root + colony.db — this is the
    // reboot: the FS is authoritative for cell existence, colony.db carries the
    // persisted cell_ids + edges + hive scopes (probe_boot_state → Reboot).
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h2.runtime())
        .await
        .expect("boot2 (reboot) bootstrap must succeed");

    // (1) Same cells present with the SAME cell_ids.
    let post_ingest = registry_entry(&h2, "/p1/ingest")
        .await
        .expect("/p1/ingest present after reboot");
    let post_sink = registry_entry(&h2, "/p1/mid/sink")
        .await
        .expect("/p1/mid/sink present after reboot");
    assert_eq!(
        post_ingest.cell_id, pre_ingest.cell_id,
        "/p1/ingest cell_id must survive the reboot"
    );
    assert_eq!(
        post_sink.cell_id, pre_sink.cell_id,
        "/p1/mid/sink cell_id must survive the reboot"
    );

    // (3) active/inactive status survived (edge-derived, recomputed on boot).
    assert!(
        post_ingest.active && post_sink.active,
        "subtree must stay active after reboot (edges persisted)"
    );

    let db_dir = td.path().to_path_buf();

    // (2a) Hive scopes still present (root + inner).
    assert!(
        hive_scope_persisted(&db_dir, "/p1"),
        "subtree-root hive scope /p1 must survive reboot"
    );
    assert!(
        hive_scope_persisted(&db_dir, "/p1/mid"),
        "inner hive scope /p1/mid must survive reboot"
    );

    // (2b) Internal edges still present.
    assert!(
        edge_persisted(&db_dir, "/p1/ingest", "/p1/mid"),
        "internal edge /p1/ingest -> /p1/mid must survive reboot"
    );
    assert!(
        edge_persisted(&db_dir, "/p1/mid", "/p1/mid/sink"),
        "internal edge /p1/mid -> /p1/mid/sink must survive reboot"
    );

    h2.shutdown().await;
}
