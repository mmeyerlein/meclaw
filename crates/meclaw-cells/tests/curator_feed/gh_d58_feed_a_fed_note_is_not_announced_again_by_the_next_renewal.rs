//! D58 feed (GH #1481, plan `u-feed.md` contract point 3, review F-H2) -- a
//! fed note is not announced again by the next renewal.
//!
//! WHY this file exists: the held state of a start window
//! (`seed_facts:<live>`) holds the ids of the notes the session was told, the
//! newest inner turn, the topic and a digest; the renewal after a closed turn
//! tells the session every note that is not in it. A note an orderer already
//! fed would be told a second time. An order that carries the id of a note
//! (`source`, `id`) has the curator enter that id into the held state -- a
//! conditional write on the value it read, because `./history` writes the
//! same key with every start window. The digest is written in the same
//! write, by the rule of `seed_fact_state` (OR-d58-2.feed.7, review F-H2):
//! kept as it was, it parted from the digest the next closed turn computes,
//! and that turn sent an empty renewal (`{"notes": []}`, `at_turn_end` "1")
//! the person never caused -- "for the person and the voice nothing changes"
//! (plan point 6). So a closed turn with nothing else moved sends nothing,
//! and the next renewal names what else moved, never the fed note. A
//! conditional write that loses to `./history` (its start window lands
//! between the read and the write, OR-d58-2.feed.9) leaves history's state
//! standing: nothing to do.

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
use curator_live::{ask_seed, closed_turn, live_keys, topic_value};
use meclaw_core::serde_json::json;
use note_once::{note, post};

const NOTE: &str = "The daughter visits this afternoon.";

/// The phase of the step that books a fed order (`curator/policy`): its
/// mark and the held state's conditional write go to the store together.
const BOOKED: &str = "feed-booked";

#[test]
fn gh_d58_feed_a_fed_note_is_not_announced_again_by_the_next_renewal() {
    if !shipped() {
        return;
    }
    let mut h = feed_hive(&[("policy", "live_feed", json!("1"))]);
    clear_of_midnight();
    let seeds = ask_seed(&mut h, &live_keys(FEED_LIVE), "start");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    let before = held(&h, FEED_LIVE);
    assert!(
        before.get("digest").is_some(),
        "the start window's state is held: {before}"
    );

    post(&mut h, json!([note("n1", "session", NOTE)]));
    let body = json!({"section": "fact", "payload": NOTE, "source": "probe", "id": "n1"});
    order(
        &mut h,
        order_head(Some(FEED_LIVE), "t3", "fact"),
        body.clone(),
    );
    let sent = feeds(&h);
    assert_eq!(sent.len(), 1, "the fact leaves: {:?} {:?}", h.out, h.stderr);
    assert_eq!(body_of(&sent[0]), body, "the body as ordered");

    let after = held(&h, FEED_LIVE);
    assert!(
        after["notes"]
            .as_array()
            .is_some_and(|n| n.contains(&json!("probe:n1"))),
        "the fed note's id is in the held state: {after}"
    );
    assert_ne!(
        after["digest"], before["digest"],
        "the digest is the state's with the note (OR-d58-2.feed.7)"
    );
    for k in ["inner_seq", "topic", "screen"] {
        assert_eq!(after[k], before[k], "{k} untouched by the order");
    }

    // Nothing else moves: the next closed turn computes the digest the order
    // wrote and sends nothing -- no empty renewal for the person.
    closed_turn(&mut h, SESSION, "t4", &live_keys(FEED_LIVE));
    assert!(
        feeds_of(&h, "renew").is_empty(),
        "a turn with nothing moved renews nothing: {:?} {:?}",
        h.out,
        h.stderr
    );

    // Something else moves; the next closed turn renews -- without the note.
    h.mark(
        backdated(20),
        SESSION,
        "t5",
        "topic",
        &topic_value("garden", "start"),
    );
    closed_turn(&mut h, SESSION, "t6", &live_keys(FEED_LIVE));
    let renew = feeds_of(&h, "renew");
    assert_eq!(
        renew.len(),
        1,
        "the topic moved, the renewal leaves: {:?} {:?}",
        h.out,
        h.stderr
    );
    let told = renew[0].body.get("notes").cloned().unwrap_or(json!([]));
    assert!(
        !told.to_string().contains("n1") && !told.to_string().contains(NOTE),
        "the fed note is not announced again: {told}"
    );

    // The race (OR-d58-2.feed.9): `./history` writes the key between the
    // order's read and its conditional write -- history's state stands, the
    // feed still leaves with its mark.
    let mut h = feed_hive(&[("policy", "live_feed", json!("1"))]);
    let seeds = ask_seed(&mut h, &live_keys(FEED_LIVE), "start");
    assert_eq!(seeds.len(), 1, "one seed: {:?} {:?}", h.out, h.stderr);
    post(&mut h, json!([note("n1", "session", NOTE)]));
    let theirs = r#"{"digest":"0000history0000","inner_seq":0,"notes":[],"screen":"","topic":""}"#;
    h.sql_at_phase = Some((
        BOOKED.to_string(),
        format!("UPDATE state SET value = '{theirs}' WHERE key = 'seed_facts:{FEED_LIVE}'"),
    ));
    order(&mut h, order_head(Some(FEED_LIVE), "t3", "fact"), body);
    assert!(
        h.sql_at_phase.is_none(),
        "the step `{BOOKED}` came: {:?}",
        h.ledger_steps
    );
    assert_eq!(
        feeds(&h).len(),
        1,
        "the fact leaves: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(feed_marks(&h).len(), 1, "{:?}", feed_marks(&h));
    assert_eq!(
        h.state(&format!("seed_facts:{FEED_LIVE}")),
        theirs,
        "the lost conditional write leaves history's state as it is"
    );
}
