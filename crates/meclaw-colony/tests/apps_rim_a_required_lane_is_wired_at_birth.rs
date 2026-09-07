//! Apps rim (2026-09-05) — a hive may INSIST on a lane, and the insistence is
//! answered at its birth.
//!
//! `params.contract.accepts[].at` (GH #559) says WHERE a lane docks. It never
//! said whether the lane has to be there at all, and for an app that is the
//! whole question: an app that observes `partial` and is instantiated without
//! the edge that carries it is not a degraded app, it is a silent one. The
//! substrate cannot know whether the emitter at the other end really speaks
//! (`emit_partials` is the channel's own decision) — but it can know whether
//! anybody drew the edge.
//!
//! So one field: `accepts[].required`. Read as *whoever gives me birth draws an
//! edge that delivers this lane*. Checked once, in the post-state stage of the
//! mutation, against the hives THIS diff gives birth to — the same list the port
//! boundary reads (GH #562/#567), for the same reason: that boundary judges what
//! the diff DRAWS, and so does this.
//!
//! # The rule table this file pins (one test per row)
//!
//! | the lane | what the diff draws | verdict |
//! |---|---|---|
//! | `required`, no `at` (a rim lane) | nothing | `hive_contract`, by name |
//! | `required`, no `at` | an edge onto the hive path that carries it | committed |
//! | `required` with `at` | only the rim edge, no v-lane | `hive_contract`, by name |
//! | `required`, wired, then `remove_edges` later | the edge taken away again | committed — birth is over |
//! | not `required` | nothing | committed — today's behaviour, unchanged |
//! | `required`, no `at` | an edge onto the hive path for a DIFFERENT lane | `hive_contract` — the router decides, not the address |
//!
//! # Why a real colony
//!
//! Every verdict is read off the colony's own edge table and registry through
//! `/colony/graph`. A passing case is proven by the node and its edges BEING
//! there, not by "the mutation did not say no"; a refusal is proven by the named
//! `error_code`, by the lane and the hive appearing in the text, and by the
//! colony being exactly what it was before.

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

/// The colony every case boots, plus the two templates it may grow.
///
/// ```text
/// /                     root hive
/// └── c                 echo cell — the caller that draws the lanes
///
/// templates/needy       hive, sealed, insisting on `in_feed` and `in_deep`
/// templates/easygoing   the same hive with nothing required
/// ```
fn plant(root: &std::path::Path) {
    write(root, "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(root, "main/c/config.json", ECHO);

    let needy = root.join("templates/needy");
    write(
        &needy,
        "template.json",
        r#"{"name":"needy","version":"1.0.0"}"#,
    );
    write(
        &needy,
        "config.json",
        r#"{"cell":{"type":"hive"},
            "params":{"ports":[],
              "contract":{"accepts":[
                {"route":"in_feed","required":true,
                 "because":"the feed this hive cannot live without"},
                {"route":"in_opt",
                 "because":"a lane it is happy to do without"},
                {"route":"in_deep","required":true,"at":["./inner"],
                 "because":"a required corridor onto the cell that reads it"}]},
              "graph":{"edges":[
                {"from":".","to":"./inner",
                 "condition":"has(hop.route) && hop.route == 'in_feed'"},
                {"from":".","to":"./inner",
                 "condition":"has(hop.route) && hop.route == 'in_opt'"},
                {"from":"./inner","to":"."}]}}}"#,
    );
    write(&needy, "inner/config.json", ECHO);

    let easy = root.join("templates/easygoing");
    write(
        &easy,
        "template.json",
        r#"{"name":"easygoing","version":"1.0.0"}"#,
    );
    write(
        &easy,
        "config.json",
        r#"{"cell":{"type":"hive"},
            "params":{"ports":[],
              "contract":{"accepts":[
                {"route":"in_opt",
                 "because":"a lane it is happy to do without"}]},
              "graph":{"edges":[
                {"from":".","to":"./inner",
                 "condition":"has(hop.route) && hop.route == 'in_opt'"}]}}}"#,
    );
    write(&easy, "inner/config.json", ECHO);
}

async fn boot(root: &std::path::Path) -> ColonyHandle {
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
    h
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

fn refusal(outcome: &MutationOutcome) -> (&str, &str) {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => (error_code, details),
        MutationOutcome::Committed { id } => {
            panic!("expected a refusal, the mutation committed as {id}")
        }
    }
}

/// The rim edge that delivers `in_feed` to the hive path.
fn rim_edge() -> JsonValue {
    json!({"from":"./c","to":"./h",
           "condition":"has(hop.route) && hop.route == 'in_feed'"})
}

/// The v-lane that delivers `in_deep` onto the declared connect point.
fn deep_edge() -> JsonValue {
    json!({"from":"./c","to":"./h/inner","lane":"in_deep",
           "condition":"has(hop.route) && hop.route == 'in_deep'"})
}

/// The birth of `/h` from `needy`, with whatever edges the case draws.
fn birth(edges: Vec<JsonValue>) -> JsonValue {
    json!({"scope":"/","diff":{
        "add_nodes":[{"name":"h","template":"needy"}],
        "add_edges": edges}})
}

// ── Row 1: a required rim lane nobody wired ──────────────────────────────────

