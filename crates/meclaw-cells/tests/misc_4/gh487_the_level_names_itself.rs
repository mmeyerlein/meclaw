//! GH #487 — `.` names the scope root in a mutation, exactly as it names the
//! level in a template graph.
//!
//! A template's `params.graph` spells *a lane that leaves this level* as
//! `{"from": "./firewall", "to": "."}`, and the boot path resolves it through
//! `Path::resolve`, where `.` means "stay at the sender". 277 of the 561 edges
//! the shipped templates declare are spelled that way. `add_edges` refused the
//! same string with `edge_schema: to='.' unknown`, because the endpoint check
//! classified `.` as a SHORT NAME and no node is called `.`.
//!
//! The topology was never unreachable — the same lane is drawable from the
//! parent scope, naming the level by its relative path. What was unreachable is
//! the SPELLING, and that costs twice: it is the spelling every shipped template
//! teaches, and obeying the workaround widens the declaration's `scope_root`
//! from the level to the level's parent, i.e. asks the broker to permit more
//! than the change needs.
//!
//! **What is fixed, and where.** `.` (and its equal-by-construction twin `./`)
//! resolves to the declaration's own `scope` — one namespace decision, in
//! `mutation::validate::scoped_name`, which is the one function every check that
//! asks "which namespace is this name in" already goes through. The apply arm
//! needed nothing: it has always resolved endpoints with `resolve_scoped_path`,
//! i.e. with the same `Path::resolve` the boot path uses.
//!
//! **The declaration is not rewritten, and that is the digest ruling.** A
//! manifest is parked under the sha256 of its own canonical bytes and submitted
//! by that digest, so a human says yes to bytes and the submitter refuses
//! anything whose bytes changed on the way. Canonicalising `.` into `./q` before
//! the digest would make the digest a digest of something nobody was shown, and
//! would change the digest of every manifest already parked. So `.` is resolved
//! at the point of USE and never in the document: `mutation_log.payload_json`
//! keeps the spelling the caller wrote. Two spellings of one node have two
//! digests — which is already true of `./q` and `q` (Befund 6) and therefore no
//! new class of ambiguity.

use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path, Uuid};
use meclaw_testing::{ColonyHandle, EmitOnceMockCellFactory};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Generous per the 30s failure-marker convention — the round trip wakes two
/// dormant cells and crosses a hive boundary twice.
const ROUND_TRIP: Duration = Duration::from_secs(30);

/// The two spellings `Path::resolve` treats as "stay at the sender". They are
/// one token as far as the substrate is concerned, and every case below runs
/// over both: "these two decide the same way" is the half that rots.
const SELF_SPELLINGS: [&str; 2] = [".", "./"];

const HIVE: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

fn cell_config(cell_type: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"{cell_type}","idle_timeout_ms":60000}},"params":{{}},
            "contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
    )
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent directory")).expect("create the directory");
    std::fs::write(p, body).expect("write the file");
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
        .expect("the mutation reaches the door");
    ack_rx.await.expect("the door acks")
}

/// Whether the persisted graph carries `from -> to`. Read from `colony.db`,
/// which is what survives a restart and therefore the only proof that a lane is
/// real rather than merely un-refused.
fn edge_persisted(root: &std::path::Path, from: &str, to: &str) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE from_path = ?1 AND to_path = ?2",
            [from, to],
            |r| r.get(0),
        )
        .expect("count the edges");
    n > 0
}

/// Every `payload_json` the mutation log holds, in insertion order.
fn logged_payloads(root: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let mut stmt = conn
        .prepare("SELECT payload_json FROM mutation_log ORDER BY created_at, id")
        .expect("prepare");
    let out = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .expect("query")
        .map(|r| r.expect("a row"))
        .collect();
    drop(stmt);
    out
}

