//! GH #1372 -- the watchdog's witness of the step that stalls the colony loop.
//!
//! Measured: under host load (one 24-thread lane, 7 of 17 lab runs) the colony
//! was ended by trips that all read `starved=colony_loop in_flight_work=false
//! cpu_denied=0 witness=kept` -- a silence the trip could not name. Two things
//! were missing from the line: which step the loop was inside when it went
//! quiet, and whether the word the verdict reads was the loop's last one at all
//! (a full heartbeat channel drops the NEWEST beat). These pins hold the
//! witness that answers both; neither changes a verdict.
//!
//! Virtual time (`start_paused`): the supervisor runs on tokio timers only, so
//! every instant below is exact -- the beats at 25 ms, the hearing tick at
//! 100 ms, five silent periods, the trip at 600 ms.

use std::time::Duration;

use meclaw_colony::watchdog::{
    Beat, HEARTBEAT_CAPACITY, MarkEdge, WatchdogOnTrip, WatchdogTrip, label_slot,
    run_watchdog_with_label_slot, send_beat,
};

const PERIOD: Duration = Duration::from_millis(100);
const THRESHOLD: u32 = 5;

struct Rig {
    hb_tx: tokio::sync::mpsc::Sender<Beat>,
    slot: meclaw_colony::watchdog::LabelSlot,
    trips: tokio::sync::mpsc::Receiver<WatchdogTrip>,
}

/// An armed supervisor on the shipped channel capacity, with the colony's slot.
async fn armed_supervisor() -> Rig {
    let (hb_tx, hb_rx) = tokio::sync::mpsc::channel::<Beat>(HEARTBEAT_CAPACITY);
    let (trip_tx, trips) = tokio::sync::mpsc::channel::<WatchdogTrip>(4);
    let (arm_tx, arm_rx) = tokio::sync::oneshot::channel::<()>();
    let (slot, reader) = label_slot();
    tokio::spawn(run_watchdog_with_label_slot(
        hb_rx,
        trip_tx,
        THRESHOLD,
        PERIOD,
        arm_rx,
        WatchdogOnTrip::Exit,
        None,
        Some(reader),
    ));
    arm_tx.send(()).expect("supervisor waits for arming");
    Rig { hb_tx, slot, trips }
}

async fn first_trip(rig: &mut Rig) -> WatchdogTrip {
    meclaw_testing::deadline::hang("the stalled loop must trip", rig.trips.recv())
        .await
        .expect("a trip")
}

/// Pin 1: a loop that enters a marked step and never leaves it trips with the
/// step at the end of `last_marks` -- entered (`>`), not left (`<`). The mark
/// is no beat: the verdict and the beat count are the ones without it.
#[tokio::test(start_paused = true)]
async fn the_trip_line_names_the_last_marks() {
    let mut rig = armed_supervisor().await;
    // Past the arming drain (tick 0) and off the tick instants.
    tokio::time::sleep(PERIOD / 4).await;
    send_beat(&rig.hb_tx, Some(&rig.slot), Beat::Working);
    rig.slot.mark("writer-send:dlq", MarkEdge::Begin);
    // ... and the step never returns.
    let trip = first_trip(&mut rig).await;
    let line = trip.to_string();
    assert_eq!(trip.silent_for, PERIOD * THRESHOLD, "{line}");
    assert_eq!(
        trip.starved(),
        "slow_work_item",
        "a mark changes no verdict: {line}"
    );
    assert_eq!(trip.beats_seen, 1, "a mark is not a beat: {line}");
    assert!(
        // From ` beats_dropped=` on: `cpu_denied` before it is the real
        // procfs growth of the test thread, a host value (review x4 I2).
        line.ends_with(" beats_dropped=0 last_marks=[-575ms working, -575ms writer-send:dlq>]]"),
        "the trip must name the step it fell into, appended last: {line}"
    );
    drop(rig.hb_tx);
}

/// Pin 2: the channel is full when the loop says `Working` and stalls. Before
/// the fix the supervisor's last word was the `Parked` still queued and the
/// verdict read a parked loop (`colony_loop`, the fatal trips of #1372, H1).
/// The refused word now reaches the supervisor through the trail the source
/// writes anyway: the loop said `Working`, so the silence is an item in flight
/// (`slow_work_item`, log-only before the work-item budget). The trip still
/// counts the refused beat and shows it, with the step after it.
#[tokio::test(start_paused = true)]
async fn a_dropped_beat_reaches_the_trip_line() {
    let mut rig = armed_supervisor().await;
    // Past the arming drain (tick 0) and off the tick instants.
    tokio::time::sleep(PERIOD / 4).await;
    // A burst that fills the channel inside one period: Working/Parked pairs.
    for _ in 0..HEARTBEAT_CAPACITY / 2 {
        send_beat(&rig.hb_tx, Some(&rig.slot), Beat::Working);
        send_beat(&rig.hb_tx, Some(&rig.slot), Beat::Parked);
    }
    // The next iteration's `Working` meets the full channel ...
    send_beat(&rig.hb_tx, Some(&rig.slot), Beat::Working);
    rig.slot.mark("writer-send:dlq", MarkEdge::Begin);
    // ... and its step never returns.
    let trip = first_trip(&mut rig).await;
    let line = trip.to_string();
    assert!(
        trip.in_flight_work,
        "the verdict reads the last SAID word, refused or not: {line}"
    );
    assert_eq!(trip.starved(), "slow_work_item", "{line}");
    assert!(
        !trip.is_fatal(WatchdogOnTrip::Exit),
        "an item in flight is not fatal before its budget: {line}"
    );
    assert_eq!(trip.beats_dropped, 1, "{line}");
    assert!(line.contains(" beats_dropped=1 "), "{line}");
    assert!(
        line.ends_with("-575ms parked, -575ms dropped:working, -575ms writer-send:dlq>]]"),
        "the refused beat and the step after it must reach the trip line: {line}"
    );
}
