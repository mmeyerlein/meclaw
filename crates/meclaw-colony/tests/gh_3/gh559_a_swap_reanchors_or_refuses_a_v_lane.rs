//! GH #559 (v-lanes, task A1) — a `swap_nodes` re-anchors a v-lane, or it
//! refuses BY NAME.
//!
//! Ruling R-V2 (2026-08-31). A v-lane ends DEEP inside a unit — `/caller →
//! /egon/talky`, not `/caller → /egon`. `swap_nodes` swings the external edges
//! of the node it replaces, and "external" has always meant *an edge naming
//! that node exactly*: a deep edge into the subtree was left alone on purpose
//! (`plan_edge_swing`, GH #256 — the subtree's own inside belongs to the
//! subtree). For a v-lane that rule produces a lane pointing into a generation
//! that just fell out of the graph. Silently.
//!
//! So a v-lane is identified the one way R-V2 allows — by SUBTREE MEMBERSHIP,
//! no owner field, no second bookkeeping — and then one of two things happens:
//!
//! * the replacement has the same relative shape → the lane is translated
//!   (`/egon/talky` → `/egon2/talky`) and the swap commits;
//! * the replacement declares no connect point at that relative path → the
//!   WHOLE swap is refused, `v_lane_unanchored`, and the old lane stands
//!   untouched. Never quietly dropped.
//!
//! Both verdicts are read off the colony's own edge table through
//! `/colony/graph`.

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

/// A unit template: a hive with one `talky` inside it. `at` is the connect
/// point its contract declares for the lane `recall` — `Some("./talky")` is the
/// form-equal successor, `None` a replacement that never invites the lane in.
fn write_unit_template(root: &std::path::Path, name: &str, at: Option<&str>) {
    let tpl = root.join("templates").join(name);
    write(&tpl, "template.json", &format!(r#"{{"name":"{name}"}}"#));
    let lane_at = at.map_or_else(String::new, |a| format!(r#""at":["{a}"],"#));
    write(
        &tpl,
        "config.json",
        &format!(
            r#"{{"cell":{{"type":"hive"}},"params":{{"contract":{{"accepts":[
                {{"route":"recall",{lane_at}"because":"the unit's own recall lane"}}]}}}}}}"#
        ),
    );
    write(&tpl, "talky/config.json", ECHO);
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

/// Every edge out of `/caller`, as `(to, lane)` — the positive receipt of both
/// cases: what the colony's own edge table says the lane points at.
async fn caller_lanes(h: &ColonyHandle) -> Vec<(String, Option<String>)> {
    read_graph(h)
        .await
        .edges
        .into_iter()
        .filter(|e| e.from == "/caller")
        .map(|e| (e.to, e.lane))
        .collect()
}

/// Boot a colony holding `/caller` and the unit `/egon` (a hive that declares
/// the `recall` lane docking at `./talky`), plus the two successor templates.
/// Returns the handle after the v-lane `/caller → /egon/talky` has been drawn.
async fn colony_with_a_v_lane(td: &tempfile::TempDir) -> ColonyHandle {
    let root = td.path();
    write(root, "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(root, "main/caller/config.json", ECHO);
    write(
        root,
        "main/egon/config.json",
        r#"{"cell":{"type":"hive"},"params":{"contract":{"accepts":[
            {"route":"recall","at":["./talky"],"because":"the unit's own recall lane"}]}}}"#,
    );
    write(root, "main/egon/talky/config.json", ECHO);
    // The two successors: form-equal, and one that never invites the lane.
    write_unit_template(root, "unit_v2", Some("./talky"));
    write_unit_template(root, "unit_bare", None);

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

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./caller","to":"./egon/talky","lane":"recall"}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the v-lane must exist before a swap can be asked about it: {outcome:?}"
    );
    h
}

// ── (a) the form-equal swap carries the lane ─────────────────────────────────

