//! GH #285 — a declared slot is an address, so an edge onto it is not dangling.
//!
//! A hive that declares a slot says: *here is an address a parent may wire, and
//! it may stand EMPTY.* The boot endpoint-existence check (A8) knew only two
//! kinds of address — a node that is planned and a node that is persisted — so
//! a birth topology wiring its own empty slot failed the boot, or failed
//! `--validate-strict`, for saying exactly what the declaration invited.
//!
//! The exemption is bought by the DECLARATION and by nothing else. That is the
//! issue's second point: *a typo cannot inherit the exemption*. So the same
//! tree is asked three times, and only the first spelling is exempt:
//!
//! 1. `gen` declared as a slot → `/h/gen` resolves.
//! 2. the entry removed → `/h/gen` is reported again (the typo case).
//! 3. the entry kept as a PLAIN port → `/h/gen` is reported again: a port is an
//!    address that names a child, a slot is an address that may have none, and
//!    only the second one is a promise about emptiness.
//!
//! `./ghost`, never declared in any of the three, is reported in all of them —
//! the check keeps its teeth where nothing vouched for the endpoint.
//!
//! The question is asked of the substrate: the hive's own `config.json` is
//! planted in a throwaway colony root, the REAL `plan_bootstrap` walks it, and
//! the REAL `declared_slot_endpoints` reads the declaration back out of that
//! plan — a test that re-derived "what a slot address looks like" could agree
//! with itself while disagreeing with the colony.
//!
//! The second half of the file buys the SECOND exemption: the same declaration
//! read at MUTATION time. A parent may wire a slot before anything fills it —
//! that is the whole point of announcing an address that may stand empty — so
//! `add_edges` onto `<hive>/<declared-slot>` commits and the edge lands in the
//! table. The teeth stay where nothing vouched: an undeclared empty address in
//! the very same hive is still `edge_schema`, and a slot address is NOT a node,
//! so a `remove_nodes` naming it still hits nothing.

use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, RegistryOverlay,
    bootstrap_from_filesystem, declared_slot_endpoints, plan_bootstrap, unresolved_boot_endpoints,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::oneshot;

/// The hive under test, and the two endpoints its birth graph wires.
const HIVE: &str = "/h";
const SLOT: &str = "/h/gen";
const GHOST: &str = "/h/ghost";

/// A hive `config.json` with the given `params.ports` list, wiring itself to
/// `./gen` and `./ghost` — neither of which exists on disk.
fn hive_config(ports: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"hive"}},"params":{{"ports":{ports},"graph":{{"edges":[
             {{"from":".","to":"./gen"}},
             {{"from":".","to":"./ghost"}}
           ]}}}}}}"#
    )
}

/// Plant the hive with `ports`, plan the tree, and return the endpoints the
/// boot check finds unresolved — with the slot exemption in force.
fn unresolved_with_ports(ports: &str) -> Vec<String> {
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    // `main/` is the root scope `/`; the hive under test is one level below it,
    // because the root scope is the one hive the port reader deliberately never
    // seals (nothing encloses it) and therefore declares no slots either.
    std::fs::create_dir_all(root.join("main/h")).expect("hive dir");
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{}}"#,
    )
    .expect("root config");
    std::fs::write(root.join("main/h/config.json"), hive_config(ports)).expect("hive config");

    let factories = CellFactoryRegistry::new();
    let overlay = RegistryOverlay::new();
    let plan = plan_bootstrap(root, &factories, &overlay)
        .expect("a hive wiring an unbuilt address still plans");
    assert_eq!(plan.edges.len(), 2, "both edges must be in the plan");

    // The declared slot addresses, derived from THIS plan's hives — the same
    // second known-set the live boot and `--validate` pass.
    let slots = declared_slot_endpoints(root, &plan);
    unresolved_boot_endpoints(&plan, &HashSet::new(), &slots)
        .into_iter()
        .map(|(_, endpoint)| endpoint.as_str().to_string())
        .collect()
}

