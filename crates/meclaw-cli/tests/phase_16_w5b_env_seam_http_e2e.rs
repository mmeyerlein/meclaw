//! W5b (short cycle, builds on W5/5534862): a production-seam pin for the env
//! source consistency wired in W5 (`cli.env → boot + colony_task →
//! handle_mutation`). W5 pinned the substitution BEHAVIOUR at colony level
//! (ColonyHandle-internal), the CLI/HTTP seam itself only compile-/gate-checked.
//! This test drives the REAL CLI lifecycle (`run_with_hooks` = the production
//! `run()` path, NOT the colony-internal ColonyHandle harness) with real HTTP
//! and proves end-to-end that the seam carries through.
//!
//! Discriminator: an env file OUTSIDE root, pinned via `--env`, against a
//! default `<root>/.env` that carries a key with a DIFFERENT value:
//!   - alt.env:            `BOOT_VAR=alt_boot`, `MUT_VAR=alt_value`
//!   - default <root>/.env: `MUT_VAR=default_value` (different!), NO `BOOT_VAR`
//!
//! Three paths, ONE CLI process:
//!  (a) Boot substitution: the boot cell uses `${BOOT_VAR}` (only in alt.env).
//!      If the colony boots through (HTTP comes up, the registry shows the
//!      cell), the boot read the pinned source — had it read `<root>/.env`,
//!      `BOOT_VAR` would be missing and `bootstrap_from_filesystem` would fail
//!      (no HTTP).
//!  (b) HTTP mutation (template instantiation) with `${MUT_VAR}`: committed +
//!      the written config.json carries `alt_value` (NOT `default_value`).
//!  (c) 2b adoption in the same process with `${MUT_VAR}`: committed + the
//!      config.json of the adopted node carries `alt_value`.
//!
//! Expectation GREEN ⇒ the seam carries through, the pin is the proof. RED ⇒ a
//! genuine finding (incomplete seam) ⇒ STOP + escalate.

use meclaw_cli::{Cli, run_with_hooks};
use std::net::SocketAddr;

/// Recursively find the first `config.json` whose parent dir is named `cell`,
/// and return its parsed `params`. Path-resolution-agnostic readback.
fn read_cell_params(root: &std::path::Path, cell: &str) -> Option<serde_json::Value> {
    fn walk(dir: &std::path::Path, cell: &str) -> Option<serde_json::Value> {
        for entry in std::fs::read_dir(dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = walk(&path, cell) {
                    return Some(found);
                }
            } else if path.file_name().is_some_and(|n| n == "config.json")
                && dir.file_name().is_some_and(|n| n == cell)
            {
                let raw = std::fs::read_to_string(&path).ok()?;
                let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
                return v.get("params").cloned();
            }
        }
        None
    }
    walk(root, cell)
}

async fn post_mutation(addr: &SocketAddr, payload: serde_json::Value) -> (u16, serde_json::Value) {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/colony/mutations"))
        .json(&payload)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.unwrap();
    (status, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_env_seam_carries_through_boot_mutation_and_adoption() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();

    // --- Env sources: pinned alt.env (outside root) vs default <root>/.env. ---
    let env_dir = tempfile::TempDir::new().unwrap();
    let alt_env = env_dir.path().join("alt.env");
    std::fs::write(&alt_env, "BOOT_VAR=alt_boot\nMUT_VAR=alt_value\n").unwrap();
    // Default source carries MUT_VAR with a DIFFERENT value, and NO BOOT_VAR.
    std::fs::write(root.join(".env"), "MUT_VAR=default_value\n").unwrap();

    // --- Boot tree: root hive + a bash boot-cell using ${BOOT_VAR} (alt-only). ---
    std::fs::create_dir_all(root.join("main/booter")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("main/booter/config.json"),
        br#"{"cell":{"type":"bash"},"params":{"command":"true","marker":"${BOOT_VAR}"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // --- A template (registered at boot via --rescan-templates) for path (b). ---
    let tpl = root.join("templates/bashtpl");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), br#"{"name":"bashtpl"}"#).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        br#"{"cell":{"type":"bash"},"params":{"command":"true"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    // --- Start the REAL CLI lifecycle with --env <alt.env> + --api + rescan. ---
    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let cli = Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: Some(alt_env.clone()),
        templates: None,
        rescan_templates: true,
        api: Some(bind),
        daemon: false,
        validate: false,
        strict: false,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        stdio_format: meclaw_cli::StdioFormat::Text,
    };
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });

    // (a) Boot succeeded ⇒ HTTP came up. If the boot had read <root>/.env (broken
    // seam), ${BOOT_VAR} would be missing and bootstrap would fail before bind —
    // addr_rx would never resolve. Receiving the addr is the boot-seam receipt.
    let actual_addr = tokio::time::timeout(std::time::Duration::from_secs(30), addr_rx)
        .await
        .expect("boot must bind HTTP within 30s — if it hangs, boot read the wrong env source")
        .expect("addr hook");

    // Positive receipt: the boot-substituted cell is registered.
    let registry: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{actual_addr}/colony/registry"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        registry.to_string().contains("booter"),
        "boot-substituted cell must be registered (proves boot read alt.env); registry: {registry}"
    );

    // (b) HTTP mutation instantiates a cell with ${MUT_VAR} → committed, and the
    // written config.json carries alt_value (the pinned source), not default_value.
    let (status, body) = post_mutation(
        &actual_addr,
        serde_json::json!({
            "scope": "/",
            "ctx": {},
            "diff": {"add_nodes": [{
                "name": "muta",
                "template": "bashtpl",
                "override_params": {"marker": "${MUT_VAR}"}
            }]}
        }),
    )
    .await;
    assert_eq!(status, 200, "mutation must commit; body: {body}");
    assert_eq!(body["mutation"]["outcome"], "committed", "body: {body}");
    let muta_params = read_cell_params(root, "muta").expect("muta config.json written");
    assert_eq!(
        muta_params["marker"], "alt_value",
        "HTTP-mutation cell must resolve ${{MUT_VAR}} from the pinned --env source, not the \
         default <root>/.env; params: {muta_params}"
    );

    // (c) 2b-adoption over the same process. Drop an UNREGISTERED node on disk
    // (post-boot ⇒ not in the registry), then adopt it with ${MUT_VAR}.
    std::fs::create_dir_all(root.join("main/adopted")).unwrap();
    std::fs::write(
        root.join("main/adopted/config.json"),
        br#"{"cell":{"type":"bash"},"params":{"command":"true","marker":"placeholder"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    let (status, body) = post_mutation(
        &actual_addr,
        serde_json::json!({
            "scope": "/",
            "ctx": {},
            "diff": {"add_nodes": [{
                "name": "adopted",
                "adopt": {"type": "bash", "version": "0.1.0"},
                "override_params": {"marker": "${MUT_VAR}"}
            }]}
        }),
    )
    .await;
    assert_eq!(status, 200, "adoption must commit; body: {body}");
    assert_eq!(body["mutation"]["outcome"], "committed", "body: {body}");
    let adopted_params =
        read_cell_params(root, "adopted").expect("adopted config.json (re-written)");
    assert_eq!(
        adopted_params["marker"], "alt_value",
        "2b-adoption must resolve ${{MUT_VAR}} from the pinned --env source; params: {adopted_params}"
    );

    // Graceful shutdown.
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), join)
        .await
        .expect("shutdown must not hang")
        .expect("join")
        .expect("run_with_hooks Ok");
}
