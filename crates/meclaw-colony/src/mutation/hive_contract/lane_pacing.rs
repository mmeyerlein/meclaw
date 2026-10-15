//! GH #1176 — the post-state lane checks run on the colony worker, paced by
//! [`LanePacer`]: they yield and beat between units, and a unit that never
//! ends is still a stuck work item.

use super::*;
use crate::mutation::rejection::MutationRejection;
use crate::watchdog::{WorkItem, WorkPulse};
use std::time::Duration;

/// A supervisor with a 50 ms window (5 × 10 ms) and a 500 ms work-item
/// budget, and a pulse that reports to it -- the keep tests' harness.
///
/// Both read the CPU denial through the seam, from a host that denies nothing
/// (GH #1199): read from the kernel, the machine's own denial was deducted
/// from the silence, up to four fifths of it -- under an 80 % quota on build01
/// the 3 s hang was declared stuck only after the check had returned (3 of 5).
///
/// The item enters work from inside the supervisor's first silent period:
/// the first reading of the denial ticks the pulse, after the arming drain,
/// so the next tick hears it, and the harness hands the pulse back only once
/// it has been said: the check's own words come after it. Ticked from the
/// test thread after a sleep, the
/// word raced the parked bar of 50 ms in real time, and under the same quota
/// the supervisor rightly judged a parked loop (`colony_loop`,
/// `beats_seen=0`, `armed_for=40ms`; 1 of 5) -- the watchdog tests' finding
/// (OR-V2.v2-fix-scan.C2.1), found while this harness still ran in real time.
async fn supervised() -> (
    WorkPulse,
    tokio::sync::mpsc::Receiver<crate::watchdog::WatchdogTrip>,
) {
    let (hb_tx, hb_rx) = tokio::sync::mpsc::channel(8);
    let (trip_tx, trip_rx) = tokio::sync::mpsc::channel(1024);
    let (armed_tx, armed_rx) = tokio::sync::oneshot::channel();
    let _ = armed_tx.send(());
    let pulse = WorkPulse::new(Some(hb_tx), WorkItem::new("mutation test scope=/t"));
    let (said_tx, said_rx) = tokio::sync::oneshot::channel::<()>();
    let unsaid = std::sync::Mutex::new(Some((pulse.clone(), said_tx)));
    let host: crate::watchdog::DenialSource = std::sync::Arc::new(move || {
        if let Some((first, said)) = unsaid.lock().unwrap().take() {
            first.tick();
            let _ = said.send(());
        }
        crate::watchdog::RunDelays::from_rows([(1, 0)])
    });
    let read = host.clone();
    tokio::spawn(crate::watchdog::run_watchdog_reading_denial(
        hb_rx,
        trip_tx,
        5,
        Duration::from_millis(10),
        armed_rx,
        crate::watchdog::WatchdogOnTrip::Exit,
        None,
        None,
        move || read(),
    ));
    meclaw_testing::deadline::hang("the supervisor's first silent period", said_rx)
        .await
        .expect("the supervisor reads the denial in its first silent period");
    (pulse.with_denial(host), trip_rx)
}

/// `n` contracts `/{prefix}0 … /{prefix}{n-1}`. With `required`, each one
/// insists on the lane `in_x`, which an empty edge table never delivers.
fn contracts(prefix: &str, n: usize, required: bool) -> Vec<HiveContract> {
    (0..n)
        .map(|i| HiveContract {
            hive_path: format!("/{prefix}{i}"),
            accepts: if required {
                vec![Lane {
                    route: "in_x".into(),
                    context: Vec::new(),
                    at: Vec::new(),
                    required: true,
                    because: format!("{prefix}{i} needs it"),
                }]
            } else {
                Vec::new()
            },
            emits: Vec::new(),
        })
        .collect()
}

/// What the two plain collectors find, in the order the post-state stage
/// runs them.
fn plain(standing: &[HiveContract], newborn: &[HiveContract]) -> MutationRejection {
    let mut r = MutationRejection::new();
    collect_lane_doors(standing, &EdgeTable::new(), &mut r);
    collect_required_lanes(newborn, &EdgeTable::new(), &mut r);
    r
}