/// 1. The declared slot resolves; the undeclared endpoint does not.
#[test]
fn a_declared_slot_is_not_a_dangling_endpoint() {
    let unresolved = unresolved_with_ports(r#"["a", {"name":"gen","slot":true,"unbound":"park"}]"#);
    assert!(
        !unresolved.iter().any(|e| e == SLOT),
        "{SLOT} is a declared slot — it must not be reported, got {unresolved:?}"
    );
    assert_eq!(
        unresolved,
        vec![GHOST.to_string()],
        "{GHOST} was never declared and must stay reported, got {unresolved:?}"
    );
    assert!(
        !unresolved.iter().any(|e| e == HIVE),
        "the hive itself is a planned scope, got {unresolved:?}"
    );
}

/// 2. The typo case: remove the declaration and exactly that endpoint returns.
#[test]
fn removing_the_declaration_reports_the_endpoint_again() {
    let unresolved = unresolved_with_ports(r#"["a"]"#);
    assert_eq!(
        unresolved,
        vec![SLOT.to_string(), GHOST.to_string()],
        "without the slot declaration both endpoints dangle, got {unresolved:?}"
    );
}

/// 3. A plain port buys no exemption — only a slot promises an empty address.
#[test]
fn a_plain_port_declaration_buys_no_exemption() {
    let unresolved = unresolved_with_ports(r#"["a", "gen"]"#);
    assert_eq!(
        unresolved,
        vec![SLOT.to_string(), GHOST.to_string()],
        "a plain port names a child that must exist, got {unresolved:?}"
    );
}

// ---------------------------------------------------------------------------
// The mutation half: the same declaration, read while the colony is running.
// ---------------------------------------------------------------------------

/// The caller that does the wiring, outside the hive under test.
const CALLER: &str = "/caller";

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

/// A live colony: root scope with an `echo` caller and a hive `/h` that
/// declares `gen` as a SLOT and has no children on disk at all — the state a
/// slot exists to describe.
///
/// Booted through the real `bootstrap_from_filesystem`, so the hive is a
/// registered scope and the mutation path reads the declaration out of the very
/// `config.json` the boot walked.
async fn live_colony(ports: &str) -> (tempfile::TempDir, ColonyHandle) {
    boot_colony(ports, false).await
}

/// The same colony with the slot FILLED: an `echo` cell really sitting at
/// `/h/gen`. Needed by the one case where the address has to be occupied first —
/// a diff cannot remove a node that was never there.
async fn live_colony_with_filled_slot(ports: &str) -> (tempfile::TempDir, ColonyHandle) {
    boot_colony(ports, true).await
}

/// One `echo` cell config, with the `emitted_target` its own path.
fn echo_config(path: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"{path}"}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
    )
}

async fn boot_colony(ports: &str, fill_slot: bool) -> (tempfile::TempDir, ColonyHandle) {
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    std::fs::create_dir_all(root.join("main/caller")).expect("caller dir");
    std::fs::create_dir_all(root.join("main/h")).expect("hive dir");
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("root config");
    std::fs::write(root.join("main/caller/config.json"), echo_config(CALLER))
        .expect("caller config");
    std::fs::write(
        root.join("main/h/config.json"),
        format!(r#"{{"cell":{{"type":"hive"}},"params":{{"ports":{ports}}}}}"#),
    )
    .expect("hive config");
    if fill_slot {
        std::fs::create_dir_all(root.join("main/h/gen")).expect("slot dir");
        std::fs::write(root.join("main/h/gen/config.json"), echo_config(SLOT))
            .expect("slot config");
    }

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(root, &echo_registry(), &h.runtime())
        .await
        .expect("a hive whose slot stands empty boots");
    (td, h)
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
        .expect("colony inbox");
    ack_rx.await.expect("mutation ack")
}

async fn read_graph(h: &ColonyHandle) -> ReadGraphReply {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: meclaw_core::Path::new("/"),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox");
    ack_rx.await.expect("graph reply")
}

/// The declaration under test, as a hive would write it.
const SLOT_PORTS: &str = r#"[{"name":"gen","slot":true,"unbound":"park"}]"#;

/// A mutation may wire the slot BEFORE anything fills it — and the edge is
/// really there afterwards, read back off the colony's own graph.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mutation_may_wire_a_declared_slot() {
    let (_td, h) = live_colony(SLOT_PORTS).await;

    let outcome = send_mutation(
        &h,
        json!({"diff":{"add_edges":[{"from":"./caller","to":"./h/gen"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "{SLOT} is a declared slot — wiring it before it is filled must commit; got {outcome:?}"
    );

    let graph = read_graph(&h).await;
    assert!(
        graph.edges.iter().any(|e| e.from == CALLER && e.to == SLOT),
        "the committed edge must be in the table, got {:?}",
        graph
            .edges
            .iter()
            .map(|e| (e.from.clone(), e.to.clone()))
            .collect::<Vec<_>>()
    );

    h.shutdown().await;
}

/// The teeth: an undeclared empty address in the SAME hive is still unknown,
/// and the refusal names it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mutation_onto_an_undeclared_empty_address_still_rejects() {
    let (_td, h) = live_colony(SLOT_PORTS).await;

    let outcome = send_mutation(
        &h,
        json!({"diff":{"add_edges":[{"from":"./caller","to":"./h/ghost"}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(
                error_code, "edge_schema",
                "an address nothing declared is an unknown endpoint; details: {details}"
            );
            assert!(
                details.contains("./h/ghost"),
                "the refusal must name the unknown endpoint, got {details}"
            );
        }
        other => panic!("{GHOST} was never declared and must reject; got {other:?}"),
    }

    let graph = read_graph(&h).await;
    assert!(
        !graph.edges.iter().any(|e| e.to == GHOST),
        "a rejected edge must not be in the table"
    );

    h.shutdown().await;
}

/// A slot is an ADDRESS, not a node: it buys the edge check an endpoint and
/// nothing else. A `remove_nodes` naming it still hits nothing — otherwise the
/// exemption would quietly turn a promise about emptiness into a node the
/// colony believes it registered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slot_is_no_node_a_remove_nodes_can_reach() {
    let (_td, h) = live_colony(SLOT_PORTS).await;

    let outcome = send_mutation(
        &h,
        json!({"diff":{"remove_nodes":[{"match":{"name":"./h/gen"}}]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "match_no_hit",
                "a declared slot is an address, never a node to disconnect"
            );
        }
        other => panic!("{SLOT} is no node — removing it must reject; got {other:?}"),
    }

    h.shutdown().await;
}

/// …and no `swap_nodes[].match` target either. The plan's interface promise is
/// structural — slot addresses never enter the `nodes`/`deep` universe the match
/// checks read — and this pins the behaviour that structure produces: the
/// existing no-match verdict, not a new one invented for slots.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slot_is_no_node_a_swap_nodes_match_can_reach() {
    let (_td, h) = live_colony(SLOT_PORTS).await;

    let outcome = send_mutation(
        &h,
        json!({"diff":{"swap_nodes":[
            {"match":{"name":"./h/gen"},"with":{"name":"./caller"}}
        ]}}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "match_no_hit",
                "a declared slot is an address, never a node to replace"
            );
        }
        other => panic!("{SLOT} is no node — swapping it must reject; got {other:?}"),
    }

    h.shutdown().await;
}

/// The one place where the exemption reaches further than GH #194 does, said out
/// loud: a diff that DISCONNECTS the node filling the slot and wires the address
/// in the same breath commits.
///
/// For every other node on its way out that combination is `edge_schema` — the
/// diff's own removals are subtracted from the post-state view before the
/// endpoint check reads it, so a lane onto a node this diff disconnects is
/// refused. A slot is the exception because the declaration outlives whatever
/// stood behind it: after the removal `/h/gen` is exactly the empty declared
/// address the hive announced, and an edge onto THAT is the edge the declaration
/// invited. The subtraction is deliberately not extended to the slot set.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_the_node_in_a_slot_and_wiring_the_address_in_one_diff_commits() {
    let (_td, h) = live_colony_with_filled_slot(SLOT_PORTS).await;

    // Pre-state receipt: the slot really is filled, so what follows is a
    // removal and not a no-hit dressed up as one.
    let outcome = send_mutation(
        &h,
        json!({"diff":{"add_edges":[{"from":"./caller","to":"./h/gen"}]}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the filled slot is wireable to begin with; got {outcome:?}"
    );

    let outcome = send_mutation(
        &h,
        json!({"diff":{
            "remove_nodes":[{"match":{"name":"./h/gen"}}],
            "add_edges":[{"from":"./caller","to":"./h/gen"}]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the address survives the node that filled it — the declaration is what \
         resolves it, not the occupant; got {outcome:?}"
    );
    h.shutdown().await;

    // The control, and the whole weight of the claim: the SAME tree, the SAME
    // diff, with `gen` declared a plain PORT instead of a slot. Nothing else
    // differs — so GH #194's subtraction is still in force and it is the
    // declaration, not the shape of the diff, that buys the exemption.
    let (_td, h) = live_colony_with_filled_slot(r#"["gen"]"#).await;
    let outcome = send_mutation(
        &h,
        json!({"diff":{
            "remove_nodes":[{"match":{"name":"./h/gen"}}],
            "add_edges":[{"from":"./caller","to":"./h/gen"}]
        }}),
    )
    .await;
    match outcome {
        MutationOutcome::Rejected { error_code, .. } => {
            assert_eq!(
                error_code, "edge_schema",
                "a plain port names a node, and a node on its way out is not an \
                 endpoint (GH #194)"
            );
        }
        other => panic!("without the slot declaration this must still reject; got {other:?}"),
    }

    h.shutdown().await;
}
