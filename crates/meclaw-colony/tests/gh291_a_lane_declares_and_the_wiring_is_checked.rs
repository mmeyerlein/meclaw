//! GH #291 — a hive lane's `accepts[].context` is a REQUIREMENT, and the
//! wiring is checked against it.
//!
//! `LaneSpec.context` used to say "declared, not enforced", and the reason it
//! gave was that "a promotion three edges upstream is indistinguishable from a
//! missing one to anything that reads a single edge". That was true of a check
//! that reads a single edge. It stopped being true when GH #185 gave the
//! substrate a backwards reachability walk over `set_context`/`delete_context`
//! — the very walk the header-locality rule uses for `consumes.context`. So the
//! lane requirement is answered with the same walk, from the caller's side of
//! the hive path.
//!
//! What is judged, and what deliberately is not:
//!
//! - judged: an edge INTO the hive path that STATES a constant `hop.route`
//!   naming a declared lane (`HeaderEdgeView::states_route`, GH #291 Task 15);
//! - not judged: an edge whose route is computed (`hop.upstream_route`) — which
//!   lane it means is knowable only once a message exists, and a check that
//!   cannot say which lane an edge means must never reject it (the same
//!   conservatism the rest of `hive_contract` is built on);
//! - not judged: an edge whose `from` is a HIVE path with no inbound edge —
//!   nothing can be delivered through it, so its contract is dormant, exactly
//!   as a hive nobody addresses is dormant rather than broken. One inbound edge
//!   lifts it.

use meclaw_colony::mutation::rejection::{MutationRejection, Stage};
use meclaw_colony::mutation::validate::{
    HeaderEdgeView, HeaderNodeView, HiveLaneRequirement, collect_hive_lane_context,
    validate_hive_lane_context,
};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::sync::oneshot;

const BECAUSE: &str = "a recall without a question is a scan";

