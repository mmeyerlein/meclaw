//! GH #283 — a default edge is DECLARABLE at boot, and an unguarded one is a
//! hint rather than a refusal.
//!
//! Task 1 gave the router its default phase: [`meclaw_colony::Edge`] carries
//! `is_default`, and `apply_edges` evaluates the defaults only after every
//! ordinary edge has declined. Nothing could set that flag — the only writer
//! was a hard-coded `false` at the two `InitialApply` conversion sites.
//!
//! Pre-state this file recorded when it was written (2026-08-22): `EdgeSpec`
//! (`crates/meclaw-colony/src/config.rs`) carries `#[serde(deny_unknown_fields)]`,
//! so `{"from": ".", "to": "./catchall", "default": true}` did not plan a
//! default edge — it refused the whole boot with `unknown field \`default\``.
//! Declaring the lane in the file was, quite literally, not expressible.
//!
//! What this file pins:
//!   (a) the flag is declarable — `"default": true` reaches
//!       `PlannedEdge::is_default`, and an edge that does not declare it stays
//!       `false`;
//!   (b) a non-boolean `"default"` refuses the boot with
//!       `BootstrapError::EdgeDefaultNotBoolean`, naming scope, `from` and `to`
//!       (serde's own type error is the source; the variant is what an operator
//!       reads);
//!   (c) a GUARDED default — `"default": true` **plus** a `condition` — plans
//!       successfully. This is the legal and recommended shape, and it is
//!       asserted out loud because the group-based design this plan used to
//!       carry forbade exactly that combination;
//!   (d) an UNGUARDED default plans successfully too, and leaves one advisory
//!       in `BootstrapPlan::advisories` naming the edge. The advisory channel
//!       is the point: it is a hint, not a refusal (Q1 ruling 2026-08-21).

use meclaw_colony::bootstrap::BootstrapError;
use meclaw_colony::{BootState, BootstrapPlan, CellFactory, CellFactoryRegistry};
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use tempfile::TempDir;

fn echo_registry() -> CellFactoryRegistry {
    let mut reg = CellFactoryRegistry::new();
    reg.insert(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    reg
}

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Writes a tree whose root hive declares `edges_json` and which carries the
/// three cells `/a`, `/b` and `/catchall`.
fn write_tree(td: &TempDir, edges_json: &str) {
    write(
        td.path(),
        "main/config.json",
        &format!(r#"{{"cell":{{"type":"hive"}},"params":{{"graph":{{"edges":{edges_json}}}}}}}"#),
    );
    for name in ["a", "b", "catchall"] {
        write(
            td.path(),
            &format!("main/{name}/config.json"),
            r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/dev/null"},
                "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
        );
    }
}

fn plan_tree(td: &TempDir) -> Result<BootstrapPlan, meclaw_colony::BootstrapErrors> {
    meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &echo_registry(),
        &Default::default(),
        BootState::FirstBoot,
        None,
    )
}

// ── (a) the flag is declarable ───────────────────────────────────────────────

/// One conditioned edge and one declared default, in the same hive. The
/// declaration has to survive the walk: `is_default` is true for the one that
/// says so and false for the one that does not.
#[test]
fn a_declared_default_reaches_the_planned_edge() {
    let td = TempDir::new().unwrap();
    write_tree(
        &td,
        r#"[{"from":"./a","to":"./b","condition":"hop.kind == 'work'"},
            {"from":"./a","to":"./catchall","default":true}]"#,
    );

    let plan = plan_tree(&td).expect("a declared default must plan");
    assert_eq!(plan.edges.len(), 2, "both declared edges must be planned");

    let conditioned = plan
        .edges
        .iter()
        .find(|e| e.to.as_str() == "/b")
        .expect("the conditioned edge must be planned");
    let default_edge = plan
        .edges
        .iter()
        .find(|e| e.to.as_str() == "/catchall")
        .expect("the default edge must be planned");

    assert!(
        !conditioned.is_default,
        "an edge that does not declare `default` is not one"
    );
    assert!(
        default_edge.is_default,
        "`\"default\": true` must reach PlannedEdge::is_default"
    );
}

// ── (b) a non-boolean `default` refuses the boot ─────────────────────────────

/// `"default": "yes"` is not a default — it is a typo with an opinion. The
/// refusal names the scope it stands in and the edge it describes, because a
/// message that locates nothing costs a grep over the whole tree.
#[test]
fn a_non_boolean_default_refuses_the_boot_and_names_the_edge() {
    let td = TempDir::new().unwrap();
    write_tree(&td, r#"[{"from":"./a","to":"./catchall","default":"yes"}]"#);

    let err = plan_tree(&td).expect_err("a non-boolean `default` must refuse the boot");
    assert!(
        err.items().iter().any(|e| matches!(
            e,
            BootstrapError::EdgeDefaultNotBoolean { scope, from, to }
                if scope.as_str() == "/" && from == "./a" && to == "./catchall"
        )),
        "expected EdgeDefaultNotBoolean naming scope/from/to, got: {:?}",
        err.items()
    );
}

// ── (c) a guarded default is legal, and recommended ──────────────────────────

/// A default edge MAY carry a condition, and this is the shape the docs
/// recommend: the default phase decides WHEN the edge is consulted (only after
/// every ordinary edge declined), the condition decides WHICH of that traffic
/// it takes. The earlier group-based design refused the combination outright;
/// this test is why that cannot come back silently.
#[test]
fn a_guarded_default_plans_successfully_and_leaves_no_advisory() {
    let td = TempDir::new().unwrap();
    write_tree(
        &td,
        r#"[{"from":"./a","to":"./catchall","condition":"hop.kind == 'work'","default":true}]"#,
    );

    let plan = plan_tree(&td).expect("a default WITH a condition must plan successfully");
    let edge = plan
        .edges
        .iter()
        .find(|e| e.to.as_str() == "/catchall")
        .expect("the guarded default must be planned");
    assert!(edge.is_default, "the guard must not clear the flag");
    assert!(
        edge.condition.is_some(),
        "the condition must be compiled onto the same edge"
    );
    assert!(
        plan.advisories.is_empty(),
        "a GUARDED default is the recommended shape and needs no hint, got: {:?}",
        plan.advisories
    );
}

// ── (d) an unguarded default is a hint, not a refusal ────────────────────────

/// An unguarded default is legal — it plans, it boots — but it swallows
/// everything that would otherwise dead-letter as `no_route`, which is a large
/// thing to do by accident. The plan therefore carries one advisory per
/// unguarded default, on the one finding channel `--validate-strict` does not
/// promote to an error.
#[test]
fn an_unguarded_default_plans_and_leaves_one_advisory() {
    let td = TempDir::new().unwrap();
    write_tree(&td, r#"[{"from":"./a","to":"./catchall","default":true}]"#);

    let plan = plan_tree(&td).expect("an unguarded default must PLAN, not refuse");
    assert_eq!(
        plan.advisories.len(),
        1,
        "exactly one advisory per unguarded default, got: {:?}",
        plan.advisories
    );
    let advisory = &plan.advisories[0];
    assert!(
        advisory.contains("/a") && advisory.contains("/catchall"),
        "the advisory must name the edge it is about, got: {advisory}"
    );
    assert!(
        advisory.contains("no_route") && advisory.contains("condition"),
        "the advisory must say what it costs and how to narrow it, got: {advisory}"
    );
}
