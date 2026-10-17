//! GH #1235 (T6-16) — the default-deny sandbox reaches a tree that is already
//! on disk, at the boot.
//!
//! The GH #85 cut was prospective: it filled `params.sandbox` in only when a
//! node was instantiated from a template, and the parser read a missing block
//! as "unsandboxed". A hand-written `code`, `bash` or `harness` cell without a
//! block therefore kept the daemon's rights for good
//! (`docs/defer-register.md`, the hand-written `code` cell row). The boot now
//! hands such a cell the same default profile instantiation writes, in the
//! runtime view only: the colony writes `config.json` at instantiation and
//! never at a boot.

use meclaw_colony::{CellFactory, CellFactoryRegistry};
use meclaw_core::serde_json::{Value, json};
use std::sync::Arc;

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    // The colony crate cannot see the real `code` factory; what this pins is
    // the params the boot hands ANY factory registered under that type.
    r.insert(
        "code".to_string(),
        Arc::new(meclaw_testing::factories::EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

#[test]
fn a_hand_written_code_cell_without_a_profile_boots_under_the_default_deny_block() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    write(root, "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    let on_disk = r#"{"cell":{"type":"code"},"params":{"runner":"python3","emitted_target":"/sink"},
        "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;
    write(root, "main/script/config.json", on_disk);

    let plan = meclaw_colony::plan_bootstrap(root, &registry(), &Default::default())
        .expect("a hand-written code cell must still boot");
    let cell = plan
        .cells
        .iter()
        .find(|c| c.path.as_str() == "/script")
        .expect("the code cell is planned");
    assert_eq!(
        cell.params.get("sandbox"),
        Some(&json!({"trust": "restricted", "network": "deny", "filesystem": {"runtime": true}})),
        "a cell type that enforces a sandbox boots under the default-deny block when it declares none"
    );
    assert_eq!(
        cell.params.get("runner"),
        Some(&Value::from("python3")),
        "the declared params stay as they are"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("main/script/config.json")).unwrap(),
        on_disk,
        "the boot writes no config.json"
    );
}
