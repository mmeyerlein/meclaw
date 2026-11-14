//! D54g kprofil (GH #1507, contract point 4) -- the curator's own feed into a
//! running session follows the profile of its target.
//!
//! WHY this file exists: with `live_feed` on, the curator checked after every
//! closed turn of a running session whether anything moved against the start
//! window, and then sent a wordless feed of section `renew`: a new session, a
//! new start window. A topic the person raised in that very session moved it
//! too, so a session renewed itself out of its own turns -- measured as four
//! renewals in eight sessions, three of them into running speech. The limits
//! a target names in its ask for a start window carry its profile, and the
//! profile field `feed_own` says what the own feed does:
//!
//! * `renew` (the shipped value, and a profile without the field): as before;
//! * `context`: ONE feed of section `context` WITH words -- only what came
//!   from outside (notes of other domains, inner turns, memory after the
//!   start window), at most `feed_budget` tokens at the target's rate, the
//!   newest first, whole units only, each with its place as it stands in the
//!   window; what does not fit waits for the next closed turn. A topic the
//!   person raised in this session moves nothing;
//! * `none`: no own feed.
//!
//! The policy refused `context` for its own feed because that feed carried no
//! words and the receiving cell refuses a section without words; with words
//! in the body the reason is gone.
//!
//! The hive in one process (`support/curator_hive.rs`); no model.

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
use curator_live::{VOICE_SESSION, closed_turn, live_keys, topic_value};
use meclaw_core::serde_json::{Value, json};
use note_once::{note, post};

/// The rate the target of these pins counts with, characters a token.
const RATE: usize = 3;

/// The curator with the own feed on, every emission checked against the
/// cells' `contract.emits`.
fn hive() -> Hive {
    feed_hive(&[("policy", "live_feed", json!("1"))])
}

/// The limits a target names in its ask: `feed_budget` tokens a feed at
/// [`RATE`] characters a token, its profile as it is.
fn limits(feed_budget: usize, profile: Value) -> Value {
    json!({"model": "test/own-session", "start_budget": 8192, "feed_budget": feed_budget,
           "chars_per_token": RATE, "profile": profile})
}

/// The start window of the running session [`FEED_LIVE`], the ask naming
/// `limits` in its hop (`None`: the ask of before).
fn start(h: &mut Hive, limits: Option<Value>) {
    h.out.clear();
    let mut hop = json!({"session_id": VOICE_SESSION, "reason": "start", "renewal_n": 0,
                         "live": FEED_LIVE});
    if let Some(l) = limits {
        hop["limits"] = l;
    }
    h.lane(
        "in_seed_ask",
        json!({"audience_set": ROUND_E, "live": FEED_LIVE}),
        hop,
        json!({"messages": []}),
    );
    assert_eq!(
        h.routed("seed").len(),
        1,
        "one seed: {:?} {:?}",
        h.out,
        h.stderr
    );
}

/// A closed turn of the running session; the feeds it left.
fn turn(h: &mut Hive, turn_id: &str) -> Vec<Msg> {
    closed_turn(h, SESSION, turn_id, &live_keys(FEED_LIVE));
    feeds(h)
}

/// The words of a feed: the section's value under `payload`, else `text` --
/// the two slots the receiving cell reads words from. "" for none.
fn words(m: &Msg) -> String {
    ["payload", "text"]
        .iter()
        .filter_map(|k| m.body.get(*k).and_then(Value::as_str))
        .find(|s| !s.trim().is_empty())
        .unwrap_or("")
        .to_string()
}

/// The line of `words` that carries `unit` whole, if any.
fn line_of<'a>(words: &'a str, unit: &str) -> Option<&'a str> {
    words.lines().find(|l| l.contains(unit))
}

/// The rows of the ledger's `state` that hold the limits of the handle
/// `live`: a row keyed by the handle whose value names a start budget.
fn limits_held(h: &Hive, live: &str) -> usize {
    h.rows(&format!(
        "SELECT key FROM state WHERE key LIKE '%{live}%' AND value LIKE '%start_budget%'"
    ))
    .len()
}

const NOTE: &str = "The neighbour brings the keys back this afternoon.";

/// The words of an inner turn from outside: a round nobody in the session
/// spoke (kind `event` on the wall).
const EVENT: &str = "The washing machine in the cellar has finished its run.";

