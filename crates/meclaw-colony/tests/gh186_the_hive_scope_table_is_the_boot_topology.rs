//! GH #186 — the hive half of the authority question #168/#178 settled for edges.
//!
//! A hive is a routing element, not an inert node: an edge whose `from` is a
//! hive is a transit pass-through, and the fan-in walk has to recurse into the
//! hive's own inbound edges to learn what that edge delivers. Which paths ARE
//! hives was still answered by the `config.json` filesystem walk, while the
//! edges around them had already become the persisted edge table.
//!
//! So a reboot could plan an edge out of `/h` while no longer knowing `/h` is a
//! hive — the directory is gone, the row in `hive_scopes` is not (scope rows
//! are append-only, same No-Delete-Policy the registry follows). The walk then
//! read the hive as a cell, a cell has no contract here, a node with no
//! contract emits nothing, and the fan-in intersection came out empty. The
//! answer was quietly wrong — a finding against a topology that delivers the
//! key perfectly well — rather than loudly absent.
//!
//! The cut is the one #168 made for edges, in the same shape so a later reader
//! sees one rule and not two: **on a Reboot the persisted tables are the
//! topology, on a FirstBoot the `config.json` files are.** `hive_scopes` was
//! already persisted and already read on this path; it just was not believed.

use meclaw_colony::{
    BootState, CellFactory, CellFactoryRegistry, bootstrap_from_filesystem, probe_boot_state,
    read_registry_overlay,
};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;

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

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The transit shape the fan-in walk exists for: `/entry` emits the hop key,
/// the hive `/h` carries it across unchanged, `/c` requires it. The obligation
/// is satisfiable only if the walk knows `/h` is a hive — read as a cell, `/h`
/// has no `emits.hop` and the intersection over `/c`'s incoming edges is empty.
fn write_transit_topology(root: &std::path::Path) {
    write(
        root,
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./entry","to":"./h"},
            {"from":"./h","to":"./c"}
        ]}}}"#,
    );
    write(
        root,
        "main/entry/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/h"},"contract":{"version":"0.1.0",
            "settings":{},"emits":{"hop":{"topic":{"type":"string"}}},"consumes":{}}}"#,
    );
    write(root, "main/h/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(
        root,
        "main/c/config.json",
        r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/c"},"contract":{"version":"0.1.0",
            "settings":{},"consumes":{"hop":{"topic":{"type":"string","required":true}}}}}"#,
    );
}

/// Boot the tree once so the InitialApply bundle commits, then shut down —
/// leaving a `colony.db` whose `edges` and `hive_scopes` tables both describe
/// the transit.
async fn boot_once(td: &tempfile::TempDir) {
    write_transit_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the first boot of an honest transit topology must succeed");
    h.shutdown().await;
}

fn hive_scope_rows(db_path: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT path FROM hive_scopes ORDER BY path")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// The defect, at the seam where the decision is made. The hive's directory is
/// wiped — the operator path, since there is no delete op — while the edges
/// through it stay in the table. The reboot must still read `/h` as a transit,
/// because `hive_scopes` still says it is one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reboot_reads_a_hive_whose_directory_is_gone_as_a_transit() {
    let td = tempfile::TempDir::new().unwrap();
    boot_once(&td).await;

    let db_path = td.path().join("colony.db");
    assert!(
        hive_scope_rows(&db_path).iter().any(|p| p == "/h"),
        "the persisted scope row is the whole basis of this fix; got {:?}",
        hive_scope_rows(&db_path)
    );

    // The wipe: the hive directory goes away, its scope row and its edges stay.
    std::fs::remove_dir_all(td.path().join("main/h")).unwrap();

    assert_eq!(
        probe_boot_state(&db_path).unwrap(),
        BootState::Reboot,
        "boot 2 must classify as a Reboot for this test to mean anything"
    );

    let plan = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &echo_registry(),
        &read_registry_overlay(&db_path).unwrap(),
        probe_boot_state(&db_path).unwrap(),
        None,
    )
    .expect("planning a reboot whose hive directory was wiped must succeed");

    assert_eq!(
        plan.edges.len(),
        2,
        "the transit edges are still the persisted topology; got {:?}",
        plan.edges
            .iter()
            .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
            .collect::<Vec<_>>()
    );
    assert!(
        plan.header_contract_findings.is_empty(),
        "the transit delivers `topic` exactly as it did before the wipe — reading /h as a \
         contract-less cell invents a violation that is not there; got {:?}",
        plan.header_contract_findings
    );

    // And the tree really comes up, not merely plans.
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the colony boots on its persisted topology");
    h.shutdown().await;
}

/// The counter-pin. On a **FirstBoot** the files ARE the topology: there is no
/// `hive_scopes` table to prefer, and the walk's hives must keep counting. If
/// the persisted set were consulted on both boot kinds, this fresh tree — whose
/// required hop key is delivered only across the transit — would fail to plan.
#[test]
fn a_first_boot_still_reads_its_hives_from_the_filesystem_walk() {
    let td = tempfile::TempDir::new().unwrap();
    write_transit_topology(td.path());

    let plan = meclaw_colony::plan_bootstrap_with_env(
        td.path(),
        &echo_registry(),
        &Default::default(),
        BootState::FirstBoot,
        None,
    )
    .expect("a fresh transit topology must plan — the walk is the source here");

    assert_eq!(plan.hives.len(), 2, "root and /h");
    assert_eq!(plan.edges.len(), 2, "the two declared transit lanes");
    assert!(
        plan.header_contract_findings.is_empty(),
        "a FirstBoot refuses rather than reports, so this list stays empty by construction"
    );
}
