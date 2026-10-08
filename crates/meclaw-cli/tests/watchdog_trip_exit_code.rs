//! Issue #6: the heartbeat watchdog is armed only after boot, and a real trip
//! ends the process as a fault.
//!
//! Two defects were seen together in parallel eval runs. The watchdog was armed
//! before the boot had finished, so a slow boot could trip it while the colony
//! was still coming up; and when it tripped it took the same `Ok(())` exit as a
//! SIGTERM, so a supervisor saw a clean stop and nothing restarted or alerted.
//!
//! The trip here is produced by the REAL supervisor observing REAL colony
//! silence: the supervisor window is set under the colony's own heartbeat
//! period (since GH #1099 the colony beats every half `watchdog_period_ms` of
//! its `colony.json`), so the supervisor sees `threshold` consecutive periods
//! without a beat — the same observation a dead or wedged colony loop produces.
//! Nothing is mocked: the colony runs, the supervisor runs, and the two clocks
//! are the only thing the tests choose.

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
        log_stderr: meclaw_cli::LogSink::Auto,
        log_file: meclaw_cli::LogSink::Auto,
        env: None,
        templates: None,
        rescan_templates: false,
        api: None,
        daemon: true,
        validate: false,
        validate_strict: false,
        env_report: false,
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
        command: None,
    }
}

/// Defect 2: a watchdog trip must NOT look like a clean exit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_watchdog_trip_ends_the_run_with_an_error_not_a_clean_exit() {
    let td = tempfile::TempDir::new().unwrap();
    let cli = cli_for(td.path());
    // GH #1099: the idle colony beats every half `watchdog_period_ms` of ITS
    // config (50 ms at the default 100 ms); an override supervisor of 3 x 5 ms
    // windows sits under that beat and trips. (3 x 20 ms no longer does: the
    // beat used to be a fixed 100 ms.)
    let tuning = WatchdogTuning {
        threshold: 3,
        period: Duration::from_millis(5),
        on_trip: meclaw_cli::WatchdogOnTrip::Exit,
    };

    // Generous failure marker (30 s convention): a trip is expected within
    // ~100 ms, the timeout only fences a hang.
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

/// A `colony.json` whose watchdog period makes the colony beat once every
/// 30 s: the colony loop wakes for its heartbeat every HALF period (GH #1099,
/// `ColonyConfig::heartbeat_interval`), so an idle colony is silent for 30 s
/// at a time.
const SLOW_BEAT_COLONY_JSON: &[u8] = br#"{"watchdog_period_ms": 60000}"#;

/// GH #84 half 1: the watchdog tuning is reachable from `colony.json`.
///
/// GH #1099 changed what a quiet colony can prove. The colony loop now beats
/// every half `watchdog_period_ms`, so a deadline taken from `colony.json` puts
/// a beat into every window by construction: an idle colony can no longer trip
/// on its own configuration (the old fixed 100 ms beat made `3 x 20 ms` trip a
/// colony doing nothing -- the false alarm #1099 removed). The test therefore
/// pins the two places `colony.json` reaches, each on the code the production
/// path runs:
///
/// * the COLONY side end to end: the file is the only thing that slows the
///   beat. It goes through the real boot (`run_with_hooks_tuned`, the function
///   `run()` calls), and a supervisor whose 3 x 20 ms window sits far under the
///   30 s beat sees real silence from a real parked loop. Under the default
///   config the same loop beats every 50 ms, which
///   `a_watchdog_trip_ends_the_run_with_an_error_not_a_clean_exit` relies on.
/// * the SUPERVISOR side: with no override, `run_with_hooks_tuned` takes its
///   deadline from `WatchdogTuning::from_colony_config` over the parsed file;
///   the same parse is checked here field by field.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn colony_json_sets_the_watchdog_deadline_on_the_production_path() {
    let parsed = meclaw_colony::colony_config::ColonyConfig::parse_str(
        r#"{"watchdog_threshold": 3, "watchdog_period_ms": 20, "watchdog_on_trip": "log-only"}"#,
    )
    .expect("a valid colony.json parses");
    let tuning = WatchdogTuning::from_colony_config(&parsed);
    assert_eq!(
        tuning.threshold, 3,
        "watchdog_threshold must reach the supervisor"
    );
    assert_eq!(
        tuning.period,
        Duration::from_millis(20),
        "watchdog_period_ms must reach the supervisor"
    );
    assert!(
        tuning.on_trip == meclaw_cli::WatchdogOnTrip::LogOnly,
        "watchdog_on_trip must reach the supervisor"
    );

    let td = tempfile::TempDir::new().unwrap();
    let cli = cli_for(td.path());
    std::fs::write(td.path().join("colony.json"), SLOW_BEAT_COLONY_JSON).unwrap();
    let supervisor = WatchdogTuning {
        threshold: 3,
        period: Duration::from_millis(20),
        on_trip: meclaw_cli::WatchdogOnTrip::Exit,
    };

    let res = tokio::time::timeout(
        Duration::from_secs(30),
        run_with_hooks_tuned(cli, None, None, Some(supervisor)),
    )
    .await
    .expect("the colony.json beat must reach the colony, so the run must end on the trip");

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

