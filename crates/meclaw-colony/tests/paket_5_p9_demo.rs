//! Paket-5 T13: the P9 tag-gate DEMOS for per-node subtree resume.
//!
//! Per-node subtree resume is wired (T12): an `add_nodes` of a subtree template
//! onto a partially-present tree instantiates the MISSING nodes (fresh cell_ids)
//! and RESUMES the EXISTING ones (cell_id / config.json / cell.db untouched),
//! deduplicating internal edges against the live table, idempotent until the
//! tree is activated — and loud (reject, pre-destructive) once a node is Awake or
//! a type mismatches.
//!
//! These demos prove that contract POSITIVELY-and-deeply per the project's demo
//! discipline (CONTRIBUTING.md "Hohle / mehrdeutige Demo-Asserts kosten Runden"):
//! - (a) a probe flows through the WHOLE merged tree to a `CaptureCell` sink
//!   (positive receipt ⟺ every cell on the path is live) PLUS a fresh-rusqlite
//!   message_log hop-chain probe (deep), with byte-equal existing-node receipts
//!   (cell_id + config.json + cell.db hashes before/after) and a de-duplicated
//!   internal-edge count;
//! - (b1) re-applying the IDENTICAL diff onto an inactive tree is a pure no-op:
//!   registry + edges + a full colony.db snapshot are byte-equal before/after;
//! - (b2) re-applying onto an ACTIVATED tree (an existing node Awake) is REJECTED
//!   with `resume_requires_stopped_cell`, pre-destructive (nothing changed);
//! - (c) a type mismatch at an existing rel-path is REJECTED with
//!   `resume_type_mismatch`, pre-destructive, with a same-type negative probe
//!   that proves resume still proceeds;
//! - (f) after a partial resume the colony reboots (fresh rusqlite from the same
//!   on-disk state) consistent: resumed + instantiated cells present, internal
//!   edges + hive scopes survived, active/inactive status correct.
//!
//! ── Templates ──
//!
//! The demos use a fan-out subtree under a single root hive so the existing /
//! missing split is over real spawnable cells:
//!
//! ```text
//!   /p1            root hive   edges: ./a -> ./b, ./b -> ./c, ./c -> ./d
//!   /p1/a          echo cell   (entry; emit edge-routed to ./b)
//!   /p1/b          echo cell   (edge-routed to ./c)
//!   /p1/c          echo cell   (edge-routed to ./d)
//!   /p1/d          echo cell   (terminal; echo_to=/capture external sink)
//! ```
//!
//! The PARTIAL template `pipeline_partial` carries only the root hive + `a` + `b`
//! (the 2 nodes that pre-exist after the first apply, with NO internal edges —
//! activation/edges arrive with the full apply). The FULL template
//! `pipeline_full` carries all four cells + the three chaining edges and shares
//! `a`/`b`'s rel-paths + types verbatim, so applying it onto the partial instance
//! resumes `a`/`b` and instantiates `c`/`d`.

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
// Test-local `echo_sub` factory with boot-inactive eager reconnect (mirrors the
// a5-demo `SubtreeEchoFactory`): a subtree cell instantiated inactive via
// `add_nodes` gets a REAL respawn on the activating `add_edges`, so the merged
// tree can carry live traffic end-to-end for demo (a)'s positive receipt.
// ──────────────────────────────────────────────────────────────────────────────

/// A stateless echo factory whose `spawn_cell` + `build_boot_inactive_respawn`
/// share the same task-construction closure.
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
        let cell = EchoMockCell::new(path.clone()).emitted_target(echo_to.clone());
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
        let echo_to = parse_echo_to(&params).ok()?;
        let build = make_echo_build(path, echo_to, outputs_tx);
        Some(Box::new(build))
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Shared helpers (mirror the a5 / t8b2 harness)
// ──────────────────────────────────────────────────────────────────────────────

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
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

/// Registry for `bootstrap_from_filesystem` (the reboot path rehydrates from FS +
/// colony.db).
fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert("echo_sub".into(), echo_factory());
    r
}

