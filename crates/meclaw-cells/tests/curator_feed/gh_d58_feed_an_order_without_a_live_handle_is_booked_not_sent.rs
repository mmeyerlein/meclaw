//! D58 feed (GH #1481, plan `u-feed.md` contract point 4 and the stumbling
//! block on `once` notes) -- an order without a live handle is booked, not
//! sent.
//!
//! WHY this file exists: "without a running session" means exactly one
//! thing: no `live` in the head -- the policy keeps no state "a session
//! runs". Such an order has nowhere to go; it is booked as a mark `feed`
//! with a reason (`no_live`) and nothing is sent or claimed, even with
//! `once`. The contract knows exactly one other mark with a reason instead of
//! a send: an order that carries the id of a `once` note (strand `note`: a
//! `once` note never rides a feed) is refused with the reason `once_note`,
//! and the held start window is not touched. The reasons are the values the
//! curator already books for a question without a handle
//! (`interrupt_skipped {reason: no_live}`); OR-d58-2.feed.3 names both.
//! A case of the same rule (OR-d58-2.feed.10): when the store refuses the
//! read of the note an order names, whether it is a `once` note is unknown,
//! and unknown is never sent -- the order is booked with the reason
//! `store_error`, like an order that cannot go.

// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_feed::*;
use curator_hive::*;
use curator_live::{ask_seed, live_keys};
use meclaw_core::serde_json::json;
use note_once::{note, once, post};

#[test]
fn gh_d58_feed_an_order_without_a_live_handle_is_booked_not_sent() {
    if !shipped() {
        return;
    }
    // No `live` in the head, with `once`: booked with its reason, no claim.
    let mut h = feed_hive(&[]);
    let mut hop = order_head(None, "t1", "fact");
    hop["once"] = json!("k1");
    order(&mut h, hop, fact_body("The bus is late."));
    assert!(feeds(&h).is_empty(), "nothing sent: {:?}", h.out);
    let marks = feed_marks(&h);
    assert_eq!(marks.len(), 1, "{marks:?} {:?}", h.stderr);
    assert_eq!(marks[0].value["reason"], json!("no_live"), "{:?}", marks[0]);
    assert_eq!(marks[0].value["section"], json!("fact"));
    assert_eq!(marks[0].value[SENDER_KEY], json!(SENDER));
    assert_eq!(marks[0].session, SESSION);
    assert_eq!(marks[0].turn, "t1");
    assert!(claims(&h).is_empty(), "no claim: {:?}", claims(&h));

    // An order carrying the id of a `once` note: refused with a mark. The
    // running session holds a start window first (review m3: the check that
    // it is not touched needs one to touch); the note comes after it, so the
    // start window does not carry it.
    let mut h = feed_hive(&[
        ("intake", "note_once", json!("1")),
        ("policy", "note_once", json!("1")),
        ("history", "note_once", json!("1")),
        ("policy", "live_feed", json!("1")),
    ]);
    clear_of_midnight();
    let seeds = ask_seed(&mut h, &live_keys(FEED_LIVE), "start");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    let held_before = h.state(&format!("seed_facts:{FEED_LIVE}"));
    assert!(
        !held_before.is_empty(),
        "the start window's state is held: {held_before:?}"
    );
    h.out.clear();
    post(
        &mut h,
        json!([once("n9", "Tell them the results are fine.", None)]),
    );
    order(
        &mut h,
        order_head(Some(FEED_LIVE), "t2", "fact"),
        json!({"section": "fact", "payload": "Tell them the results are fine.",
               "source": "probe", "id": "n9"}),
    );
    assert!(
        feeds(&h).is_empty(),
        "a once note never rides a feed: {:?}",
        h.out
    );
    let marks = feed_marks(&h);
    assert_eq!(marks.len(), 1, "{marks:?} {:?}", h.stderr);
    assert_eq!(
        marks[0].value["reason"],
        json!("once_note"),
        "{:?}",
        marks[0]
    );
    assert_eq!(marks[0].turn, "t2");
    assert_eq!(
        h.state(&format!("seed_facts:{FEED_LIVE}")),
        held_before,
        "the held start window is not touched"
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // OR-d58-2.feed.10: an order naming a plain note goes ...
    let mut h = feed_hive(&[
        ("intake", "note_once", json!("1")),
        ("policy", "note_once", json!("1")),
        ("history", "note_once", json!("1")),
    ]);
    post(
        &mut h,
        json!([note("n7", "session", "The parcel came today.")]),
    );
    let body = json!({"section": "fact", "payload": "The parcel came today.",
                      "source": "probe", "id": "n7"});
    order(
        &mut h,
        order_head(Some(FEED_LIVE), "t3", "fact"),
        body.clone(),
    );
    assert_eq!(feeds(&h).len(), 1, "{:?} {:?}", h.out, h.stderr);
    // ... but not when the store refuses the read of the note.
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("fd-note".to_string(), "store_error".to_string())];
    order(&mut h, order_head(Some(FEED_LIVE), "t4", "fact"), body);
    assert!(h.refuse_legs.is_empty(), "the read of the note was refused");
    assert!(
        feeds(&h).is_empty(),
        "an unread note never rides a feed: {:?}",
        h.out
    );
    let refused: Vec<FeedMark> = feed_marks(&h)
        .into_iter()
        .filter(|m| m.turn == "t4")
        .collect();
    assert_eq!(refused.len(), 1, "{refused:?} {:?}", h.stderr);
    assert_eq!(
        refused[0].value["reason"],
        json!("store_error"),
        "{:?}",
        refused[0]
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}
