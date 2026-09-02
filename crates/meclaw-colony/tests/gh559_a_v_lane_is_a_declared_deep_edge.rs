//! GH #559 (v-lanes, task A1) — a v-lane is a DECLARED deep edge.
//!
//! A v-lane is not a new edge class. It is the R12 deep edge the substrate has
//! always accepted, used deliberately: one edge from a caller straight onto the
//! rim it wants, instead of a hand-through chain that re-declares the same lane
//! on every level in between.
//!
//! What was missing is the declaration. Ruling R-V1 (2026-08-31) put it on the
//! two ends and the levels between them:
//!
//! * the edge NAMES its lane (`add_edges[].lane`) — a CEL guard cannot be read
//!   back reliably, so the lane has to be said out loud;
//! * the TARGET names the connect point (`params.contract.accepts[].at`) — a
//!   sealed hive stays sealed, and the v-lane is the one exception the template
//!   itself pronounces;
//! * a level in between that DECLARES the lane may not be skipped (it stamps or
//!   filters), and a level that declares nothing is transparent.
//!
//! # The rule table this file pins (one test per row)
//!
//! | crossed level between LCA and endpoint | what its contract says | verdict |
//! |---|---|---|
//! | unsealed | nothing | transparent — skipped |
//! | unsealed | lane declared, no matching `at` | `v_lane_mandatory_hop` |
//! | sealed (`params.ports`) | `at` carries the relative path to the endpoint | allowed |
//! | sealed | nothing, or `at` without a hit | `hive_port_boundary` (unchanged) |
//! | the target hive itself | `at` does not name the endpoint for this lane | `v_lane_no_connect_point` |
//!
//! An edge WITHOUT `lane` keeps exactly today's behaviour — that is the fifth
//! test, and it is the one that says this whole feature is opt-in.
//!
//! # Why a real colony
//!
//! Every verdict is read off the colony's own edge table through
//! `/colony/graph`. A passing case is proven by the edge BEING there with the
//! lane it declared, not by "the mutation did not say no"; a refusal is proven
//! by the named `error_code` AND by the edge being absent afterwards. The pure
//! stage-6 helpers have their own unit tests next to the implementation; what
//! this file asks is whether the wired-up substrate reaches the same verdict.

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

/// The tree every case boots:
///
/// ```text
/// /                     root hive
/// ├── caller            echo cell — the outside end of the v-lane
/// └── outer             hive — the crossed level
///     └── inner         hive — the TARGET hive
///         └── target    echo cell — the deep endpoint
/// ```
///
/// `outer_params` and `inner_params` are the two `params` blocks under test:
/// they carry `ports` (the seal) and `contract` (the lane declaration with its
/// `at` connect points).
fn plant(root: &std::path::Path, outer_params: &str, inner_params: &str) {
    write(root, "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(root, "main/caller/config.json", ECHO);
    write(
        root,
        "main/outer/config.json",
        &format!(r#"{{"cell":{{"type":"hive"}},"params":{outer_params}}}"#),
    );
    write(
        root,
        "main/outer/inner/config.json",
        &format!(r#"{{"cell":{{"type":"hive"}},"params":{inner_params}}}"#),
    );
    write(root, "main/outer/inner/target/config.json", ECHO);
}

/// The `contract` half of a `params` block: accepts `recall`, docked at `at`.
fn accepts_recall_at(at: &str) -> String {
    format!(
        r#""contract":{{"accepts":[{{"route":"recall","at":["{at}"],
            "because":"the lane this level speaks about"}}]}}"#
    )
}

/// The same, as a complete `params` block.
fn params_accepting_recall_at(at: &str) -> String {
    format!("{{{}}}", accepts_recall_at(at))
}

async fn boot(root: &std::path::Path) -> ColonyHandle {
    let h = ColonyHandle::new_with_echo_at(root);
    let mut factories = CellFactoryRegistry::new();
    factories.insert(
        "echo".to_string(),
        std::sync::Arc::new(EchoCellFactory) as std::sync::Arc<dyn meclaw_colony::CellFactory>,
    );
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

/// The v-lane every case draws: `/caller` straight onto `/outer/inner/target`,
/// naming the lane it carries.
fn v_lane() -> JsonValue {
    json!({"scope":"/","diff":{"add_edges":[
        {"from":"./caller","to":"./outer/inner/target","lane":"recall"}
    ]}})
}

/// The POSITIVE receipt: the edge onto the deep endpoint, as the colony's own
/// edge table reports it over `/colony/graph`.
async fn deep_edge(h: &ColonyHandle) -> Option<meclaw_colony::api_dto::GraphEdgeDto> {
    read_graph(h)
        .await
        .edges
        .into_iter()
        .find(|e| e.to == "/outer/inner/target" && e.from == "/caller")
}

fn refusal_code(outcome: &MutationOutcome) -> &str {
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => error_code,
        MutationOutcome::Committed { id } => {
            panic!("expected a refusal, the mutation committed as {id}")
        }
    }
}

