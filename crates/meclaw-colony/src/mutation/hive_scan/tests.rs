use super::*;
use crate::mutation::port_boundary::collect_sealed_hives;

/// `n` sealed hives `/h0 … /h{n-1}` under `root`.
fn sealed_hives(root: &std::path::Path, n: usize) -> Vec<Path> {
    std::fs::create_dir_all(root.join("main")).unwrap();
    std::fs::write(root.join("main/config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
    (0..n)
        .map(|i| {
            let dir = root.join(format!("main/h{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("config.json"),
                r#"{"cell":{"type":"hive"},"params":{"ports":["gate"],"required_drains":[]}}"#,
            )
            .unwrap();
            Path::new(&format!("/h{i}"))
        })
        .collect()
}

/// A host that keeps `share` of every nanosecond of one thread off the CPU,
/// on the tokio clock (GH #1199) -- the watchdog tests' `denied`, as ONE
/// source the supervisor and the pulse share. `denied(0.0)` is a host that
/// denies nothing: the verdicts of before GH #1216, whatever the kernel of
/// the machine running the test does meanwhile.
fn denied(share: f64) -> crate::watchdog::DenialSource {
    let since = tokio::time::Instant::now();
    std::sync::Arc::new(move || {
        let ns = since.elapsed().as_nanos() as f64 * share;
        crate::watchdog::RunDelays::from_rows([(1, ns as u64)])
    })
}

/// A supervisor with a `threshold` × `period` window and a work-item budget
/// of ten windows, and a pulse that reports to it and knows that budget, on a
/// host that denies nothing.
async fn supervised_with(
    threshold: u32,
    period: Duration,
) -> (
    WorkPulse,
    tokio::sync::mpsc::Receiver<crate::watchdog::WatchdogTrip>,
) {
    supervised_on(threshold, period, denied(0.0)).await
}

/// [`supervised_with`] on `host`: the supervisor and the pulse read the CPU
/// denial from the same source.
async fn supervised_on(
    threshold: u32,
    period: Duration,
    host: crate::watchdog::DenialSource,
) -> (
    WorkPulse,
    tokio::sync::mpsc::Receiver<crate::watchdog::WatchdogTrip>,
) {
    let (hb_tx, hb_rx) = tokio::sync::mpsc::channel(8);
    let (trip_tx, trip_rx) = tokio::sync::mpsc::channel(1024);
    let (armed_tx, armed_rx) = tokio::sync::oneshot::channel();
    let _ = armed_tx.send(());
    let read = host.clone();
    tokio::spawn(crate::watchdog::run_watchdog_reading_denial(
        hb_rx,
        trip_tx,
        threshold,
        period,
        armed_rx,
        crate::watchdog::WatchdogOnTrip::Exit,
        None,
        None,
        move || read(),
    ));
    let budget = period * threshold * crate::watchdog::WORK_ITEM_BUDGET_FACTOR;
    let pulse = WorkPulse::new(Some(hb_tx), WorkItem::new("mutation test scope=/t"))
        .with_budget(budget)
        .with_denial(host);
    tokio::time::sleep(period).await;
    pulse.tick();
    (pulse, trip_rx)
}

/// The keep tests' harness: a 50 ms window (5 × 10 ms), a 500 ms budget.
async fn supervised() -> (
    WorkPulse,
    tokio::sync::mpsc::Receiver<crate::watchdog::WatchdogTrip>,
) {
    supervised_with(5, Duration::from_millis(10)).await
}

/// The first fatal trip. No deadline of its own: it runs under [`run_clock`],
/// whose hang marker ends a supervisor that never declares the hang.
async fn first_fatal(
    trips: &mut tokio::sync::mpsc::Receiver<crate::watchdog::WatchdogTrip>,
) -> crate::watchdog::WatchdogTrip {
    loop {
        let trip = trips
            .recv()
            .await
            .expect("the supervisor returned without a fatal trip");
        if trip.is_fatal(crate::watchdog::WatchdogOnTrip::Exit) {
            return trip;
        }
    }
}

/// The step of the virtual clock while a read is held.
const STEP: Duration = Duration::from_millis(1);

/// Reads the test holds (welle-val2 v2-fix-b, #1199): the pace of a held
/// config reports that its read has begun, then blocks until the test lets it
/// go -- a read as slow as the test says, never a guessed sleep. Dropping the
/// [`Held`] lets every held read go, so a failed test does not keep the
/// runtime's blocking pool.
///
/// Virtual time (AGENTS.md rule 11, time logic): every wait of a scan is
/// `spawn_blocking`, a tokio channel or a tokio timer, and a running blocking
/// task holds the paused clock -- honest reads cost no virtual time at all.
/// While a read is held the clock moves only by [`advance_beside`] and
/// [`run_clock`], so a read timeout or a supervisor window runs out exactly.
struct Held {
    begun: tokio::sync::mpsc::UnboundedReceiver<String>,
    release: Option<std::sync::mpsc::Sender<()>>,
}

/// A pace that holds every read `which` names, and the test's side of it.
fn hold(
    which: impl Fn(&str) -> bool + Send + Sync + 'static,
) -> (impl Fn(&str) + Send + Sync + 'static, Held) {
    let (begun_tx, begun) = tokio::sync::mpsc::unbounded_channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let released = std::sync::Mutex::new(released);
    let pace = move |hive: &str| {
        if which(hive) {
            let _ = begun_tx.send(hive.to_string());
            // `Err`: the test has let go of every read.
            let _ = released.lock().unwrap().recv();
        }
    };
    let held = Held {
        begun,
        release: Some(release),
    };
    (pace, held)
}

impl Held {
    /// The next held read that has begun, for a scan another task drives.
    async fn begun(&mut self) -> String {
        self.begun
            .recv()
            .await
            .expect("the scan ended before the held read began")
    }

    /// The next held read that has begun, driving `scan` meanwhile.
    async fn begun_under<F: std::future::Future + Unpin>(&mut self, scan: &mut F) -> String {
        tokio::select! {
            biased;
            _ = scan => panic!("the scan ended before the held read began"),
            hive = self.begun.recv() => hive.expect("the pace lives as long as the scan"),
        }
    }

    /// Lets the one read that is held now go.
    fn release_one(&self) {
        if let Some(tx) = &self.release {
            tx.send(()).expect("a held read waits for its release");
        }
    }

    /// Lets every held read go, now and from here on.
    fn release_all(&mut self) {
        self.release = None;
    }
}

/// Lets `by` of virtual time pass in [`STEP`]s while `scan` runs beside it --
/// a held read that takes `by`. The scan must not end meanwhile.
async fn advance_beside<F: std::future::Future + Unpin>(scan: &mut F, by: Duration) {
    let mut ran = Duration::ZERO;
    while ran < by {
        tokio::select! {
            biased;
            _ = &mut *scan => panic!("the scan ended while a read was held"),
            () = std::future::ready(()) => {}
        }
        tokio::time::advance(STEP).await;
        ran += STEP;
    }
}

/// Drives `fut` on the paused clock in [`STEP`]s while a read is held. A
/// future still pending after [`meclaw_testing::deadline::HANG`] of virtual
/// time is a hang, never a verdict on speed.
async fn run_clock<F: std::future::Future>(what: &str, fut: F) -> F::Output {
    let mut ran = Duration::ZERO;
    tokio::pin!(fut);
    loop {
        tokio::select! {
            biased;
            out = &mut fut => return out,
            () = std::future::ready(()) => {}
        }
        assert!(
            ran < meclaw_testing::deadline::HANG,
            "HANG: {what} (virtual {ran:?})"
        );
        tokio::time::advance(STEP).await;
        ran += STEP;
    }
}

/// GH #1176, the defect: 24 hives at 40 ms a read -- a contended disk, each
/// read shorter than one window -- are 960 ms of honest work, almost twice
/// the work-item budget. On the worker with no beat that is a fatal
/// `stuck_work_item`; paced, it is a scan that makes progress.
///
/// Each read is held for 40 ms of virtual time ([`Held`]), so the supervisor
/// sees exactly that lag, tick by tick.
#[tokio::test(start_paused = true)]
async fn a_long_hive_scan_under_lag_is_not_a_stuck_work_item() {
    let td = tempfile::TempDir::new().unwrap();
    let paths = sealed_hives(td.path(), 24);
    let (pulse, mut trips) = supervised().await;
    let (pace, mut held) = hold(|_| true);
    let mut gate = HiveScanGate::default();

    let got = {
        let scan = scan_hives_paced(
            &mut gate,
            td.path(),
            paths.clone(),
            &pulse,
            "hive-scan",
            HIVE_READ_TIMEOUT,
            pace,
            |scan| collect_sealed_hives(scan.root(), scan.paths()),
        );
        tokio::pin!(scan);
        for i in 0..24 {
            assert_eq!(held.begun_under(&mut scan).await, format!("/h{i}"));
            advance_beside(&mut scan, Duration::from_millis(40)).await;
            held.release_one();
        }
        scan.await
    }
    .expect("a scan that makes progress does not time out");

    while let Ok(trip) = trips.try_recv() {
        assert!(
            !trip.is_fatal(crate::watchdog::WatchdogOnTrip::Exit),
            "a hive scan that makes progress must not end the process: {trip}"
        );
    }
    assert_eq!(
        got,
        collect_sealed_hives(td.path(), paths.iter()),
        "the paced scan reads exactly what the plain reader reads"
    );
    drop(pulse);
}

/// GH #1176, the guard: the beats are per hive and come before the read, so
/// a read that never returns is still a stuck work item -- fatal under
/// `exit`, and declared WHILE it hangs. A timer that beats while the scan
/// runs would hide it.
#[tokio::test(start_paused = true)]
async fn a_hung_hive_read_is_still_a_stuck_work_item() {
    let td = tempfile::TempDir::new().unwrap();
    let paths = sealed_hives(td.path(), 4);
    let (pulse, mut trips) = supervised().await;
    let root = td.path().to_path_buf();
    let (pace, mut held) = hold(|hive| hive == "/h1");

    let scan = tokio::spawn(async move {
        let got = scan_hives_paced(
            &mut HiveScanGate::default(),
            &root,
            paths,
            &pulse,
            "hive-scan",
            HIVE_READ_TIMEOUT,
            pace,
            |scan| collect_sealed_hives(scan.root(), scan.paths()),
        )
        .await;
        (got, pulse)
    });

    assert_eq!(held.begun().await, "/h1");
    let fatal = run_clock(
        "a fatal trip while the read of /h1 hangs",
        first_fatal(&mut trips),
    )
    .await;
    assert!(
        !scan.is_finished(),
        "the trip falls while the read hangs, not after it: {fatal}"
    );
    assert_eq!(fatal.starved(), "stuck_work_item", "was: {fatal}");
    assert!(
        fatal.to_string().contains("hive=/h1"),
        "the trip names the hive whose read hung: {fatal}"
    );
    held.release_all();
    let (got, pulse) = scan.await.unwrap();
    assert_eq!(got.unwrap().len(), 4);
    drop(pulse);
}

/// GH #1176, rule 12: a read that does not return within the read timeout
/// ends the scan with a clean error naming the hive -- the caller refuses
/// the mutation instead of holding the colony loop.
#[tokio::test(start_paused = true)]
async fn a_hung_hive_read_ends_in_a_clean_timeout() {
    let td = tempfile::TempDir::new().unwrap();
    let paths = sealed_hives(td.path(), 4);
    let (pace, mut held) = hold(|hive| hive == "/h2");
    let mut gate = HiveScanGate::default();
    let started = tokio::time::Instant::now();

    let silent = WorkPulse::silent();
    let scan = scan_hives_paced(
        &mut gate,
        td.path(),
        paths,
        &silent,
        "post-state",
        Duration::from_millis(100),
        pace,
        |scan| collect_sealed_hives(scan.root(), scan.paths()),
    );
    tokio::pin!(scan);
    assert_eq!(held.begun_under(&mut scan).await, "/h2");
    let got = run_clock("the read timeout of the hung read of /h2", scan).await;

    let err = got.expect_err("a read past the timeout ends the scan");
    assert_eq!(err.hive.as_deref(), Some("/h2"));
    assert_eq!(err.what, "post-state");
    assert_eq!(err.behind, None);
    assert_eq!(
        started.elapsed(),
        Duration::from_millis(100),
        "the timeout returns after exactly the read timeout, while the read still hangs"
    );
    assert!(err.message().contains("/h2"), "{}", err.message());
    held.release_all();
}

/// GH #1176, single-flight: while a scan an earlier caller gave up on still
/// hangs, the next scan of the same colony does not start a second read. It
/// waits its own timeout for the stuck one and ends with the same timeout,
/// naming the scan and the hive it is behind; once the stuck read returns,
/// scans run again.
#[tokio::test(start_paused = true)]
async fn a_second_scan_waits_for_a_hung_one_and_never_runs_beside_it() {
    let td = tempfile::TempDir::new().unwrap();
    let paths = sealed_hives(td.path(), 4);
    let mut gate = HiveScanGate::default();
    let (pace, mut held) = hold(|hive| hive == "/h2");

    let first = {
        let silent = WorkPulse::silent();
        let scan = scan_hives_paced(
            &mut gate,
            td.path(),
            paths.clone(),
            &silent,
            "hive-scan",
            Duration::from_millis(100),
            pace,
            |scan| collect_sealed_hives(scan.root(), scan.paths()),
        );
        tokio::pin!(scan);
        assert_eq!(held.begun_under(&mut scan).await, "/h2");
        run_clock("the first scan's read timeout on /h2", scan).await
    };
    let first = first.expect_err("the first scan times out on /h2");
    assert_eq!(first.hive.as_deref(), Some("/h2"));

    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = reads.clone();
    let started = tokio::time::Instant::now();
    let second = run_clock(
        "the second scan's wait behind the stuck one",
        scan_hives_paced(
            &mut gate,
            td.path(),
            paths.clone(),
            &WorkPulse::silent(),
            "post-state",
            Duration::from_millis(100),
            move |_| {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
            |scan| collect_sealed_hives(scan.root(), scan.paths()),
        ),
    )
    .await;
    let err = second.expect_err("a scan behind a stuck one ends with the timeout");
    assert_eq!(
        started.elapsed(),
        Duration::from_millis(100),
        "it waits its own timeout for the stuck one, exactly"
    );
    assert_eq!(err.behind, Some("hive-scan"), "{err:?}");
    assert_eq!(err.hive.as_deref(), Some("/h2"));
    assert_eq!(err.what, "post-state");
    assert!(
        err.message().contains("still reading '/h2'"),
        "{}",
        err.message()
    );
    assert_eq!(
        reads.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the second scan read nothing beside the stuck one"
    );

    // The stuck read returns; the next scan waits for its walk to end, then runs.
    held.release_all();
    let third = scan_hives_paced(
        &mut gate,
        td.path(),
        paths.clone(),
        &WorkPulse::silent(),
        "slot-rebuild",
        Duration::from_secs(5),
        |_| {},
        |scan| collect_sealed_hives(scan.root(), scan.paths()),
    )
    .await
    .expect("once the stuck read returns, the next scan runs");
    assert_eq!(third.len(), 4);
}

/// GH #1176, the timeout follows the watchdog (rule 12 under the supervisor):
/// it is twice the work-item budget of whatever supervisor the pulse reports
/// to, so a hung read is declared stuck under `exit` BEFORE the scan gives up
/// -- here with a 60 ms window (3 × 20 ms), 600 ms budget, 1.2 s timeout.
#[tokio::test(start_paused = true)]
async fn under_any_watchdog_a_hung_read_trips_before_the_scan_times_out() {
    let td = tempfile::TempDir::new().unwrap();
    let paths = sealed_hives(td.path(), 4);
    let (pulse, mut trips) = supervised_with(3, Duration::from_millis(20)).await;
    assert_eq!(scan_timeout(&pulse), Duration::from_millis(1_200));
    assert_eq!(scan_timeout(&WorkPulse::silent()), HIVE_READ_TIMEOUT);
    let root = td.path().to_path_buf();
    let (pace, mut held) = hold(|hive| hive == "/h1");

    let scan = tokio::spawn(async move {
        let timeout = scan_timeout(&pulse);
        let got = scan_hives_paced(
            &mut HiveScanGate::default(),
            &root,
            paths,
            &pulse,
            "hive-scan",
            timeout,
            pace,
            |scan| collect_sealed_hives(scan.root(), scan.paths()),
        )
        .await;
        (got, pulse)
    });

    assert_eq!(held.begun().await, "/h1");
    let fatal = run_clock(
        "a fatal trip while the read of /h1 hangs",
        first_fatal(&mut trips),
    )
    .await;
    assert!(
        !scan.is_finished(),
        "the supervisor declares the hang before the scan's own timeout: {fatal}"
    );
    assert_eq!(fatal.starved(), "stuck_work_item", "was: {fatal}");
    let (got, pulse) = run_clock("the scan's own timeout on the hung read", scan)
        .await
        .unwrap();
    assert_eq!(
        got.expect_err("then the timeout ends it").hive.as_deref(),
        Some("/h1")
    );
    held.release_all();
    drop(pulse);
}

/// GH #1199 (K10), pin a: under a host that keeps the whole silence off the
/// CPU the supervisor deducts that denial (GH #1216) and declares the hung
/// read stuck only after up to five budgets -- and the scan timeout credits
/// the SAME denial, read from the same source, so the verdict still comes
/// first. The timeout grows by the denial factor and no further: 5 × 1.2 s.
#[tokio::test(start_paused = true)]
async fn under_a_host_denying_the_whole_silence_a_hung_read_trips_before_the_scan_times_out() {
    let td = tempfile::TempDir::new().unwrap();
    let paths = sealed_hives(td.path(), 4);
    let (pulse, mut trips) = supervised_on(3, Duration::from_millis(20), denied(1.0)).await;
    let root = td.path().to_path_buf();
    let (pace, mut held) = hold(|hive| hive == "/h1");

    let scan = tokio::spawn(async move {
        let timeout = scan_timeout(&pulse);
        let got = scan_hives_paced(
            &mut HiveScanGate::default(),
            &root,
            paths,
            &pulse,
            "hive-scan",
            timeout,
            pace,
            |scan| collect_sealed_hives(scan.root(), scan.paths()),
        )
        .await;
        (got, pulse)
    });

    assert_eq!(held.begun().await, "/h1");
    let fatal = run_clock(
        "a fatal trip while the read of /h1 hangs on a denying host",
        first_fatal(&mut trips),
    )
    .await;
    assert!(
        !scan.is_finished(),
        "the supervisor declares the hang before the scan's own timeout: {fatal}"
    );
    assert_eq!(fatal.starved(), "stuck_work_item", "was: {fatal}");
    let (got, pulse) = run_clock("the scan's own timeout on the hung read", scan)
        .await
        .unwrap();
    let err = got.expect_err("then the timeout ends it");
    assert_eq!(err.hive.as_deref(), Some("/h1"));
    assert_eq!(
        err.after,
        Duration::from_millis(6_000),
        "the timeout credits the denial up to the factor of the supervisor"
    );
    held.release_all();
    drop(pulse);
}

/// GH #1199 (K10), pin b: on a host that denies nothing the scan timeout is
/// what it was -- twice the budget, to the tick, and the scan ends there.
#[tokio::test(start_paused = true)]
async fn on_a_host_denying_nothing_the_scan_times_out_at_twice_the_budget() {
    let td = tempfile::TempDir::new().unwrap();
    let paths = sealed_hives(td.path(), 4);
    let (pulse, _trips) = supervised_on(3, Duration::from_millis(20), denied(0.0)).await;
    assert_eq!(scan_timeout(&pulse), Duration::from_millis(1_200));
    let (pace, mut held) = hold(|hive| hive == "/h2");
    let mut gate = HiveScanGate::default();
    let started = tokio::time::Instant::now();

    let scan = scan_hives_paced(
        &mut gate,
        td.path(),
        paths,
        &pulse,
        "post-state",
        scan_timeout(&pulse),
        pace,
        |scan| collect_sealed_hives(scan.root(), scan.paths()),
    );
    tokio::pin!(scan);
    assert_eq!(held.begun_under(&mut scan).await, "/h2");
    let got = run_clock("the read timeout of the hung read of /h2", scan).await;

    let err = got.expect_err("a read past the timeout ends the scan");
    assert_eq!(err.hive.as_deref(), Some("/h2"));
    assert_eq!(err.after, Duration::from_millis(1_200));
    assert_eq!(
        started.elapsed(),
        Duration::from_millis(1_200),
        "without a denial the scan waits exactly twice the budget"
    );
    held.release_all();
}

/// A first scan, on no supervisor, given up on after 100 ms while its read of
/// `/h2` hangs: `gate` keeps it in flight, the returned [`Held`] keeps the read.
async fn give_up_on_a_hung_scan(
    gate: &mut HiveScanGate,
    root: &std::path::Path,
    paths: Vec<Path>,
) -> Held {
    let (pace, mut held) = hold(|hive| hive == "/h2");
    let silent = WorkPulse::silent();
    let scan = scan_hives_paced(
        gate,
        root,
        paths,
        &silent,
        "hive-scan",
        Duration::from_millis(100),
        pace,
        |scan| collect_sealed_hives(scan.root(), scan.paths()),
    );
    tokio::pin!(scan);
    assert_eq!(held.begun_under(&mut scan).await, "/h2");
    let first = run_clock("the first scan's read timeout on /h2", scan)
        .await
        .expect_err("the first scan times out on /h2");
    assert_eq!(first.hive.as_deref(), Some("/h2"));
    held
}

/// GH #1199 (K10), pin c (re-check I1): the wait BEHIND a scan an earlier
/// caller gave up on credits the same denial as a read of its own. Under a
/// host that keeps the whole silence off the CPU, a second scan behind a hung
/// one is declared stuck by the supervisor while it still waits, and gives up
/// at the denial factor and no later: 5 × 1.2 s.
#[tokio::test(start_paused = true)]
async fn under_a_host_denying_the_whole_silence_a_scan_behind_a_hung_one_trips_before_it_times_out()
{
    let td = tempfile::TempDir::new().unwrap();
    let paths = sealed_hives(td.path(), 4);
    let (pulse, mut trips) = supervised_on(3, Duration::from_millis(20), denied(1.0)).await;
    let mut gate = HiveScanGate::default();
    let mut held = give_up_on_a_hung_scan(&mut gate, td.path(), paths.clone()).await;
    let root = td.path().to_path_buf();

    let scan = tokio::spawn(async move {
        let timeout = scan_timeout(&pulse);
        let got = scan_hives_paced(
            &mut gate,
            &root,
            paths,
            &pulse,
            "post-state",
            timeout,
            |_| {},
            |scan| collect_sealed_hives(scan.root(), scan.paths()),
        )
        .await;
        (got, pulse)
    });

    let fatal = run_clock(
        "a fatal trip while the second scan waits behind the hung read of /h2",
        first_fatal(&mut trips),
    )
    .await;
    assert!(
        !scan.is_finished(),
        "the supervisor declares the hang before the wait behind it times out: {fatal}"
    );
    assert_eq!(fatal.starved(), "stuck_work_item", "was: {fatal}");
    let (got, pulse) = run_clock("the second scan's wait behind the hung read", scan)
        .await
        .unwrap();
    let err = got.expect_err("a scan behind a stuck one ends with the timeout");
    assert_eq!(err.behind, Some("hive-scan"), "{err:?}");
    assert_eq!(err.hive.as_deref(), Some("/h2"));
    assert_eq!(
        err.after,
        Duration::from_millis(6_000),
        "the wait behind credits the denial up to the factor of the supervisor"
    );
    held.release_all();
    drop(pulse);
}

/// GH #1199 (K10), pin d (re-check I1): on a host that denies nothing the
/// wait behind a hung scan is what it was -- the scan's own timeout, twice the
/// budget, to the tick.
#[tokio::test(start_paused = true)]
async fn on_a_host_denying_nothing_a_scan_behind_a_hung_one_waits_exactly_twice_the_budget() {
    let td = tempfile::TempDir::new().unwrap();
    let paths = sealed_hives(td.path(), 4);
    let (pulse, _trips) = supervised_on(3, Duration::from_millis(20), denied(0.0)).await;
    assert_eq!(scan_timeout(&pulse), Duration::from_millis(1_200));
    let mut gate = HiveScanGate::default();
    let mut held = give_up_on_a_hung_scan(&mut gate, td.path(), paths.clone()).await;

    let started = tokio::time::Instant::now();
    let got = run_clock(
        "the second scan's wait behind the hung read of /h2",
        scan_hives_paced(
            &mut gate,
            td.path(),
            paths,
            &pulse,
            "post-state",
            scan_timeout(&pulse),
            |_| {},
            |scan| collect_sealed_hives(scan.root(), scan.paths()),
        ),
    )
    .await;

    let err = got.expect_err("a scan behind a stuck one ends with the timeout");
    assert_eq!(err.behind, Some("hive-scan"), "{err:?}");
    assert_eq!(err.hive.as_deref(), Some("/h2"));
    assert_eq!(err.after, Duration::from_millis(1_200));
    assert_eq!(
        started.elapsed(),
        Duration::from_millis(1_200),
        "without a denial the wait behind lasts exactly twice the budget"
    );
    held.release_all();
}

fn timed_out(hive: &str) -> Result<(), HiveScanTimeout> {
    Err(HiveScanTimeout {
        what: "hive-scan",
        hive: Some(hive.to_string()),
        after: Duration::from_millis(400),
        behind: None,
    })
}

/// GH #1176 (M1), validation and post-state: a timed-out scan refuses the
/// mutation with `hive_config_timeout` naming the hive, and hands the caller
/// nothing to judge with; a scan that came back passes its value through and
/// refuses nothing.
#[test]
fn a_timed_out_scan_refuses_the_mutation_it_was_judging() {
    use crate::mutation::rejection::{MutationRejection, Stage};

    let mut rejection = MutationRejection::new();
    let got = read_or_refuse(timed_out("/h1"), Stage::RequiredDrains, &mut rejection);
    assert!(got.is_none(), "nothing to judge with after a timeout");
    assert_eq!(
        rejection.error_code(),
        Some("hive_config_timeout"),
        "the timeout refuses the diff (fail-closed): {}",
        rejection.render()
    );
    assert!(
        rejection.render().contains("/h1"),
        "the refusal names the hive whose read hung: {}",
        rejection.render()
    );

    let mut clean = MutationRejection::new();
    assert_eq!(
        read_or_refuse(Ok(7_u8), Stage::ContractLocality, &mut clean),
        Some(7)
    );
    assert_eq!(
        clean.error_code(),
        None,
        "a scan that came back refuses nothing"
    );
}

/// GH #1176 (M1), slot rebuild: a timed-out rebuild leaves the dirty flag set
/// and hands no tables (the caller closes the hive boundary, fail-closed, and a
/// later pass reads again); a rebuild that came back clears it.
#[test]
fn a_timed_out_slot_rebuild_hands_no_tables_and_stays_dirty() {
    let mut dirty = true;
    assert!(rebuilt_or_kept(timed_out("/h1"), &mut dirty).is_err());
    assert!(dirty, "the flag stays set, so the next pass reads again");

    assert_eq!(rebuilt_or_kept(Ok(3_u8), &mut dirty).ok(), Some(3));
    assert!(!dirty, "a rebuild that came back clears the flag");
}

/// A template tree `<library>/pair`: a hive root with two echo children.
fn template_tree(library: &std::path::Path) -> std::path::PathBuf {
    let root = library.join("pair");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    for child in ["a", "b"] {
        std::fs::create_dir_all(root.join(child)).unwrap();
        std::fs::write(
            root.join(child).join("config.json"),
            r#"{"cell":{"type":"echo"},"params":{}}"#,
        )
        .unwrap();
    }
    root
}

/// GH #1193: the template subtree reads of an `add_nodes` entry run in a
/// config scan, each `config.json` announced before it is touched -- so a
/// read that does not come back ends in a clean timeout that names the file,
/// instead of holding the colony worker with no beat. Without the
/// announcement the scan never hears of the hung read and returns whenever
/// the disk does.
#[tokio::test(start_paused = true)]
async fn a_hung_template_subtree_read_ends_in_a_clean_timeout_naming_the_file() {
    let td = tempfile::TempDir::new().unwrap();
    let library = td.path().join("templates");
    let root = template_tree(&library);
    let (pace, mut held) = hold(|file| file == "template:pair/a/config.json");
    let mut gate = HiveScanGate::default();
    let started = tokio::time::Instant::now();

    let shown_under = library.clone();
    let silent = WorkPulse::silent();
    let scan = scan_hives_paced(
        &mut gate,
        td.path(),
        Vec::new(),
        &silent,
        "template-scan",
        Duration::from_millis(100),
        pace,
        move |scan| {
            crate::mutation::subtree::read_subtrees(
                &[root],
                &crate::templates::TemplatesRegistry::default(),
                &|p: &std::path::Path| {
                    let shown = p.strip_prefix(&shown_under).unwrap_or(p);
                    scan.step(&format!("template:{}", shown.display()))
                },
            )
        },
    );
    tokio::pin!(scan);
    assert_eq!(
        held.begun_under(&mut scan).await,
        "template:pair/a/config.json"
    );
    let got = run_clock("the read timeout of the hung template read", scan).await;

    let err = got.expect_err("a template read past the timeout ends the scan");
    assert_eq!(err.what, "template-scan");
    assert_eq!(err.hive.as_deref(), Some("template:pair/a/config.json"));
    assert_eq!(
        started.elapsed(),
        Duration::from_millis(100),
        "the timeout returns after exactly the read timeout, while the read still hangs"
    );
    held.release_all();
}

/// GH #1193: a tree the scan parsed is kept on the mutation's snapshot, and
/// every later `parse_subtree` of it (stages 2-4, the subtree pre-checks, the
/// contracts of newborn hives, staging) answers from memory -- not with a
/// second synchronous walk on the worker. Pinned by breaking the file after
/// the scan: the kept parse still answers, a fresh snapshot sees the break.
#[test]
fn a_kept_template_parse_answers_without_reading_the_tree_again() {
    let td = tempfile::TempDir::new().unwrap();
    let root = template_tree(&td.path().join("templates"));
    let mut snapshot = crate::templates::TemplatesRegistry::default();
    let announced = std::cell::RefCell::new(Vec::<std::path::PathBuf>::new());
    let parsed = crate::mutation::subtree::read_subtrees(
        std::slice::from_ref(&root),
        &snapshot,
        &|p: &std::path::Path| announced.borrow_mut().push(p.to_path_buf()),
    );
    let announced = announced.into_inner();
    for rel in ["config.json", "a/config.json", "b/config.json"] {
        assert!(
            announced.contains(&root.join(rel)),
            "every config.json of the tree is announced before its read, {rel} was not: \
             {announced:?}"
        );
    }
    snapshot.keep_parsed_subtrees(parsed);

    std::fs::write(root.join("a/config.json"), "not json").unwrap();

    let kept = crate::mutation::subtree::parse_subtree(&root, &snapshot)
        .expect("the kept parse answers without reading the broken file");
    assert_eq!(kept.cells.len(), 3, "{kept:?}");
    assert!(
        crate::mutation::subtree::parse_subtree(
            &root,
            &crate::templates::TemplatesRegistry::default()
        )
        .is_err(),
        "a snapshot without the kept parse reads the tree and sees the break"
    );
}

/// GH #1176 re-check (u1 L2 / u2 L2): while a scan an earlier caller gave up
/// on still hangs, `still_stuck` answers at once with the timeout naming that
/// scan and its hive -- the slot rebuild asks it and does not wait behind the
/// hang for another read timeout. Once the read has returned, it is cleared.
#[tokio::test(start_paused = true)]
async fn a_pass_that_must_not_wait_learns_of_a_hung_scan_at_once() {
    let td = tempfile::TempDir::new().unwrap();
    let paths = sealed_hives(td.path(), 3);
    let mut gate = HiveScanGate::default();
    let (pace, mut held) = hold(|hive| hive == "/h1");
    let first = {
        let silent = WorkPulse::silent();
        let scan = scan_hives_paced(
            &mut gate,
            td.path(),
            paths,
            &silent,
            "hive-scan",
            Duration::from_millis(100),
            pace,
            |scan| collect_sealed_hives(scan.root(), scan.paths()),
        );
        tokio::pin!(scan);
        assert_eq!(held.begun_under(&mut scan).await, "/h1");
        run_clock("the first scan's read timeout on /h1", scan).await
    };
    let first = first.expect_err("the first scan times out on /h1");
    assert_eq!(first.hive.as_deref(), Some("/h1"));

    // The read of /h1 is still held: an answer now is one that did not wait.
    let stuck = gate
        .still_stuck("slot-rebuild")
        .expect("the hung scan is still in flight");
    assert_eq!(
        stuck.after,
        Duration::ZERO,
        "asking does not wait: {stuck:?}"
    );
    assert_eq!(stuck.what, "slot-rebuild");
    assert_eq!(stuck.behind, Some("hive-scan"));
    assert_eq!(stuck.hive.as_deref(), Some("/h1"));

    held.release_all();
    let mut hung = gate.in_flight.take().expect("the gate holds the hung scan");
    (&mut hung.task).await.unwrap();
    gate.in_flight = Some(hung);
    assert!(
        gate.still_stuck("slot-rebuild").is_none(),
        "a scan that has ended is no reason to refuse"
    );
    assert!(gate.in_flight.is_none(), "and it is cleared");
}

/// GH #1176 re-check (u2 L3): a scan the async side gave up on stops walking
/// -- the walk ends at the first announcement nobody hears, it does not read
/// the remaining configs for a result nobody takes.
#[tokio::test(start_paused = true)]
async fn an_abandoned_scan_reads_no_further_config() {
    let td = tempfile::TempDir::new().unwrap();
    let paths = sealed_hives(td.path(), 4);
    let mut gate = HiveScanGate::default();
    let (pace, mut held) = hold(|hive| hive == "/h1");
    let (seen_tx, seen_rx) = std::sync::mpsc::channel::<String>();
    let got = {
        let silent = WorkPulse::silent();
        let scan = scan_hives_paced(
            &mut gate,
            td.path(),
            paths,
            &silent,
            "hive-scan",
            Duration::from_millis(100),
            pace,
            move |scan| {
                for p in scan.paths() {
                    let _ = seen_tx.send(p.as_str().to_string());
                }
            },
        );
        tokio::pin!(scan);
        assert_eq!(held.begun_under(&mut scan).await, "/h1");
        run_clock("the read timeout on /h1", scan).await
    };
    got.expect_err("the scan times out on /h1");
    held.release_all();
    let abandoned = gate.in_flight.take().expect("the gate holds the scan");
    abandoned.task.await.unwrap();
    let seen: Vec<String> = seen_rx.try_iter().collect();
    assert_eq!(
        seen,
        vec!["/h0".to_string(), "/h1".to_string()],
        "after the timeout the walk ends at the next announcement"
    );
}
