//! GH #178 — the boot-time header-contract check must run on the topology the
//! colony will actually run with.
//!
//! `plan_bootstrap_with_env` built its `header_edges` from the `params.graph`
//! blocks of the `config.json` files on disk. Edges created by mutations live
//! in the persisted edge table and never entered that set, so the check ran on
//! a *partial* graph — and partial is worse than empty. A hive whose doors were
//! written down (`{"from": "."}`, per the boundary rule) gave its interior cell
//! an incoming edge in the file view, which took away the lenient
//! "no incoming edge ⇒ ingress-at-birth" branch, while the `set_context` setter
//! that really promotes the key sat on a mutation edge the check could not see.
//! Nothing about the message flow had changed. The colony simply stopped
//! booting, after the writes were committed, with no way back but a rollback.
//!
//! Two rules come out of that, and this file pins both:
//!
//! 1. **One authority.** On a Reboot the persisted edge table is the running
//!    topology — for the edge load and for the header check alike. A setter on
//!    a mutation edge is visible; a door somebody wrote into a `config.json`
//!    of an already-running colony is not part of the graph (the runtime has
//!    always ignored those hints on a reboot) and cannot invent an obligation.
//! 2. **A committed colony is reported, never refused.** Seeing the real graph
//!    means seeing real defects that were invisible before. A violation found
//!    at reboot names the node, the key and the rule and lets the colony come
//!    up; `--validate --validate-strict` is where it is an error. Refusing the
//!    boot of a topology that is already on disk gives the operator a crash
//!    loop and no way to act on it. A FirstBoot — where the file IS the
//!    topology and somebody is authoring it right now — still fails loud.

use meclaw_colony::{
    BootState, BootstrapError, CellFactory, CellFactoryRegistry, MutationOutcome,
    bootstrap_from_filesystem, probe_boot_state, read_registry_overlay,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tokio::sync::oneshot;

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

/// An echo cell whose `contract.consumes` block is written verbatim, so a test
/// can move a required key in or out between two boots (config.json is
/// live-read at every boot).
fn cell(root: &std::path::Path, rel: &str, emitted_target: &str, consumes: &str) {
    std::fs::create_dir_all(root.join(rel)).unwrap();
    std::fs::write(
        root.join(rel).join("config.json"),
        format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"{emitted_target}"}},
                "contract":{{"version":"0.1.0","settings":{{}},"consumes":{consumes}}}}}"#
        ),
    )
    .unwrap();
}