// ── Row 3: sealed level + matching `at` → allowed ────────────────────────────

/// `/outer` is SEALED (`params.ports`) and would refuse this endpoint today —
/// `hive_port_boundary`, because `/outer/inner/target` lies below it and the
/// other end does not. It declares the lane `recall` with an `at` that carries
/// the relative path to the endpoint, so the seal opens for THIS lane and no
/// other, and the target hive names its connect point.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_v_lane_passes_a_sealed_level() {
    let td = tempfile::TempDir::new().unwrap();
    plant(
        td.path(),
        &format!(
            r#"{{"ports":["rim"],{}}}"#,
            accepts_recall_at("./inner/target")
        ),
        &params_accepting_recall_at("./target"),
    );
    let h = boot(td.path()).await;

    let outcome = send_mutation(&h, v_lane()).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a declared v-lane passes a sealed level: {outcome:?}"
    );

    let edge = deep_edge(&h)
        .await
        .expect("the v-lane stands in the edge table");
    assert_eq!(
        edge.lane.as_deref(),
        Some("recall"),
        "the edge carries the lane it declared"
    );

    h.shutdown().await;
}

// ── Row 4: sealed level without `at` → the existing refusal stays ────────────

/// The seal is not weakened by the mere presence of a `lane`. `/outer` is
/// sealed and says nothing about `recall`, so the deep endpoint below it is
/// still an endpoint past the port — the answer is the one it has always been.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sealed_level_without_at_still_refuses() {
    let td = tempfile::TempDir::new().unwrap();
    plant(
        td.path(),
        r#"{"ports":["rim"]}"#,
        &params_accepting_recall_at("./target"),
    );
    let h = boot(td.path()).await;

    let outcome = send_mutation(&h, v_lane()).await;
    assert_eq!(
        refusal_code(&outcome),
        "hive_port_boundary",
        "a sealed level that declares nothing about the lane keeps its seal: {outcome:?}"
    );
    assert!(
        deep_edge(&h).await.is_none(),
        "a refused v-lane leaves no edge behind"
    );

    h.shutdown().await;
}

// ── Row 2: a declaring level may not be skipped ──────────────────────────────

/// `/outer` is UNSEALED — today a deep edge would sail past it. But it declares
/// the lane `recall` in its contract, which is the statement "I take part in
/// this lane" (stamp, filter, guard). Skipping it would silently drop whatever
/// it contributes, so the v-lane is refused BY NAME rather than quietly routed
/// around. Declaring an `at` for the lane is how a level says "you may pass".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declaring_level_may_not_be_skipped() {
    let td = tempfile::TempDir::new().unwrap();
    plant(
        td.path(),
        r#"{"contract":{"accepts":[{"route":"recall",
            "because":"I stamp audience and channel onto this lane"}]}}"#,
        &params_accepting_recall_at("./target"),
    );
    let h = boot(td.path()).await;

    let outcome = send_mutation(&h, v_lane()).await;
    assert_eq!(
        refusal_code(&outcome),
        "v_lane_mandatory_hop",
        "a level that declares the lane is a mandatory hop: {outcome:?}"
    );
    assert!(
        deep_edge(&h).await.is_none(),
        "a refused v-lane leaves no edge behind"
    );

    h.shutdown().await;
}

// ── Row 5: the target hive must name the connect point ───────────────────────

