//! Issue #6: the heartbeat watchdog is armed only after boot, and a real trip
//! ends the process as a fault.
//!
//! Two defects were seen together in parallel eval runs. The watchdog was armed
//! before the boot had finished, so a slow boot could trip it while the colony
//! was still coming up; and when it tripped it took the same `Ok(())` exit as a
//! SIGTERM, so a supervisor saw a clean stop and nothing restarted or alerted.
//!
//! The trip here is produced by the REAL supervisor observing REAL colony
//! silence: `WatchdogTuning` puts the supervisor deadline (3 × 20 ms = 60 ms)
//! under the colony's own heartbeat period (100 ms), so the supervisor sees
//! `threshold` consecutive periods without a beat — the same observation a dead
//! or wedged colony loop produces. Nothing is mocked: the colony runs, the
//! supervisor runs, and the deadline is the only thing the test chooses.

use meclaw_cli::{Cli, WatchdogTuning, run_with_hooks_tuned};
use std::time::Duration;

/// Minimal bootable root: one empty hive, no cells.
fn cli_for(root: &std::path::Path) -> Cli {
    let main_dir = root.join("main");
    std::fs::create_dir_all(&main_dir).unwrap();
    std::fs::write(main_dir.join("config.json"), br#"{"cell":{"type":"hive"}}"#).unwrap();
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

/// Defect 2: a watchdog trip must NOT look like a clean exit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_watchdog_trip_ends_the_run_with_an_error_not_a_clean_exit() {
    let td = tempfile::TempDir::new().unwrap();
    let cli = cli_for(td.path());
    let tuning = WatchdogTuning {
        threshold: 3,
        period: Duration::from_millis(20),
        on_trip: meclaw_cli::WatchdogOnTrip::Exit,
    };

    // Generous failure marker (30 s convention): a trip is expected within
    // ~150 ms, the timeout only fences a hang.
    let res = tokio::time::timeout(
        Duration::from_secs(30),
        run_with_hooks_tuned(cli, None, None, Some(tuning)),
    )
    .await
    .expect("the run must end on the watchdog trip, not hang");

    let err = res.expect_err("a watchdog trip must exit non-zero (Err), not Ok(())");
    let msg = format!("{err}");
    assert!(
        msg.contains("watchdog"),
        "the error must name the watchdog as the cause, was: {msg}"
    );
}

/// Defect 1: the same tuning that trips a running colony must not be able to
/// fire while the colony is still booting — the supervisor is disarmed until
/// the filesystem bootstrap has completed. The positive receipt is that a run
/// whose boot FAILS reports the boot failure, never a watchdog trip: on that
/// path the arming sender is dropped unfired.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_boot_reports_the_boot_failure_and_never_a_watchdog_trip() {
    let td = tempfile::TempDir::new().unwrap();
    // Two root dirs → `MultipleRootDirs`, a bootstrap-plan failure. The colony
    // task is already spawned at that point, so a watchdog armed at spawn time
    // would be counting during the whole failing boot.
    for name in ["one", "two"] {
        let d = td.path().join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config.json"), br#"{"cell":{"type":"hive"}}"#).unwrap();
    }
    let mut cli = cli_for(td.path());
    cli.daemon = true;
    let tuning = WatchdogTuning {
        threshold: 1,
        period: Duration::from_millis(1),
        on_trip: meclaw_cli::WatchdogOnTrip::Exit,
    };

    let res = tokio::time::timeout(
        Duration::from_secs(30),
        run_with_hooks_tuned(cli, None, None, Some(tuning)),
    )
    .await
    .expect("a failing boot must end the run, not hang");

    let err = res.expect_err("a failing bootstrap must not exit 0");
    let msg = format!("{err}");
    assert!(
        msg.contains("bootstrap"),
        "the failure must be reported as the boot failure it is, was: {msg}"
    );
    assert!(
        !msg.contains("watchdog"),
        "the watchdog must stay disarmed until boot completes, was: {msg}"
    );
}

// ---------------------------------------------------------------- GH #84

