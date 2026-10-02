//! GH #935 -- the identity budget splits an identity pack into a short form
//! in the system part and the full text as one snapshot at the head of the
//! window.
//!
//! Why: up to curator 1.3.0 the whole identity a pack carries stood in the
//! system part of EVERY call, so a long identity went to the provider in full
//! on every turn. A pack may now carry a second family, `identity_short`, and
//! a knob per role, `identity_budget`, decides what the model reads:
//!
//! - `full` (the default of every role): nothing changes. A leaf of
//!   `identity_short` is skipped when the system tree is built, so the brief
//!   is the one 1.3.0 sent, byte for byte.
//! - `split`: the system family `identity` carries the short form alone, in
//!   EVERY call. The full text stands as ONE element at the head of
//!   `messages` from the first call of a session on (and from the call after
//!   a rebuild of the session's round or a renewal of that session): a
//!   snapshot. The following calls of the session take no new one -- the
//!   window holds the same element, byte for byte (the provider's prefix
//!   cache stays warm), even when the pack changes in between; only the short
//!   form moves. It stays until the next rebuild of THIS round or renewal of
//!   the session's own window (a rebuild another session caused, a rebuild
//!   of another round, or another session's renewal, leaves it standing), a
//!   new session starts or the round changes. Since GH #943 every round has a
//!   plan of its own, because one plan per hive let the rebuild of one round
//!   replace the window of another: a rebuild is an event only for the
//!   windows of its round. Nothing of it is trimmable: it is no block of the
//!   wall, a release cannot reach it.
//! - The snapshot, the renewal it was taken after and the note below are kept
//!   PER SESSION (`identity_full:<session>`, `renewed_last:<session>`,
//!   `identity_short_missing:<session>`): two sessions of one hive, of two
//!   rounds, never move each other's snapshot (review I-4) -- an event in one
//!   round must not show as a jump in the window of another.
//! - `split` on a pack without a short form behaves like `full` and says so
//!   once per session (a `marks` row `identity_short_missing`, under the
//!   round of the call that wrote it).
//!
//! Measured where the curator meets the model: the message on route `brain`
//! (its `system` tree and its `messages`), plus the ledger rows the contract
//! names. The hive runs in one process (`support/curator_hive.rs`): the
//! shipped scripts, the shipped edges under the colony's CEL, a ledger behind
//! the store's own dispatcher; the clock and the summarizer are recorders.
//!
//! The `full` case is pinned against a fixture recorded from curator 1.3.0
//! (`fixtures/gh935_full_1_3_0.json`). With `MECLAW_GH935_BLESS=1` the test
//! records it instead: it then sends the pack WITHOUT `identity_short`,
//! because 1.3.0 refuses a pack with a family it does not know as a whole.
//!
//! Guarded like every template-reading test (GH #49).

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Map, Value, json};

/// The full identity -- deliberately long, one line (a raw substring search
/// over a serialised tree must be able to find it).
const VOLL: &str = "You are Tern, the navigator of the long watch. You keep the charts, the \
                    logbook and the course; you speak plainly, you name the bearing before \
                    the reason, and you never guess a depth you have not sounded.";
const KURZ: &str = "You are Tern, the navigator.";
const VOLL2: &str = "You are Tern, the navigator of the dog watch. You keep the stars, the \
                     soundings and the tide tables; you answer in one sentence and you mark \
                     every guess as a guess.";
const KURZ2: &str = "You are Tern, the night navigator.";

const VOLL3: &str = "You are Tern, the navigator of the morning watch. You keep the log of the \
                     landfall and the pilot's notes; you speak last and you never round a \
                     bearing.";
const KURZ3: &str = "You are Tern, the morning navigator.";

/// Two rounds of one hive, as a channel stamps them (unsorted, with spaces)
/// and as the ledger stores them (canonical).
const EA: &str = r#"["member:e", "member:a"]"#;
const CANON_EA: &str = r#"["member:a","member:e"]"#;
const EB: &str = r#"["member:e", "member:b"]"#;
const CANON_EB: &str = r#"["member:b","member:e"]"#;

const FIXTURE: &str = "crates/meclaw-cells/tests/fixtures/gh935_full_1_3_0.json";
const BLESS: &str = "MECLAW_GH935_BLESS";

fn split_hive(more: &[(&str, &str, Value)]) -> Hive {
    let mut over: Vec<(&str, &str, Value)> = vec![("policy", "identity_budget", json!("split"))];
    over.extend(more.iter().cloned());
    Hive::with(&over)
}