fn keys(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

fn hives(v: &[&str]) -> BTreeSet<String> {
    keys(v)
}

/// The lane the memory hive declares: send me `in_query`, and have
/// `recall_query` promoted by the time you do.
fn recall_lane() -> Vec<HiveLaneRequirement> {
    vec![HiveLaneRequirement {
        hive_path: "/m".into(),
        route: "in_query".into(),
        context: vec!["recall_query".into()],
        because: Some(BECAUSE.into()),
    }]
}

/// An edge that STATES the lane, with whatever context modifiers the case needs.
fn stating(from: &str, to: &str) -> HeaderEdgeView {
    HeaderEdgeView {
        from: from.into(),
        to: to.into(),
        states_route: Some("in_query".into()),
        ..Default::default()
    }
}

/// A plain edge — no route stated, no context touched.
fn plain(from: &str, to: &str) -> HeaderEdgeView {
    HeaderEdgeView {
        from: from.into(),
        to: to.into(),
        ..Default::default()
    }
}

fn no_nodes() -> BTreeMap<String, HeaderNodeView> {
    BTreeMap::new()
}

/// The refusal has to say what it protects — the rule the rest of
/// `hive_contract` follows. Key, lane, hive, and the hive's own sentence.
///
/// The sentence is measured on the RENDERED line, because that is the refusal:
/// it reaches an author through `MutationRejection::render`, which appends
/// `Violation::because` once. The prose message deliberately does not quote it
/// a second time (see below).
#[test]
fn a_lane_without_its_promoted_key_is_refused_and_the_refusal_quotes_the_contract() {
    let edges = vec![stating("/caller", "/m")];
    let err = validate_hive_lane_context(&recall_lane(), &edges, &no_nodes(), &hives(&["/m"]))
        .expect_err("nothing in this graph promotes recall_query");
    let msg = format!("{err:?}");
    for needle in ["recall_query", "in_query", "/m"] {
        assert!(
            msg.contains(needle),
            "the refusal must name {needle}; got {msg}"
        );
    }
    assert_eq!(err.error_code(), "hive_contract");

    let mut rejection = MutationRejection::new();
    collect_hive_lane_context(
        &recall_lane(),
        &edges,
        &no_nodes(),
        &hives(&["/m"]),
        &mut rejection,
    );
    assert!(
        rejection.render().contains(BECAUSE),
        "the rendered refusal must quote the hive's own sentence: {}",
        rejection.render()
    );
}

/// …and exactly ONCE. `Violation::because` carries the sentence for a
/// structured reader and `render` appends it for a human one; quoting it inside
/// the prose message as well duplicated it in every line. `memory-hive`'s
/// `in_query` sentence is ~1.4 kB, so the duplicate was most of the refusal an
/// author had to read past to find the missing key.
#[test]
fn the_rendered_refusal_quotes_the_contract_exactly_once() {
    let mut rejection = MutationRejection::new();
    collect_hive_lane_context(
        &recall_lane(),
        &[stating("/caller", "/m")],
        &no_nodes(),
        &hives(&["/m"]),
        &mut rejection,
    );
    let rendered = rejection.render();
    assert_eq!(
        rendered.matches(BECAUSE).count(),
        1,
        "the lane's sentence belongs in the line once: {rendered}"
    );
    let entry = &rejection.entries()[0];
    assert_eq!(
        entry.because.as_deref(),
        Some(BECAUSE),
        "and the structured field still carries it, unparsed"
    );
    assert!(
        !entry.message.contains(BECAUSE),
        "the prose message is the half that must not repeat it: {}",
        entry.message
    );
}

/// The promotion on the judged edge itself is the shortest way to be right.
#[test]
fn a_promotion_on_the_judged_edge_satisfies_the_lane() {
    let edges = vec![HeaderEdgeView {
        set_context: keys(&["recall_query"]),
        ..stating("/caller", "/m")
    }];
    validate_hive_lane_context(&recall_lane(), &edges, &no_nodes(), &hives(&["/m"]))
        .expect("the caller promotes the key on the way in");
}

/// The case the old doc comment called guesswork: the promotion happened two
/// edges upstream and the key is still on the message. GH #185's walk sees it.
#[test]
fn a_promotion_two_edges_upstream_satisfies_the_lane() {
    let edges = vec![
        HeaderEdgeView {
            set_context: keys(&["recall_query"]),
            ..plain("/root", "/mid")
        },
        plain("/mid", "/caller"),
        stating("/caller", "/m"),
    ];
    validate_hive_lane_context(&recall_lane(), &edges, &no_nodes(), &hives(&["/m"]))
        .expect("a key promoted upstream and never deleted is present at the hive path");
}

/// And the other end of the same walk: a `delete_context` on the way severs it.
#[test]
fn a_delete_on_the_way_breaks_the_promotion() {
    let edges = vec![
        HeaderEdgeView {
            set_context: keys(&["recall_query"]),
            ..plain("/root", "/mid")
        },
        HeaderEdgeView {
            delete_context: keys(&["recall_query"]),
            ..plain("/mid", "/caller")
        },
        stating("/caller", "/m"),
    ];
    let err = validate_hive_lane_context(&recall_lane(), &edges, &no_nodes(), &hives(&["/m"]))
        .expect_err("the key is deleted before it reaches the hive path");
    assert!(format!("{err:?}").contains("recall_query"), "{err:?}");
}

/// Unknown is not the same as wrong: an edge that states no constant lane is
/// not judged at all.
#[test]
fn an_edge_that_states_no_constant_lane_is_not_judged() {
    let edges = vec![plain("/caller", "/m")];
    validate_hive_lane_context(&recall_lane(), &edges, &no_nodes(), &hives(&["/m"]))
        .expect("which lane this edge means is knowable only once a message exists");
}

/// Dormancy: the caller side is a hive path nothing addresses, so no message
/// can be delivered through this edge. A freshly instantiated composite looks
/// exactly like this, and refusing it would make one impossible to install.
#[test]
fn a_caller_hive_nobody_addresses_is_dormant_not_broken() {
    let edges = vec![stating("/h", "/m")];
    validate_hive_lane_context(&recall_lane(), &edges, &no_nodes(), &hives(&["/h", "/m"]))
        .expect("nothing can reach /h, so nothing can travel this edge");
}

/// And the dormancy lifts the moment a delivery becomes possible.
#[test]
fn one_inbound_edge_lifts_the_dormancy() {
    let edges = vec![plain("/x", "/h"), stating("/h", "/m")];
    let err =
        validate_hive_lane_context(&recall_lane(), &edges, &no_nodes(), &hives(&["/h", "/m"]))
            .expect_err("a message can now arrive at /h and go on into /m without the key");
    assert!(format!("{err:?}").contains("recall_query"), "{err:?}");
}

/// The case the facade cannot see, and the reason the collecting core exists:
/// two callers, two violations, one refusal — each with its own address and the
/// hive's own sentence about the lane.
#[test]
fn two_callers_missing_the_key_produce_two_violations() {
    let edges = vec![stating("/c1", "/m"), stating("/c2", "/m")];
    let mut rejection = MutationRejection::new();
    collect_hive_lane_context(
        &recall_lane(),
        &edges,
        &no_nodes(),
        &hives(&["/m"]),
        &mut rejection,
    );
    let entries = rejection.entries();
    assert_eq!(entries.len(), 2, "both callers are named; got {entries:?}");
    for entry in entries {
        assert_eq!(entry.stage, Stage::ContractLocality);
        assert_eq!(entry.code, "hive_contract");
        assert_eq!(entry.because.as_deref(), Some(BECAUSE));
    }
    let addresses: Vec<&str> = entries
        .iter()
        .filter_map(|e| e.address.as_deref())
        .collect();
    assert_eq!(
        addresses,
        vec!["/c1 -> /m", "/c2 -> /m"],
        "each violation carries the edge it concerns"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Task 17 — the same rule, from the outside: a real colony, a real mutation,
// and a real boot.
//
// Everything above measures the pure check. What an author actually meets is
// the pipeline: the lane-context rule is the FOURTH check of stage 6 (contract
// locality), collected next to the port boundary, the inbound lanes and the
// header-contract locality, and emitted through the ONE reject the collecting
// pipeline sends. Pre-destructive, like every other stage-6 verdict.
// ──────────────────────────────────────────────────────────────────────────────

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

fn echo_cell(root: &std::path::Path, rel: &str, emitted_target: &str) {
    std::fs::create_dir_all(root.join(rel)).unwrap();
    std::fs::write(
        root.join(rel).join("config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"{emitted_target}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

fn hive_dir(root: &std::path::Path, rel: &str, params: &Value) {
    std::fs::create_dir_all(root.join(rel)).unwrap();
    std::fs::write(
        root.join(rel).join("config.json"),
        meclaw_core::serde_json::to_string(&json!({"cell": {"type": "hive"}, "params": params}))
            .unwrap(),
    )
    .unwrap();
}

/// A sealed hive that accepts ONE lane, requires `keys` on it, and has a door
/// behind the lane — the shape every migrated hive template has.
fn contracted_hive(keys: &[&str], because: &str) -> Value {
    json!({
        "ports": [],
        "contract": {
            "accepts": [{"route": "in_query", "context": keys, "because": because}],
            "emits": []
        },
        "graph": {"edges": [
            {"from": ".", "to": "./glue",
             "condition": "has(hop.route) && hop.route == 'in_query'"}
        ]}
    })
}

/// `/caller`, `/mem` (contracted) and `/mem/glue`, with the root hive carrying
/// the `graph` block the case needs (empty unless the boot case fills it).
fn write_topology(root: &std::path::Path, hive_params: &Value, root_edges: Value) {
    hive_dir(root, "main", &json!({"graph": {"edges": root_edges}}));
    echo_cell(root, "main/caller", "/caller");
    hive_dir(root, "main/mem", hive_params);
    echo_cell(root, "main/mem/glue", "/mem/glue");
}

/// The caller addresses the HIVE and states the lane. `promote` names the keys
/// it puts into context on the way in — the whole question this rule asks.
fn wire_in_query(promote: &[&str]) -> Value {
    let mut modifier = json!({"set_hop": {"route": "'in_query'"}});
    if !promote.is_empty() {
        let set: meclaw_core::serde_json::Map<String, Value> = promote
            .iter()
            .map(|k| ((*k).to_string(), json!(format!("'{k}-value'"))))
            .collect();
        modifier["set_context"] = Value::Object(set);
    }
    json!({"diff": {"add_edges": [
        {"from": "./caller", "to": "./mem", "modifier": modifier}
    ]}})
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

/// The graph as an operator would read it back — nodes and edges, sorted.
async fn fingerprint(h: &ColonyHandle) -> (Vec<String>, Vec<(String, String)>) {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: meclaw_core::Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let reply = ack_rx.await.unwrap();
    let mut nodes: Vec<String> = reply.nodes.iter().map(|n| n.path.clone()).collect();
    nodes.sort();
    let mut edges: Vec<(String, String)> = reply
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    edges.sort();
    (nodes, edges)
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("bootstrap");
    h
}

/// (a) The refusal, through the whole pipeline: `hive_contract`, naming key,
/// lane, hive and the hive's own sentence — and the graph is what it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_without_its_promoted_key_is_refused() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(
        td.path(),
        &contracted_hive(&["recall_query"], BECAUSE),
        json!([]),
    );
    let h = boot(&td).await;

    let before = fingerprint(&h).await;
    let outcome = send_mutation(&h, wire_in_query(&[])).await;
    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "hive_contract", "{outcome:?}");
            for needle in ["recall_query", "in_query", "/mem", BECAUSE] {
                assert!(
                    details.contains(needle),
                    "the refusal must name {needle}; got {details}"
                );
            }
        }
        other => panic!("a lane wired without its key must be refused, got {other:?}"),
    }
    assert_eq!(
        before,
        fingerprint(&h).await,
        "the reject is pre-destructive: the graph is what it was"
    );
    h.shutdown().await;
}

/// (b) The rule refuses the omission, not the wiring: promote the key and the
/// SAME edge commits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_wiring_with_the_promotion_commits() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(
        td.path(),
        &contracted_hive(&["recall_query"], BECAUSE),
        json!([]),
    );
    let h = boot(&td).await;

    match send_mutation(&h, wire_in_query(&["recall_query"])).await {
        MutationOutcome::Committed { .. } => {}
        other => panic!("the promoted lane must commit, got {other:?}"),
    }
    assert!(
        fingerprint(&h)
            .await
            .1
            .contains(&("/caller".to_string(), "/mem".to_string())),
        "the caller wired the hive, not a cell inside it"
    );
    h.shutdown().await;
}

