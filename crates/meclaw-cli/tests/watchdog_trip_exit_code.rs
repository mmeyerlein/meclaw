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
        strict: false,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
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
    };

    // Generous failure marker (30 s convention): a trip is expected within
    // ~150 ms, the timeout only fences a hang.
    let res = tokio::time::timeout(
        Duration::from_secs(30),
        run_with_hooks_tuned(cli, None, None, tuning),
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
    };

    let res = tokio::time::timeout(
        Duration::from_secs(30),
        run_with_hooks_tuned(cli, None, None, tuning),
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
