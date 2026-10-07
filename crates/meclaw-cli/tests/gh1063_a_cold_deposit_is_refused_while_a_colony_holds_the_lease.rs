//! GH #1063 — the cold deposit keeps the single-writer rule (GH #160).
//!
//! `meclaw --vault-add` may now lay down the database of a vault that was
//! grown but never woken. It must not do that — nor write anything — while a
//! colony holds the root: the colony is about to wake that cell, and the cell
//! owns its `cell.db`. The check is the same root-lease check `--vault-add`
//! makes today, and it runs before stdin or the database are touched.

use meclaw_cli::vault_cli::{self, VaultCommand};

#[test]
fn gh1063_a_cold_deposit_is_refused_while_a_colony_holds_the_lease() {
    let td = tempfile::TempDir::new().unwrap();
    let vault = td.path().join("main/vault");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(
        vault.join("config.json"),
        r#"{"cell":{"type":"vault"},"params":{"broker":"/main/invoke"}}"#,
    )
    .unwrap();

    // This test process holds the root, the way a running colony does.
    let lease = meclaw_cli::lease::acquire(td.path()).expect("lease");
    let err = vault_cli::run(
        td.path(),
        "/main/vault",
        VaultCommand::Add("openrouter".into()),
        "prompt",
        None,
    )
    .expect_err("no deposit while a colony holds the root")
    .to_string();
    assert!(err.contains("a meclaw colony is running"), "{err}");
    assert!(
        !vault.join("cell.db").exists(),
        "nothing was laid down under a running colony"
    );
    drop(lease);
}