/// GH #84 half 1: the tuning is reachable from `colony.json`.
///
/// The receipt is deliberately the production entry point — `run_with_hooks`,
/// the one `run()` calls, with NO override argument. The only thing this test
/// writes is a file in the colony root; if the deadline still came from the
/// hard-wired default, a colony that beats every 100 ms would never miss a
/// 5 × 100 ms window and the run would hang until the failure marker.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn colony_json_sets_the_watchdog_deadline_on_the_production_path() {
    let td = tempfile::TempDir::new().unwrap();
    let cli = cli_for(td.path());
    // 3 × 20 ms = 60 ms, under the colony's own 100 ms heartbeat period.
    std::fs::write(
        td.path().join("colony.json"),
        br#"{"watchdog_threshold": 3, "watchdog_period_ms": 20}"#,
    )
    .unwrap();

    let res = tokio::time::timeout(
        Duration::from_secs(30),
        meclaw_cli::run_with_hooks(cli, None, None),
    )
    .await
    .expect("colony.json must reach the watchdog, so the run must end on the trip");

    let err = res.expect_err("a watchdog trip must exit non-zero (Err), not Ok(())");
    let msg = format!("{err}");
    assert!(
        msg.contains("watchdog"),
        "the error must name the watchdog as the cause, was: {msg}"
    );
    // GH #84 half 3: the trip carries evidence, not only a deadline.
    for needle in ["starved=", "silent_for=", "supervisor_lag=", "colony_task="] {
        assert!(
            msg.contains(needle),
            "the trip must be actionable and name {needle} — was: {msg}"
        );
    }
    // GH #165: the end-to-end receipt that the corroboration is REAL on the
    // production path and did not merely fail to load. The colony was parked
    // (nothing in flight), the independent witness kept finishing its work units
    // on the same runtime — so this trip implicates the colony loop and is
    // correctly fatal. If either control had been missing or broken, the process
    // would have kept running and this test would have hung instead.
    for needle in [
        "in_flight_work=false",
        "witness=kept",
        "starved=colony_loop",
    ] {
        assert!(
            msg.contains(needle),
            "the trip must be corroborated, not merely measured — {needle} missing \
             in: {msg}"
        );
    }
}

/// GH #84 half 1, the other half: reachable must not mean changed. A colony root
/// with NO `colony.json` runs the pre-#84 deadline (5 × 100 ms against a 100 ms
/// heartbeat), so a healthy colony must survive well past it and stop only when
/// it is told to. This is the regression lock on "no behaviour change by
/// default".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_colony_json_the_default_deadline_does_not_trip_a_healthy_colony() {
    let td = tempfile::TempDir::new().unwrap();
    let cli = cli_for(td.path());
    assert!(!td.path().join("colony.json").exists());
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let run = tokio::spawn(meclaw_cli::run_with_hooks(cli, None, Some(shutdown_rx)));
    // Six times the default 500 ms window. A default that had moved would have
    // tripped several times over inside this.
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    shutdown_tx.send(()).expect("the run must still be alive");

    let res = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("the run must end on the shutdown hook")
        .expect("the run task must not panic");
    res.expect("a healthy colony under the DEFAULT deadline must exit Ok, not on a trip");
}

/// GH #84 half 3: `watchdog_on_trip: "log-only"` keeps the colony running.
///
/// Same deadline as the trip test above — the supervisor trips again and again —
/// but the process must survive all of it and still end cleanly on its shutdown
/// signal. The trips themselves are not silent (they go to stderr and to
/// `tracing`); what this pins is that they no longer take the colony with them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn log_only_reports_the_trip_and_keeps_the_colony_running() {
    let td = tempfile::TempDir::new().unwrap();
    let cli = cli_for(td.path());
    std::fs::write(
        td.path().join("colony.json"),
        br#"{"watchdog_threshold": 3, "watchdog_period_ms": 20,
             "watchdog_on_trip": "log-only"}"#,
    )
    .unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let run = tokio::spawn(meclaw_cli::run_with_hooks(cli, None, Some(shutdown_rx)));
    // Long enough for many trip windows to come and go under `exit` semantics.
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    shutdown_tx
        .send(())
        .expect("log-only must have kept the run alive");

    let res = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("the run must end on the shutdown hook, not hang")
        .expect("the run task must not panic");
    res.expect("under log-only a silence trip must not end the process");
}

/// A `colony.json` the substrate cannot run with is a boot failure, not a clamp.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_zero_watchdog_period_is_a_boot_failure() {
    let td = tempfile::TempDir::new().unwrap();
    let cli = cli_for(td.path());
    std::fs::write(
        td.path().join("colony.json"),
        br#"{"watchdog_period_ms": 0}"#,
    )
    .unwrap();

    let res = tokio::time::timeout(
        Duration::from_secs(30),
        meclaw_cli::run_with_hooks(cli, None, None),
    )
    .await
    .expect("an invalid colony.json must fail the boot, not hang");
    let err = res.expect_err("a zero supervisor period must not boot");
    let msg = format!("{err}");
    assert!(
        msg.contains("watchdog_period_ms"),
        "the failure must name the offending key, was: {msg}"
    );
}
