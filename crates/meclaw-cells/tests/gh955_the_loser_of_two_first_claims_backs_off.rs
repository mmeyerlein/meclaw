//! GH #955 -- the loser of two first rebuild claims of one round backs off.
//!
//! WHY this file exists: since GH #943 a round's rebuild runs under a claim of
//! its own, the `state` row `rebuild_running:<rk>`. The FIRST claim of a round
//! has no row to compare against, so it is an `insert`, and the unique index
//! on `state.key` (GH #915) turns that insert into the compare-and-set: of two
//! first claims of the same round, one inserts and the other's leg affects no
//! row, and `./policy` parks it at `rb-claim` ("another trigger claimed the
//! rebuild first"). The logic read correctly, but nothing drove two first
//! claims at once.
//!
//! Driven here in the curator hive of `support/curator_hive.rs`, whose queue
//! is one FIFO for every chain in flight: two strikes of one round's order,
//! fired together (`Hive::fire_together`), interleave step by step -- both
//! probes read before either claims, both claims reach the store. Two claims:
//!
//! (a) the loser parks, the winner's rebuild asks the summarizer once and
//!     writes one plan, and the claim is free afterwards -- no second plan, no
//!     lost rebuild;
//! (b) when the store refuses the winner's next bundle (`rb-data`), the
//!     rebuild is not lost either: the next trigger of the round takes it
//!     over, rather than merging into a claim nobody runs any more;
//! (c) the release of (b) frees the chain's OWN claim only (review M-1): a
//!     claim another chain took over while the refused bundle was on its way
//!     back (stale, `REBUILD_STALE_S`) stays where it is.
//!
//! No provider, no clock: the summarizer and the clock are answered by hand.
//! Guarded like every template-reading test (GH #49).

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const SESSION: &str = "s-955";
const ROUND: &str = r#"["member:e", "member:a"]"#;
const SUMMARY_WORDS: &str = "cw955 summary: what was said.";

/// A hive that condenses every older turn of a round (`keep_recent` 1), so a
/// rebuild asks the summarizer; its ledger may refuse (the loser's insert
/// meets the unique index, and (b) refuses a bundle on purpose).
fn hive() -> Hive {
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    h.ledger_may_refuse = true;
    h
}

/// One whole turn in the round, the answer back on the tap with `extra`.
fn turn_in(h: &mut Hive, turn: &str, says: &str, reply: &str, extra: Value) -> Msg {
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": SESSION, "turn_id": turn, "iter": "0", "channel": "test",
               "audience_set": ROUND}),
        json!({"session_id": SESSION, "turn_id": turn, "iter": "0", "phase": ""}),
        json!({"messages": [user(says)], "system": mode("Be brief.")}),
    );
    let calls = h.routed("brain");
    assert_eq!(
        calls.len(),
        1,
        "one call per turn: {:?} {:?}",
        h.out,
        h.stderr
    );
    let call = calls[0].clone();
    h.out.clear();
    h.tap(&call, "stop", extra, json!([said(reply)]));
    call
}

/// A tap that stamps a cache expiry far ahead: the clock is ordered for the
/// cold cache, and the test strikes it by hand.
fn cold() -> Value {
    json!({"cache_expires_at": "2099-01-01T00:00:00Z"})
}

/// Two turns of the round, the second ordering the strike of its cold cache.
fn ordered(h: &mut Hive, n: usize) -> Msg {
    turn_in(
        h,
        &format!("t-{n}a"),
        &format!("cw955 {n}a: the parcel left the depot."),
        &format!("cw955 {n}a reply: noted."),
        json!({}),
    );
    turn_in(
        h,
        &format!("t-{n}b"),
        &format!("cw955 {n}b: the lease is signed."),
        &format!("cw955 {n}b reply: noted."),
        cold(),
    );
    h.clock_order_for(ROUND)
        .unwrap_or_else(|| panic!("the round ordered no strike: {:?}", h.clock))
}

fn claim_key() -> String {
    format!("rebuild_running:{}", round_key(ROUND))
}

/// The `state` inserts `./policy` sent for `key`.
fn inserts_of(h: &Hive, key: &str) -> usize {
    h.ledger_ops
        .iter()
        .filter(|(from, op)| {
            from == "policy"
                && op["operation"] == "insert"
                && op["table"] == "state"
                && op["row"]["key"] == key
        })
        .count()
}

fn said_times(h: &Hive, words: &str) -> usize {
    h.stderr.iter().filter(|l| l.contains(words)).count()
}