/// `egon2` is instantiated from a template with the SAME relative shape — a
/// `talky` inside, and a contract that docks `recall` at `./talky`. The swap
/// therefore has somewhere to put the lane, and puts it there: after it,
/// `/caller` reaches `/egon2/talky` and nothing reaches into the retired
/// generation any more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_formequal_swap_carries_the_v_lane() {
    let td = tempfile::TempDir::new().unwrap();
    let h = colony_with_a_v_lane(&td).await;

    // The shape a generation change uses (GH #256): the successor is grown and
    // the slot's edges are swung in ONE mutation, so there is no instant at
    // which the lane hangs in the air.
    let swapped = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"egon2","template":"unit_v2"}],
            "swap_nodes":[{"match":{"name":"egon"},"with":{"name":"egon2"}}]
        }}),
    )
    .await;
    assert!(
        matches!(swapped, MutationOutcome::Committed { .. }),
        "a form-equal successor takes the lane: {swapped:?}"
    );

    let lanes = caller_lanes(&h).await;
    assert!(
        lanes.contains(&("/egon2/talky".to_string(), Some("recall".to_string()))),
        "the v-lane is re-anchored onto the successor, lane and all: {lanes:?}"
    );
    assert!(
        !lanes.iter().any(|(to, _)| to == "/egon/talky"),
        "and nothing still reaches into the retired generation: {lanes:?}"
    );

    h.shutdown().await;
}

/// The same swap, written with the two spellings a diff really carries: the
/// `add_nodes` entry says `./egon2`, the `swap_nodes[].with` says `egon2`.
///
/// They are the SAME node (Befund 6 / GH #179) and every other reader on the
/// mutation surface resolves before deciding. The v-lane check has to as well:
/// on raw strings it found no `add_nodes` entry for the successor, therefore no
/// template, therefore no contract — and refused a lane that had a perfectly
/// good home. A refusal produced by a spelling is worse than no check.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_successors_template_is_found_through_either_spelling() {
    let td = tempfile::TempDir::new().unwrap();
    let h = colony_with_a_v_lane(&td).await;

    let swapped = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"./egon2","template":"unit_v2"}],
            "swap_nodes":[{"match":{"name":"egon"},"with":{"name":"egon2"}}]
        }}),
    )
    .await;
    assert!(
        matches!(swapped, MutationOutcome::Committed { .. }),
        "`./egon2` and `egon2` are one node: {swapped:?}"
    );

    let lanes = caller_lanes(&h).await;
    assert!(
        lanes.contains(&("/egon2/talky".to_string(), Some("recall".to_string()))),
        "the lane is re-anchored, not refused over a spelling: {lanes:?}"
    );

    h.shutdown().await;
}

// ── (b) a successor without a connect point refuses the whole swap ───────────

/// `egon2` comes from a template whose contract knows the lane but names no
/// connect point. There is no honest place to put the v-lane, so the swap is
/// refused as a whole — not applied halfway, not applied with the lane dropped.
/// The old lane stands exactly as it did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_connect_point_refuses_the_swap() {
    let td = tempfile::TempDir::new().unwrap();
    let h = colony_with_a_v_lane(&td).await;

    let swapped = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"egon2","template":"unit_bare"}],
            "swap_nodes":[{"match":{"name":"egon"},"with":{"name":"egon2"}}]
        }}),
    )
    .await;
    let MutationOutcome::Rejected {
        error_code,
        details,
        ..
    } = &swapped
    else {
        panic!("a lane with nowhere to go must refuse the swap: {swapped:?}");
    };
    assert_eq!(
        error_code, "v_lane_unanchored",
        "refused by name, never silently dropped: {details}"
    );
    assert!(
        details.contains("recall") && details.contains("/egon2"),
        "the refusal names the lane and the successor that owes it a home: {details}"
    );

    let lanes = caller_lanes(&h).await;
    assert!(
        lanes.contains(&("/egon/talky".to_string(), Some("recall".to_string()))),
        "a refused swap leaves the old lane byte-identical: {lanes:?}"
    );

    h.shutdown().await;
}