/// The errand the agent that holds the session handed out itself, as the
/// wall files it (kind `delegation`).
const ERRAND: &str = "[delegation d1]\nLook up when the orchard opens on Sunday.";

/// The advice a model call of the round carried in its window, as the wall
/// files it (kind `advice`).
const ADVICE: &str = "[advice from your reasoning core]\nAsk about the greenhouse first.";

/// The target of the start window in the pin that contributes to it.
const PART_TARGET: &str = "decide";

#[test]
fn gh_d54g_kprofil_a_change_from_outside_goes_out_as_context_within_the_feed_budget() {
    if !shipped() {
        return;
    }
    // Sixty tokens at three characters a token: 180 characters a feed.
    let budget_chars = 60 * RATE;
    let mut h = hive();
    clear_of_midnight();
    start(&mut h, Some(limits(60, json!({"feed_own": "context"}))));

    // A note of another domain arrives after the start window.
    post(&mut h, json!([note("n1", "session", NOTE)]));
    let sent = turn(&mut h, "t4");
    assert_eq!(
        sent.len(),
        1,
        "the closed turn sends one feed: {:?} {:?}",
        h.out,
        h.stderr
    );
    let feed = &sent[0];
    assert_eq!(
        feed.hop["section"],
        json!("context"),
        "the own feed of a target with `feed_own` `context` is no renewal: {:?}",
        feed.hop
    );
    assert_eq!(feed.hop["live"], json!(FEED_LIVE), "{:?}", feed.hop);
    assert_eq!(feed.hop["turn_id"], json!("t4"), "{:?}", feed.hop);
    let said = words(feed);
    assert!(
        !said.trim().is_empty(),
        "a `context` section carries words: {:?}",
        feed.body
    );
    let line = line_of(&said, NOTE)
        .unwrap_or_else(|| panic!("the note goes out whole, as one unit: {said:?}"));
    assert!(
        line.starts_with("Notes"),
        "the unit carries its place as it stands in the window: {line:?}"
    );
    assert!(
        said.chars().count() <= budget_chars,
        "the feed is inside the feed budget ({budget_chars} characters): {} {said:?}",
        said.chars().count()
    );
    assert_eq!(
        feed_marks_of(&h, "context").len(),
        1,
        "the own feed takes the one road and books its mark: {:?}",
        feed_marks(&h)
    );
    assert!(feeds_of(&h, "renew").is_empty(), "no renewal beside it");

    // Nothing else moved: the next closed turn sends nothing, the note is
    // not fed twice.
    let again = turn(&mut h, "t5");
    assert!(
        again.is_empty(),
        "what was fed is not fed again: {again:?} {:?}",
        h.stderr
    );

    // Review 2 I1: an inner turn from outside -- a round nobody in the
    // session spoke, on the wall as `event` -- goes out the same way: ONE
    // feed, its words whole, behind its place as the start window writes it.
    h.row(backdated(0), SESSION, "e1", "event", &user(EVENT), 1);
    let sent = turn(&mut h, "t6");
    assert_eq!(
        sent.len(),
        1,
        "the inner turn from outside sends one feed: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(
        sent[0].hop["section"],
        json!("context"),
        "{:?}",
        sent[0].hop
    );
    let said = words(&sent[0]);
    assert_eq!(
        said,
        format!("(event) {EVENT}"),
        "the inner turn goes out whole, behind its place, and nothing beside it"
    );
    assert!(
        said.chars().count() <= budget_chars,
        "inside the feed budget: {} {said:?}",
        said.chars().count()
    );
    assert_eq!(
        feed_marks_of(&h, "context").len(),
        2,
        "its mark is booked like the note's: {:?}",
        feed_marks(&h)
    );
    let again = turn(&mut h, "t7");
    assert!(
        again.is_empty(),
        "an inner turn that was fed is not fed again: {again:?} {:?}",
        h.stderr
    );
    assert!(feeds_of(&h, "renew").is_empty(), "no renewal beside it");
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

#[test]
fn gh_d54g_kprofil_a_change_from_the_sessions_own_turns_moves_nothing() {
    if !shipped() {
        return;
    }
    // The start window has a target other domains contribute to
    // (`seed_target`), so a contribution is a fact of the start window.
    let mut h = feed_hive(&[
        ("policy", "live_feed", json!("1")),
        ("history", "seed_target", json!(PART_TARGET)),
        ("policy", "seed_target", json!(PART_TARGET)),
    ]);
    clear_of_midnight();
    start(&mut h, Some(limits(500, json!({"feed_own": "context"}))));

    // The person raises a new topic in this very session: nothing from
    // outside moved, so nothing is fed -- no renewal, no context.
    h.mark(
        backdated(20),
        SESSION,
        "t4",
        "topic",
        &topic_value("garden", "start"),
    );
    for turn_id in ["t5", "t6"] {
        let sent = turn(&mut h, turn_id);
        assert!(
            sent.is_empty(),
            "a topic of the session's own turns feeds nothing ({turn_id}): {sent:?} {:?}",
            h.stderr
        );
    }

    // A contribution to the start window's target moves nothing either: it
    // carries no mark of where it came from, so under `context` it is no
    // news from outside (OR-d54g.kprofil.6) -- no renewal, no context.
    h.lane(
        "in_part",
        json!({"audience_set": ROUND_E}),
        json!({}),
        json!({"messages": [], "target": PART_TARGET, "source": "presenter",
               "value": [{"name": "orchard", "purpose": "what grows there"}]}),
    );
    // Review 2 M3: the contribution stands in the ledger -- it was taken,
    // and still it moves nothing.
    assert!(
        keys_with(&h, "part:")
            .iter()
            .any(|k| k.contains(PART_TARGET) && k.contains("presenter")),
        "the contribution is held for the target: {:?} {:?}",
        keys_with(&h, ""),
        h.stderr
    );
    let sent = turn(&mut h, "t6b");
    assert!(
        sent.is_empty(),
        "a contribution to the start window's target feeds nothing: {sent:?} {:?}",
        h.stderr
    );

    // OR-d54g.kprofil.11 and .15: an inner turn that has its own road into
    // the session moves nothing. The errand the session handed out itself
    // (kind `delegation`) is a move of its own turns, and its result comes
    // back as a feed of its own; the advice a model call of the round
    // carried (kind `advice`) reaches the session with that call's answer.
    h.row(backdated(0), SESSION, "d1", "delegation", &user(ERRAND), 1);
    let sent = turn(&mut h, "t6c");
    assert!(
        sent.is_empty(),
        "the session's own errand feeds nothing: {sent:?} {:?}",
        h.stderr
    );
    h.row(backdated(0), SESSION, "a1", "advice", &user(ADVICE), 1);
    let sent = turn(&mut h, "t6d");
    assert!(
        sent.is_empty(),
        "advice a call of the round carried feeds nothing: {sent:?} {:?}",
        h.stderr
    );
    assert!(
        feed_marks(&h).is_empty(),
        "nothing went, nothing is booked: {:?}",
        feed_marks(&h)
    );

    // Something from outside still goes out, and it says nothing of the
    // topic the session raised itself nor of the contribution.
    post(&mut h, json!([note("n1", "session", NOTE)]));
    let sent = turn(&mut h, "t7");
    assert_eq!(sent.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        sent[0].hop["section"],
        json!("context"),
        "{:?}",
        sent[0].hop
    );
    let said = words(&sent[0]);
    assert!(said.contains(NOTE), "the note from outside: {said:?}");
    assert!(
        !said.contains("garden"),
        "the session's own topic is no news to it: {said:?}"
    );
    assert!(
        !said.contains("orchard"),
        "neither a contribution nor the session's own errand is fed as context: {said:?}"
    );
    assert!(
        !said.contains("greenhouse")
            && !said.contains("(delegation)")
            && !said.contains("(advice)"),
        "no inner turn with a road of its own rides the feed: {said:?}"
    );
    let again = turn(&mut h, "t8");
    assert!(
        again.is_empty(),
        "and none of them goes later: {again:?} {:?}",
        h.stderr
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

#[test]
fn gh_d54g_kprofil_what_does_not_fit_waits_for_the_next_turn() {
    if !shipped() {
        return;
    }
    // Forty tokens at three characters a token: 120 characters a feed. Each
    // note fits alone with its place, no two fit together.
    let budget_chars = 40 * RATE;
    let notes = [
        (
            "n1",
            "The chimney sweep comes on Tuesday morning at nine o'clock.",
        ),
        (
            "n2",
            "The library holds the two ordered books until Friday noon.",
        ),
        (
            "n3",
            "The choir rehearsal moves to the parish hall this Thursday.",
        ),
    ];
    // Review 2 I1: two inner turns from outside among the units, older than
    // the notes; each fits alone, no two units fit together.
    let events = [
        (
            "e1",
            "The post office keeps the parcel until Saturday at noon.",
        ),
        (
            "e2",
            "The plumber moved the visit to Wednesday after lunch.",
        ),
    ];
    // OR-d54g.kprofil.10: a note that is over the budget alone (the oldest
    // note). It is never cut, and it waits while anything else can go.
    let big = "The landlord writes that the stairwell is painted next week, floor by floor \
               from the top, that the bicycles have to leave the hall by Monday evening and \
               that the cellar doors stay locked until the paint has dried.";
    assert!(
        "Notes: ".len() + big.chars().count() > budget_chars,
        "the fixture is over the budget alone"
    );
    let mut h = hive();
    clear_of_midnight();
    start(&mut h, Some(limits(40, json!({"feed_own": "context"}))));
    h.row(backdated(3), SESSION, "e1", "event", &user(events[0].1), 1);
    h.row(backdated(2), SESSION, "e2", "event", &user(events[1].1), 1);
    post(&mut h, json!([note("n0", "session", big)]));
    // One after the other, so the last one is the newest.
    for (id, text) in notes {
        post(&mut h, json!([note(id, "session", text)]));
    }

    let mut order: Vec<&str> = Vec::new();
    let mut fed_turns = 0;
    let mut ended = false;
    for n in 4..14 {
        let sent = turn(&mut h, &format!("t{n}"));
        if sent.is_empty() {
            ended = true;
            break;
        }
        assert_eq!(sent.len(), 1, "one feed a turn: {sent:?} {:?}", h.stderr);
        assert_eq!(
            sent[0].hop["section"],
            json!("context"),
            "{:?}",
            sent[0].hop
        );
        let said = words(&sent[0]);
        if said.contains(&big[..24]) {
            // OR-d54g.kprofil.10: the unit over the budget goes alone and
            // whole, and only in a turn whose feed would be empty without it.
            assert_eq!(
                said,
                format!("Notes: {big}"),
                "the unit over the budget goes whole and alone"
            );
            assert_eq!(
                order.len(),
                notes.len() + events.len(),
                "it waited until everything that fits had gone: {order:?}"
            );
            order.push("n0");
            fed_turns += 1;
            continue;
        }
        assert!(
            said.chars().count() <= budget_chars,
            "every feed is inside the feed budget ({budget_chars} characters): {} {said:?}",
            said.chars().count()
        );
        fed_turns += 1;
        let before = order.len();
        for (id, text) in events {
            // An inner turn goes whole, behind its place, or not at all.
            if said.contains(&text[..24]) {
                assert!(
                    said.contains(&format!("(event) {text}")),
                    "no inner turn is cut inside: {id} in {said:?}"
                );
                order.push(id);
            }
        }
        for (id, text) in notes {
            // A unit goes whole or not at all: no feed carries a part of one.
            let head = &text[..24];
            if said.contains(head) {
                assert!(
                    said.contains(text),
                    "no unit is cut inside: {id} in {said:?}"
                );
                order.push(id);
            }
        }
        assert!(
            order.len() > before,
            "a feed carries at least one whole unit: {said:?}"
        );
    }
    assert_eq!(
        order.first(),
        Some(&"n3"),
        "the newest goes first: {order:?}"
    );
    let mut each = order.clone();
    each.sort_unstable();
    assert_eq!(
        each,
        vec!["e1", "e2", "n0", "n1", "n2", "n3"],
        "what did not fit waited and went out later, each unit once -- the inner turn \
         fed while an older one waited is not fed again: {order:?}"
    );
    assert_eq!(
        order.last(),
        Some(&"n0"),
        "the unit over the budget went last, alone: {order:?}"
    );
    assert!(
        ended,
        "after the last unit a closed turn sends nothing: {order:?}"
    );
    assert!(
        fed_turns > 1,
        "three units over the budget of one feed take more than one turn: {fed_turns}"
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

#[test]
fn gh_d54g_kprofil_without_the_field_the_own_feed_renews_as_before() {
    if !shipped() {
        return;
    }
    // What a renewal of before looks like: wordless, at the turn's end, the
    // body naming what moved.
    let renews_as_before = |h: &mut Hive| {
        post(h, json!([note("n1", "session", NOTE)]));
        let sent = turn(h, "t4");
        assert_eq!(sent.len(), 1, "{:?} {:?}", h.out, h.stderr);
        assert_eq!(sent[0].hop["section"], json!("renew"), "{:?}", sent[0].hop);
        assert_eq!(sent[0].hop["at_turn_end"], json!("1"), "{:?}", sent[0].hop);
        assert_eq!(words(&sent[0]), "", "a renewal carries no words");
        assert_eq!(
            sent[0].body["notes"][0]["id"],
            json!("n1"),
            "the body names what moved: {:?}",
            sent[0].body
        );
    };

    // A target that names limits and a profile without `feed_own`: its
    // limits are held, and its own feed is the renewal of before.
    let mut h = hive();
    clear_of_midnight();
    start(&mut h, Some(limits(500, json!({}))));
    assert_eq!(
        limits_held(&h, FEED_LIVE),
        1,
        "the limits of the target are held for its handle: {:?}",
        keys_with(&h, "")
    );
    renews_as_before(&mut h);
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // A target that names no limits at all: the same.
    let mut h = hive();
    start(&mut h, None);
    renews_as_before(&mut h);
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // Review 2 M4: the third value. `none` is no own feed at all -- the same
    // note, the same closed turn, nothing sent and nothing booked.
    let mut h = hive();
    start(&mut h, Some(limits(500, json!({"feed_own": "none"}))));
    post(&mut h, json!([note("n1", "session", NOTE)]));
    let sent = turn(&mut h, "t4");
    assert!(
        sent.is_empty(),
        "`feed_own` `none` sends no own feed: {sent:?} {:?}",
        h.stderr
    );
    assert!(
        feed_marks(&h).is_empty(),
        "and books no mark: {:?}",
        feed_marks(&h)
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

/// A carried note (GH #1436, `carry_notes`) is a fact of the sessions after
/// the one it was born in, never of the running session it was born in
/// (`not_live`, its handle): `seed_notes` leaves it out of that session's
/// seed and of its renewal feed. The `context` feed reads the notes by the
/// same rule, so such a row is no unit of the own feed either -- else the
/// crisis call itself would be told the crisis "was answered" while it is
/// still in it.
#[test]
fn gh_d54g_kprofil_a_carried_note_of_the_running_session_is_no_context_unit() {
    if !shipped() {
        return;
    }
    const CARRIED: &str = "Earlier the person spoke of a crisis and it was answered.";
    let mut h = hive();
    clear_of_midnight();
    start(&mut h, Some(limits(500, json!({"feed_own": "context"}))));

    // Two notes after the start window: one of another domain, and one the
    // ledger holds as carried out of this very running session.
    post(
        &mut h,
        json!([note("n1", "session", NOTE), note("n2", "session", CARRIED)]),
    );
    let born =
        h.db.execute(
            "UPDATE notes SET not_session = ?1, not_live = ?2 \
             WHERE source = 'probe' AND note_id = 'n2'",
            [SESSION, FEED_LIVE],
        )
        .expect("mark the carried note");
    assert_eq!(born, 1, "one row is the carried note");

    let sent = turn(&mut h, "t4");
    assert_eq!(
        sent.len(),
        1,
        "the note from outside still goes out: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(
        sent[0].hop["section"],
        json!("context"),
        "{:?}",
        sent[0].hop
    );
    let said = words(&sent[0]);
    assert!(
        line_of(&said, NOTE).is_some(),
        "the note from outside goes out whole: {said:?}"
    );
    assert!(
        !said.contains(CARRIED),
        "a note carried out of this running session is no unit of its feed: {said:?}"
    );
    let again = turn(&mut h, "t5");
    assert!(
        again.is_empty(),
        "nor does it go out on a later turn: {again:?} {:?}",
        h.stderr
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}