/// Both levels are open and say nothing, so the seal never enters into it. The
/// refusal comes from the OTHER end of R-V1: a v-lane docks where the target
/// says it docks. `/outer/inner` names no connect point for `recall`, so there
/// is nothing for this edge to attach to and the refusal says which lane and
/// which endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeclared_connect_point_is_refused_by_name() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path(), r#"{}"#, r#"{}"#);
    let h = boot(td.path()).await;

    let outcome = send_mutation(&h, v_lane()).await;
    assert_eq!(
        refusal_code(&outcome),
        "v_lane_no_connect_point",
        "the target hive never declared where this lane docks: {outcome:?}"
    );
    let MutationOutcome::Rejected { details, .. } = &outcome else {
        unreachable!("checked above")
    };
    assert!(
        details.contains("recall") && details.contains("/outer/inner"),
        "the refusal names the lane and the hive that owes the connect point: {details}"
    );
    assert!(
        deep_edge(&h).await.is_none(),
        "a refused v-lane leaves no edge behind"
    );

    h.shutdown().await;
}

// ── One template, two doors, one meaning ────────────────────────────────────

/// A template may declare a lane on its OWN deep edge, and the declaration has
/// to survive both doors into a colony.
///
/// The boot reads `params.graph.edges[]` through `config::EdgeSpec` and carries
/// the key; instantiation walks the same template through `subtree::EdgeSpec`,
/// which is a second, hand-rolled reading. A key the second reading does not
/// know is not an error there — it is silence, and the result is one template
/// that declares a v-lane when it is BOOTED and an ordinary edge when it is
/// INSTANTIATED. Nobody would see it until a later swap failed to re-anchor a
/// lane nobody knew was gone.
///
/// Neither door judges these edges — a subtree-internal edge is a template's
/// statement about itself (ruling 2026-08-15), and the boot enforces no seal
/// either. What they owe is to MEAN the same thing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_template_declared_lane_survives_both_doors() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();

    // Door one: the birth topology declares a v-lane of its own.
    write(
        root,
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./caller","to":"./outer/inner/target","lane":"recall"}
        ]}}}"#,
    );
    write(root, "main/caller/config.json", ECHO);
    write(
        root,
        "main/outer/config.json",
        r#"{"cell":{"type":"hive"}}"#,
    );
    write(
        root,
        "main/outer/inner/config.json",
        &format!(
            r#"{{"cell":{{"type":"hive"}},"params":{}}}"#,
            params_accepting_recall_at("./target")
        ),
    );
    write(root, "main/outer/inner/target/config.json", ECHO);

    // Door two: a template that declares the same shape about its own inside.
    let tpl = root.join("templates/unit_lane");
    write(&tpl, "template.json", r#"{"name":"unit_lane"}"#);
    write(
        &tpl,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./inner/target","lane":"recall"}
        ]}}}"#,
    );
    write(&tpl, "inner/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(&tpl, "inner/target/config.json", ECHO);

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

    let booted = deep_edge(&h).await.expect("the booted v-lane stands");
    assert_eq!(
        booted.lane.as_deref(),
        Some("recall"),
        "door one: the boot carries the declaration"
    );

    let grown = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"unit","template":"unit_lane"}]}}),
    )
    .await;
    assert!(
        matches!(grown, MutationOutcome::Committed { .. }),
        "the template instantiates: {grown:?}"
    );

    let instantiated = read_graph(&h)
        .await
        .edges
        .into_iter()
        .find(|e| e.from == "/unit" && e.to == "/unit/inner/target")
        .expect("the template's own deep edge stands in the instance");
    assert_eq!(
        instantiated.lane.as_deref(),
        Some("recall"),
        "door two: instantiation carries the same declaration — one template \
         cannot mean two things"
    );

    h.shutdown().await;
}

// ── The dedup trap: a v-lane must never be reported and not exist ────────────