fn hive(root: &std::path::Path, rel: &str, params: &str) {
    std::fs::create_dir_all(root.join(rel)).unwrap();
    std::fs::write(
        root.join(rel).join("config.json"),
        format!(r#"{{"cell":{{"type":"hive"}},"params":{params}}}"#),
    )
    .unwrap();
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Mutation {
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

/// Boot the tree once, apply the mutations, shut down — leaving a `colony.db`
/// whose edge table carries edges no `config.json` in the tree declares.
async fn boot_once_with(td: &tempfile::TempDir, payloads: Vec<Value>) {
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the first boot must succeed");
    for payload in payloads {
        let outcome = send_mutation(&h, payload).await;
        assert!(
            matches!(outcome, MutationOutcome::Committed { .. }),
            "the setup mutation must commit for this test to mean anything, got {outcome:?}"
        );
    }
    h.shutdown().await;
}

// ──────────────────────────────────────────────────────────────────────────────
// 1. One authority
// ──────────────────────────────────────────────────────────────────────────────

/// The reported shape: a hive `/h` whose interior `/h/b` requires an ingress
/// context key, and whose only real promoter is a mutation edge inside the
/// hive. Sealing the hive — writing its two doors into its own `config.json` —
/// must not be able to stop the colony from booting.
///
/// The doors are a closed pair (`. -> ./b` and `./b -> .`), which is what takes
/// the ingress-at-birth fallback away in the file-only view: walking back from
/// `/h/b` finds no node without an incoming edge, so the lenient branch is
/// gone and the strict one has nothing to find. In the running graph the
/// setter is right there on `/h/a -> /h/b`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sealing_a_hive_does_not_brick_a_colony_whose_setter_is_a_mutation_edge() {
    let td = tempfile::TempDir::new().unwrap();
    hive(td.path(), "main", r#"{"graph":{"edges":[]}}"#);
    cell(td.path(), "main/entry", "/h", "{}");
    hive(td.path(), "main/h", r#"{"graph":{"edges":[]}}"#);
    cell(td.path(), "main/h/a", "/h/b", "{}");
    cell(
        td.path(),
        "main/h/b",
        "/h/b",
        r#"{"context":{"chat_id":{"type":"string","required":true}}}"#,
    );

    // The colony is wired by mutation, the way a grown one is: a lane into the
    // hive, and the promotion inside it. Nothing of this is in any file.
    boot_once_with(
        &td,
        vec![
            json!({"scope": "/", "diff": {"add_edges": [{"from": "./entry", "to": "./h"}]}}),
            json!({"scope": "/h", "diff": {"add_edges": [
                {"from": "./a", "to": "./b", "modifier": {"set_context": {"chat_id": "'c1'"}}}
            ]}}),
        ],
    )
    .await;

    // The boundary-rule migration: the hive writes down its own doors.
    hive(
        td.path(),
        "main/h",
        r#"{"graph":{"edges":[{"from":".","to":"./b"},{"from":"./b","to":"."}]}}"#,
    );

    assert_eq!(
        probe_boot_state(&td.path().join("colony.db")).unwrap(),
        BootState::Reboot,
        "boot 2 must classify as a Reboot for this test to mean anything"
    );

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let report = bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime()).await;
    let report = report.expect(
        "declaring a hive's doors must not turn a running colony into one that will not \
         boot (before the fix: HeaderContractViolation on chat_id)",
    );
    h.shutdown().await;

    assert_eq!(
        report.edge_count, 2,
        "the reboot runs the two persisted edges — the freshly written doors are hints a \
         reboot ignores, and therefore impose nothing"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// 2. A committed colony is reported, never refused
// ──────────────────────────────────────────────────────────────────────────────

/// A tree whose persisted topology genuinely cannot deliver a required key.
/// `/b` asks for a non-ingress `tenant_id`, its only in-edge sets nothing, and
/// there is no lenient branch to fall through to.
fn tree_with_a_real_violation(td: &tempfile::TempDir) {
    hive(td.path(), "main", r#"{"graph":{"edges":[]}}"#);
    cell(td.path(), "main/a", "/b", "{}");
    cell(td.path(), "main/b", "/b", "{}");
}

/// Seeing the real graph finds real defects — and a defect in a topology that
/// is already committed must be **named**, not answered with a refusal to
/// start. The operator gets the node, the key and the rule; the colony comes
/// up; `--validate --validate-strict` is where the same finding is an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_violation_in_the_persisted_topology_is_reported_and_the_colony_still_boots() {
    let td = tempfile::TempDir::new().unwrap();
    tree_with_a_real_violation(&td);
    boot_once_with(
        &td,
        vec![json!({"scope": "/", "diff": {"add_edges": [{"from": "./a", "to": "./b"}]}})],
    )
    .await;

    // `/b` starts asking for a key nothing on its in-edge promotes. config.json
    // is live-read, so this takes effect at the next boot — exactly how a
    // contract tightens under a colony that has been running for days.
    cell(
        td.path(),
        "main/b",
        "/b",
        r#"{"context":{"tenant_id":{"type":"string","required":true}}}"#,
    );

    let db_path = td.path().join("colony.db");
    let plan = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &echo_registry(),
        &read_registry_overlay(&db_path).unwrap(),
        probe_boot_state(&db_path).unwrap(),
        None,
    )
    .expect("a reboot must not be refused over a contract its own edge table cannot satisfy");

    assert!(
        plan.header_contract_findings
            .iter()
            .any(|f| f.contains("/b") && f.contains("tenant_id")),
        "the finding must name the node and the key an operator has to act on; got {:?}",
        plan.header_contract_findings
    );

    // And the same tree really does come up, rather than only planning.
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the colony boots and reports, it does not crash-loop");
    h.shutdown().await;
}

/// The counter-pin. On a **FirstBoot** the file is the topology and somebody is
/// writing it right now — the same violation is still a hard, loud boot
/// failure. The leniency above is about committed state, not about the rule.
#[test]
fn a_first_boot_still_refuses_a_topology_that_cannot_deliver_a_required_key() {
    let td = tempfile::TempDir::new().unwrap();
    hive(
        td.path(),
        "main",
        r#"{"graph":{"edges":[{"from":"./a","to":"./b"}]}}"#,
    );
    cell(td.path(), "main/a", "/b", "{}");
    cell(
        td.path(),
        "main/b",
        "/b",
        r#"{"context":{"tenant_id":{"type":"string","required":true}}}"#,
    );

    let errs = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &echo_registry(),
        &Default::default(),
        BootState::FirstBoot,
        None,
    )
    .expect_err("a fresh tree that cannot deliver a required key must not plan");

    assert!(
        errs.items().iter().any(|e| matches!(
            e,
            BootstrapError::HeaderContractViolation { reason }
                if reason.contains("/b") && reason.contains("tenant_id")
        )),
        "expected a HeaderContractViolation naming /b + tenant_id, got {errs:?}"
    );
}
