//! A gesture the screen cannot absorb reaches the member EXACTLY once (GH #1210,
//! VORGABEN TV-A2/TV-A3).
//!
//! The browser probe `workshop/tools/display-lab/tap.mjs` reads a gesture through a
//! marker: one event of its own, sent behind the gesture over the same LiveView channel,
//! so a second arrival of the gesture on that path lands before it. A second arrival over
//! compose -> member -> compose is NOT ordered by that marker; the probe follows
//! `parent_message_id` instead (re-check tv-bn-i H2 4b). What no browser run can say is
//! "exactly once" -- a run reads a page, not the colony's delivery. This lock says it
//! where messages are made:
//!
//!   * the gesture and the marker go out on the page's OWN socket, in that order;
//!   * the marker's arrival at compose is the positive event that the ingress (an I/O
//!     half-task `quiesce` does not cover) delivered everything sent before it;
//!   * `ColonyHandle::quiesce()` then proves that every hop following from what was
//!     delivered has run -- the hand-on out of the hive included;
//!   * the message log, read through the colony (`log_mark`/`log_since`), and the
//!     member's door (the marked egress) each hold the gesture exactly once.
//!
//! No window of time decides anything here: every wait is an event under the HANG mark
//! of `meclaw_testing::deadline`.

// harness_fold: #[path = "support/display_colony.rs"]
#[allow(unused_imports)]
use crate::display_colony; // harness_fold: mod display_colony;

use display_colony::{
    Boot, PROBE, SCREEN, Why, body_of, boot, have_python, hop_of, library_ships, next_text, push,
};
use meclaw_colony::api_dto::{MessageLogDto, MessageLogFilter};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::deadline::hang_with;

/// A semantic event no curator absorbs: compose hands it out of the hive to the owner of
/// the window it names (compose.py `pass_event`).
const GESTURE: &str = "pick";

/// The probe's marker (tap.mjs `markBehindTheGesture`): an event no window owns.
const MARKER_EVENT: &str = "tap-probe-marker";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_millis() as u64
}

/// The rows that are the browser event `name` arriving at compose.
fn arrivals<'a>(rows: &'a [MessageLogDto], name: &str) -> Vec<&'a MessageLogDto> {
    let compose = format!("{SCREEN}/compose");
    rows.iter()
        .filter(|row| row.to_path == compose)
        .filter(|row| hop_of(row)["route"] == "event")
        .filter(|row| body_of(row)["event"]["name"] == name)
        .collect()
}

/// The hand-ons compose sent out of the hive for the gesture: route `event`, from
/// compose, addressed to the window's owner on the hop (compose.py `pass_event`).
fn hand_ons<'a>(rows: &'a [MessageLogDto], owner: &str) -> Vec<&'a MessageLogDto> {
    let compose = format!("{SCREEN}/compose");
    rows.iter()
        .filter(|row| row.from_path == compose)
        .filter(|row| hop_of(row)["route"] == "event" && hop_of(row)["owner"] == owner)
        .filter(|row| body_of(row)["event"]["name"] == GESTURE)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gesture_reaches_the_member_exactly_once() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // A long linger and fade: the screen's clock stays out of the reading.
    let mut colony = boot(Boot {
        linger_ms: 60_000,
        fade_ms: 120_000,
        ..Boot::default()
    })
    .await;
    colony
        .app_put(
            json!({"do": "card", "view_id": "card", "at": now_ms(), "text": "a card"}),
            "card",
        )
        .await;
    let oid = colony.oid(PROBE, "card");

    let mut ws = colony.socket("monitor").await;
    // What left the hive before the gesture is not the gesture's.
    colony.left_the_hive().await;
    let mark = colony.mark_log().await;
    let mut seq = colony.log_changes();

    push(&mut ws, GESTURE, json!({"for": oid})).await;
    let _ = next_text(&mut ws).await; // the reply to the push
    push(&mut ws, MARKER_EVENT, json!({})).await;
    let _ = next_text(&mut ws).await;

    // The positive event: the marker reached compose. `web` forwards one viewer's events
    // in order (TV-A1), so every arrival of the gesture on this path is logged by now.
    let compose = format!("{SCREEN}/compose");
    let to_compose = MessageLogFilter {
        to_path_prefix: Some(compose.clone()),
        ..Default::default()
    };
    // The HANG line names the last log sequence the writer published (TV-Z3, re-check
    // tv-bn-layout C fix1 H2): a writer that stood still and one that wrote past the
    // marker are two different findings.
    let last_seq = seq.clone();
    hang_with(
        "the marker reaches compose",
        async {
            loop {
                let rows = colony.rows_since(&mark, to_compose.clone()).await;
                if !arrivals(&rows, MARKER_EVENT).is_empty() {
                    return;
                }
                seq.changed().await;
            }
        },
        move || {
            format!(
                "the colony's log sequence stood at {:?}",
                last_seq.at().because(Why::Report)
            )
        },
    )
    .await;
    // TV-A2: everything that follows from what was delivered has run -- the hand-on
    // compose -> member included, which the marker does not order.
    colony.settle().await;

    let rows = colony.rows_since(&mark, MessageLogFilter::default()).await;
    let gestures = arrivals(&rows, GESTURE);
    assert_eq!(
        gestures.len(),
        1,
        "the gesture reached compose {} times, not once",
        gestures.len()
    );
    assert_eq!(
        arrivals(&rows, MARKER_EVENT).len(),
        1,
        "the marker reached compose more than once"
    );
    let handed = hand_ons(&rows, PROBE);
    assert_eq!(
        handed.len(),
        1,
        "compose handed the gesture to its member {} times, not once",
        handed.len()
    );
    assert_eq!(
        handed[0].parent_message_id.as_deref(),
        Some(gestures[0].id.as_str()),
        "the hand-on does not descend from the gesture's arrival"
    );

    // The member's door: what left the hive marked, after the barrier.
    let mut at_member: Vec<Value> = Vec::new();
    for m in colony.left_the_hive().await {
        if m.headers.hop.get("route").and_then(Value::as_str) == Some("event")
            && m.headers.hop.get("owner").and_then(Value::as_str) == Some(PROBE)
        {
            at_member.push(json!(m.headers.hop));
        }
    }
    assert_eq!(
        at_member.len(),
        1,
        "the member's door took the gesture {} times, not once: {at_member:?}",
        at_member.len()
    );

    colony.shutdown().await;
}