/// Edge identity is the five ROUTING terms (`EdgeTable::contains_equal`); the
/// lane is deliberately not among them — it is a declaration ABOUT an edge, not
/// something the router reads. That leaves one trap, and it sits exactly on the
/// path this feature exists for.
///
/// R-V3 migrates a hand-through chain onto one declared lane. Somebody draws
/// the v-lane onto a pair the table already holds without a lane: content-equal
/// on all five terms, so the dedup skips the insert and the caller is handed
/// `Committed` for an edge that does not exist and never will. Silently.
///
/// Refused by name instead, pre-destructively, with both ways out in the text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_is_never_swallowed_by_an_identical_blank_edge() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path(), r#"{}"#, &params_accepting_recall_at("./target"));
    let h = boot(td.path()).await;

    // The blank hand-through edge that is already there.
    let plain = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./caller","to":"./outer/inner/target"}
        ]}}),
    )
    .await;
    assert!(
        matches!(plain, MutationOutcome::Committed { .. }),
        "the blank edge is the pre-state: {plain:?}"
    );

    // The migration, written naively.
    let outcome = send_mutation(&h, v_lane()).await;
    assert_eq!(
        refusal_code(&outcome),
        "edge_schema",
        "a v-lane the table would swallow is refused, not committed: {outcome:?}"
    );
    let MutationOutcome::Rejected { details, .. } = &outcome else {
        unreachable!("checked above")
    };
    assert!(
        details.contains("remove it in the same diff or drop the lane"),
        "the refusal names both ways out: {details}"
    );

    // And the pre-state is untouched: still one edge, still blank.
    let edges = read_graph(&h)
        .await
        .edges
        .into_iter()
        .filter(|e| e.from == "/caller" && e.to == "/outer/inner/target")
        .collect::<Vec<_>>();
    assert_eq!(edges.len(), 1, "no second edge was laid: {edges:?}");
    assert_eq!(
        edges[0].lane, None,
        "and the one that stands is the blank one"
    );

    h.shutdown().await;
}

/// A colony whose target invites BOTH lanes in. That isolates what the cases
/// below are about: with only `recall` declared, a rename to `bundle` would be
/// refused twice over — once for having no connect point, once for the
/// disagreement — and the second refusal, the one under test, would hide
/// behind the first.
async fn colony_accepting_both_lanes(td: &tempfile::TempDir) -> ColonyHandle {
    plant(
        td.path(),
        r#"{}"#,
        r#"{"contract":{"accepts":[
            {"route":"recall","at":["./target"],"because":"the lane it starts on"},
            {"route":"bundle","at":["./target"],"because":"the lane it would move to"}
        ]}}"#,
    );
    boot(td.path()).await
}

/// A colony whose v-lane `/caller -> /outer/inner/target` on lane `lane` is
/// already drawn. The starting point of the two renaming cases below.
async fn colony_holding_a_v_lane(td: &tempfile::TempDir, lane: &str) -> ColonyHandle {
    let h = colony_accepting_both_lanes(td).await;
    let drawn = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./caller","to":"./outer/inner/target","lane":lane}
        ]}}),
    )
    .await;
    assert!(
        matches!(drawn, MutationOutcome::Committed { .. }),
        "the standing v-lane is the pre-state: {drawn:?}"
    );
    h
}

/// The trap has three faces, not one, and the other two are just as reachable
/// from R-V3 as the first: RENAMING a lane, and DROPPING one.
///
/// Here the table holds `recall` and the entry says `bundle`. Five terms equal,
/// so the dedup skips the insert — `Committed`, and the edge still carries
/// `recall`. A rename that reports success and renames nothing is worse than a
/// rename that fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renaming_a_lane_on_a_standing_edge_is_refused() {
    let td = tempfile::TempDir::new().unwrap();
    let h = colony_holding_a_v_lane(&td, "recall").await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./caller","to":"./outer/inner/target","lane":"bundle"}
        ]}}),
    )
    .await;
    assert_eq!(
        refusal_code(&outcome),
        "edge_schema",
        "a lane the table would not take is refused, not committed: {outcome:?}"
    );
    let MutationOutcome::Rejected { details, .. } = &outcome else {
        unreachable!("checked above")
    };
    assert!(
        details.contains("with lane 'recall' already exists") && details.contains("match its lane"),
        "the refusal names the standing lane and the way out: {details}"
    );
    assert_eq!(
        deep_edge(&h)
            .await
            .expect("the edge stands")
            .lane
            .as_deref(),
        Some("recall"),
        "and the pre-state is untouched"
    );

    h.shutdown().await;
}

/// The third face: the entry declares NO lane while the table holds one. The
/// old guard let this through entirely — it only ever asked about entries that
/// carried a lane — so dropping a lane reported `Committed` and left the lane
/// exactly where it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_a_lane_from_a_standing_edge_is_refused() {
    let td = tempfile::TempDir::new().unwrap();
    let h = colony_holding_a_v_lane(&td, "recall").await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./caller","to":"./outer/inner/target"}
        ]}}),
    )
    .await;
    assert_eq!(
        refusal_code(&outcome),
        "edge_schema",
        "silently keeping the lane is not a commit: {outcome:?}"
    );
    let MutationOutcome::Rejected { details, .. } = &outcome else {
        unreachable!("checked above")
    };
    assert!(
        details.contains("declares no lane") && details.contains("with lane 'recall'"),
        "the refusal names both sides of the disagreement: {details}"
    );

    h.shutdown().await;
}