/// A colony with one level and one cell inside it:
///
/// ```text
/// /                 root hive
/// /probe            emits once, then captures  (the outside)
/// /unit             the LEVEL — `.` at scope /unit
/// /unit/q           emits once                  (the inside)
/// ```
///
/// The boot graph wires the level to the world (`/probe -> /unit`) and the
/// inside back out (`/unit/q -> /probe`), so every node is connected and active
/// before any mutation runs — the lane under test is the only thing that is not
/// there yet, which is the whole point.
async fn colony() -> (
    tempfile::TempDir,
    ColonyHandle,
    mpsc::Receiver<meclaw_core::Message>,
) {
    let td = tempfile::TempDir::new().expect("a temporary root");
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
             {"from":"./probe","to":"./unit"},
             {"from":"./unit/q","to":"./probe"}]}}}"#,
    );
    write(td.path(), "main/probe/config.json", &cell_config("probe"));
    write(td.path(), "main/unit/config.json", HIVE);
    write(td.path(), "main/unit/q/config.json", &cell_config("inner"));
    // One template, so a diff can put a node INSIDE the level and wire it to
    // the level in the same breath — the shape a level-growing recipe writes.
    write(
        td.path(),
        "templates/inner/template.json",
        r#"{"name":"inner"}"#,
    );
    write(
        td.path(),
        "templates/inner/config.json",
        &cell_config("inner"),
    );

    // Two factories, two cell types, because each carries its own capture
    // channel. The probe's is the receipt this file reads; the inner cell's is
    // never consulted — it emits on its first and only delivery.
    let (capture_tx, capture_rx) = mpsc::channel(8);
    let (unused_tx, _unused_rx) = mpsc::channel(8);
    let probe: Arc<dyn CellFactory> = Arc::new(EmitOnceMockCellFactory::new(
        Path::new("/nowhere"),
        json!({"messages": []}),
        capture_tx,
    ));
    let inner: Arc<dyn CellFactory> = Arc::new(EmitOnceMockCellFactory::new(
        Path::new("/nowhere"),
        json!({"messages": []}),
        unused_tx,
    ));

    let h = ColonyHandle::new_with_factories_at(
        &td,
        vec![
            ("probe".to_string(), probe.clone()),
            ("inner".to_string(), inner.clone()),
        ],
    );
    let mut reg = CellFactoryRegistry::new();
    reg.insert("probe".into(), probe);
    reg.insert("inner".into(), inner);
    bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
        .await
        .expect("the colony boots");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("the rescan reaches the colony");
    ack_rx
        .await
        .expect("the rescan acks")
        .expect("GH #440: the rescan must not have aborted");
    // The channel has to outlive the boot: `_unused_rx` is dropped at the end of
    // this function, which would close the inner cell's capture channel. It is
    // never read, and a closed send is ignored by the mock, so that is fine.
    (td, h, capture_rx)
}

fn committed(outcome: &MutationOutcome) -> bool {
    matches!(outcome, MutationOutcome::Committed { .. })
}

// ── the level is an endpoint, on both sides of the arrow ────────────────────

/// The reported case, in the direction the catalogue writes most often: a lane
/// that LEAVES the level, declared at the level.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_out_of_the_level_names_the_level_with_a_dot() {
    for dot in SELF_SPELLINGS {
        let (td, h, _cap) = colony().await;
        let outcome = send_mutation(
            &h,
            json!({"scope": "/unit",
                   "diff": {"add_edges": [{"from": "./q", "to": dot}]}}),
        )
        .await;
        assert!(
            committed(&outcome),
            "an exit lane spelled `{dot}` must commit; got {outcome:?}"
        );
        h.shutdown().await;
        assert!(
            edge_persisted(td.path(), "/unit/q", "/unit"),
            "and the lane `./q -> {dot}` stands in the persisted graph as /unit/q -> /unit"
        );
    }
}

/// The other side of the arrow. `{"from": ".", "to": "./cell"}` is the wiring
/// the hive boundary mandates — a level serving its own inside — and it is the
/// form `docs/cell-types.md` § "Die Hive-Grenze" writes out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_into_the_level_names_the_level_with_a_dot() {
    for dot in SELF_SPELLINGS {
        let (td, h, _cap) = colony().await;
        let outcome = send_mutation(
            &h,
            json!({"scope": "/unit",
                   "diff": {"add_edges": [{"from": dot, "to": "./q"}]}}),
        )
        .await;
        assert!(
            committed(&outcome),
            "an entry lane spelled `{dot}` must commit; got {outcome:?}"
        );
        h.shutdown().await;
        assert!(
            edge_persisted(td.path(), "/unit", "/unit/q"),
            "and the lane `{dot} -> ./q` stands in the persisted graph as /unit -> /unit/q"
        );
    }
}