/// An identity pack on `in_pack`; the pack must be taken whole (empty
/// `error_code` on `pack_ack`).
fn pack(h: &mut Hive, system: Value) {
    h.out.clear();
    h.lane("in_pack", json!({}), json!({}), json!({ "system": system }));
    let ack = h.routed("pack_ack");
    assert_eq!(
        ack.len(),
        1,
        "one pack, one ack: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(
        ack[0].hop["error_code"], "",
        "the pack is taken: {:?}",
        ack[0].hop
    );
}

fn both(full: &str, short: &str) -> Value {
    json!({"identity": {"text": full}, "identity_short": {"text": short}})
}

/// The element the full identity travels in.
fn block(full: &str) -> Value {
    json!({"origin": "system", "type": "text", "text": full})
}

/// The system tree the model's `llm` holds after `call`: the policy sends
/// changed families (and the gated ones) as whole `$replace` roots, so the
/// tree the model reads is the fold of every brief so far.
fn hold(held: &mut Map<String, Value>, call: &Msg) {
    for (f, node) in call
        .body
        .get("system")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
    {
        let mut n = node.as_object().cloned().unwrap_or_default();
        n.remove("$replace");
        if n.is_empty() {
            held.remove(&f);
        } else {
            held.insert(f, Value::Object(n));
        }
    }
}

fn system_elements(call: &Msg) -> Vec<Value> {
    call.messages()
        .into_iter()
        .filter(|m| m["origin"] == "system")
        .collect()
}

/// What a `split` call with a short form must show: the short form as the
/// whole `identity` family of the tree the model holds, no `identity_short`
/// family anywhere, no full text anywhere in the system part, and exactly one
/// system element -- `full` -- at the head of `messages`.
fn assert_split(call: &Msg, held: &Map<String, Value>, short: &str, full: &str, what: &str) {
    assert_eq!(
        held.get("identity"),
        Some(&json!({ "text": short })),
        "{what}: the identity family is the short form alone: {held:?}"
    );
    let sent = call.body.get("system").cloned().unwrap_or(Value::Null);
    for tree in [canonical(&Value::Object(held.clone())), canonical(&sent)] {
        for f in [VOLL, VOLL2] {
            assert!(
                !tree.contains(f),
                "{what}: a full identity in the system part: {tree}"
            );
        }
        assert!(
            !tree.contains("identity_short"),
            "{what}: no identity_short family reaches the model: {tree}"
        );
    }
    let msgs = call.messages();
    assert_eq!(
        msgs.first(),
        Some(&block(full)),
        "{what}: the full identity heads the window: {msgs:?}"
    );
    assert_eq!(
        system_elements(call).len(),
        1,
        "{what}: exactly one identity element: {msgs:?}"
    );
}

/// A whole turn in the round `round` (the harness's `turn` runs in its
/// standard round only): the round in on `in_curate`, the final answer back
/// on the tap with `extra` as the provider's usage.
fn turn_in(
    h: &mut Hive,
    round: &str,
    session: &str,
    turn_id: &str,
    ask: &str,
    reply: &str,
    extra: Value,
) -> Msg {
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": session, "turn_id": turn_id, "iter": "0", "channel": "test",
               "audience_set": round}),
        json!({"session_id": session, "turn_id": turn_id, "iter": "0", "phase": ""}),
        json!({"messages": [user(ask)], "system": mode("Be brief.")}),
    );
    let calls = h.routed("brain");
    assert_eq!(
        calls.len(),
        1,
        "one round, one call: {:?} {:?}",
        h.out,
        h.stderr
    );
    let call = calls[0].clone();
    h.tap(&call, "stop", extra, json!([said(reply)]));
    call
}

/// How many ledger writes (anything but a read or a delete) the hive sent
/// for the state key `key` since `ledger_ops` was last cleared -- measured at
/// the store, not read off a script.
fn state_writes(h: &Hive, key: &str) -> usize {
    h.ledger_ops
        .iter()
        .filter(|(_, op)| {
            op["table"] == "state"
                && !matches!(op["operation"].as_str(), Some("select") | Some("delete"))
                && (op["row"]["key"] == key || op["where"]["key"] == key)
        })
        .count()
}

/// How many `identity_full` blocks the hive wrote since `ledger_ops` was
/// last cleared.
fn snapshot_blocks_written(h: &Hive) -> usize {
    h.ledger_ops
        .iter()
        .filter(|(_, op)| {
            op["table"] == "blocks"
                && op["operation"] == "insert"
                && op["row"]["kind"] == "identity_full"
        })
        .count()
}

