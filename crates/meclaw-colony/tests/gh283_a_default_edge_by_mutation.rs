//! GH #283 — the last door: a MUTATION may declare a default edge.
//!
//! Tasks 1–5b gave the substrate everything except this one entrance. The
//! router evaluates the default phase only after every regular out-edge of the
//! same sender declined (T1), a `params.graph` may declare it (T2), it persists
//! and rehydrates (T3/T5b), it is the fifth term of edge identity (T4) and it
//! survives the two rebuilding paths — swap/move and subtree instantiation
//! (T5). What was left is the way an operator changes a RUNNING colony:
//! `add_edges`.
//!
//! # Pre-state (recorded before the implementation, Step 2 of the task)
//!
//! The `add_edges` loop does NOT reject unknown top-level edge keys — the
//! allow-list it carries is the one for `modifier` keys
//! (`validate.rs`, "add_edges[].modifier unknown key"). So `"default": true`
//! was accepted and then dropped on the floor: the apply arm built its
//! `edge_table::Edge` candidate with a literal `is_default: false`. The caller
//! got `Committed` for an edge that looks declared, is spelled the way the
//! template DSL spells it, and routes as an ORDINARY always-edge — it fires
//! BESIDE the regular edges instead of behind them, which is double delivery on
//! exactly the surface #283 reports.
//!
//! # What is asserted, and why it is behaviour
//!
//! Like the T5 suite, every routing case reads the phase off the colony's own
//! DELIVERIES rather than off a field: one probe whose headers match the
//! regular edge must reach the regular sink and leave the default sink silent,
//! and one probe that matches nothing must reach the default sink. When this
//! file was written `/colony/graph` did not expose the flag at all (the
//! `GraphEdgeDto` gap noted in T4); GH #367 closed that gap, but the colony's
//! delivery remains the honest read here — and the stronger one, since a field
//! assertion would still pass over a table that routes wrongly for some other
//! reason. The graph-read half is pinned in
//! `gh367_the_graph_names_the_default_phase.rs`.
//!
//! Case (d) is the exception and reads `colony.db` on purpose: it asks whether
//! a SECOND edge exists beside a content-equal regular one, and routing cannot
//! answer that (the regular edge decides first, so the second row is invisible
//! from the outside precisely while the dedup would be swallowing it).

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ContractView, DiskBlobStore, MutationOutcome,
    RespawnFn, SpawnedCellKind, bootstrap_from_filesystem, cell_task,
};
use meclaw_core::{
    Body, CellEmission, JsonValue, Message, MessageBuilder, Path, Uuid,
    serde_json::{Map, Value, json},
};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

// ── Harness (shape shared with `gh283_a_default_edge_survives_rewiring.rs`) ───

const CELL: &str = r#""contract":{"version":"0.1.0","settings":{},"consumes":{}}"#;

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

/// A probe addressed at `to`, carrying one `context` header pair.
///
/// The guard reads `context`, not `hop`, and that is forced rather than
/// stylistic: `hop` is the immediately-preceding cell's own output and is
/// REPLACED on every emission (`Headers::carry_context_with_hop`), so a hop key
/// set on the probe is gone by the time the probed cell's emission is routed.
fn probe(to: &str, kind: &str) -> Message {
    let mut headers: Map<String, Value> = Map::new();
    headers.insert("kind".to_string(), json!(kind));
    MessageBuilder::new(Path::new(to))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"ping"}]}),
        ))
        .context(headers)
        .ttl(8)
        .build()
}

/// Bounded wait for a receipt (30s failure-marker convention). A miss prints
/// the dead-letter queue, which is where a mis-routed probe ends up.
async fn recv_one(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, ctx: &str) -> Message {
    match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("{ctx}: capture channel closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("{ctx}: no receipt within 30s; DLQ: {dlq:?}")
        }
    }
}

/// Assert nothing arrives — the semantic discriminator of this file, so the
/// window is short on purpose: the two emissions of a double delivery leave the
/// same `route()` call, so by the time the other sink has its receipt the wrong
/// one would already be in flight.
async fn assert_silent(rx: &mut mpsc::Receiver<Message>, ctx: &str) {
    let got = tokio::time::timeout(Duration::from_millis(700), rx.recv()).await;
    assert!(
        got.is_err(),
        "{ctx}: this sink must stay silent, got {got:?}"
    );
}

// ── An echo factory that can be woken after boot ─────────────────────────────

struct SubtreeEchoFactory;

fn parse_echo_to(params: &JsonValue) -> Result<Path, String> {
    params
        .get("echo_to")
        .and_then(|v| v.as_str())
        .map(Path::new)
        .ok_or_else(|| "params.echo_to missing or not a string".to_string())
}

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
            cell_task(p, rx, o, cell, None, None, None).await;
        });
        (tx, join, peace_rx, backstop_rx)
    }
}