/// The hive says it cannot live without `in_feed`, and the diff that gives it
/// birth draws nothing. Refused by name, with the lane, the hive path and the
/// hive's own sentence in the text — and the colony is what it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_required_lane_left_unwired_refuses_the_birth_by_name() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;

    let outcome = send_mutation(&h, birth(vec![])).await;
    let (code, details) = refusal(&outcome);
    assert_eq!(
        code, "hive_contract",
        "a required lane nobody wired refuses the birth: {outcome:?}"
    );
    assert!(
        details.contains("in_feed") && details.contains("/h"),
        "the refusal names the lane and the hive that insists on it: {details}"
    );
    assert!(
        details.contains("in_deep"),
        "and it collects: both unwired lanes in one refusal, not just the first: {details}"
    );
    assert!(
        details.contains("the feed this hive cannot live without"),
        "and it carries the hive's own sentence: {details}"
    );

    let graph = read_graph(&h).await;
    assert!(
        !graph.nodes.iter().any(|n| n.path.starts_with("/h")),
        "a refused birth leaves no node behind: {:?}",
        graph.nodes
    );

    h.shutdown().await;
}

// ── Row 2: the same birth, wired ─────────────────────────────────────────────

/// The same template, the same diff, plus the two edges it owes. Both stand in
/// the colony's own edge table afterwards — that is the positive receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wired_required_lane_lets_the_birth_through() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;

    let outcome = send_mutation(&h, birth(vec![rim_edge(), deep_edge()])).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a birth that wires what the hive insists on commits: {outcome:?}"
    );

    let graph = read_graph(&h).await;
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.from == "/c" && e.to == "/h" && e.lane.is_none()),
        "the rim edge stands: {:?}",
        graph.edges
    );
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.from == "/c" && e.to == "/h/inner" && e.lane.as_deref() == Some("in_deep")),
        "and so does the v-lane onto the connect point: {:?}",
        graph.edges
    );

    h.shutdown().await;
}

// ── Row 3: a required lane with connect points needs its v-lane ──────────────

/// A lane that names `at` docks BELOW the rim by declaration, so an edge onto
/// the hive path is not the edge it asked for. Only the rim lane is wired here,
/// and the refusal says which lane is still missing and where it belongs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_required_connect_point_needs_its_v_lane() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;

    let outcome = send_mutation(&h, birth(vec![rim_edge()])).await;
    let (code, details) = refusal(&outcome);
    assert_eq!(
        code, "hive_contract",
        "a required connect point nobody docked on refuses the birth: {outcome:?}"
    );
    assert!(
        details.contains("in_deep") && details.contains("./inner"),
        "the refusal names the lane and the connect point it wanted: {details}"
    );
    assert!(
        !details.contains("in_feed"),
        "and it does not name the lane that IS wired: {details}"
    );

    let graph = read_graph(&h).await;
    assert!(
        !graph.nodes.iter().any(|n| n.path.starts_with("/h")),
        "a refused birth leaves no node behind: {:?}",
        graph.nodes
    );

    h.shutdown().await;
}

// ── Row 4: the check is a birth check, and birth is over ─────────────────────

/// The documented limit. A standing hive that loses the edge again is
/// `remove_edges` business: the mutation removing it instantiates nothing, so
/// there is no birth to judge and the substrate does not judge one. The door
/// check of standing hives stays exactly what it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_standing_hive_is_not_judged_again() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;

    let born = send_mutation(&h, birth(vec![rim_edge(), deep_edge()])).await;
    assert!(
        matches!(born, MutationOutcome::Committed { .. }),
        "the wired birth is the pre-state: {born:?}"
    );

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"remove_edges":[{"match":{"from":"./c","to":"./h"}}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "taking the edge away again is not a birth: {outcome:?}"
    );

    let graph = read_graph(&h).await;
    assert!(
        !graph.edges.iter().any(|e| e.from == "/c" && e.to == "/h"),
        "and the edge really went: {:?}",
        graph.edges
    );
    assert!(
        graph.nodes.iter().any(|n| n.path == "/h/inner"),
        "while the hive stands on: {:?}",
        graph.nodes
    );

    h.shutdown().await;
}

// ── Row 5: an optional lane is the world as it was ───────────────────────────

/// Absent and `false` are the same statement, and that statement is today's
/// behaviour: a hive whose lanes are all optional is born into a colony that
/// wires none of them, exactly as every hive was born before this field
/// existed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_optional_lane_changes_nothing() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"e","template":"easygoing"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "an optional lane demands nothing of the mutation: {outcome:?}"
    );

    let graph = read_graph(&h).await;
    assert!(
        graph.nodes.iter().any(|n| n.path == "/e/inner"),
        "the hive stands, unwired: {:?}",
        graph.nodes
    );

    h.shutdown().await;
}

// ── Row 6: an edge onto the hive path is not automatically the edge ──────────

/// The verdict comes from the ROUTER, not from the address. `./c -> ./h` is an
/// edge onto the hive path, and a check that asked "does anything address this
/// hive" would call `in_feed` wired. It does not: the guard names `in_opt`, so a
/// message on `in_feed` would never take it, and the refusal still stands.
///
/// This is the same reason the door check runs `apply_edges` instead of reading
/// condition text — the shipped templates open whole families of lanes with one
/// `startsWith('in_')`, and no string comparison survives that.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edge_for_another_lane_does_not_count_as_wiring() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;

    let outcome = send_mutation(
        &h,
        birth(vec![
            json!({"from":"./c","to":"./h",
                   "condition":"has(hop.route) && hop.route == 'in_opt'"}),
            deep_edge(),
        ]),
    )
    .await;
    let (code, details) = refusal(&outcome);
    assert_eq!(
        code, "hive_contract",
        "an edge the required lane would never take is no wiring: {outcome:?}"
    );
    assert!(
        details.contains("in_feed"),
        "and the refusal still names the lane that has no edge: {details}"
    );
    assert!(
        !details.contains("in_deep"),
        "while the connect point that IS docked on stays out of it: {details}"
    );

    let graph = read_graph(&h).await;
    assert!(
        !graph.nodes.iter().any(|n| n.path.starts_with("/h")),
        "a refused birth leaves no node behind: {:?}",
        graph.nodes
    );

    h.shutdown().await;
}
