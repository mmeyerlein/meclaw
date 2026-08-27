//! U9 (roadmap 2026-06-11; RULED A8 2026-06-12): headless mode is legitimate.
//! `--daemon` without `--api` starts the colony WITHOUT an HTTP server — the
//! flags are independent of each other (daemon = the process runs; api = HTTP
//! server). Before: `--daemon` without `--api` was a no-op (an immediate
//! `return Ok(())` BEFORE the `colony_task` spawn — nothing booted, no timer
//! ticked).
//!
//! U9-a proves headless POSITIVELY via a real timer-tick receipt: a `timer` cell
//! with `cron */1` bumps `iteration_n` in its `cell.db` on every fire. If the
//! colony runs headless, `iteration_n > 0` after a short while.
//! U9-b proves that `--daemon --api` delivers BOTH: HTTP server on AND running.

use meclaw_cli::{Cli, run_with_hooks};
use std::net::SocketAddr;

const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-000000000001";

/// Root hive + a `timer` cell with a seconds cron. `emit_to` points at a
/// non-existent path → the fire goes to the DLQ (irrelevant), but `iteration_n`
/// is bumped on every repeating fire (timer/cell.rs `bump_iteration`).
fn write_timer_tree(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("main/tick")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("main/tick/config.json"),
        format!(
            r#"{{"cell":{{"type":"timer"}},"params":{{"query_timeout_ms":2000,"schedules":[{{"schedule_id":"{SCHEDULE_ID}","schedule_name":"tick","cron":"*/1 * * * * *","emit_to":"/main/sink","emit_body":{{"messages":[]}}}}]}},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

fn headless_cli(root: &std::path::Path) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: None,
        daemon: true,
        validate: false,
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

/// Read the persisted `iteration_n` from the timer's `cell.db`. Read AFTER the
/// graceful shutdown so the timer task's connection is released (no lock race).
fn read_iteration_n(cell_db: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(cell_db).unwrap();
    conn.query_row(
        "SELECT iteration_n FROM schedules WHERE schedule_id = ?1",
        [SCHEDULE_ID],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn daemon_without_api_runs_headless_and_timer_ticks() {
    let td = tempfile::TempDir::new().unwrap();
    write_timer_tree(td.path());
    let cli = headless_cli(td.path());

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    // No addr_hook: headless never binds an HTTP listener.
    let join = tokio::spawn(async move { run_with_hooks(cli, None, Some(shutdown_rx)).await });

    // Give the `cron */1` timer time to fire repeatedly (~3 ticks).
    tokio::time::sleep(std::time::Duration::from_millis(3500)).await;

    // If the colony had returned a No-op (old behavior), the future would already
    // be finished and no timer would have bumped anything.
    assert!(
        !join.is_finished(),
        "headless colony must keep running until shutdown, not No-op-return"
    );

    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), join)
        .await
        .expect("headless shutdown must not hang")
        .expect("join")
        .expect("run_with_hooks Ok");

    let n = read_iteration_n(&td.path().join("main/tick/cell.db"));
    assert!(
        n >= 1,
        "timer must have ticked at least once while headless (iteration_n={n})"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn daemon_with_api_serves_http_and_runs() {
    let td = tempfile::TempDir::new().unwrap();
    let main_dir = td.path().join("main");
    std::fs::create_dir_all(&main_dir).unwrap();
    std::fs::write(main_dir.join("config.json"), br#"{"cell":{"type":"hive"}}"#).unwrap();
    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();

    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let mut cli = headless_cli(td.path());
    cli.daemon = true;
    cli.api = Some(bind);

    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });

    let actual_addr = addr_rx.await.unwrap();
    let resp = reqwest::Client::new()
        .get(format!("http://{actual_addr}/colony/registry"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "--daemon --api must still serve HTTP"
    );

    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), join)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