impl CellFactory for SubtreeEchoFactory {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        parse_echo_to(params).map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
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
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<DiskBlobStore>>,
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

    #[allow(clippy::too_many_arguments)]
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
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let echo_to = parse_echo_to(&params).ok()?;
        let build = make_echo_build(path, echo_to, outputs_tx);
        Some(Box::new(build))
    }
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo_sub".into(),
        Arc::new(SubtreeEchoFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo_sub".to_string(),
        Arc::new(SubtreeEchoFactory) as Arc<dyn CellFactory>,
    )]
}

/// A colony with one sender `/src` and two capture sinks. `boot_edges` is the
/// `params.graph.edges` array the root hive declares — every case here starts
/// from a DIFFERENT boot topology and then mutates it, which is the whole point.
async fn colony_with(
    boot_edges: &str,
) -> (
    TempDir,
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        &format!(r#"{{"cell":{{"type":"hive"}},"params":{{"graph":{{"edges":[{boot_edges}]}}}}}}"#),
    );
    write(
        td.path(),
        "main/src/config.json",
        &format!(r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/reg_sink"}},{CELL}}}"#),
    );

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());

    // Anti-cascade: both sinks are resolved BEFORE bootstrap and probe.
    let (reg_tx, reg_rx) = mpsc::channel(16);
    let (fb_tx, fb_rx) = mpsc::channel(16);
    h.spawn(Path::new("/reg_sink"), move || {
        CaptureCell::new(reg_tx.clone())
    })
    .await;
    h.spawn(Path::new("/fb_sink"), move || {
        CaptureCell::new(fb_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap succeeds");

    (td, h, reg_rx, fb_rx)
}

/// The regular arm every mixed-phase case boots with: guarded, to `/reg_sink`.
const REGULAR_ARM: &str =
    r#"{"from":"/src","to":"/reg_sink","condition":"context.kind == 'work'"}"#;

/// The two probes that read a sender's default phase off its deliveries.
async fn assert_default_phase(
    h: &ColonyHandle,
    reg_rx: &mut mpsc::Receiver<Message>,
    fb_rx: &mut mpsc::Receiver<Message>,
    ctx: &str,
) {
    // 1. A message the REGULAR edge takes. The default must stay silent — that
    //    is the whole of the phase, and the half a dropped flag breaks.
    h.send(probe("/src", "work")).await;
    let got = recv_one(h, reg_rx, &format!("{ctx}: the regular lane")).await;
    assert_eq!(got.target, Path::new("/reg_sink"));
    assert_silent(
        fb_rx,
        &format!("{ctx}: the default must not fire beside a regular edge"),
    )
    .await;

    // 2. A message no regular edge takes. Now the default is the consumer of
    //    what would otherwise dead-letter as `no_route`.
    h.send(probe("/src", "play")).await;
    let got = recv_one(h, fb_rx, &format!("{ctx}: the default lane")).await;
    assert_eq!(got.target, Path::new("/fb_sink"));
    assert_silent(
        reg_rx,
        &format!("{ctx}: the guarded regular edge must not fire"),
    )
    .await;
}

/// The `is_default` column of every persisted edge row `from -> to`, in row
/// order. Read AFTER `shutdown()`, so the writer has drained.
fn persisted_phases(root: &std::path::Path, from: &str, to: &str) -> Vec<i64> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut stmt = conn
        .prepare("SELECT is_default FROM edges WHERE from_path = ?1 AND to_path = ?2")
        .unwrap();
    let mut rows: Vec<i64> = stmt
        .query_map([from, to], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows.sort_unstable();
    rows
}

// ── (a) a mutation-declared default routes as a default ──────────────────────

/// The headline claim: `add_edges` with `"default": true` commits, and the
/// colony afterwards routes the sender's traffic in two phases.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mutation_may_add_a_default_edge() {
    let (_td, h, mut reg_rx, mut fb_rx) = colony_with(REGULAR_ARM).await;

    // Pre-state receipt: without the default, unmatched traffic reaches nobody.
    h.send(probe("/src", "play")).await;
    assert_silent(&mut fb_rx, "before the mutation: no default exists yet").await;
    assert_silent(&mut reg_rx, "before the mutation: the guard declines").await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_edges":[{"from":"src","to":"fb_sink","default":true}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "an add_edges default must commit; got {outcome:?}"
    );

    assert_default_phase(&h, &mut reg_rx, &mut fb_rx, "after the mutation").await;

    h.shutdown().await;
}

// ── (b) a non-boolean `default` is `edge_schema`, pre-destructively ──────────

/// The type check. The diff carries a perfectly valid second entry, so the
/// assertions also say the WHOLE diff was refused — the #276 invariant: a
/// rejected mutation leaves no residue, and this refusal comes from validate,
/// before anything is applied at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_boolean_default_is_refused_as_edge_schema() {
    let (td, h, mut reg_rx, mut fb_rx) = colony_with(REGULAR_ARM).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_edges":[
                {"from":"src","to":"reg_sink"},
                {"from":"src","to":"fb_sink","default":"yes"}
            ]
        }}),
    )
    .await;
    let MutationOutcome::Rejected {
        error_code,
        details,
        ..
    } = &outcome
    else {
        panic!("a non-boolean default must be refused; got {outcome:?}");
    };
    assert_eq!(error_code, "edge_schema", "details: {details}");
    assert!(
        details.contains("add_edges[].default") && details.contains("boolean"),
        "the refusal names the field and the expected type; got {details}"
    );

    // No residue, read as behaviour: the valid sibling entry would have been an
    // UNGUARDED regular edge to `/reg_sink`, so it would take everything. It
    // took nothing, and the refused default took nothing either.
    h.send(probe("/src", "play")).await;
    assert_silent(&mut reg_rx, "the refused diff's valid sibling entry").await;
    assert_silent(&mut fb_rx, "the refused default entry").await;

    let root = td.path().to_path_buf();
    h.shutdown().await;
    assert!(
        persisted_phases(&root, "/src", "/fb_sink").is_empty(),
        "the refused default reaches no edge row"
    );
    assert_eq!(
        persisted_phases(&root, "/src", "/reg_sink"),
        vec![0],
        "the edge table still holds exactly the one edge the boot declared"
    );
}

