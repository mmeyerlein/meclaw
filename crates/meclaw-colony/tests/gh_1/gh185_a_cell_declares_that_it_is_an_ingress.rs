//! GH #185 — "I am where messages enter" is something a cell SAYS, not
//! something the shape of the graph is read to imply.
//!
//! `context_key_reachable` used to treat any node without an incoming edge as
//! the graph entry, and to hand it every key in `INGRESS_CONTEXT_KEYS`. Two
//! things are wrong with that, and this file pins both ends of the fix:
//!
//! 1. **In-degree is not a property of a cell.** The ordinary shape of a
//!    connector — a proxy that accepts inbound traffic AND gets the answers
//!    routed back to it — has an incoming edge, so the inference took the
//!    entry branch away from precisely the cells that are entries. Once #178
//!    let the check see the running graph, that stopped being theoretical.
//! 2. **"Entry" was all-or-nothing.** A node that happened to have in-degree 0
//!    was granted the whole standard header set. A declaration names the keys,
//!    so the check can verify WHICH keys a cell may mint.
//!
//! The declaration is `contract.ingress.context`: the context keys this cell
//! puts on a message at birth. It is bounded by `INGRESS_CONTEXT_KEYS` — a
//! cell may narrow the standard set, never widen it.

use meclaw_colony::mutation::validate::{
    HeaderEdgeView, HeaderNodeView, validate_header_contract_locality,
};
use meclaw_colony::{
    BootState, BootstrapError, CellFactory, CellFactoryRegistry, bootstrap_from_filesystem,
    plan_bootstrap_with_env, probe_boot_state, read_registry_overlay,
};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

// ──────────────────────────────────────────────────────────────────────────────
// Pure level — the check answers locally
// ──────────────────────────────────────────────────────────────────────────────

fn keys(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

fn edge(from: &str, to: &str) -> HeaderEdgeView {
    HeaderEdgeView {
        from: from.into(),
        to: to.into(),
        ..Default::default()
    }
}

/// The reported shape, in the smallest form that carries it: `/proxy` is the
/// entry AND the sink, because that is what a chat connector is. It requires
/// `chat_id`, and nothing on the loop promotes it — the value is born when the
/// proxy mints the message.
fn proxy_loop(
    ingress_context: BTreeSet<String>,
) -> (BTreeMap<String, HeaderNodeView>, Vec<HeaderEdgeView>) {
    let nodes = BTreeMap::from([
        (
            "/proxy".to_string(),
            HeaderNodeView {
                required_context: keys(&["chat_id"]),
                ingress_context,
                ..Default::default()
            },
        ),
        ("/worker".to_string(), HeaderNodeView::default()),
    ]);
    let edges = vec![edge("/proxy", "/worker"), edge("/worker", "/proxy")];
    (nodes, edges)
}

/// The failure this replaces: an ingress that also receives replies has an
/// incoming edge, so the in-degree inference refused it.
#[test]
fn a_proxy_that_receives_its_own_replies_is_still_an_ingress_when_it_says_so() {
    let (nodes, edges) = proxy_loop(keys(&["chat_id"]));
    validate_header_contract_locality(&nodes, &edges, &BTreeSet::new())
        .expect("a cell that declares it mints chat_id at birth satisfies its own requirement");
}

/// The counter-pin: without the declaration the same topology is refused, and
/// the refusal names what to add.
#[test]
fn an_ingress_that_does_not_say_so_is_refused_and_the_message_names_the_declaration() {
    let (nodes, edges) = proxy_loop(BTreeSet::new());
    let err = validate_header_contract_locality(&nodes, &edges, &BTreeSet::new())
        .expect_err("nothing in this graph promotes chat_id, so nothing may satisfy it");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("contract.ingress.context") && msg.contains("chat_id"),
        "an author staring at this needs the key AND the field to write; got {msg}"
    );
}

/// In-degree stops being an answer in the other direction too: a node with no
/// incoming edge at all is no longer handed the standard header set.
#[test]
fn an_edge_less_node_is_not_an_ingress_by_virtue_of_having_no_edges() {
    let nodes = BTreeMap::from([(
        "/lonely".to_string(),
        HeaderNodeView {
            required_context: keys(&["turn_id"]),
            ..Default::default()
        },
    )]);
    validate_header_contract_locality(&nodes, &[], &BTreeSet::new())
        .expect_err("having no neighbours is not a claim about being an entry point");
}

/// The declaration is narrower than the old branch by construction: it grants
/// exactly the named keys, not all of `INGRESS_CONTEXT_KEYS`.
#[test]
fn a_declaration_grants_only_the_keys_it_names() {
    let nodes = BTreeMap::from([(
        "/proxy".to_string(),
        HeaderNodeView {
            required_context: keys(&["chat_id", "locale"]),
            ingress_context: keys(&["chat_id"]),
            ..Default::default()
        },
    )]);
    let err = validate_header_contract_locality(&nodes, &[], &BTreeSet::new())
        .expect_err("locale was not claimed, so it is not born here");
    assert!(
        format!("{err:?}").contains("locale"),
        "the unmet key must be the one named"
    );
}