/// The other side of the rule, and the one that keeps the dedup a dedup: two
/// entries that AGREE about the lane are not a disagreement. A re-applied
/// complete diff (the Phase-15 builder re-sends everything it knows) has to
/// stay idempotent — the edge the caller asked for really is there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn re_declaring_the_same_lane_stays_idempotent() {
    let td = tempfile::TempDir::new().unwrap();
    let h = colony_holding_a_v_lane(&td, "recall").await;

    let again = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./caller","to":"./outer/inner/target","lane":"recall"}
        ]}}),
    )
    .await;
    assert!(
        matches!(again, MutationOutcome::Committed { .. }),
        "the same declaration twice is the same edge: {again:?}"
    );

    let edges = read_graph(&h)
        .await
        .edges
        .into_iter()
        .filter(|e| e.from == "/caller" && e.to == "/outer/inner/target")
        .collect::<Vec<_>>();
    assert_eq!(edges.len(), 1, "and no twin was laid beside it: {edges:?}");

    h.shutdown().await;
}

/// The way out the refusal names, taken: the blank edge leaves in the SAME
/// diff, and the v-lane lands. This is the migration shape R-V3 asks for, and
/// it has to stay possible — a check that counted the doomed edge would refuse
/// precisely the diff it is telling people to write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_the_blank_edge_in_the_same_diff_lets_the_lane_land() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path(), r#"{}"#, &params_accepting_recall_at("./target"));
    let h = boot(td.path()).await;

    send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./caller","to":"./outer/inner/target"}
        ]}}),
    )
    .await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "remove_edges":[{"match":{"from":"./caller","to":"./outer/inner/target"}}],
            "add_edges":[{"from":"./caller","to":"./outer/inner/target","lane":"recall"}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the documented migration must commit: {outcome:?}"
    );

    let edge = deep_edge(&h).await.expect("the v-lane stands");
    assert_eq!(
        edge.lane.as_deref(),
        Some("recall"),
        "and it is the DECLARED one that stands, not the blank one it replaced"
    );

    h.shutdown().await;
}

/// GH #564, face 1: the same trap turned INWARD. Both entries live in ONE
/// diff, so there is no standing edge to disagree with — stage 6's neighbour
/// check compares each entry against the pre-state only, and the pre-state is
/// blank. The apply arm then dedups against the GROWING table: it inserts
/// `recall`, finds the second entry content-equal on the five routing terms
/// and swallows `bundle`, and the caller is handed `Committed` for a lane that
/// was never laid.
///
/// Ruling (2026-09-02): `lane` does NOT become a sixth identity term — two
/// edges differing only in the lane would both route, which is a double
/// delivery. The intra-diff face gets its own pre-destructive check instead,
/// mirroring the standing one: same code, same stage, and the text names both
/// entries and both lanes, because the caller cannot see from the outside
/// which two of their lines collapsed into one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_lanes_on_one_edge_inside_one_diff_are_refused() {
    let td = tempfile::TempDir::new().unwrap();
    let h = colony_accepting_both_lanes(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./caller","to":"./outer/inner/target","lane":"recall"},
            {"from":"./caller","to":"./outer/inner/target","lane":"bundle"}
        ]}}),
    )
    .await;
    assert_eq!(
        refusal_code(&outcome),
        "edge_schema",
        "the second lane would be swallowed by the growing table: {outcome:?}"
    );
    let MutationOutcome::Rejected { details, .. } = &outcome else {
        unreachable!("checked above")
    };
    assert!(
        details.contains("add_edges[0]")
            && details.contains("add_edges[1]")
            && details.contains("'recall'")
            && details.contains("'bundle'"),
        "the refusal names both entries and both lanes: {details}"
    );
    assert!(
        deep_edge(&h).await.is_none(),
        "and nothing was applied — the check is pre-destructive"
    );

    h.shutdown().await;
}