// ── (c) a default edge may carry a condition ─────────────────────────────────

/// The legal guarded form, asserted explicitly: a `condition` does not turn the
/// edge back into a regular one, it narrows what the default phase accepts.
///
/// The guard deliberately OVERLAPS the regular arm (`kind != 'idle'` covers
/// `kind == 'work'`), because that is where the two readings differ: as a
/// regular edge it fires beside the regular arm, as a default it waits behind
/// it. Without the overlap the case would pass over an ignored flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_default_edge_may_carry_a_condition() {
    let (_td, h, mut reg_rx, mut fb_rx) = colony_with(REGULAR_ARM).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_edges":[{
                "from":"src","to":"fb_sink",
                "condition":"context.kind != 'idle'","default":true
            }]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a guarded default is the legal form and must commit; got {outcome:?}"
    );

    assert_default_phase(&h, &mut reg_rx, &mut fb_rx, "the guarded default").await;

    // And the guard is real: traffic neither arm names reaches nobody.
    h.send(probe("/src", "idle")).await;
    assert_silent(&mut fb_rx, "the guarded default declines what it excludes").await;
    assert_silent(&mut reg_rx, "the regular arm declines it too").await;

    h.shutdown().await;
}

// ── (d) the default lands BESIDE a content-equal regular edge ────────────────

/// The T4 identity, seen from the mutation side. `from`, `to`, `condition` and
/// `modifier` are the same as the live regular edge's — only the phase differs,
/// and on the four-term identity the apply arm's dedup would swallow the insert
/// and report `Committed` over a topology missing an edge nobody reported.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_default_lands_beside_an_otherwise_equal_regular_edge() {
    let (td, h, _reg_rx, _fb_rx) =
        colony_with(r#"{"from":"/src","to":"/fb_sink","condition":"context.kind == 'work'"}"#)
            .await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_edges":[{
                "from":"src","to":"fb_sink",
                "condition":"context.kind == 'work'","default":true
            }]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the second edge differs in its phase and must land; got {outcome:?}"
    );

    let root = td.path().to_path_buf();
    h.shutdown().await;
    assert_eq!(
        persisted_phases(&root, "/src", "/fb_sink"),
        vec![0, 1],
        "two edges, one per phase — the dedup must not swallow the default"
    );
}

// ── (e) an unguarded default is a hint, never a refusal ──────────────────────

/// An unguarded default takes everything the sender would otherwise
/// dead-letter — the thing a hint exists for, and a legal topology. The verdict
/// is `Committed`; the advisory is a `warn` line (the same sentence
/// `bootstrap.rs` puts in `BootstrapPlan::advisories`), which no mutation
/// channel carries and which this test therefore cannot read without a global
/// subscriber. What it CAN say is that the advisory did not become a refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unguarded_default_commits() {
    let (_td, h, mut reg_rx, mut fb_rx) = colony_with("").await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_edges":[{"from":"src","to":"fb_sink","default":true}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "an unguarded default is a hint, not a refusal; got {outcome:?}"
    );

    // And it is the consumer of everything, since there is no regular arm.
    h.send(probe("/src", "anything")).await;
    let got = recv_one(&h, &mut fb_rx, "the lone unguarded default").await;
    assert_eq!(got.target, Path::new("/fb_sink"));
    assert_silent(&mut reg_rx, "nothing else is wired").await;

    h.shutdown().await;
}
