//! GH #260 — the declaration leg of the substrate's write boundary: what a
//! `config.json` says has to arrive in the `ContractView` the substrate spawns
//! the cell with.
//!
//! The rule itself is pinned where it lives, next to the slot it bounds
//! (`meclaw_colony::db_transfer`, unit tests). What cannot be shown there is
//! that `contract.write_surface` is a real declaration rather than a struct
//! field nobody fills: the boot path has to read it off the file and carry it
//! into the view every factory forwards.
//!
//! Two cells, one boot: one declares the boundary, one says nothing. The one
//! that says nothing must come out `Open` — absence has to keep meaning "no
//! change", or every colony that predates this key would silently acquire a
//! boundary it never asked for.

use meclaw_core::WriteSurface;
use std::sync::Arc;

const CONTRACT_TAIL: &str = r#""version":"0.1.0","settings":{},"consumes":{}"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn echo_factories() -> meclaw_colony::CellFactoryRegistry {
    let mut m = meclaw_colony::CellFactoryRegistry::new();
    m.insert(
        "echo".to_string(),
        Arc::new(meclaw_testing::factories::EchoCellFactory) as Arc<dyn meclaw_colony::CellFactory>,
    );
    m
}

#[test]
fn a_declared_write_surface_reaches_the_view_the_substrate_spawns_with() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/sealed/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"/sink"}},
                "contract":{{{CONTRACT_TAIL},"write_surface":"internal"}}}}"#
        ),
    );
    write(
        td.path(),
        "main/plain/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"/sink"}},
                "contract":{{{CONTRACT_TAIL}}}}}"#
        ),
    );

    // `main/` is the colony root marker, so the two cells under it come out as
    // `/sealed` and `/plain` — the same shape every other bootstrap test uses.
    let plan = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect("boot must plan");
    let of = |p: &str| {
        plan.cells
            .iter()
            .find(|c| c.path.as_str() == p)
            .unwrap_or_else(|| panic!("{p} must be planned"))
            .contract_view
            .write_surface
    };

    assert_eq!(
        of("/sealed"),
        WriteSurface::Internal,
        "a declared boundary that does not reach the view is not a boundary"
    );
    assert_eq!(
        of("/plain"),
        WriteSurface::Open,
        "absence must keep meaning 'no change' — every colony older than this key relies on it"
    );
}