/// And the claim itself is bounded: a cell may narrow the standard header set,
/// never invent a key that is born at ingress.
#[test]
fn a_cell_may_not_claim_a_key_that_is_not_born_at_ingress() {
    let nodes = BTreeMap::from([(
        "/proxy".to_string(),
        HeaderNodeView {
            ingress_context: keys(&["tenant_id"]),
            ..Default::default()
        },
    )]);
    let err = validate_header_contract_locality(&nodes, &[], &BTreeSet::new())
        .expect_err("tenant_id is not a standard header key, so no cell may mint it at birth");
    assert!(
        format!("{err:?}").contains("tenant_id"),
        "the refusal names the key that was claimed"
    );
}

/// A setter edge keeps working exactly as before — the declaration is an
/// ADDITIONAL root, not a replacement for promotion.
#[test]
fn an_edge_promotion_still_satisfies_the_requirement_without_any_declaration() {
    let nodes = BTreeMap::from([
        ("/a".to_string(), HeaderNodeView::default()),
        (
            "/b".to_string(),
            HeaderNodeView {
                required_context: keys(&["chat_id"]),
                ..Default::default()
            },
        ),
    ]);
    let edges = vec![HeaderEdgeView {
        from: "/a".into(),
        to: "/b".into(),
        set_context: keys(&["chat_id"]),
        ..Default::default()
    }];
    validate_header_contract_locality(&nodes, &edges, &BTreeSet::new())
        .expect("set_context is untouched by this change");
}

// ──────────────────────────────────────────────────────────────────────────────
// Boot level — FirstBoot and Reboot, on the same tree
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

/// The proxy-shaped tree: `/main/proxy` is entry and sink, `/main/worker`
/// answers. `ingress` is written verbatim so a test can leave it out.
fn proxy_tree(root: &std::path::Path, ingress: &str) {
    std::fs::create_dir_all(root.join("main/proxy")).unwrap();
    std::fs::create_dir_all(root.join("main/worker")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./proxy","to":"./worker"},
            {"from":"./worker","to":"./proxy"}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("main/proxy/config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"/worker"}},
                "contract":{{"version":"0.1.0","settings":{{}}{ingress},
                "consumes":{{"context":{{"chat_id":{{"type":"string","required":true}}}}}}}}}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("main/worker/config.json"),
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/proxy"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

const DECLARED: &str = r#","ingress":{"context":["chat_id"]}"#;

/// The whole point, end to end: a correctly declared ingress comes up on a
/// FirstBoot — where the file IS the topology and the strict branch applies —
/// and comes up again on the Reboot, with nothing to report. The #178 floor
/// (report-don't-refuse on a Reboot) stays where it is; this proves the
/// declared shape never needs it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_ingress_boots_on_a_first_boot_and_again_on_a_reboot() {
    let td = tempfile::TempDir::new().unwrap();
    proxy_tree(td.path(), DECLARED);

    let db_path = td.path().join("colony.db");
    assert_eq!(
        probe_boot_state(&db_path).unwrap(),
        BootState::FirstBoot,
        "boot 1 must be the strict case for this test to mean anything"
    );

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("a declared ingress passes the strict FirstBoot branch");
    h.shutdown().await;

    assert_eq!(
        probe_boot_state(&db_path).unwrap(),
        BootState::Reboot,
        "boot 2 must be the reported case"
    );
    let plan = plan_bootstrap_with_env(
        td.path(),
        &echo_registry(),
        &read_registry_overlay(&db_path).unwrap(),
        probe_boot_state(&db_path).unwrap(),
        None,
    )
    .expect("the reboot plans");
    assert!(
        plan.header_contract_findings.is_empty(),
        "a declared ingress leaves the reboot report empty — it does not merely survive it; \
         got {:?}",
        plan.header_contract_findings
    );

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("and the same tree comes up a second time");
    h.shutdown().await;
}

/// The breaking half, stated as a test: an ingress that does not declare
/// itself is refused on a FirstBoot, and the refusal is the migration note.
#[test]
fn an_undeclared_ingress_is_refused_on_a_first_boot() {
    let td = tempfile::TempDir::new().unwrap();
    proxy_tree(td.path(), "");
    let errs = plan_bootstrap_with_env(
        td.path(),
        &echo_registry(),
        &Default::default(),
        BootState::FirstBoot,
        None,
    )
    .expect_err("the file is the topology and it does not say where messages enter");
    assert!(
        errs.items().iter().any(|e| matches!(
            e,
            BootstrapError::HeaderContractViolation { reason }
                if reason.contains("contract.ingress.context") && reason.contains("chat_id")
        )),
        "the boot failure has to say what to add; got {errs:?}"
    );
}