/// The session's snapshot state, parsed: canonical
/// `{audience, element, epoch, hash, renewed, session}`.
fn snapshot_of(h: &Hive, session: &str) -> Value {
    let text = h.state(&format!("identity_full:{session}"));
    sj::from_str(&text).unwrap_or_else(|e| panic!("identity_full:{session} = {text:?}: {e}"))
}

/// The element at the head of a call's window.
fn head_of(call: &Msg) -> String {
    canonical(call.messages().first().expect("a window with a head"))
}

#[test]
fn a_split_pack_sends_the_short_form_in_the_system_part_and_the_full_one_at_the_head() {
    if !shipped() {
        return;
    }
    let mut h = split_hive(&[]);
    pack(&mut h, both(VOLL, KURZ));
    let mut held = Map::new();
    let call = turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    hold(&mut held, &call);
    assert_split(&call, &held, KURZ, VOLL, "call 1");
    // The element is a block of its own in the ledger, at its place in the call.
    let el = &call.messages()[0];
    let rows = h.rows("SELECT body FROM blocks WHERE kind = 'identity_full'");
    assert_eq!(rows.len(), 1, "one identity_full block: {rows:?}");
    assert_eq!(rows[0][0], json!(canonical(el)));
    assert_eq!(
        listed_messages(&h, &call).first(),
        Some(el),
        "call_blocks list the element first"
    );
}

/// Call 2 of a `split` session (review I-6, OR-S3-71): its system part
/// carries the short form alone -- never the full pack in `system.identity`
/// -- and it takes NO new snapshot: the element at the head of the window is
/// the one of call 1, byte for byte, no `identity_full` block is written and
/// the session's state `identity_full:<session>` stays as it was. A new pack
/// in between moves the short form, not the snapshot.
#[test]
fn the_next_call_of_the_session_carries_the_short_form_and_takes_no_new_snapshot() {
    if !shipped() {
        return;
    }
    let mut h = split_hive(&[]);
    pack(&mut h, both(VOLL, KURZ));
    let mut held = Map::new();
    let first = turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    hold(&mut held, &first);
    assert_split(&first, &held, KURZ, VOLL, "call 1");
    let head = canonical(&first.messages()[0]);
    let snap = h.state("identity_full:s1");
    assert!(!snap.is_empty(), "call 1 took the session's snapshot");
    let blocks_before = h.rows("SELECT hash, first_seen FROM blocks WHERE kind = 'identity_full'");
    // Call 2, nothing changed.
    h.ledger_ops.clear();
    let second = turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    hold(&mut held, &second);
    assert_split(&second, &held, KURZ, VOLL, "call 2");
    let sent = canonical(&second.body.get("system").cloned().unwrap_or(Value::Null));
    assert!(
        !sent.contains(VOLL),
        "call 2 sends no full pack in its system part: {sent}"
    );
    assert_eq!(
        canonical(&second.messages()[0]),
        head,
        "call 2 holds the element of call 1"
    );
    assert_eq!(
        snapshot_blocks_written(&h),
        0,
        "call 2 writes no identity_full block: {:?}",
        h.ledger_ops
    );
    assert_eq!(
        state_writes(&h, "identity_full:s1"),
        0,
        "call 2 takes no new snapshot"
    );
    assert_eq!(
        h.state("identity_full:s1"),
        snap,
        "the snapshot state stands"
    );
    assert_eq!(
        h.rows("SELECT hash, first_seen FROM blocks WHERE kind = 'identity_full'"),
        blocks_before,
        "the identity_full block stands"
    );
    // A new pack in between: the short form moves, the snapshot does not.
    pack(&mut h, both(VOLL2, KURZ2));
    let third = turn(&mut h, "s1", "t3", "q3", "a3", json!({}));
    hold(&mut held, &third);
    assert_split(&third, &held, KURZ2, VOLL, "call after a new pack");
    assert_eq!(
        canonical(&third.messages()[0]),
        head,
        "the snapshot outlives a new pack within the session"
    );
    assert_eq!(
        h.state("identity_full:s1"),
        snap,
        "the snapshot state stands"
    );
}