/// The other side of the intra-diff rule, and the one that keeps a re-applied
/// diff a no-op: two entries of one diff that AGREE about the lane are not a
/// disagreement. A generator that lists the same edge twice (the Phase-15
/// builder re-sends everything it knows) still commits, and lays exactly one
/// edge — the dedup doing its job, not swallowing a declaration.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_lane_twice_inside_one_diff_stays_idempotent() {
    let td = tempfile::TempDir::new().unwrap();
    let h = colony_accepting_both_lanes(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_edges":[
            {"from":"./caller","to":"./outer/inner/target","lane":"recall"},
            {"from":"./caller","to":"./outer/inner/target","lane":"recall"}
        ]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the same declaration twice in one diff is the same edge: {outcome:?}"
    );

    let edges = read_graph(&h)
        .await
        .edges
        .into_iter()
        .filter(|e| e.from == "/caller" && e.to == "/outer/inner/target")
        .collect::<Vec<_>>();
    assert_eq!(edges.len(), 1, "and no twin was laid beside it: {edges:?}");
    assert_eq!(
        edges[0].lane.as_deref(),
        Some("recall"),
        "carrying the lane both entries named"
    );

    h.shutdown().await;
}

// ── Row 0: no `lane` → today's behaviour, byte for byte ──────────────────────

/// The whole feature is opt-in on one field. The SAME deep edge without `lane`
/// is the R12 depth-port edge the substrate has always accepted: it commits,
/// stands in the edge table, and reports no lane. And where a seal refused it
/// before, it is refused for the same old reason — the new checks never run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plain_deep_edge_keeps_todays_behaviour() {
    let plain = json!({"scope":"/","diff":{"add_edges":[
        {"from":"./caller","to":"./outer/inner/target"}
    ]}});

    // (a) open levels: accepted, exactly as before — and NO lane on the wire.
    let open = tempfile::TempDir::new().unwrap();
    plant(open.path(), r#"{}"#, r#"{}"#);
    let h = boot(open.path()).await;
    let outcome = send_mutation(&h, plain.clone()).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "an undeclared deep edge is the R12 edge the substrate always took: {outcome:?}"
    );
    let edge = deep_edge(&h).await.expect("the deep edge stands");
    assert_eq!(edge.lane, None, "an ordinary edge declares no lane");
    h.shutdown().await;

    // (b) sealed level: refused for the reason it was always refused for.
    let sealed = tempfile::TempDir::new().unwrap();
    plant(sealed.path(), r#"{"ports":["rim"]}"#, r#"{}"#);
    let h = boot(sealed.path()).await;
    let outcome = send_mutation(&h, plain).await;
    assert_eq!(
        refusal_code(&outcome),
        "hive_port_boundary",
        "the seal is untouched by this task: {outcome:?}"
    );
    h.shutdown().await;
}

// ── Task A4 Step 1: the lane is on the `/colony/graph` wire ──────────────────

use meclaw_colony::colony_dispatch::build_graph_read_reply;
use meclaw_colony::edge_table::{Edge, EdgeTable};
use meclaw_colony::{CellStatus, RegistryEntry};
use meclaw_core::ActorHandle;

fn wire_entry(path: &Path) -> RegistryEntry {
    let (sender, _receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
    RegistryEntry {
        handle: ActorHandle::new(path.clone(), sender),
        respawn: Box::new(|| unreachable!("fixture cell is never respawned")),
        wake: None,
        restart_count: 0,
        restart_limit: 5,
        cell_id: Uuid::now_v7(),
        cell_type: "echo".into(),
        status: CellStatus::Awake,
        eager_on_reconnect: true,
        active: true,
        failed: false,
        dormant: false,
        stop_tx: None,
        death_ack_rx: None,
    }
}

/// One declared v-lane (`in_pack`) and one ordinary edge beside it, so "the
/// wire names the lane" and "the wire names nothing" are distinguishable.
fn one_lane_one_plain() -> (std::collections::HashMap<Path, RegistryEntry>, EdgeTable) {
    let mut registry = std::collections::HashMap::new();
    let mut edges = EdgeTable::new();
    for cell in ["/caller", "/deep", "/plain"] {
        let p = Path::new(cell);
        registry.insert(p.clone(), wire_entry(&p));
    }
    let mut edge = |to: &str, lane: Option<&str>| {
        edges.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new("/caller"),
            to: Path::new(to),
            condition: None,
            modifier: None,
            is_default: false,
            lane: lane.map(str::to_string),
        });
    };
    edge("/deep", Some("in_pack"));
    edge("/plain", None);
    (registry, edges)
}

