//! GH #1176 — the post-state lane checks run on the colony worker, paced by
//! [`LanePacer`]: they yield and beat between units, and a unit that never
//! ends is still a stuck work item.

use super::*;
use crate::mutation::rejection::MutationRejection;
use crate::watchdog::{WorkItem, WorkPulse};
use std::time::Duration;

/// A supervisor with a 50 ms window (5 × 10 ms) and a 500 ms work-item
/// budget, and a pulse that reports to it -- the keep tests' harness.
async fn supervised() -> (
    WorkPulse,
    tokio::sync::mpsc::Receiver<crate::watchdog::WatchdogTrip>,
) {
    let (hb_tx, hb_rx) = tokio::sync::mpsc::channel(8);
    let (trip_tx, trip_rx) = tokio::sync::mpsc::channel(1024);
    let (armed_tx, armed_rx) = tokio::sync::oneshot::channel();
    let _ = armed_tx.send(());
    tokio::spawn(crate::watchdog::run_watchdog(
        hb_rx,
        trip_tx,
        5,
        Duration::from_millis(10),
        armed_rx,
        crate::watchdog::WatchdogOnTrip::Exit,
        None,
    ));
    let pulse = WorkPulse::new(Some(hb_tx), WorkItem::new("mutation test scope=/t"));
    tokio::time::sleep(Duration::from_millis(10)).await;
    pulse.tick();
    (pulse, trip_rx)
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
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_lane_check_under_lag_is_not_a_stuck_work_item() {
    let (pulse, mut trips) = supervised().await;
    let standing = contracts("c", 12, false);
    let newborn = contracts("n", 12, true);

    let mut got = MutationRejection::new();
    let lag = |_: &str| std::thread::sleep(Duration::from_millis(40));
    collect_post_state_lanes_paced(
        &standing,
        &newborn,
        &EdgeTable::new(),
        &mut got,
        &mut LanePacer::with_pace(&pulse, &lag),
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
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hung_lane_check_is_still_a_stuck_work_item() {
    let (pulse, mut trips) = supervised().await;
    let hang = Duration::from_millis(3_000);

    let check = tokio::spawn(async move {
        let started = std::time::Instant::now();
        let mut got = MutationRejection::new();
        let pace = move |hive: &str| {
            if hive == "/n1" {
                std::thread::sleep(hang);
            }
        };
        collect_post_state_lanes_paced(
            &contracts("c", 4, false),
            &contracts("n", 4, true),
            &EdgeTable::new(),
            &mut got,
            &mut LanePacer::with_pace(&pulse, &pace),
        )
        .await;
        (got, started.elapsed(), pulse)
    });

    let fatal = loop {
        let trip = tokio::time::timeout(Duration::from_secs(30), trips.recv())
            .await
            .expect("a hung check must be declared stuck")
            .expect("the supervisor returned without a fatal trip");
        if trip.is_fatal(crate::watchdog::WatchdogOnTrip::Exit) {
            break trip;
        }
    };
    assert!(
        !check.is_finished(),
        "the trip falls while the check hangs, not after it: {fatal}"
    );
    assert_eq!(fatal.starved(), "stuck_work_item", "was: {fatal}");
    assert!(
        fatal.to_string().contains("op=lane-doors"),
        "the trip names the lane check: {fatal}"
    );
    let (got, took, pulse) = check.await.unwrap();
    assert!(took >= hang, "{took:?}");
    assert_eq!(got.entries().len(), 4);
    drop(pulse);
}

/// GH #1176, rule 13: the check gives the worker back between units. On a
/// single-threaded runtime a task spawned before the check can only run if
/// the check yields; at 2 ms a unit every unit closes a 1 ms slice, so the
/// spawned task runs after the first one.
#[tokio::test]
async fn the_lane_check_yields_the_worker_between_units() {
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = ran.clone();
    tokio::spawn(async move {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let (r, s) = (ran.clone(), seen.clone());
    let pace = move |_: &str| {
        std::thread::sleep(Duration::from_millis(2));
        if r.load(std::sync::atomic::Ordering::SeqCst) {
            s.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    };
    let mut got = MutationRejection::new();
    collect_post_state_lanes_paced(
        &contracts("c", 2, false),
        &contracts("n", 2, true),
        &EdgeTable::new(),
        &mut got,
        &mut LanePacer::with_pace(&WorkPulse::silent(), &pace),
    )
    .await;

    assert!(
        seen.load(std::sync::atomic::Ordering::SeqCst),
        "another task on the same worker ran while the check was under way"
    );
    assert_eq!(got.entries().len(), 2);
}