/// `.` is inside its own scope, so it is never `scope_out_of_bounds` — the
/// containment guard resolves it to the guard scope itself, and a scope contains
/// itself. Asserted through the negative: whatever a refusal says, it must not
/// say that.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_level_is_in_bounds_of_its_own_scope() {
    let (_td, h, _cap) = colony().await;
    let outcome = send_mutation(
        &h,
        json!({"scope": "/unit",
               "diff": {"add_edges": [{"from": "./q", "to": "."}]}}),
    )
    .await;
    if let MutationOutcome::Rejected { error_code, .. } = &outcome {
        assert_ne!(
            error_code, "scope_out_of_bounds",
            "`.` addresses the scope root, which is in bounds by definition"
        );
    }
    h.shutdown().await;
}

/// `remove_edges` reads the same vocabulary. A lane a caller could draw with `.`
/// and then not take away again would be a one-way grammar.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_edges_takes_the_lane_the_level_names() {
    let (td, h, _cap) = colony().await;
    assert!(
        committed(
            &send_mutation(
                &h,
                json!({"scope": "/unit",
                       "diff": {"add_edges": [{"from": "./q", "to": "."}]}}),
            )
            .await
        ),
        "the lane is drawn first"
    );
    let outcome = send_mutation(
        &h,
        json!({"scope": "/unit",
               "diff": {"remove_edges": [{"match": {"from": "./q", "to": "."}}]}}),
    )
    .await;
    assert!(
        committed(&outcome),
        "removing the same lane by the same spelling must commit; got {outcome:?}"
    );
    h.shutdown().await;
    assert!(
        !edge_persisted(td.path(), "/unit/q", "/unit"),
        "and the lane is gone from the persisted graph"
    );
}

// ── what the spelling buys: the narrow scope ────────────────────────────────

/// The shape a level-growing recipe writes: put a node INSIDE the container and
/// wire it to the container, in one declaration, **scoped at the container**.
///
/// This is the half of the issue that is about authority rather than
/// convenience. Before the fix, an edge between the container and something in
/// it could only be declared one level UP — `{"from": "./orgs", "to":
/// "./orgs/mm"}` at scope `/os` instead of `{"from": ".", "to": "./mm"}` at
/// scope `/os/orgs` — so the declaration's `scope_root` was the container's
/// parent, and a broker asked to permit the change was asked to permit the
/// parent. Growing one organisation asked for the OS shell.
///
/// The substrate no longer stands in the way of the narrow scope. What a given
/// recipe renders is the recipe's business (`grow_level` is not touched here);
/// this asserts only that the narrow form is expressible and commits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_level_can_be_grown_inside_the_container_it_is_scoped_at() {
    let (td, h, _cap) = colony().await;
    let outcome = send_mutation(
        &h,
        json!({"scope": "/unit",
               "diff": {
                   "add_nodes": [{"name": "grown", "template": "inner"}],
                   "add_edges": [
                       {"from": ".", "to": "./grown"},
                       {"from": "./grown", "to": "."}]}}),
    )
    .await;
    assert!(
        committed(&outcome),
        "a node plus its two lanes to the container, scoped AT the container, \
         must commit; got {outcome:?}"
    );
    h.shutdown().await;
    assert!(edge_persisted(td.path(), "/unit", "/unit/grown"));
    assert!(edge_persisted(td.path(), "/unit/grown", "/unit"));
}

// ── the boundary: the rule resolves, it does not invent ─────────────────────