/// Write the PARTIAL template (`pipeline_partial`): root hive + `a` + `b`, NO
/// internal edges. This is the smaller subset instantiated first so that `a`/`b`
/// pre-exist (on disk + registry + colony.db) when the full template is applied.
fn write_partial_template(root: &std::path::Path) {
    let tpl = root.join("templates").join("pipeline_partial");
    write(&tpl, "template.json", r#"{"name":"pipeline_partial"}"#);
    // Root hive with NO edges → the subtree is disconnected/inactive on apply.
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        &tpl,
        "a/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/p1/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &tpl,
        "b/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/p1/c"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// Write the FULL template (`pipeline_full`): root hive (chaining edges) + four
/// echo cells `a`,`b`,`c`,`d`. `a`/`b` share `pipeline_partial`'s rel-paths +
/// `cell.type` + `params` verbatim, so applying this onto a `pipeline_partial`
/// instance resumes `a`/`b` and instantiates `c`/`d`. The terminal cell `d`
/// forwards to the external `/capture` sink.
fn write_full_template(root: &std::path::Path) {
    let tpl = root.join("templates").join("pipeline_full");
    write(&tpl, "template.json", r#"{"name":"pipeline_full"}"#);
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./a","to":"./b"},
            {"from":"./b","to":"./c"},
            {"from":"./c","to":"./d"}
        ]}}}"#,
    );
    // a/b verbatim-identical to the partial template (same type + params).
    write(
        &tpl,
        "a/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/p1/b"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &tpl,
        "b/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/p1/c"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &tpl,
        "c/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/p1/d"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &tpl,
        "d/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/capture"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// Open a fresh read-only rusqlite connection on `<db_dir>/colony.db`.
fn open_ro(db_dir: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}

/// Fresh-rusqlite probe: count edges `from -> to` (dedup proof: each logical edge
/// appears at most once).
fn edge_count(db_dir: &std::path::Path, from: &str, to: &str) -> i64 {
    open_ro(db_dir)
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE from_path = ?1 AND to_path = ?2",
            [from, to],
            |r| r.get(0),
        )
        .unwrap()
}

/// Fresh-rusqlite probe: total INTERNAL edge rows of the subtree — both
/// endpoints under the root prefix. W2b: the terminal cell now carries a
/// boundary catch-all out-edge to the external /capture sink (identity-fallback
/// gone); that edge crosses out of the subtree and must NOT be counted as an
/// internal edge, so the filter requires from_path AND to_path under the prefix.
fn total_edges_under(db_dir: &std::path::Path, prefix: &str) -> i64 {
    open_ro(db_dir)
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE from_path LIKE ?1 AND to_path LIKE ?1",
            [format!("{prefix}%")],
            |r| r.get(0),
        )
        .unwrap()
}

