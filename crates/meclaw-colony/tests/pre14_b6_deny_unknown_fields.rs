//! Pre-14 Pass-2 rejection-test B6 — P1-7 `deny_unknown_fields` on boot configs.
//!
//! Pass-1 hardened two boot-time config deserializers against typo/forward-incompat
//! drift by adding `#[serde(deny_unknown_fields)]`:
//!   * `ColonyConfig` (`colony.json`) — `colony_config.rs`.
//!   * `HiveParams` (a hive `config.json`'s `params` block, which carries ONLY
//!     `graph`) — `config.rs`, parsed during `plan_bootstrap`.
//!
//! Both tests inject a genuinely-unknown key and assert the boot read returns
//! `Err` that NAMES the offending field (so an operator sees the typo, not a
//! silent accept). A trivial pass is impossible: serde must actually reject.

use meclaw_colony::CellFactoryRegistry;
use meclaw_colony::bootstrap::BootstrapError;
use meclaw_colony::bootstrap_from_filesystem;
use meclaw_colony::colony_config::read_colony_config;
use meclaw_testing::ColonyHandle;

// ── (a) colony.json with an unknown/typo field → hard boot-read failure ──────

#[test]
fn colony_json_unknown_field_hard_fails_boot_read() {
    let td = tempfile::TempDir::new().unwrap();
    // `blob_inline_max_byts` is a typo of `blob_inline_max_bytes`. With
    // deny_unknown_fields, serde must reject rather than silently dropping it.
    std::fs::write(
        td.path().join("colony.json"),
        r#"{"blob_inline_max_bytes": 64, "blob_inline_max_byts": 32}"#,
    )
    .unwrap();

    let err = read_colony_config(td.path())
        .expect_err("colony.json with an unknown field must hard-fail the boot read");
    let msg = format!("{err}");
    assert!(
        msg.contains("unknown field"),
        "error must flag the unknown field, got: {msg}"
    );
    assert!(
        msg.contains("blob_inline_max_byts"),
        "error must NAME the offending field, got: {msg}"
    );
}

// ── (a') positive control: a known-only colony.json reads cleanly ────────────

#[test]
fn colony_json_known_fields_only_reads_ok() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::write(
        td.path().join("colony.json"),
        r#"{"blob_inline_max_bytes": 64}"#,
    )
    .unwrap();
    let cfg = read_colony_config(td.path()).expect("known-only colony.json must read");
    assert_eq!(cfg.blob_inline_max_bytes, 64);
}

// ── (b) a hive's params block with a non-`graph` key → boot fails ────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hive_params_unknown_key_hard_fails_boot() {
    let td = tempfile::TempDir::new().unwrap();
    // Root hive whose `params` carries `dead_letters` alongside `graph`. A hive's
    // params block accepts ONLY `graph` (Befund 21) → deny_unknown_fields rejects.
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]},"dead_letters":"/sink"}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new();
    let err = bootstrap_from_filesystem(td.path(), &CellFactoryRegistry::new(), &h.runtime())
        .await
        .expect_err("hive params with an unknown key must fail the bootstrap plan");

    let reason = err
        .items()
        .iter()
        .find_map(|e| match e {
            BootstrapError::InvalidJson { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected an InvalidJson boot error, got: {:?}", err.items()));

    assert!(
        reason.contains("unknown field"),
        "boot error must flag the unknown field, got: {reason}"
    );
    assert!(
        reason.contains("dead_letters"),
        "boot error must NAME the offending field, got: {reason}"
    );

    h.shutdown().await;
}

// ── (b') positive control: a hive with only `graph` in params boots cleanly ──

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hive_params_graph_only_boots_ok() {
    let td = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new();
    bootstrap_from_filesystem(td.path(), &CellFactoryRegistry::new(), &h.runtime())
        .await
        .expect("graph-only hive params must boot");
    h.shutdown().await;
}