#[test]
fn the_loser_of_two_first_claims_backs_off() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let order = ordered(&mut h, 1);
    assert_eq!(
        h.rows(&format!(
            "SELECT 1 FROM state WHERE key = '{}'",
            claim_key()
        ))
        .len(),
        0,
        "the round has a claim row before its first rebuild"
    );

    h.fire_together(&[order.clone(), order.clone()]);

    assert_eq!(
        inserts_of(&h, &claim_key()),
        2,
        "the two strikes did not both reach the first claim -- the interleave proves \
         nothing: {:?}",
        h.stderr
    );
    assert_eq!(
        said_times(&h, "another trigger claimed the rebuild first"),
        1,
        "exactly one of the two first claims parks as the loser: {:?}",
        h.stderr
    );
    assert_eq!(
        h.summ.len(),
        1,
        "the winner asks the summarizer once, the loser never: {:?}",
        h.stderr
    );
    assert!(
        !h.state_in("rebuild_running", ROUND).is_empty(),
        "the winner holds the claim while its summary is out"
    );

    h.answer(SUMMARY_WORDS, "stop");
    let plan = h.plan_in(ROUND);
    assert!(
        plan["summary"].as_str().is_some_and(|s| !s.is_empty()),
        "the winner's rebuild wrote no plan with its summary: {plan} {:?}",
        h.stderr
    );
    assert_eq!(
        inserts_of(&h, &format!("window_plan:{}", round_key(ROUND))),
        1,
        "one plan of the round, not two: {:?}",
        h.stderr
    );
    assert_eq!(
        h.rows("SELECT id FROM summaries").len(),
        1,
        "one summary of the round"
    );
    assert_eq!(
        h.state_in("rebuild_running", ROUND),
        "",
        "the claim is free after the winner's rebuild"
    );
    assert!(h.summ.is_empty(), "nothing else asks the summarizer");
}

#[test]
fn a_refused_winner_hands_the_rebuild_to_the_next_trigger() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let order = ordered(&mut h, 1);
    // The store refuses the winner's next bundle after its claim.
    h.refuse_phase = Some("rb-data".to_string());
    h.fire_together(&[order.clone(), order.clone()]);
    assert!(
        h.refuse_phase.is_none(),
        "no winner sent its `rb-data` bundle: {:?}",
        h.stderr
    );
    assert_eq!(
        said_times(&h, "another trigger claimed the rebuild first"),
        1,
        "the loser parks: {:?}",
        h.stderr
    );
    assert!(h.summ.is_empty(), "the refused rebuild asked nothing");

    // The next trigger of the round: a later turn, its own strike.
    let next = ordered(&mut h, 2);
    h.fire(&next);
    assert_eq!(
        h.summ.len(),
        1,
        "the next trigger merged into the claim of a rebuild the store refused -- the \
         rebuild is lost until the claim goes stale: {:?}",
        h.stderr
    );
    h.answer(SUMMARY_WORDS, "stop");
    let plan = h.plan_in(ROUND);
    assert!(
        plan["summary"].as_str().is_some_and(|s| !s.is_empty()),
        "the next trigger's rebuild wrote no plan: {plan} {:?}",
        h.stderr
    );
    assert_eq!(
        h.state_in("rebuild_running", ROUND),
        "",
        "the claim is free"
    );
}

/// Review M-1 of GH #955: the release of a refused bundle is a compare-and-set
/// on the chain's own claim. WHY: an unconditional free of
/// `rebuild_running:<rk>` also frees a claim another chain holds by then --
/// one that took the round over as stale while this chain's bundle was out --
/// and a third trigger would then rebuild beside that running rebuild.
/// Driven deterministically: the claim row is overwritten with a foreign claim
/// right before the store refuses this chain's `rb-data` (`sql_at_phase`).
#[test]
fn a_refused_rebuild_leaves_a_claim_taken_over_alone() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let order = ordered(&mut h, 1);
    let foreign = canonical(&json!({
        "reason": "cache", "token": "t-foreign-955", "at": "2099-01-01T00:00:00Z",
        "call": "c-foreign-955", "session": SESSION
    }));
    h.sql_at_phase = Some((
        "rb-data".to_string(),
        format!(
            "UPDATE state SET value = '{foreign}' WHERE key = '{}'",
            claim_key()
        ),
    ));
    h.refuse_phase = Some("rb-data".to_string());
    h.fire(&order);
    assert!(
        h.sql_at_phase.is_none() && h.refuse_phase.is_none(),
        "the rebuild never sent its `rb-data` bundle: {:?}",
        h.stderr
    );
    assert!(h.summ.is_empty(), "the refused rebuild asked nothing");
    assert_eq!(
        h.state_in("rebuild_running", ROUND),
        foreign,
        "the refused rebuild freed a claim that is no longer its own: {:?}",
        h.stderr
    );
    assert_eq!(
        said_times(&h, "is another's now"),
        1,
        "the release names why it left the claim alone: {:?}",
        h.stderr
    );
}
