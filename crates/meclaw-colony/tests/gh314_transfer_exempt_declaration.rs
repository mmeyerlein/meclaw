//! GH #314 — the declaration leg of the transfer exemption: what a
//! `config.json` says has to arrive in the `ContractView` the substrate spawns
//! the cell with.
//!
//! The rule itself is pinned where it lives, next to the slot it bounds
//! (`meclaw_colony::db_transfer`, unit tests), and end-to-end against a real
//! vault in `meclaw-cells` (`gh314_a_vault_does_not_travel`). What cannot be
//! shown in either place is that `contract.transfer` is a real declaration
//! rather than a struct field nobody fills: the boot path has to read it off the
//! file and carry it into the view every factory forwards.
//!
//! Two cells, one boot: one declares the exemption, one says nothing. The one
//! that says nothing must come out `All` — absence has to keep meaning "no
//! change", or every colony that predates this key would silently stop
//! answering a slot it always answered.

use meclaw_core::{TransferPolicy, WriteSurface};
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
fn a_declared_transfer_exemption_reaches_the_view_the_substrate_spawns_with() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/exempt/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"emitted_target":"/sink"}},
                "contract":{{{CONTRACT_TAIL},"transfer":"none"}}}}"#
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

    let plan = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect("boot must plan");
    let view = |p: &str| {
        plan.cells
            .iter()
            .find(|c| c.path.as_str() == p)
            .unwrap_or_else(|| panic!("{p} must be planned"))
            .contract_view
            .transfer_bounds()
    };

    assert_eq!(
        view("/exempt").policy,
        TransferPolicy::None,
        "a declared exemption that does not reach the view is not an exemption"
    );
    assert!(view("/exempt").is_exempt());
    assert_eq!(
        view("/plain").policy,
        TransferPolicy::All,
        "absence must keep meaning 'no change' — every colony older than this key relies on it"
    );

    // The two declarations are independent (GH #260 + GH #314): declaring one
    // must not switch the other on. An exempt cell that silently also became
    // write-sealed would be a second boundary nobody wrote down.
    assert_eq!(view("/exempt").write_surface, WriteSurface::Open);
}

/// The other half of the same independence: `write_surface: "internal"` alone
/// says nothing about whether the database travels — which is exactly the gap
/// #314 was opened for. An `export` is a read, and the write surface never
/// bounded a read.
#[test]
fn a_write_sealed_cell_is_not_exempt_by_that_alone() {
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

    let plan = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect("boot must plan");
    let bounds = plan
        .cells
        .iter()
        .find(|c| c.path.as_str() == "/sealed")
        .expect("/sealed must be planned")
        .contract_view
        .transfer_bounds();

    assert_eq!(bounds.write_surface, WriteSurface::Internal);
    assert_eq!(
        bounds.policy,
        TransferPolicy::All,
        "a cell that wants both sealed declares both"
    );
}