/// The root scope is **not** a special case, and the first draft of this file
/// assumed it was. `.` at scope `/` names the root hive — the colony's own top
/// hive-scope marker, a perfectly ordinary node — and that is the lane a marked
/// answer leaves by (`-> .` at whichever hive it ran out of graph at, GH #163).
/// A rule that resolves the scope root has to resolve THIS one too, or it would
/// be a rule with a hole at the top.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_root_scope_names_the_root_hive() {
    let (td, h, _cap) = colony().await;
    let outcome = send_mutation(
        &h,
        json!({"scope": "/", "diff": {"add_edges": [{"from": "./probe", "to": "."}]}}),
    )
    .await;
    assert!(
        committed(&outcome),
        "`.` at the root scope names the root hive; got {outcome:?}"
    );
    h.shutdown().await;
    assert!(edge_persisted(td.path(), "/probe", "/"));
}

/// What the rule does NOT do: invent a node. A scope root nothing answers to —
/// no registry row, no hive scope — is still an unknown endpoint, refused with
/// the `edge_schema` an unknown endpoint always carried.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scope_root_nothing_answers_to_is_still_unknown() {
    let (_td, h, _cap) = colony().await;
    let outcome = send_mutation(
        &h,
        json!({"scope": "/ghost", "diff": {"add_edges": [{"from": ".", "to": "."}]}}),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected { error_code, .. } => assert_eq!(
            error_code, "edge_schema",
            "`.` at a scope with nothing behind it names no node: {outcome:?}"
        ),
        other => panic!("a lane onto an empty scope root must be refused; got {other:?}"),
    }
    h.shutdown().await;
}

// ── the digest ruling: the document keeps the spelling it was written in ────

/// The manifest a human approved and the manifest that is applied have to be
/// the same bytes — that is what parking under a digest buys. So the resolution
/// of `.` happens where the endpoint is USED and nowhere else: the audit row
/// carries the caller's own spelling back out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_committed_declaration_keeps_the_spelling_it_was_written_in() {
    let (td, h, _cap) = colony().await;
    assert!(
        committed(
            &send_mutation(
                &h,
                json!({"scope": "/unit",
                       "diff": {"add_edges": [{"from": "./q", "to": "."}]}}),
            )
            .await
        ),
        "the lane commits"
    );
    h.shutdown().await;
    let payloads = logged_payloads(td.path());
    let hit = payloads
        .iter()
        .find(|p| p.contains("add_edges"))
        .unwrap_or_else(|| panic!("no add_edges row in the mutation log: {payloads:?}"));
    let parsed: Value = meclaw_core::serde_json::from_str(hit).expect("the row is JSON");
    assert_eq!(
        parsed["diff"]["add_edges"][0]["to"],
        json!("."),
        "the audit row must carry `.`, not a canonicalised `./…` — a rewritten \
         declaration is a declaration whose digest no longer describes it"
    );
}

// ── and it fires ───────────────────────────────────────────────────────────

/// A drawn lane that nothing travels is a row in a table. The round trip:
///
/// ```text
/// /probe --(boot edge)--> /unit --(THE LANE)--> /unit/q --(boot edge)--> /probe
/// ```
///
/// The middle hop is the mutation's `{"from": ".", "to": "./q"}`: the level
/// forwarding into its own inside, evaluated as an ordinary hive transit. The
/// receipt is positive — the probe's capture channel receives its second
/// delivery — and it can only arrive if the lane the level named routed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lane_the_level_names_carries_a_message() {
    let (_td, h, mut capture_rx) = colony().await;
    let outcome = send_mutation(
        &h,
        json!({"scope": "/unit",
               "diff": {"add_edges": [{"from": ".", "to": "./q"}]}}),
    )
    .await;
    assert!(
        committed(&outcome),
        "the transit lane must commit before it can carry anything; got {outcome:?}"
    );

    h.send(
        MessageBuilder::new(Path::new("/probe"))
            .body(Body::Inline(json!({"messages": []})))
            .build(),
    )
    .await;

    match tokio::time::timeout(ROUND_TRIP, capture_rx.recv()).await {
        Ok(Some(_)) => {}
        Ok(None) => panic!("the probe's capture channel closed before the round trip finished"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!(
                "the message never came back through the level within {ROUND_TRIP:?}; DLQ: {dlq:?}"
            )
        }
    }
    h.shutdown().await;
}