/// The `in_query` lane of the SHIPPED `memory-hive`, read out of the template
/// tree: which keys it names and what it says about them. `None` in a tree that
/// does not carry the template (the public export ships a subset — GH #49).
fn shipped_in_query_lane() -> Option<(Vec<String>, String)> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/memory-hive/config.json");
    let raw = std::fs::read_to_string(p).ok()?;
    let v: Value = meclaw_core::serde_json::from_str(&raw).ok()?;
    let lane = v["params"]["contract"]["accepts"]
        .as_array()?
        .iter()
        .find(|l| l["route"] == "in_query")?;
    let keys = lane["context"]
        .as_array()?
        .iter()
        .filter_map(|k| k.as_str().map(str::to_string))
        .collect::<Vec<String>>();
    Some((keys, lane["because"].as_str()?.to_string()))
}

/// (c) The shipped DECLARATION is satisfiable. The requirement is not invented
/// here — it is the `in_query` lane of `templates/memory-hive` as it ships, keys
/// and sentence both. A caller that promotes what the shipped lane names
/// commits; drop ONE of those keys and the refusal names exactly that key.
///
/// This is the declaration half of #291's second acceptance bullet. The other
/// half — the shipped template actually INSTALLING through the mutation path,
/// with the substrate's real `code`/`store`/`timer`/`llm` factories — cannot be
/// asked from this crate, which has no cell factories at all; it lives in
/// `meclaw-cells`, in `gh291_the_shipped_memory_hive_instantiates`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_memory_hive_lane_is_satisfiable_and_the_missing_key_is_named() {
    let Some((keys, because)) = shipped_in_query_lane() else {
        return;
    };
    assert!(
        keys.len() > 1,
        "the shipped in_query lane names several keys; got {keys:?}"
    );
    let all: Vec<&str> = keys.iter().map(String::as_str).collect();
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path(), &contracted_hive(&all, &because), json!([]));
    let h = boot(&td).await;

    // One key short — and it is the one the refusal names.
    let dropped = all[0];
    let outcome = send_mutation(&h, wire_in_query(&all[1..])).await;
    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "hive_contract", "{outcome:?}");
            assert!(
                details.contains(dropped),
                "the refusal must name the one key that is missing ({dropped}); got {details}"
            );
        }
        other => panic!("the shipped lane must refuse a caller one key short, got {other:?}"),
    }

    match send_mutation(&h, wire_in_query(&all)).await {
        MutationOutcome::Committed { .. } => {}
        other => panic!("the shipped lane must commit when the caller promotes it, got {other:?}"),
    }
    h.shutdown().await;
}