#[test]
fn a_rebuild_takes_the_snapshot_anew() {
    if !shipped() {
        return;
    }
    // A small window, so the second report crosses `compress_at` (the
    // pattern of `curator_cells.rs`, `compress_at_crossed_rebuilds`).
    let mut h = split_hive(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "context_window", json!(1000)),
    ]);
    pack(&mut h, both(VOLL, KURZ));
    let mut held = Map::new();
    let c1 = turn(
        &mut h,
        "s1",
        "t1",
        "q1",
        "a1",
        json!({"tokens_prompt": 400}),
    );
    hold(&mut held, &c1);
    assert_split(&c1, &held, KURZ, VOLL, "call 1");
    let c2 = turn(
        &mut h,
        "s1",
        "t2",
        "q2",
        "a2",
        json!({"tokens_prompt": 500}),
    );
    hold(&mut held, &c2);
    assert_split(&c2, &held, KURZ, VOLL, "call 2, before the rebuild");
    pack(&mut h, both(VOLL2, KURZ2));
    let add = last_add(&h);
    assert_eq!(add.body["emit_body"]["reason"], "compress", "{add:?}");
    h.fire(&add);
    while !h.summ.is_empty() {
        h.answer("S1: q1 a1.", "stop");
    }
    assert_ne!(
        h.plan_in(ROUND_E),
        json!({}),
        "the rebuild finished: {:?}",
        h.stderr
    );
    // The call runs in the round the rebuild ran in, because a snapshot is
    // taken anew only after a rebuild of its own round (GH #943).
    let c3 = turn(&mut h, "s1", "t3", "q3", "a3", json!({}));
    hold(&mut held, &c3);
    assert_split(
        &c3,
        &held,
        KURZ2,
        VOLL2,
        "the call after the rebuild of its round",
    );
}

#[test]
fn a_renewal_takes_the_snapshot_anew() {
    if !shipped() {
        return;
    }
    let mut h = split_hive(&[]);
    pack(&mut h, both(VOLL, KURZ));
    let mut held = Map::new();
    let c1 = turn(&mut h, "call-1", "t1", "q1", "a1", json!({}));
    hold(&mut held, &c1);
    assert_split(&c1, &held, KURZ, VOLL, "call 1");
    pack(&mut h, both(VOLL2, KURZ2));
    let c2 = turn(&mut h, "call-1", "t2", "q2", "a2", json!({}));
    hold(&mut held, &c2);
    assert_split(&c2, &held, KURZ2, VOLL, "call 2, before the renewal");
    // The member renewed the live session of the call (as `gh896` sends it).
    h.out.clear();
    h.lane(
        "in_renewed",
        json!({"session_id": "call-1", "call_id": "call-1"}),
        json!({"call_id": "call-1", "session_id": "call-1", "renewal_n": 1,
               "renewed_at": 1_790_000_000_000_u64}),
        json!({"messages": []}),
    );
    assert_eq!(
        h.routed("sidecar").len(),
        1,
        "the renewal ran: {:?} {:?}",
        h.out,
        h.stderr
    );
    let c3 = turn(&mut h, "call-1", "t3", "q3", "a3", json!({}));
    hold(&mut held, &c3);
    assert_split(&c3, &held, KURZ2, VOLL2, "the call after the renewal");
}

#[test]
fn a_split_pack_without_a_short_form_stays_full_and_says_so_once() {
    if !shipped() {
        return;
    }
    let mut h = split_hive(&[]);
    pack(&mut h, json!({"identity": {"text": VOLL}}));
    let mut held = Map::new();
    let c1 = turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    hold(&mut held, &c1);
    let c2 = turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    hold(&mut held, &c2);
    for (call, what) in [(&c1, "call 1"), (&c2, "call 2")] {
        assert_eq!(
            held.get("identity"),
            Some(&json!({ "text": VOLL })),
            "{what}: the full identity stays in the system part"
        );
        assert!(
            system_elements(call).is_empty(),
            "{what}: no identity element: {:?}",
            call.messages()
        );
    }
    let marks = h.rows(
        "SELECT session_id, value FROM marks WHERE kind = 'identity_short_missing' ORDER BY seq",
    );
    assert_eq!(marks.len(), 1, "one mark per session: {marks:?}");
    assert_eq!(marks[0][0], json!("s1"));
    assert_eq!(
        marks[0][1], c1.hop["curator_call"],
        "the mark names the first call"
    );
}

// ══════════════════════════════════════════ per session, per round (review I-4, I-5)