/// GH #1176, the defect: 12 standing and 12 newborn contracts are 36 units
/// (a head each, a required lane per newborn one); at 40 ms a unit -- route
/// probes on a loaded host -- that is 1.44 s of honest work, almost three
/// times the 500 ms work-item budget. With no beat that is a fatal
/// `stuck_work_item`; paced, it is a check that makes progress, and it finds
/// exactly what the plain collectors find.
///
/// Each unit takes 40 ms of virtual time ([`UnitPace`]), so the supervisor
/// sees exactly that lag, tick by tick (welle-val2 v2-fix-scan, #1199: lifted
/// from a real-time test whose units slept on the worker thread).
#[tokio::test(start_paused = true)]
async fn a_long_lane_check_under_lag_is_not_a_stuck_work_item() {
    let (pulse, mut trips) = supervised().await;
    let standing = contracts("c", 12, false);
    let newborn = contracts("n", 12, true);

    let mut got = MutationRejection::new();
    let lag: &UnitPace = &|_| Box::pin(tokio::time::sleep(Duration::from_millis(40)));
    collect_post_state_lanes_paced(
        &standing,
        &newborn,
        &EdgeTable::new(),
        &mut got,
        &mut LanePacer::with_pace(&pulse, lag),
    )
    .await;

    while let Ok(trip) = trips.try_recv() {
        assert!(
            !trip.is_fatal(crate::watchdog::WatchdogOnTrip::Exit),
            "a lane check that makes progress must not end the process: {trip}"
        );
    }
    let plain = plain(&standing, &newborn);
    assert_eq!(plain.entries().len(), 12, "the fixture has twelve findings");
    assert_eq!(
        got, plain,
        "the paced check finds what the plain check finds"
    );
    drop(pulse);
}

/// GH #1176, the guard: the beats come between units, never from a timer, so
/// ONE contract whose check does not end leaves the work item silent and the
/// supervisor declares it stuck -- while it hangs, not afterwards.
///
/// The unit of `/n1` is held until the test has seen the fatal trip, so the
/// trip can only come from the silence INSIDE that unit: anything that beats
/// while it hangs leaves no fatal trip, and the wait for it ends at the
/// `HANG` marker of the virtual clock.
#[tokio::test(start_paused = true)]
async fn a_hung_lane_check_is_still_a_stuck_work_item() {
    let (pulse, mut trips) = supervised().await;
    // Every unit of `/n1` (its head and its lane) waits until the release.
    let (release, held) = tokio::sync::watch::channel(false);
    let pace: Box<UnitPace> = Box::new(move |hive: &str| {
        let mut held = held.clone();
        let hangs = hive == "/n1";
        Box::pin(async move {
            if hangs {
                let _ = held.wait_for(|released| *released).await;
            }
        })
    });

    let check = tokio::spawn(async move {
        let mut got = MutationRejection::new();
        collect_post_state_lanes_paced(
            &contracts("c", 4, false),
            &contracts("n", 4, true),
            &EdgeTable::new(),
            &mut got,
            &mut LanePacer::with_pace(&pulse, &*pace),
        )
        .await;
        (got, pulse)
    });

    let fatal =
        meclaw_testing::deadline::hang("a fatal trip while the check of /n1 hangs", async {
            loop {
                let trip = trips
                    .recv()
                    .await
                    .expect("the supervisor returned without a fatal trip");
                if trip.is_fatal(crate::watchdog::WatchdogOnTrip::Exit) {
                    return trip;
                }
            }
        })
        .await;
    assert!(
        !check.is_finished(),
        "the trip falls while the check hangs, not after it: {fatal}"
    );
    assert_eq!(fatal.starved(), "stuck_work_item", "was: {fatal}");
    assert_eq!(
        fatal.cpu_denied,
        Duration::ZERO,
        "the check is judged on a host that denies nothing -- the verdict of every machine: {fatal}"
    );
    assert!(
        fatal.to_string().contains("op=lane-doors"),
        "the trip names the lane check: {fatal}"
    );
    release.send(true).expect("the held units still listen");
    let (got, pulse) = meclaw_testing::deadline::hang("the released check ends", check)
        .await
        .unwrap();
    assert_eq!(got.entries().len(), 4);
    drop(pulse);
}

/// GH #1176, rule 13: the check gives the worker back between units. On a
/// single-threaded runtime a task spawned before the check can only run if
/// the check yields; every unit holds the worker for two slices without
/// yielding (a CPU stretch, as a route probe is), so every unit closes a
/// slice and the spawned task runs after the first one.
#[tokio::test]
async fn the_lane_check_yields_the_worker_between_units() {
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = ran.clone();
    tokio::spawn(async move {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let (r, s) = (ran.clone(), seen.clone());
    let pace: Box<UnitPace> = Box::new(move |_: &str| {
        let stretch = tokio::time::Instant::now();
        while stretch.elapsed() < 2 * LANE_CHECK_SLICE {
            std::hint::spin_loop();
        }
        if r.load(std::sync::atomic::Ordering::SeqCst) {
            s.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Box::pin(std::future::ready(()))
    });
    let mut got = MutationRejection::new();
    collect_post_state_lanes_paced(
        &contracts("c", 2, false),
        &contracts("n", 2, true),
        &EdgeTable::new(),
        &mut got,
        &mut LanePacer::with_pace(&WorkPulse::silent(), &*pace),
    )
    .await;

    assert!(
        seen.load(std::sync::atomic::Ordering::SeqCst),
        "another task on the same worker ran while the check was under way"
    );
    assert_eq!(got.entries().len(), 2);
}
