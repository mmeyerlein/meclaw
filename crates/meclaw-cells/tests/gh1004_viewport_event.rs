//! GH #1004: a browser event never parks its socket, and a semantic event never
//! waits for the handler.
//!
//! Measured on the deployed pan (plan W4 § 1): the socket loop awaited the
//! handler's verdict inside its `incoming` arm, so while one event was open it
//! read neither the next browser frame nor its own outbound queue — the diffs
//! for that viewer stood behind its own event, and on zoom-out the browser saw
//! 0 frames for 2–5 s while the cell answered 8–10 bundles a second.
//!
//! These are the function and order locks, safe beside a parallel suite: no
//! wall-clock bound in here. The numbers (reply latency, event cadence at the
//! colony, diff delay behind an open event) live in
//! `gh1004_viewport_event_measure.rs`, `#[ignore]`, run once alone.
//!
//! The shape every lock uses: bundles are queued in the cell's mailbox first,
//! so the handler is busy for all of them (it serves its mailbox before its
//! browser events), and only then does the browser speak.

#[path = "support/web_fixture.rs"]
mod web_fixture;

use meclaw_core::serde_json::{Value, json};
use web_fixture::{Lab, Shape, Viewer, is_event, update};

/// The page: a 4 000-figure pool, like the plan's lab.
const FIGURES: usize = 4_000;
/// Figures one bundle moves.
const PER_BUNDLE: usize = 400;

/// One bundle of the pool: `PER_BUNDLE` figures, a different slice each round.
fn pool_bundle(round: usize) -> Vec<Value> {
    (0..PER_BUNDLE)
        .map(|j| {
            let i = (round * PER_BUNDLE + j) % FIGURES;
            update(&format!("fig-{i}"), json!({"x": (round * 7 + j) as i64}))
        })
        .collect()
}

/// Read frames until the reply to `msg_ref`; return it and how many diffs
/// came first.
async fn until_reply(v: &mut Viewer, msg_ref: &str) -> (Value, usize) {
    let mut diffs = 0;
    loop {
        let frame = v.next_frame().await;
        if frame[3] == json!("diff") {
            diffs += 1;
        } else if frame[3] == json!("phx_reply") && frame[1] == json!(msg_ref) {
            return (frame, diffs);
        }
    }
}

/// T1: the `ok` of a semantic event is owed at hand-over, not after the
/// handler got to it. With every queued bundle still ahead of the event, its
/// emission cannot have happened yet when the reply arrives — before the fix
/// the reply was written only after `emit_semantic`, so the emission was
/// always already there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1004_a_semantic_event_does_not_wait_for_a_running_bundle() {
    const BUNDLES: usize = 20;
    let mut lab = Lab::start(Shape::with_figures(FIGURES)).await;
    let mut v = lab.viewer().await;

    for round in 0..BUNDLES {
        lab.enqueue(pool_bundle(round)).await;
    }
    v.push_event("p1", "pick", json!({"for": "fig-1"})).await;
    let (reply, _) = until_reply(&mut v, "p1").await;
    assert_eq!(reply[4]["status"], json!("ok"), "reply: {reply}");

    let so_far = lab.emitted_so_far();
    let answers = so_far.iter().filter(|e| !is_event(e)).count();
    assert!(
        !so_far.iter().any(is_event),
        "the reply waited for the handler: the pick was already emitted when it \
         arrived ({answers} of {BUNDLES} bundles answered by then)"
    );

    // And the event still reaches the colony, once the handler gets to it.
    loop {
        let e = lab.next_emission().await;
        if is_event(&e) {
            assert_eq!(e.content["header"]["event_name"], json!("pick"));
            break;
        }
    }
}