/// Two sessions of two rounds, interleaved, on a pack WITHOUT a short form:
/// each session is told once, and only once, under its own round -- the
/// note's state is the session's (`identity_short_missing:<session>`), so
/// the other session's call between two of its own does not make it forget
/// it was told (review I-4 (a): s1, s2, s1 used to write three marks).
#[test]
fn interleaved_sessions_without_a_short_form_are_each_told_once_under_their_round() {
    if !shipped() {
        return;
    }
    let mut h = split_hive(&[]);
    pack(&mut h, json!({"identity": {"text": VOLL}}));
    let mut held = Map::new();
    let calls = [
        turn_in(&mut h, EA, "s1", "t1", "q1", "a1", json!({})),
        turn_in(&mut h, EB, "s2", "u1", "p1", "b1", json!({})),
        turn_in(&mut h, EA, "s1", "t2", "q2", "a2", json!({})),
        turn_in(&mut h, EB, "s2", "u2", "p2", "b2", json!({})),
    ];
    for (i, call) in calls.iter().enumerate() {
        hold(&mut held, call);
        assert_eq!(
            held.get("identity"),
            Some(&json!({ "text": VOLL })),
            "call {i}: the full identity stays in the system part"
        );
        assert!(
            system_elements(call).is_empty(),
            "call {i}: no identity element: {:?}",
            call.messages()
        );
    }
    for (session, canon, first) in [("s1", CANON_EA, &calls[0]), ("s2", CANON_EB, &calls[1])] {
        let marks = h.rows(&format!(
            "SELECT audience_set, value FROM marks WHERE kind = 'identity_short_missing' \
             AND session_id = '{session}' ORDER BY seq"
        ));
        assert_eq!(marks.len(), 1, "{session}: one mark per session: {marks:?}");
        assert_eq!(
            marks[0][0],
            json!(canon),
            "{session}: the mark carries its session's round"
        );
        assert_eq!(
            marks[0][1], first.hop["curator_call"],
            "{session}: the mark names the session's first call"
        );
        assert_eq!(
            h.rows(&format!(
                "SELECT COUNT(*) FROM state WHERE key = 'identity_short_missing:{session}'"
            ))[0][0],
            json!(1),
            "{session}: the note is kept under the session"
        );
    }
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM state WHERE key = 'identity_short_missing'")[0][0],
        json!(0),
        "no hive-wide note"
    );
}

/// Two `split` sessions of two rounds, interleaved: each takes ONE snapshot
/// and holds it -- the other session's call between two of its own takes
/// none for it (review I-4 (b): a hive-wide snapshot state made the two
/// sessions re-snapshot each other on every call, block delete + insert
/// each time, against the cache the snapshot is for).
#[test]
fn interleaved_split_sessions_each_hold_their_own_snapshot() {
    if !shipped() {
        return;
    }
    let mut h = split_hive(&[]);
    pack(&mut h, both(VOLL, KURZ));
    let mut held = Map::new();
    h.ledger_ops.clear();
    let c1 = turn_in(&mut h, EA, "s1", "t1", "q1", "a1", json!({}));
    hold(&mut held, &c1);
    assert_split(&c1, &held, KURZ, VOLL, "s1 call 1");
    let c2 = turn_in(&mut h, EB, "s2", "u1", "p1", "b1", json!({}));
    hold(&mut held, &c2);
    assert_split(&c2, &held, KURZ, VOLL, "s2 call 1");
    let snap1 = h.state("identity_full:s1");
    let snap2 = h.state("identity_full:s2");
    let c3 = turn_in(&mut h, EA, "s1", "t2", "q2", "a2", json!({}));
    hold(&mut held, &c3);
    assert_split(&c3, &held, KURZ, VOLL, "s1 call 2");
    let c4 = turn_in(&mut h, EB, "s2", "u2", "p2", "b2", json!({}));
    hold(&mut held, &c4);
    assert_split(&c4, &held, KURZ, VOLL, "s2 call 2");
    assert_eq!(head_of(&c3), head_of(&c1), "s1 holds its element");
    assert_eq!(head_of(&c4), head_of(&c2), "s2 holds its element");
    for (session, canon, first, snap) in
        [("s1", CANON_EA, &c1, &snap1), ("s2", CANON_EB, &c2, &snap2)]
    {
        assert_eq!(
            state_writes(&h, &format!("identity_full:{session}")),
            1,
            "{session}: one snapshot over four interleaved calls: {:?}",
            h.ledger_ops
        );
        assert_eq!(
            &h.state(&format!("identity_full:{session}")),
            snap,
            "{session}: the snapshot state stands"
        );
        let s = snapshot_of(&h, session);
        assert_eq!(s["session"], session, "{session}: {s}");
        assert_eq!(
            s["audience"], canon,
            "{session}: the round it was taken in: {s}"
        );
        assert_eq!(
            canonical(&s["element"]),
            head_of(first),
            "{session}: the state holds the element it sent: {s}"
        );
        assert_eq!(
            s["hash"],
            json!(sha256_hex(&canonical(&s["element"]))),
            "{session}: {s}"
        );
    }
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM state WHERE key = 'identity_full'")[0][0],
        json!(0),
        "no hive-wide snapshot"
    );
}