/// Counts the `watchdog trip` events the CLI's trip reporter emits, split by
/// their `fatal` field. A process-wide subscriber, because the reporter runs on
/// a runtime worker, not on the test's thread.
mod trip_events {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing_subscriber::layer::SubscriberExt;

    pub static NON_FATAL: AtomicUsize = AtomicUsize::new(0);
    pub static FATAL: AtomicUsize = AtomicUsize::new(0);

    struct Counter;

    #[derive(Default)]
    struct Fields {
        is_trip: bool,
        fatal: Option<bool>,
    }

    impl tracing::field::Visit for Fields {
        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            if field.name() == "fatal" {
                self.fatal = Some(value);
            }
        }
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" && format!("{value:?}") == "watchdog trip" {
                self.is_trip = true;
            }
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Counter {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut f = Fields::default();
            event.record(&mut f);
            match (f.is_trip, f.fatal) {
                (true, Some(false)) => {
                    NON_FATAL.fetch_add(1, Ordering::SeqCst);
                }
                (true, Some(true)) => {
                    FATAL.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        }
    }

    /// Install once per process; later calls are no-ops.
    pub fn install() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let sub = tracing_subscriber::registry().with(Counter);
            tracing::subscriber::set_global_default(sub)
                .expect("no other global subscriber in this test binary");
        });
    }
}

/// GH #84 half 3: `on_trip: log-only` keeps the colony running.
///
/// The supervisor trips again and again — the receipt is positive: the CLI's
/// trip reporter emitted non-fatal `watchdog trip` events while the run was
/// alive, none of them fatal — and the process must survive all of it and still
/// end cleanly on its shutdown signal. The silence is real silence of a parked
/// loop: `colony.json` slows the colony's beat to one per 30 s (see
/// [`SLOW_BEAT_COLONY_JSON`]) under a 3 x 20 ms supervisor window.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn log_only_reports_the_trip_and_keeps_the_colony_running() {
    use std::sync::atomic::Ordering;
    trip_events::install();
    let td = tempfile::TempDir::new().unwrap();
    let cli = cli_for(td.path());
    std::fs::write(td.path().join("colony.json"), SLOW_BEAT_COLONY_JSON).unwrap();
    let supervisor = WatchdogTuning {
        threshold: 3,
        period: Duration::from_millis(20),
        on_trip: meclaw_cli::WatchdogOnTrip::LogOnly,
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let run = tokio::spawn(run_with_hooks_tuned(
        cli,
        None,
        Some(shutdown_rx),
        Some(supervisor),
    ));
    // Wait for the first reported trip (event-driven bound: 30 s marker), then
    // let more trip windows come and go under `log-only`.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while trip_events::NON_FATAL.load(Ordering::SeqCst) == 0 {
        assert!(
            !run.is_finished(),
            "log-only must keep the run alive while it trips"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "the supervisor must report a trip within the failure marker"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    shutdown_tx
        .send(())
        .expect("log-only must have kept the run alive");

    let res = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("the run must end on the shutdown hook, not hang")
        .expect("the run task must not panic");
    res.expect("under log-only a silence trip must not end the process");
    assert!(
        trip_events::NON_FATAL.load(Ordering::SeqCst) >= 2,
        "the supervisor must have kept tripping under log-only"
    );
    assert_eq!(
        trip_events::FATAL.load(Ordering::SeqCst),
        0,
        "no trip may be reported fatal under log-only"
    );
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
