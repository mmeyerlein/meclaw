//! GH #1326 — `meclaw --validate --apply <manifest>` refuses offline what the
//! colony refuses at birth.
//!
//! `--validate` checked the params of every cell in the filesystem tree but
//! ignored `--apply`: a cell a manifest grows from a template (`add_nodes` with
//! `override_params`) met `CellFactory::validate_params` only at the live
//! mutation door. The case that found it: an `llm` cell with provider
//! `anthropic` and `length_continuations: 1` passed `--validate` and was then
//! refused at birth as `anthropic_unsupported_param`.
//!
//! The dry run stages the manifest through the door's own staging path into a
//! scratch directory outside the root, so the refusal text is the door's. No
//! model is called and no colony starts: `--validate` returns before the spawn.

use meclaw_cli::{Cli, run};
use std::path::{Path, PathBuf};

fn cli_validate_apply(root: PathBuf, manifest: PathBuf) -> Cli {
    Cli {
        root,
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        log_stderr: meclaw_cli::LogSink::Auto,
        log_file: meclaw_cli::LogSink::Auto,
        env: None,
        templates: None,
        rescan_templates: false,
        api: None,
        daemon: false,
        validate: true,
        validate_strict: false,
        env_report: false,
        apply: Some(manifest),
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
        command: None,
    }
}

/// A root hive and, in the colony's template library, one single-cell
/// template `brain`: an `llm` cell on the native anthropic wire.
fn write_tree(root: &Path) {
    std::fs::create_dir_all(root.join("main")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    let tpl = root.join("templates/brain");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(
        tpl.join("template.json"),
        br#"{"name":"brain","version":"1.0.0"}"#,
    )
    .unwrap();
    let cfg = serde_json::json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "anthropic",
            "model": "claude-test",
            "api_key": "test-key-gh1326",
            "x_title": "meclaw-lab gh1326",
            "http_referer": "https://example.test"
        },
        "contract": {
            "version": "0.1.0",
            "settings": {},
            "consumes": {"body": {"messages": {"type": "array"}}}
        }
    });
    std::fs::write(
        tpl.join("config.json"),
        serde_json::to_vec_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

fn write_manifest(dir: &Path, scope: &str, override_params: serde_json::Value) -> PathBuf {
    write_manifest_entry(
        dir,
        serde_json::json!({
            "scope": scope,
            "diff": {"add_nodes": [{
                "name": "brain",
                "template": "brain",
                "override_params": override_params
            }]}
        }),
    )
}

fn write_manifest_entry(dir: &Path, entry: serde_json::Value) -> PathBuf {
    let manifest = serde_json::json!({"manifest": [entry]});
    let path = dir.join("manifest.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    path
}

/// The dry run writes nothing into the colony root: no `.staging`, no grown
/// node (No-Delete makes a stray write there permanent).
fn assert_root_untouched(root: &Path) {
    assert!(
        !root.join(".staging").exists(),
        "the dry run staged inside the colony root"
    );
    assert!(
        !root.join("main/brain").exists(),
        "the dry run grew the node into the colony root"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grown_anthropic_cell_with_a_length_continuation_is_refused() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path().join("colony");
    write_tree(&root);
    let manifest = write_manifest(
        td.path(),
        "/",
        serde_json::json!({"length_continuations": 1}),
    );

    let err = run(cli_validate_apply(root.clone(), manifest))
        .await
        .expect_err("the birth refusal must fail --validate");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("anthropic_unsupported_param") && msg.contains("length_continuations"),
        "the door's own refusal text is named: {msg}"
    );
    assert!(
        msg.contains("manifest entry 1"),
        "the refusal names the manifest entry: {msg}"
    );
    assert_root_untouched(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_manifest_without_the_override_passes() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path().join("colony");
    write_tree(&root);
    let manifest = write_manifest(td.path(), "/", serde_json::json!({}));

    run(cli_validate_apply(root.clone(), manifest))
        .await
        .expect("a manifest the door would take passes --validate");
    assert_root_untouched(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scope_carrying_a_variable_is_read_literally_and_does_not_hide_the_refusal() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path().join("colony");
    write_tree(&root);
    let manifest = write_manifest(
        td.path(),
        "/${MEMBER}",
        serde_json::json!({"length_continuations": 1}),
    );

    let err = run(cli_validate_apply(root.clone(), manifest))
        .await
        .expect_err(
            "a literal `${VAR}` scope, as the door reads it, must not skip the param check",
        );
    let msg = format!("{err:#}");
    assert!(msg.contains("anthropic_unsupported_param"), "{msg}");
    assert_root_untouched(&root);
}

/// A multi-cell template (a hive with two cells) goes through the subtree
/// staging; the refusal of a cell inside it arrives, and nothing is staged
/// into the root.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_cell_inside_a_subtree_template_is_refused() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path().join("colony");
    write_tree(&root);
    let tpl = root.join("templates/pair");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(
        tpl.join("template.json"),
        br#"{"name":"pair","version":"1.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        tpl.join("config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    let brain_cfg = std::fs::read(root.join("templates/brain/config.json")).unwrap();
    let mut refused: serde_json::Value = serde_json::from_slice(&brain_cfg).unwrap();
    refused["params"]["length_continuations"] = serde_json::json!(1);
    for (cell, cfg) in [
        ("a", brain_cfg.clone()),
        ("b", serde_json::to_vec_pretty(&refused).unwrap()),
    ] {
        std::fs::create_dir_all(tpl.join(cell)).unwrap();
        std::fs::write(tpl.join(cell).join("config.json"), cfg).unwrap();
    }
    let manifest = write_manifest_entry(
        td.path(),
        serde_json::json!({
            "scope": "/",
            "diff": {"add_nodes": [{"name": "pair", "template": "pair"}]}
        }),
    );

    let err = run(cli_validate_apply(root.clone(), manifest))
        .await
        .expect_err("a refused cell inside a subtree must fail --validate");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("anthropic_unsupported_param") && msg.contains("manifest entry 1"),
        "{msg}"
    );
    assert_root_untouched(&root);
    assert!(
        !root.join("main/pair").exists(),
        "the dry run grew the subtree into the colony root"
    );
}

/// The with-side of a `swap_nodes` is a fresh instantiation from a template;
/// its params meet the same check, and nothing is staged into the root.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_swap_with_side_is_refused() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path().join("colony");
    write_tree(&root);
    let manifest = write_manifest_entry(
        td.path(),
        serde_json::json!({
            "scope": "/",
            "diff": {"swap_nodes": [{
                "match": {"name": "old"},
                "with": {
                    "name": "brain2",
                    "template": "brain",
                    "params": {"length_continuations": 1}
                }
            }]}
        }),
    );

    let err = run(cli_validate_apply(root.clone(), manifest))
        .await
        .expect_err("a refused swap with-side must fail --validate");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("anthropic_unsupported_param") && msg.contains("manifest entry 1"),
        "{msg}"
    );
    assert_root_untouched(&root);
    assert!(
        !root.join("main/brain2").exists(),
        "the dry run grew the with-side into the colony root"
    );
}
