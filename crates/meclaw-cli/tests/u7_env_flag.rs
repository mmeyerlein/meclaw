//! U7 (roadmap 2026-06-11): `--env <path>` overrides the `.env` location for
//! the boot substitution path (spec CLI table overview Z.476, Phase-6 flag,
//! default `<root>/.env`; substitution model: Befund 4 / 8c73186).
//!
//! Both tests run the in-process `--validate` arm: it shares `plan_bootstrap`
//! with the real boot, returns `Err` on any plan error (no process::exit on
//! this path) and needs no spawns.

use meclaw_cli::{Cli, run};

/// Minimal valid tree whose cell params need `${U7_VAR}`: a hive marker plus
/// one bash cell (built-in factory) with the contract mandatory keys.
fn write_var_tree(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("demo/a")).unwrap();
    std::fs::write(
        root.join("demo/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("demo/a/config.json"),
        br#"{"cell":{"type":"bash"},"params":{"marker":"${U7_VAR}"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
}

fn cli_for(root: &std::path::Path, env: Option<std::path::PathBuf>) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env,
        templates: None,
        rescan_templates: false,
        api: None,
        daemon: false,
        validate: true,
        validate_strict: false,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_resolves_vars_from_env_flag_path() {
    let td = tempfile::TempDir::new().unwrap();
    write_var_tree(td.path());
    // `.env` lives OUTSIDE the root — only reachable via the flag.
    let env_dir = tempfile::TempDir::new().unwrap();
    let env_file = env_dir.path().join("custom.env");
    std::fs::write(&env_file, "U7_VAR=set\n").unwrap();

    run(cli_for(td.path(), Some(env_file)))
        .await
        .expect("--validate must pass when --env supplies the variable");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_without_env_flag_falls_back_to_root_env_and_fails() {
    let td = tempfile::TempDir::new().unwrap();
    write_var_tree(td.path());
    // No `.env` anywhere: default `<root>/.env` is absent → env_var_missing.
    let err = run(cli_for(td.path(), None)).await;
    assert!(
        err.is_err(),
        "--validate must fail when ${{U7_VAR}} has no value at the default path"
    );
}
