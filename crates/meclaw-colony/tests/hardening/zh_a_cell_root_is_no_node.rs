//! GH #1234 — a root a cell declares relative to its hive lies inside the
//! colony root, where the boot walker reads. What a cell writes there is its
//! data, not topology: a `config.json` in it (a project the coder pipeline
//! builds, a file an assistant saves on request) must neither become a node nor
//! fail the next boot.
//!
//! The walker used to descend into every directory and parse every
//! `config.json` it found. One that is no meclaw config is
//! `BootstrapError::InvalidJson`, and one such error fails the whole boot plan
//! (re-check H4 part A, I1). The spawn that creates a relative root now leaves
//! a marker in it, and the walker steps into no directory that carries one.

use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_testing::ColonyHandle;
use meclaw_testing::deadline::hang;
use std::sync::Arc;

fn echo() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(meclaw_testing::factories::EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in echo() {
        r.insert(name, f);
    }
    r
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_foreign_config_json_in_a_relative_root_is_no_node_and_no_boot_error() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    write(
        root,
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        root,
        "main/tool/config.json",
        r#"{"cell":{"type":"echo"},
            "params":{"emitted_target":"/sink","transfer":{"base_path":"exchange"}},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let h = ColonyHandle::new_with_factories_at(&td, echo());
    hang(
        "the first boot",
        bootstrap_from_filesystem(root, &registry(), &h.runtime()),
    )
    .await
    .expect("the first boot must come up");
    hang("the colony shuts down", h.shutdown()).await;

    let exchange = root.join("main/exchange");
    assert!(
        exchange.is_dir(),
        "the spawn must create the relative root in the enclosing hive"
    );
    // What a cell writes into its own root: a config.json that is no meclaw
    // config, at the root and one directory further down.
    write(
        &exchange,
        "config.json",
        r#"{"name":"a-project","version":"1.0.0"}"#,
    );
    write(&exchange, "app/config.json", r#"{"name":"an-app"}"#);

    let plan = meclaw_colony::plan_bootstrap(root, &registry(), &Default::default())
        .expect("a config.json a cell wrote into its own root must not stop the next boot");
    let planned: Vec<&str> = plan.cells.iter().map(|c| c.path.as_str()).collect();
    assert!(
        planned.contains(&"/tool"),
        "the cell that owns the root must still boot: {planned:?}"
    );
    assert!(
        !planned.iter().any(|p| p.starts_with("/exchange")),
        "nothing below a cell's own root may become a node: {planned:?}"
    );
}
