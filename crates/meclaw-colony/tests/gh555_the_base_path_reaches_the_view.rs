//! GH #555 — the declaration leg of the transfer fence: what a `config.json`
//! says in `params.transfer.base_path` has to arrive in the view the substrate
//! spawns the cell with.
//!
//! The fence itself is pinned next to the slot it bounds
//! (`gh555_the_slot_writes_its_own_files`). What cannot be shown there is that
//! `params.transfer.base_path` is a real declaration rather than a struct field
//! nobody fills: the boot path has to read it off the file and carry it into the
//! `TransferBounds` every spawn helper forwards.
//!
//! Three cells, one boot: one declares a fence, one declares none, one declares
//! a directory that is not there. The third is the load-bearing one — a member
//! whose export directory does not exist yet must still boot and still pass
//! `--validate`, which is exactly why the parse is PURE (no `canonicalize`, no
//! existence check). `templates/member/README.md` records the day that rule was
//! learned the other way round.

use meclaw_colony::BootstrapError;
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
fn a_declared_base_path_reaches_the_view_the_substrate_spawns_with() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/fenced/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo"}},
                "params":{{"emitted_target":"/sink","transfer":{{"base_path":"/srv/meclaw-export"}}}},
                "contract":{{{CONTRACT_TAIL}}}}}"#
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
    let bounds = |p: &str| {
        plan.cells
            .iter()
            .find(|c| c.path.as_str() == p)
            .unwrap_or_else(|| panic!("{p} must be planned"))
            .contract_view
            .transfer_bounds()
    };

    assert_eq!(
        bounds("/fenced").base_path.as_deref(),
        Some(std::path::Path::new("/srv/meclaw-export")),
        "a declared fence that does not reach the view is not a fence"
    );
    assert!(
        bounds("/plain").base_path.is_none(),
        "absence must keep meaning 'this cell names no path of its own'"
    );

    // The fence is independent of the two contract declarations beside it
    // (GH #260 + GH #314): naming a directory says nothing about who may write
    // or whether the database travels at all.
    assert_eq!(
        bounds("/fenced").policy,
        meclaw_core::TransferPolicy::All,
        "a fence must not switch the exemption on"
    );
    assert_eq!(
        bounds("/fenced").write_surface,
        meclaw_core::WriteSurface::Open,
        "a fence must not switch the write surface on"
    );
}

/// The parse is PURE: it checks the string and `is_absolute` and asks the
/// filesystem nothing. A cell whose export directory does not exist yet boots,
/// and `--validate` passes — the refusal comes at the first `to`/`from`, as a
/// `transfer_io_error` on the message.
///
/// This is the whole reason the interim export sink was a `code` cell and not a
/// `file` cell (`templates/member/README.md`): a `file` cell canonicalises its
/// `base_path` at validation, so a member whose lane nobody had used yet failed
/// to boot. That trap must not be rebuilt here.
#[test]
fn a_fence_that_is_not_there_yet_still_boots_and_still_validates() {
    let td = tempfile::TempDir::new().unwrap();
    let absent = td.path().join("not-created-yet").join("deeper");
    assert!(!absent.exists());
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/fenced/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo"}},
                "params":{{"emitted_target":"/sink","transfer":{{"base_path":{}}}}},
                "contract":{{{CONTRACT_TAIL}}}}}"#,
            meclaw_core::serde_json::Value::from(absent.to_str().unwrap())
        ),
    );

    let plan = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect("a fence that is not there yet must not stop a boot");
    let bounds = plan
        .cells
        .iter()
        .find(|c| c.path.as_str() == "/fenced")
        .expect("/fenced must be planned")
        .contract_view
        .transfer_bounds();
    assert_eq!(
        bounds.base_path.as_deref(),
        Some(absent.as_path()),
        "the path is carried verbatim — not canonicalised, not resolved"
    );
}

/// The other half: a fence that cannot be one is a LOUD boot error, not a
/// silent `None`. A relative `base_path` has no fixed meaning — the cell task
/// knows no colony root — so a colony that booted with one would answer
/// `to`/`from` against whatever directory the process happened to start in.
#[test]
fn a_relative_base_path_is_a_loud_boot_error() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/wrong/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo"}},
                "params":{{"emitted_target":"/sink","transfer":{{"base_path":"export"}}}},
                "contract":{{{CONTRACT_TAIL}}}}}"#
        ),
    );

    let err = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect_err("a relative fence must stop the boot");
    assert!(
        err.items().iter().any(|e| matches!(
            e,
            BootstrapError::InvalidTransferBasePath { reason, .. }
                if reason.contains("absolute") && reason.contains("export")
        )),
        "the boot error must name the key and the offending value: {:?}",
        err.items()
    );
}

/// And a `transfer` block that is not an object at all — the same loudness, for
/// the same reason: a typo that quietly means "no fence" is the worst outcome
/// this key can have.
#[test]
fn a_transfer_block_that_is_not_an_object_is_a_loud_boot_error() {
    let td = tempfile::TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/wrong/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo"}},
                "params":{{"emitted_target":"/sink","transfer":"/srv/export"}},
                "contract":{{{CONTRACT_TAIL}}}}}"#
        ),
    );

    let err = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect_err("a malformed transfer block must stop the boot");
    assert!(
        err.items()
            .iter()
            .any(|e| matches!(e, BootstrapError::InvalidTransferBasePath { .. })),
        "got: {:?}",
        err.items()
    );
}