/// A renewal of one session and a rebuild a call of that session caused
/// leave the snapshot of another session standing, while the session they
/// belong to takes a new one (review I-4 (c): with hive-wide keys an event
/// in {e,b} made the window head of {e,a} jump -- a time channel). The
/// renewal runs through the curator's own lane (`in_renewed`), which writes
/// `renewed_last:<session>`; the rebuild is the one of
/// `a_rebuild_takes_the_snapshot_anew` (a small window crossed by the
/// report of s2's call).
#[test]
fn a_renewal_or_rebuild_of_another_session_leaves_the_snapshot_standing() {
    if !shipped() {
        return;
    }
    let mut h = split_hive(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "context_window", json!(1000)),
    ]);
    pack(&mut h, both(VOLL, KURZ));
    let mut held = Map::new();
    let a1 = turn_in(&mut h, EA, "s1", "t1", "q1", "a1", json!({}));
    hold(&mut held, &a1);
    assert_split(&a1, &held, KURZ, VOLL, "s1 call 1");
    let b1 = turn_in(&mut h, EB, "s2", "u1", "p1", "b1", json!({}));
    hold(&mut held, &b1);
    assert_split(&b1, &held, KURZ, VOLL, "s2 call 1");
    let head1 = head_of(&a1);

    // s2 is renewed, with a new pack in between.
    pack(&mut h, both(VOLL2, KURZ2));
    h.out.clear();
    h.lane(
        "in_renewed",
        json!({"session_id": "s2", "call_id": "s2", "channel": "test", "audience_set": EB}),
        json!({"call_id": "s2", "session_id": "s2", "renewal_n": 1,
               "renewed_at": 1_790_000_000_000_u64}),
        json!({"messages": []}),
    );
    assert_eq!(
        h.routed("sidecar").len(),
        1,
        "the renewal ran: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        !h.state("renewed_last:s2").is_empty(),
        "the renewal is kept under its session"
    );
    let stray =
        h.rows("SELECT COUNT(*) FROM state WHERE key IN ('renewed_last', 'renewed_last:s1')");
    assert_eq!(stray[0][0], json!(0), "no hive-wide renewal and none of s1");
    h.ledger_ops.clear();
    let a2 = turn_in(&mut h, EA, "s1", "t2", "q2", "a2", json!({}));
    hold(&mut held, &a2);
    assert_split(&a2, &held, KURZ2, VOLL, "s1 after s2's renewal");
    assert_eq!(head_of(&a2), head1, "s2's renewal moved s1's snapshot");
    assert_eq!(state_writes(&h, "identity_full:s1"), 0);
    let b2 = turn_in(&mut h, EB, "s2", "u2", "p2", "b2", json!({}));
    hold(&mut held, &b2);
    assert_split(&b2, &held, KURZ2, VOLL2, "s2 after its renewal");

    // A rebuild caused by s2's call, with a third pack in between.
    let b3 = turn_in(
        &mut h,
        EB,
        "s2",
        "u3",
        "p3",
        "b3",
        json!({"tokens_prompt": 600}),
    );
    hold(&mut held, &b3);
    assert_split(&b3, &held, KURZ2, VOLL2, "s2 before the rebuild");
    pack(&mut h, both(VOLL3, KURZ3));
    let add = last_add(&h);
    assert_eq!(add.body["emit_body"]["reason"], "compress", "{add:?}");
    h.fire(&add);
    while !h.summ.is_empty() {
        h.answer("S1: p1 b1.", "stop");
    }
    // The rebuild is {e,b}'s: s2's round holds the new plan, and s1's round
    // {e,a} has none, because since GH #943 a rebuild writes the plan of its
    // own round only -- s1's snapshot stands for that reason alone, and for
    // the session rule (review I-4) besides.
    let plan = h.plan_in(EB);
    assert_ne!(plan, json!({}), "the rebuild finished: {:?}", h.stderr);
    assert_eq!(
        plan["session"], "s2",
        "s2's call caused the rebuild: {plan}"
    );
    assert_eq!(h.plan(), plan, "the newest plan is {{e,b}}'s");
    assert_eq!(h.plan_in(EA), json!({}), "{{e,a}} was not rebuilt");
    h.ledger_ops.clear();
    let a3 = turn_in(&mut h, EA, "s1", "t3", "q3", "a3", json!({}));
    hold(&mut held, &a3);
    assert_split(&a3, &held, KURZ3, VOLL, "s1 after s2's rebuild");
    assert_eq!(head_of(&a3), head1, "s2's rebuild moved s1's snapshot");
    assert_eq!(state_writes(&h, "identity_full:s1"), 0);
    let b4 = turn_in(&mut h, EB, "s2", "u4", "p4", "b4", json!({}));
    hold(&mut held, &b4);
    assert_split(&b4, &held, KURZ3, VOLL3, "s2 after its rebuild");
}