/// T2: while this viewer's `object:set` is open (the handler is busy with the
/// bundles ahead of it), the diffs of those bundles still reach the same
/// socket. Before the fix the loop sat in `answer` and wrote none of them
/// until the reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1004_a_viewer_keeps_receiving_frames_while_its_event_is_pending() {
    const BUNDLES: usize = 8;
    let lab = Lab::start(Shape::with_figures(FIGURES)).await;
    let mut v = lab.viewer().await;

    for round in 0..BUNDLES {
        lab.enqueue(pool_bundle(round)).await;
    }
    v.push_event(
        "s1",
        "object:set",
        json!({"id": "fig-0", "prop": "y", "value": 9}),
    )
    .await;
    let (reply, before) = until_reply(&mut v, "s1").await;
    assert_eq!(reply[4]["status"], json!("ok"), "reply: {reply}");
    assert!(
        before >= BUNDLES / 2,
        "only {before} of {BUNDLES} diffs reached the socket while its own \
         object:set was open"
    );
}

/// L-W6 (plan W4, addendum): diffs never wait behind an open event. Twenty
/// bundles run while one `object:set` is open; every diff but the last one
/// (whose push may race the verdict on the way out) is on the socket before
/// the event's reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1004_diffs_never_wait_behind_an_open_event() {
    const BUNDLES: usize = 20;
    let lab = Lab::start(Shape::with_figures(FIGURES)).await;
    let mut v = lab.viewer().await;

    for round in 0..BUNDLES {
        lab.enqueue(pool_bundle(round)).await;
    }
    v.push_event(
        "s1",
        "object:set",
        json!({"id": "fig-0", "prop": "y", "value": 9}),
    )
    .await;
    let (reply, before) = until_reply(&mut v, "s1").await;
    assert_eq!(reply[4]["status"], json!("ok"), "reply: {reply}");
    assert!(
        before >= BUNDLES - 1,
        "{before} of {BUNDLES} diffs reached the socket before the reply of the \
         event they were queued behind"
    );
}

/// T3: a hundred events sent back to back reach the colony in the order they
/// were sent — the replies may now overtake each other, the hand-over may not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1004_events_of_one_viewer_keep_their_order() {
    const EVENTS: usize = 100;
    let mut lab = Lab::start(Shape::with_figures(1_000)).await;
    let mut v = lab.viewer().await;

    for n in 0..EVENTS {
        v.push_event(&format!("e{n}"), "viewport", json!({"n": n}))
            .await;
    }
    let mut replies = 0;
    while replies < EVENTS {
        let frame = v.next_frame().await;
        if frame[3] == json!("phx_reply") {
            assert_eq!(frame[4]["status"], json!("ok"), "reply: {frame}");
            replies += 1;
        }
    }
    let mut seen = Vec::with_capacity(EVENTS);
    while seen.len() < EVENTS {
        let e = lab.next_emission().await;
        if is_event(&e) {
            seen.push(e.content["event"]["value"]["n"].as_u64().expect("n"));
        }
    }
    let sent: Vec<u64> = (0..EVENTS as u64).collect();
    assert_eq!(seen, sent, "the colony heard the events out of order");
}

/// T5: `object:set` stays a local write, ordered with the bundles: its verdict
/// still comes from the handler, so what was written before it, by it and
/// after it lands in that order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1004_an_object_set_stays_ordered_with_the_writes() {
    let mut lab = Lab::start(Shape::with_figures(1_000)).await;
    let mut v = lab.viewer().await;

    lab.call(vec![update("fig-0", json!({"x": 1, "y": 1}))])
        .await;
    v.push_event(
        "s1",
        "object:set",
        json!({"id": "fig-0", "prop": "x", "value": 2}),
    )
    .await;
    let (reply, _) = until_reply(&mut v, "s1").await;
    assert_eq!(reply[4]["status"], json!("ok"), "reply: {reply}");
    // A bundle after the verdict sees the browser's write and keeps it.
    lab.call(vec![update("fig-0", json!({"y": 2}))]).await;
    let page = lab.get_page().await;
    assert!(
        page.contains(r#"data-id="fig-0" data-x="2" data-y="2""#),
        "the object:set did not land between the two bundles"
    );
    // And a bundle after that overwrites it.
    lab.call(vec![update("fig-0", json!({"x": 3}))]).await;
    let page = lab.get_page().await;
    assert!(
        page.contains(r#"data-id="fig-0" data-x="3" data-y="2""#),
        "the last write is not the one that stands"
    );
}