fn wire_edge(reply: &JsonValue, to: &str) -> JsonValue {
    reply["graph"]["edges"]
        .as_array()
        .expect("the reply carries an edges array")
        .iter()
        .find(|e| e["to"] == json!(to))
        .unwrap_or_else(|| panic!("edge to {to} must be in the graph: {reply}"))
        .clone()
}

/// `/colony/graph` is the only read of the topology anybody outside the colony
/// gets. A v-lane whose licence is invisible there is an edge nobody can review,
/// so the reply body — not just the DTO — has to say the lane out loud. The
/// ordinary edge beside it omits the key entirely: absence IS the statement.
#[test]
fn the_graph_reply_names_the_lane_an_edge_carries() {
    let (registry, edges) = one_lane_one_plain();
    let reply = build_graph_read_reply(&registry, &edges, &json!({}));

    assert_eq!(
        wire_edge(&reply, "/deep")["lane"],
        json!("in_pack"),
        "a declared v-lane names its lane on the /colony/graph wire"
    );
    assert!(
        wire_edge(&reply, "/plain").get("lane").is_none(),
        "an ordinary edge carries no lane key at all: {}",
        wire_edge(&reply, "/plain")
    );
}

// ── The reboot door: a lane that does not survive the disk is not a lane ─────

/// GH #559, the durability half (review 2026-09-01, T1).
///
/// `lane` is the one term of a v-lane that the ROUTER never reads. Nothing in
/// delivery would notice if it were dropped on the way to `colony.db` or lost
/// on the way back — routing is a flat lookup over the five routing terms, and
/// all five would still be there. What WOULD notice is the next `swap_nodes`:
/// `v_lane_reanchor_verdict` asks the successor's contract about THIS lane, so
/// an edge that comes back from a reboot lane-less is silently re-anchorable
/// onto a hive that never vouched for it. The declaration outliving the process
/// is therefore load-bearing, and it is checked here at the DB door rather than
/// through a whole colony: one write, one fresh connection, one read.
///
/// The ordinary edge in the same table is the discriminator — a rehydration
/// that handed every edge the same lane would pass a one-row test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persisted_lane_survives_the_rehydration() {
    use meclaw_colony::persist::colony_db::ColonyDb;
    use meclaw_colony::persist::writer::ColonyWriteOp;

    let td = tempfile::TempDir::new().unwrap();
    let db_path = td.path().join("colony.db");

    let declared = Uuid::now_v7();
    let ordinary = Uuid::now_v7();
    {
        let db = ColonyDb::open(&db_path).expect("open colony.db");
        db.send_op(ColonyWriteOp::InsertEdge {
            id: declared.to_string(),
            from: "/caller".to_string(),
            to: "/member/assistants/scribe/talky".to_string(),
            created_at: 1_700_000_000,
            condition: None,
            modifier: None,
            is_default: false,
            lane: Some("in_pack".to_string()),
        })
        .await;
        db.send_op(ColonyWriteOp::InsertEdge {
            id: ordinary.to_string(),
            from: "/caller".to_string(),
            to: "/sink".to_string(),
            created_at: 1_700_000_001,
            condition: None,
            modifier: None,
            is_default: false,
            lane: None,
        })
        .await;
        db.shutdown_async().await;
    }

    // A fresh handle over the same file: the process boundary a reboot crosses.
    let reopened = ColonyDb::open(&db_path).expect("re-open colony.db");
    let edges = reopened.read_edges().expect("the edge table rehydrates");

    let back = edges
        .iter()
        .find(|e| e.id == declared)
        .expect("the v-lane row comes back");
    assert_eq!(
        back.lane.as_deref(),
        Some("in_pack"),
        "the declared lane must survive the disk — without it the next \
         swap_nodes re-anchors the edge with nothing to check against"
    );
    assert_eq!(
        back.to.as_str(),
        "/member/assistants/scribe/talky",
        "the rehydrated row is the deep edge that was written"
    );

    let plain = edges
        .iter()
        .find(|e| e.id == ordinary)
        .expect("the ordinary row comes back too");
    assert_eq!(
        plain.lane, None,
        "an ordinary edge stays lane-less: absence is the statement, and a \
         rehydration that invented a lane here would pass a one-row test"
    );

    // `shutdown_async` rather than `shutdown`: the sync one blocks on the
    // writer's join and would block the very runtime driving this test.
    reopened.shutdown_async().await;
}