/// One session moves from round {e,a} to {e,b}: the new round takes a new
/// snapshot under its own audience, and the element taken under {e,a} is
/// never sent under {e,b} (review I-5). `blocks` has no audience column, so
/// the round of a snapshot lives in the state alone -- pinned here.
#[test]
fn a_session_that_changes_its_round_takes_a_new_snapshot() {
    if !shipped() {
        return;
    }
    let mut h = split_hive(&[]);
    pack(&mut h, both(VOLL, KURZ));
    let mut held = Map::new();
    let c1 = turn_in(&mut h, EA, "s1", "t1", "q1", "a1", json!({}));
    hold(&mut held, &c1);
    assert_split(&c1, &held, KURZ, VOLL, "{e,a}");
    let first = snapshot_of(&h, "s1");
    assert_eq!(first["audience"], CANON_EA, "{first}");
    // The identity changes; under {e,a} the snapshot would stand.
    pack(&mut h, both(VOLL2, KURZ2));
    h.ledger_ops.clear();
    let c2 = turn_in(&mut h, EB, "s1", "t2", "q2", "a2", json!({}));
    hold(&mut held, &c2);
    assert_split(&c2, &held, KURZ2, VOLL2, "{e,b}");
    assert_ne!(
        head_of(&c2),
        head_of(&c1),
        "a new snapshot for the new round"
    );
    assert_eq!(
        state_writes(&h, "identity_full:s1"),
        1,
        "{:?}",
        h.ledger_ops
    );
    let second = snapshot_of(&h, "s1");
    assert_eq!(second["audience"], CANON_EB, "{second}");
    assert_ne!(second, first, "the state names the new round's snapshot");
    let c3 = turn_in(&mut h, EB, "s1", "t3", "q3", "a3", json!({}));
    hold(&mut held, &c3);
    assert_split(&c3, &held, KURZ2, VOLL2, "{e,b} again");
    for (call, what) in [(&c2, "{e,b} call 1"), (&c3, "{e,b} call 2")] {
        let window = canonical(&Value::Array(call.messages()));
        assert!(
            !window.contains(VOLL),
            "{what}: the element taken under {{e,a}} went out under {{e,b}}: {window}"
        );
    }
}

/// The note of a `split` session without a short form carries the round of
/// the call that wrote it, and a later call of the session in another round
/// does not write a second one (review I-5).
#[test]
fn the_missing_short_form_note_carries_the_round_of_its_call() {
    if !shipped() {
        return;
    }
    let mut h = split_hive(&[]);
    pack(&mut h, json!({"identity": {"text": VOLL}}));
    let c1 = turn_in(&mut h, EB, "s1", "t1", "q1", "a1", json!({}));
    turn_in(&mut h, EA, "s1", "t2", "q2", "a2", json!({}));
    let marks = h.rows(
        "SELECT session_id, audience_set, value FROM marks \
         WHERE kind = 'identity_short_missing' ORDER BY seq",
    );
    assert_eq!(marks.len(), 1, "one mark per session: {marks:?}");
    assert_eq!(marks[0][0], json!("s1"));
    assert_eq!(
        marks[0][1],
        json!(CANON_EB),
        "the round of the call: {marks:?}"
    );
    assert_eq!(marks[0][2], c1.hop["curator_call"]);
}

