//! Phase-16 W1a A8 (Ruling 2026-06-12): `--validate` endpoint-existence
//! semantics. A static run has no running colony, so it cannot see
//! runtime-spawned cells — a dangling `params.graph` endpoint is a WARNING
//! (exit 0), and the new `--validate-strict` flag promotes it to a hard error
//! (exit != 0). The operator decides, nginx -t style.

use meclaw_cli::{Cli, run};

/// A root hive wiring `. → /sink` where `/sink` has no FS directory.
fn write_dangling_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[{"from":".","to":"/sink"}]}}}"#,
    )
    .unwrap();
}

fn cli_for(root: &std::path::Path, strict: bool) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: None,
        daemon: false,
        validate: true,
        validate_strict: strict,
        apply: None,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        sandbox_probe: false,
        vault: None,
        vault_add: None,
        vault_status: false,
        vault_revoke: None,
        vault_key_source: "auto".to_string(),
        vault_key_file: None,
        stdio_format: meclaw_cli::StdioFormat::Text,
    }
}

/// Plain `--validate`: a dangling endpoint only WARNS → exit 0 (Ok).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_warns_on_dangling_endpoint_exit_zero() {
    let td = tempfile::TempDir::new().unwrap();
    write_dangling_topology(td.path());
    run(cli_for(td.path(), false))
        .await
        .expect("--validate must warn (not fail) on a dangling endpoint");
}

/// `--validate --validate-strict`: the same dangling endpoint becomes a hard error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_strict_fails_on_dangling_endpoint() {
    let td = tempfile::TempDir::new().unwrap();
    write_dangling_topology(td.path());
    let res = run(cli_for(td.path(), true)).await;
    assert!(
        res.is_err(),
        "--validate --validate-strict must FAIL on a dangling endpoint"
    );
}

/// A root hive whose single edge is an unguarded default into `/colony/graph`
/// — an endpoint that resolves, so the tree's ONLY defect is the unguarded
/// default itself.
fn write_unguarded_default_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
              {"from":".","to":"/colony/graph","default":true}]}}}"#,
    )
    .unwrap();
}

/// GH #283 (ruling Q1 2026-08-21): an unguarded default is a HINT, and
/// `--validate-strict` must NOT promote it.
///
/// The other three finding channels (`unresolved_boot_endpoints`,
/// `unregistered_nodes`, `header_contract_findings`) are all promoted by this
/// flag — which is precisely why the advisory got a channel of its own. This
/// test is the executable form of that ruling: exit 0 under the strictest flag
/// the CLI has. If a later change moves the advisory onto a promoting channel,
/// this is what stops it.
/// The exit-0 half alone would pass on a tree that produces NO advisory at all
/// (a dropped `default` key reads exactly like a clean topology from out here),
/// so the same tree is planned directly first: there IS a finding, and the
/// strict run still exits 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unguarded_default_is_a_note_not_a_strict_error() {
    let td = tempfile::TempDir::new().unwrap();
    write_unguarded_default_topology(td.path());

    let plan = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &meclaw_cli::built_in_factories(),
        &Default::default(),
        meclaw_colony::BootState::FirstBoot,
        None,
    )
    .expect("an unguarded default must plan");
    assert_eq!(
        plan.advisories.len(),
        1,
        "the tree under test must actually raise the advisory, else the exit-0 assertion \
         below is vacuous; got: {:?}",
        plan.advisories
    );

    run(cli_for(td.path(), true)).await.expect(
        "an unguarded default is a note: --validate-strict must NOT promote it to an error",
    );
}

/// GH #285 — a hive at `/h` declaring `params.ports` as given, wiring itself to
/// `./gen`, which has no directory. `main/` is the root scope `/`.
fn write_slot_topology(td: &std::path::Path, ports: &str) {
    std::fs::create_dir_all(td.join("main/h")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{}}"#,
    )
    .unwrap();
    std::fs::write(
        td.join("main/h/config.json"),
        format!(
            r#"{{"cell":{{"type":"hive"}},"params":{{"ports":{ports},
                 "graph":{{"edges":[{{"from":".","to":"./gen"}}]}}}}}}"#
        ),
    )
    .unwrap();
}

/// The tree really wires `/h/gen` and really has no node there — otherwise
/// every exit-0 assertion below would be about a topology with no question in
/// it. Asked of the plan the CLI itself builds.
fn assert_edge_to_an_unbuilt_gen(td: &std::path::Path) {
    let plan = meclaw_colony::plan_bootstrap_with_env(
        td,
        &meclaw_cli::built_in_factories(),
        &Default::default(),
        meclaw_colony::BootState::FirstBoot,
        None,
    )
    .expect("a hive wiring an unbuilt address must still plan");
    assert!(
        plan.edges.iter().any(|e| e.to.as_str() == "/h/gen"),
        "the edge under test must be in the plan, got {:?}",
        plan.edges.iter().map(|e| e.to.as_str()).collect::<Vec<_>>()
    );
    assert!(
        !plan.cells.iter().any(|c| c.path.as_str() == "/h/gen")
            && !plan.hives.iter().any(|h| h.path.as_str() == "/h/gen"),
        "/h/gen must be an address with NO node behind it, else the exemption is not \
         what resolves it"
    );
}

/// GH #285, the exemption END-TO-END: a DECLARED slot resolves through the real
/// CLI wiring — plan → `declared_slot_endpoints` → `unresolved_boot_endpoints`
/// → strict promotion — and the strictest flag the CLI has exits 0.
///
/// The unit tests next door call the check directly with a slot set they derive
/// themselves, so they stay green even if a call site stops deriving one. This
/// is the test that goes red then: it never names the helper, it only plants a
/// tree and asks the CLI.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_slot_survives_validate_strict() {
    let td = tempfile::TempDir::new().unwrap();
    write_slot_topology(
        td.path(),
        r#"[{"name":"gen","slot":true,"unbound":"park"}]"#,
    );
    assert_edge_to_an_unbuilt_gen(td.path());

    run(cli_for(td.path(), true)).await.expect(
        "a declared slot is an address that may stand empty: --validate-strict must NOT \
         report the edge onto it",
    );
}

/// GH #285, the other half of the same pair: remove the declaration and the
/// SAME tree fails `--validate-strict` again — the typo case, "a typo cannot
/// inherit the exemption".
///
/// The plain `--validate` run beside it is what makes the failure specific: a
/// broken tree would fail BOTH runs, so warn-then-promote is the signature of
/// this channel and of no other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_the_declaration_the_same_edge_fails_validate_strict() {
    let td = tempfile::TempDir::new().unwrap();
    write_slot_topology(td.path(), "[]");
    assert_edge_to_an_unbuilt_gen(td.path());

    run(cli_for(td.path(), false))
        .await
        .expect("undeclared: plain --validate only WARNS about the dangling endpoint");
    assert!(
        run(cli_for(td.path(), true)).await.is_err(),
        "undeclared: --validate-strict must promote the dangling endpoint to an error"
    );
}