/// The boot half, through the REAL boot projection: the same defect written
/// into the birth topology is REPORTED (and `--validate-strict` turns the
/// report into an error), never a refusal to start. The birth topology is the
/// author's sovereign design — the same split the port boundary and GH #178
/// already use.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_boot_reports_the_defect_and_the_colony_still_comes_up() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(
        td.path(),
        &contracted_hive(&["recall_query"], BECAUSE),
        json!([{"from": "./caller", "to": "./mem",
                "modifier": {"set_hop": {"route": "'in_query'"}}}]),
    );

    let db_path = td.path().join("colony.db");
    let plan = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &echo_registry(),
        &meclaw_colony::read_registry_overlay(&db_path).unwrap(),
        meclaw_colony::probe_boot_state(&db_path).unwrap(),
        None,
    )
    .expect("a lane-context defect must not refuse the boot");
    let finding = plan
        .header_contract_findings
        .iter()
        .find(|f| f.contains("recall_query") && f.contains("in_query") && f.contains("/mem"))
        .unwrap_or_else(|| {
            panic!(
                "the finding must name the key, the lane and the hive; got {:?}",
                plan.header_contract_findings
            )
        });
    // And it must carry the hive's own sentence, exactly once. A boot report is
    // the surface where saying what the rule protects matters MOST: nothing is
    // refused, so an operator who cannot see the reason has no cause to act.
    // The sentence lives in `Violation::because`, so the finding has to be
    // RENDERED — cloning the prose message alone silently drops it.
    assert_eq!(
        finding.matches(BECAUSE).count(),
        1,
        "the boot finding must quote the lane's sentence once: {finding}"
    );

    // And the tree really does come up, rather than only planning.
    let h = boot(&td).await;
    h.shutdown().await;
}