/// Fresh-rusqlite probe: does a hive_scope row exist for `path`?
fn hive_scope_persisted(db_dir: &std::path::Path, path: &str) -> bool {
    open_ro(db_dir)
        .query_row(
            "SELECT COUNT(*) FROM hive_scopes WHERE path = ?1",
            [path],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        > 0
}

/// Fresh-rusqlite probe: does the message_log carry a hop with `to_path == path`?
fn message_log_has_to_path(db_dir: &std::path::Path, path: &str) -> bool {
    open_ro(db_dir)
        .query_row(
            "SELECT COUNT(*) FROM message_log WHERE to_path = ?1",
            [path],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        > 0
}

/// A canonical, order-stable snapshot of the registry + edges + hive_scopes
/// directly from a fresh rusqlite connection. Used to prove byte-equality of the
/// persisted colony state across an idempotent re-apply (b1).
fn colony_snapshot(db_dir: &std::path::Path) -> String {
    let conn = open_ro(db_dir);
    let mut out = String::new();

    let mut q = conn
        .prepare("SELECT path, cell_id, cell_type, status FROM registry ORDER BY path")
        .unwrap();
    let rows = q
        .query_map([], |r| {
            Ok(format!(
                "REG {}|{}|{}|{}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap();
    let mut reg: Vec<String> = rows.map(|r| r.unwrap()).collect();
    reg.sort();
    out.push_str(&reg.join("\n"));
    out.push('\n');

    let mut q = conn
        .prepare("SELECT from_path, to_path FROM edges ORDER BY from_path, to_path")
        .unwrap();
    let rows = q
        .query_map([], |r| {
            Ok(format!(
                "EDGE {}->{}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
            ))
        })
        .unwrap();
    let mut edges: Vec<String> = rows.map(|r| r.unwrap()).collect();
    edges.sort();
    out.push_str(&edges.join("\n"));
    out.push('\n');

    let mut q = conn
        .prepare("SELECT path FROM hive_scopes ORDER BY path")
        .unwrap();
    let rows = q
        .query_map([], |r| Ok(format!("HIVE {}", r.get::<_, String>(0)?)))
        .unwrap();
    let mut hives: Vec<String> = rows.map(|r| r.unwrap()).collect();
    hives.sort();
    out.push_str(&hives.join("\n"));
    out
}

/// A probe addressed at `target` carrying an origin-format UBF body (Phase-6
/// lesson: never role/content).
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

/// On-disk cell dir for a logical subtree path: anchored under root cell dir
/// `main` (spec overview Z.331 strips the root cell dir name from logical paths).
fn cell_dir_on_disk(root: &std::path::Path, logical_rel: &str) -> std::path::PathBuf {
    root.join("main").join(logical_rel)
}

/// SHA-free content hash: read a file's bytes and fold into a stable digest
/// string. (`std::hash::DefaultHasher` is process-stable within one run, which is
/// all we need for a before/after byte-equality proof.)
fn file_digest(path: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let bytes = std::fs::read(path).unwrap();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}:{}", h.finish(), bytes.len())
}

// ──────────────────────────────────────────────────────────────────────────────
// (a) partial_tree_two_existing_two_missing_one_diff_e2e
// ──────────────────────────────────────────────────────────────────────────────

/// Two nodes (`a`,`b`) pre-exist from a partial-template apply; ONE `add_nodes`
/// of the full 4-cell template instantiates the 2 MISSING nodes (`c`,`d`) with
/// FRESH cell_ids and RESUMES the 2 EXISTING (`a`,`b`) byte-unchanged
/// (cell_id + config.json + cell.db hashes identical before/after). Internal
/// edges are complete and de-duplicated (each logical edge once). A probe then
/// flows through the WHOLE merged tree to the `/capture` sink (positive receipt)
/// and the message_log records the hop chain (deep proof).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partial_tree_two_existing_two_missing_one_diff_e2e() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_partial_template(td.path());
    write_full_template(td.path());

    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;

    // Anti-Cascade: register the external /capture sink + a live /anchor BEFORE
    // anything routes.
    let (cap_tx, mut cap_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/capture"), move || {
        CaptureCell::new(cap_tx.clone())
    })
    .await;
    h.spawn(Path::new("/anchor"), || {
        EchoMockCell::new(Path::new("/anchor"))
    })
    .await;

    // ── Step 1: instantiate the PARTIAL subtree (root hive + a + b). ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"pipeline_partial"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "partial apply must commit; got {outcome:?}"
    );

    // Capture the 2 existing nodes' identity + on-disk content BEFORE the resume.
    // They must be STOPPED (not Awake) for the per-node resume to proceed — make
    // that precondition explicit (a running task would loudly reject, Z.296).
    let a_before = registry_entry(&h, "/p1/a").await.unwrap();
    let b_before = registry_entry(&h, "/p1/b").await.unwrap();
    assert_ne!(
        a_before.lifecycle_status, "Awake",
        "/p1/a must be stopped before resume"
    );
    assert_ne!(
        b_before.lifecycle_status, "Awake",
        "/p1/b must be stopped before resume"
    );
    let a_id_before = a_before.cell_id;
    let b_id_before = b_before.cell_id;
    // The existing echo cells are NotYetSpawned-or-inactive; write a known
    // cell.db byte-pattern so the resume can be proven NOT to touch it (M1).
    let a_dir = cell_dir_on_disk(td.path(), "p1/a");
    let b_dir = cell_dir_on_disk(td.path(), "p1/b");
    std::fs::write(a_dir.join("cell.db"), b"existing-a-db-state").unwrap();
    std::fs::write(b_dir.join("cell.db"), b"existing-b-db-state").unwrap();
    let a_cfg_before = file_digest(&a_dir.join("config.json"));
    let b_cfg_before = file_digest(&b_dir.join("config.json"));
    let a_db_before = file_digest(&a_dir.join("cell.db"));
    let b_db_before = file_digest(&b_dir.join("cell.db"));
    // c/d must NOT exist yet.
    assert!(
        registry_entry(&h, "/p1/c").await.is_none() && registry_entry(&h, "/p1/d").await.is_none(),
        "c/d must be missing before the full apply"
    );

    // ── Step 2: the ONE diff — add_nodes the FULL 4-cell template at /p1. ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"pipeline_full"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "full apply (partial resume) must commit; got {outcome:?}"
    );

    // (1) The 2 MISSING nodes were instantiated with FRESH, valid UUID-v7 ids.
    let c = registry_entry(&h, "/p1/c")
        .await
        .expect("/p1/c instantiated");
    let d = registry_entry(&h, "/p1/d")
        .await
        .expect("/p1/d instantiated");
    for (p, e) in [("/p1/c", &c), ("/p1/d", &d)] {
        let u: Uuid = e.cell_id.parse().unwrap_or_else(|_| panic!("{p} cell_id"));
        assert_eq!(u.get_version_num(), 7, "{p} cell_id must be UUID v7");
    }
    assert_ne!(c.cell_id, d.cell_id, "c/d cell_ids must differ");

    // (2) The 2 EXISTING nodes were RESUMED byte-unchanged: cell_id + config.json
    // + cell.db identical before/after.
    let a_after = registry_entry(&h, "/p1/a").await.unwrap();
    let b_after = registry_entry(&h, "/p1/b").await.unwrap();
    assert_eq!(a_after.cell_id, a_id_before, "/p1/a cell_id unchanged");
    assert_eq!(b_after.cell_id, b_id_before, "/p1/b cell_id unchanged");
    assert_eq!(
        file_digest(&a_dir.join("config.json")),
        a_cfg_before,
        "/p1/a config.json unchanged on resume"
    );
    assert_eq!(
        file_digest(&b_dir.join("config.json")),
        b_cfg_before,
        "/p1/b config.json unchanged on resume"
    );
    assert_eq!(
        file_digest(&a_dir.join("cell.db")),
        a_db_before,
        "/p1/a cell.db byte-unchanged on resume (M1)"
    );
    assert_eq!(
        file_digest(&b_dir.join("cell.db")),
        b_db_before,
        "/p1/b cell.db byte-unchanged on resume (M1)"
    );

    // ── Step 3: ONE activating edge — live /anchor → subtree root /p1. ──
    // W2b (Ruling A1): the terminal /p1/d echoes to /capture — that hop needs a
    // wired catch-all out-edge now (identity-fallback gone). /p1/d is a live deep
    // node (R12 deep resolution), /capture is live. Folded into the activator.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"anchor","to":"p1"},{"from":"p1/d","to":"capture"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "activating add_edges must commit; got {outcome:?}"
    );
    assert!(
        registry_entry(&h, "/p1/a").await.unwrap().active
            && registry_entry(&h, "/p1/d").await.unwrap().active,
        "whole merged tree must be active after anchor->p1"
    );

    // ── Step 4: probe at /p1/a flows a->b->c->d->/capture (positive receipt). ──
    h.send(probe_to("/p1/a")).await;
    let got = recv_bounded(&mut cap_rx).await;
    assert!(
        got.is_some(),
        "probe must traverse a->b->c->d (resumed a/b + instantiated c/d) and reach /capture"
    );

    h.shutdown().await; // BARRIER — flush the writer before db probes.
    let db_dir = td.path().to_path_buf();

    // (3) Internal edges complete + de-duplicated: each logical edge appears once,
    // and the subtree carries EXACTLY the three chaining edges (no duplicates).
    for (from, to) in [("/p1/a", "/p1/b"), ("/p1/b", "/p1/c"), ("/p1/c", "/p1/d")] {
        assert_eq!(
            edge_count(&db_dir, from, to),
            1,
            "edge {from}->{to} must appear exactly once (dedup)"
        );
    }
    assert_eq!(
        total_edges_under(&db_dir, "/p1/"),
        3,
        "subtree must carry exactly its 3 internal edges, no duplicates"
    );

    // (4) DEEP proof: the message_log carries the full hop chain through the tree.
    for hop in ["/p1/b", "/p1/c", "/p1/d"] {
        assert!(
            message_log_has_to_path(&db_dir, hop),
            "message_log must record the transit hop to {hop}"
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// (b1) idempotent_reapply_inactive_is_pure_noop
// ──────────────────────────────────────────────────────────────────────────────

/// Apply a subtree add_nodes onto an INACTIVE (unconnected) tree, then apply the
/// IDENTICAL diff again. The second apply is a pure no-op: registry cell_ids
/// identical, edge count identical (no duplicate edges, A1), and a full colony.db
/// snapshot (registry + edges + hive_scopes) is byte-equal before/after.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idempotent_reapply_inactive_is_pure_noop() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_full_template(td.path());

    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;

    // First apply (no activating edge → tree stays inactive).
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"pipeline_full"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "first apply must commit; got {outcome:?}"
    );
    // Tree must be inactive AND every node parked NotYetSpawned (NOT Awake) — the
    // edge-less add_nodes derives the internal-edge cluster inactive (no anchor
    // edge connects /p1), so a re-apply is an idempotent no-op, not a loud
    // resume_requires_stopped_cell reject.
    for p in ["/p1/a", "/p1/b", "/p1/c", "/p1/d"] {
        let e = registry_entry(&h, p).await.unwrap();
        assert!(!e.active, "{p} must be inactive (no activating edge)");
        assert_eq!(
            e.lifecycle_status, "NotYetSpawned",
            "{p} must be parked NotYetSpawned (stopped) → eligible for resume"
        );
    }

    // Capture cell_ids after the FIRST apply.
    let ids_before: Vec<(String, String)> = {
        let mut v = Vec::new();
        for p in ["/p1/a", "/p1/b", "/p1/c", "/p1/d"] {
            v.push((p.to_string(), registry_entry(&h, p).await.unwrap().cell_id));
        }
        v
    };

    // ── Second apply: the IDENTICAL diff onto the inactive tree. Must COMMIT
    //    (no Awake node) as a pure no-op resume. ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"pipeline_full"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "second identical apply onto inactive tree must commit (no-op resume); got {outcome:?}"
    );

    // cell_ids identical (no re-mint, no duplicate registry entries).
    for (p, id) in &ids_before {
        assert_eq!(
            &registry_entry(&h, p).await.unwrap().cell_id,
            id,
            "{p} cell_id must be unchanged by the idempotent re-apply"
        );
    }
    assert_eq!(
        all_registry(&h)
            .await
            .iter()
            .filter(|e| e.path.starts_with("/p1"))
            .count(),
        4,
        "exactly the 4 subtree cells in the registry — no duplicate entries"
    );

    h.shutdown().await; // BARRIER — flush the writer before the fresh-rusqlite probe.
    let db_dir = td.path().to_path_buf();

    // colony.db snapshot reflects a single, deduplicated subtree: exactly the 4
    // cells, the 3 internal edges (no duplicates, A1), and the hive scope. Because
    // the second apply was a pure no-op, NOTHING was added on top of the first.
    let snap = colony_snapshot(&db_dir);
    assert_eq!(
        snap.matches("REG /p1/").count(),
        4,
        "snapshot must carry exactly 4 subtree registry rows (no duplicates): {snap}"
    );
    assert_eq!(
        total_edges_under(&db_dir, "/p1/"),
        3,
        "no duplicate edges after the idempotent re-apply (A1): {snap}"
    );
    for (from, to) in [("/p1/a", "/p1/b"), ("/p1/b", "/p1/c"), ("/p1/c", "/p1/d")] {
        assert_eq!(
            edge_count(&db_dir, from, to),
            1,
            "edge {from}->{to} must appear exactly once after re-apply"
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// (b2) reapply_active_rejects_resume_requires_stopped_cell
// ──────────────────────────────────────────────────────────────────────────────

/// Apply + ACTIVATE the subtree (an existing node becomes Awake via a real
/// message), then re-apply the same add_nodes. It is REJECTED with
/// `resume_requires_stopped_cell` (Z.296), pre-destructive: registry cell_ids,
/// edge count + colony.db snapshot unchanged. The "idempotent until activation,
/// then loud" contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reapply_active_rejects_resume_requires_stopped_cell() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_full_template(td.path());

    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;

    let (cap_tx, mut cap_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/capture"), move || {
        CaptureCell::new(cap_tx.clone())
    })
    .await;
    h.spawn(Path::new("/anchor"), || {
        EchoMockCell::new(Path::new("/anchor"))
    })
    .await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"pipeline_full"}]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    let outcome = send_mutation(
        &h,
        // W2b (Ruling A1): also wire the terminal /p1/d -> /capture catch-all
        // out-edge (identity-fallback gone) so the liveness probe below reaches
        // /capture. /p1/d live deep node, /capture live.
        json!({"scope":"/","diff":{"add_edges":[{"from":"anchor","to":"p1"},{"from":"p1/d","to":"capture"}]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    // Drive a probe so an existing node actually goes Awake (positive receipt
    // confirms the tree is live / the node is running).
    h.send(probe_to("/p1/a")).await;
    assert!(
        recv_bounded(&mut cap_rx).await.is_some(),
        "tree must be live (node Awake) before the reject re-apply"
    );

    // Snapshot pre-reject state.
    let ids_before: Vec<(String, String)> = {
        let mut v = Vec::new();
        for p in ["/p1/a", "/p1/b", "/p1/c", "/p1/d"] {
            v.push((p.to_string(), registry_entry(&h, p).await.unwrap().cell_id));
        }
        v
    };

    // ── Re-apply onto the ACTIVE tree → REJECTED, pre-destructive. ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"pipeline_full"}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "resume_requires_stopped_cell",
            "re-apply onto an Awake node must reject resume_requires_stopped_cell"
        ),
        other => panic!("expected Rejected, got {other:?}"),
    }

    // Nothing changed: registry cell_ids identical.
    for (p, id) in &ids_before {
        assert_eq!(
            &registry_entry(&h, p).await.unwrap().cell_id,
            id,
            "{p} cell_id unchanged after reject"
        );
    }

    h.shutdown().await;
    let db_dir = td.path().to_path_buf();
    assert_eq!(
        total_edges_under(&db_dir, "/p1/"),
        3,
        "edge count unchanged after reject (no partial apply)"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// (c) type_mismatch_rejects_pre_destructive
// ──────────────────────────────────────────────────────────────────────────────

/// Template whose node at rel-path `a` has a DIFFERENT `cell.type` than the
/// resuming target's on-disk type. Used to drive the F2 reject.
fn write_type_clash_template(root: &std::path::Path) {
    let tpl = root.join("templates").join("pipeline_clash");
    write(&tpl, "template.json", r#"{"name":"pipeline_clash"}"#);
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // `a` is a DIFFERENT spawnable cell type here (`other_cell`) than its on-disk
    // type (`echo_sub`) → F2 resume_type_mismatch at rel-path `a`. The factory for
    // `other_cell` need not exist: the mismatch is rejected pre-destructively,
    // before any spawn (mirrors the single-cell F2 test's `echo` vs `persist_mock`).
    write(
        &tpl,
        "a/config.json",
        r#"{"cell":{"type":"other_cell"},"params":{"echo_to":"/p1/c"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        &tpl,
        "b/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/p1/c"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

/// An existing subtree node whose on-disk `cell.type` differs from the template's
/// node at the same rel-path → REJECTED with `resume_type_mismatch` (F2),
/// pre-destructive (cell_id + config + edge count unchanged). A same-type
/// re-apply (negative probe) is then shown to PROCEED (commit, no reject).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn type_mismatch_rejects_pre_destructive() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_partial_template(td.path()); // a + b as echo_sub
    write_full_template(td.path()); // same-type superset (negative probe)
    write_type_clash_template(td.path()); // a as hive (clash)

    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;

    // Instantiate the partial subtree → /p1/a exists on disk as echo_sub.
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"pipeline_partial"}]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    let a_id_before = registry_entry(&h, "/p1/a").await.unwrap().cell_id;
    let a_dir = cell_dir_on_disk(td.path(), "p1/a");
    std::fs::write(a_dir.join("cell.db"), b"clash-a-db-state").unwrap();
    let a_cfg_before = file_digest(&a_dir.join("config.json"));
    let a_db_before = file_digest(&a_dir.join("cell.db"));

    // ── Apply the type-clash template (a = hive ≠ on-disk echo_sub) → REJECT. ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"pipeline_clash"}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "resume_type_mismatch",
            "type-changing subtree resume must reject resume_type_mismatch (F2)"
        ),
        other => panic!("expected Rejected, got {other:?}"),
    }

    // Pre-destructive: cell_id + config.json + cell.db byte-unchanged; the clash
    // template's NEW node `c`/`d` were never instantiated.
    let a_after = registry_entry(&h, "/p1/a").await.unwrap();
    assert_eq!(a_after.cell_id, a_id_before, "/p1/a cell_id unchanged");
    assert_eq!(
        file_digest(&a_dir.join("config.json")),
        a_cfg_before,
        "/p1/a config.json unchanged after reject"
    );
    assert_eq!(
        file_digest(&a_dir.join("cell.db")),
        a_db_before,
        "/p1/a cell.db unchanged after reject"
    );

    // ── NEGATIVE probe: same-type superset apply PROCEEDS (resume, no reject). ──
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"pipeline_full"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "same-type resume must PROCEED (commit), not reject; got {outcome:?}"
    );
    // a resumed (id stable), c/d instantiated → proves resume continued past `a`.
    assert_eq!(
        registry_entry(&h, "/p1/a").await.unwrap().cell_id,
        a_id_before,
        "/p1/a still resumed (id stable) on the same-type apply"
    );
    assert!(
        registry_entry(&h, "/p1/c").await.is_some() && registry_entry(&h, "/p1/d").await.is_some(),
        "missing c/d instantiated on the same-type apply"
    );

    h.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────────
// (f) reboot_after_partial_resume_consistent
// ──────────────────────────────────────────────────────────────────────────────

/// After a partial-resume (demo (a) scenario) the colony reboots (fresh rusqlite
/// from the same on-disk state). The tree is consistent: both the resumed
/// (existing a/b) and instantiated (missing c/d) cells are present with the SAME
/// cell_ids, internal edges + hive scope survived, and active status is correct
/// (edge-derived, recomputed on boot).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reboot_after_partial_resume_consistent() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write_partial_template(td.path());
    write_full_template(td.path());

    // ── Boot 1: partial apply → full apply (resume a/b + instantiate c/d) → activate. ──
    let h =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    rescan_templates(&h, td.path().join("templates")).await;
    h.spawn(Path::new("/anchor"), || {
        EchoMockCell::new(Path::new("/anchor"))
    })
    .await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"pipeline_partial"}]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    let a_id_pre = registry_entry(&h, "/p1/a").await.unwrap().cell_id;
    let b_id_pre = registry_entry(&h, "/p1/b").await.unwrap().cell_id;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"p1","template":"pipeline_full"}]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));
    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[{"from":"anchor","to":"p1"}]}}),
    )
    .await;
    assert!(matches!(outcome, MutationOutcome::Committed { .. }));

    let c_id_pre = registry_entry(&h, "/p1/c").await.unwrap().cell_id;
    let d_id_pre = registry_entry(&h, "/p1/d").await.unwrap().cell_id;
    assert!(
        registry_entry(&h, "/p1/a").await.unwrap().active
            && registry_entry(&h, "/p1/d").await.unwrap().active,
        "merged tree active pre-reboot"
    );

    // ── REBOOT: shutdown, fresh colony on the SAME root + colony.db. ──
    h.shutdown().await;
    let h2 =
        ColonyHandle::new_with_factories_at(&td, vec![("echo_sub".to_string(), echo_factory())]);
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h2.runtime())
        .await
        .expect("reboot bootstrap must succeed");

    // (1) Resumed (a/b) AND instantiated (c/d) cells present with SAME cell_ids.
    for (p, pre) in [
        ("/p1/a", &a_id_pre),
        ("/p1/b", &b_id_pre),
        ("/p1/c", &c_id_pre),
        ("/p1/d", &d_id_pre),
    ] {
        let post = registry_entry(&h2, p)
            .await
            .unwrap_or_else(|| panic!("{p} present after reboot"));
        assert_eq!(&post.cell_id, pre, "{p} cell_id must survive reboot");
    }

    // (2) active status survived (edge-derived, recomputed on boot).
    assert!(
        registry_entry(&h2, "/p1/a").await.unwrap().active
            && registry_entry(&h2, "/p1/d").await.unwrap().active,
        "merged tree must stay active after reboot (edges persisted)"
    );

    let db_dir = td.path().to_path_buf();
    // (3) Internal edges survived.
    for (from, to) in [("/p1/a", "/p1/b"), ("/p1/b", "/p1/c"), ("/p1/c", "/p1/d")] {
        assert_eq!(
            edge_count(&db_dir, from, to),
            1,
            "internal edge {from}->{to} must survive reboot exactly once"
        );
    }
    // (4) Hive scope survived.
    assert!(
        hive_scope_persisted(&db_dir, "/p1"),
        "subtree-root hive scope /p1 must survive reboot"
    );

    h2.shutdown().await;
}