/// Replaces what a run makes up anew and nothing else: the call ids (random
/// per run; a body that named one would differ between any two runs) and
/// RFC 3339 stamps (the wall clock of the run). The scenario below is chosen
/// so that neither is expected in a brief's system tree or messages -- the
/// normalisation is a guard against a flaky pin, not a mask over content.
fn normalised(v: &Value, ids: &[String]) -> Value {
    match v {
        Value::String(s) => {
            let mut s = s.clone();
            for (i, id) in ids.iter().enumerate() {
                if !id.is_empty() {
                    s = s.replace(id.as_str(), &format!("<call:{i}>"));
                }
            }
            Value::String(without_stamps(&s))
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| normalised(x, ids)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), normalised(x, ids)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.fff][Z|+HH:MM]` -> `<ts>`.
fn without_stamps(s: &str) -> String {
    let b = s.as_bytes();
    let shape = b"dddd-dd-ddTdd:dd:dd";
    let fits = |at: usize| {
        at + shape.len() <= b.len()
            && shape.iter().zip(&b[at..]).all(|(p, c)| {
                if *p == b'd' {
                    c.is_ascii_digit()
                } else {
                    p == c
                }
            })
    };
    let mut out = String::new();
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        if fits(i) {
            let mut j = i + shape.len();
            if j < b.len() && b[j] == b'.' {
                j += 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            if j < b.len() && b[j] == b'Z' {
                j += 1;
            } else if j + 6 <= b.len()
                && (b[j] == b'+' || b[j] == b'-')
                && b[j + 1].is_ascii_digit()
                && b[j + 2].is_ascii_digit()
                && b[j + 3] == b':'
            {
                j += 6;
            }
            out.push_str(&s[last..i]);
            out.push_str("<ts>");
            i = j;
            last = j;
        } else {
            i += 1;
        }
    }
    out.push_str(&s[last..]);
    out
}

#[test]
fn the_full_budget_sends_what_1_3_0_sent() {
    if !shipped() {
        return;
    }
    let bless = std::env::var(BLESS).is_ok_and(|v| v == "1");
    let mut h = Hive::new();
    let mut p = json!({"identity": {"text": VOLL}, "persona": {"voice": {"text": "calm"}}});
    if !bless {
        // The short form rides along and must change nothing.
        p["identity_short"] = json!({"text": KURZ});
    }
    pack(&mut h, p);
    let calls = [
        turn(&mut h, "s1", "t1", "q1", "a1", json!({})),
        turn(&mut h, "s1", "t2", "q2", "a2", json!({})),
        turn(&mut h, "s1", "t3", "q3", "a3", json!({})),
    ];
    let ids: Vec<String> = calls
        .iter()
        .map(|c| c.hop["curator_call"].as_str().unwrap_or("").to_string())
        .collect();
    let got = Value::Array(
        calls
            .iter()
            .map(|c| {
                normalised(
                    &json!({"system": c.body.get("system").cloned().unwrap_or(Value::Null),
                            "messages": c.messages()}),
                    &ids,
                )
            })
            .collect(),
    );
    let path = repo(FIXTURE);
    if bless {
        std::fs::write(
            &path,
            sj::to_string_pretty(&json!({ "calls": got })).expect("serialise") + "\n",
        )
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        return;
    }
    assert!(
        path.is_file(),
        "{} is missing: record it once against the curator 1.3.0 scripts with {BLESS}=1",
        path.display()
    );
    let want = read_json(&path);
    assert_eq!(
        canonical(&got),
        canonical(&want["calls"]),
        "`full` sends the brief of 1.3.0, byte for byte"
    );
}

/// Review M-4: a round-less call on a `split` pack without a short form writes
/// two marks -- `missing_audience` at `epoch_us(stamp)` (OR-BD-4) and the
/// note at one past it. `marks` has no key to order two rows of one `seq`.
#[test]
fn a_round_less_note_is_one_seq_past_its_calls_missing_audience() {
    if !shipped() {
        return;
    }
    let mut h = split_hive(&[]);
    pack(&mut h, json!({"identity": {"text": VOLL}}));
    let c1 = turn_in(&mut h, "", "s1", "t1", "q1", "a1", json!({}));
    let seqs = h.rows(&format!(
        "SELECT kind, seq FROM marks WHERE value = '{}' \
         AND kind IN ('missing_audience', 'identity_short_missing') ORDER BY seq",
        c1.hop["curator_call"].as_str().unwrap()
    ));
    assert_eq!(seqs.len(), 2, "both marks of the call: {seqs:?}");
    assert_eq!(seqs[0][0], json!("missing_audience"), "{seqs:?}");
    assert_eq!(
        seqs[1][1].as_i64().unwrap(),
        seqs[0][1].as_i64().unwrap() + 1,
        "the note is one past the call's own seq: {seqs:?}"
    );
}
