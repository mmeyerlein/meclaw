//! GH #1225 (K-1, OR-KF-5) -- a replayed round stamps the time of its turn on
//! the wall.
//!
//! WHY this file exists: `./policy` decides by the age of a wall row what the
//! window shows raw, what it shows condensed and what it drops. Until 1.11.5
//! the age was `at`, the ingest clock `./intake` stamps on every row, so a week
//! replayed in a minute (a lab, a memory benchmark) was one day to the window:
//! the replay measured a different system. The ledger now carries
//! `wall.turn_at` (text, default `""`), and `./intake` writes the turn's own
//! time there on a replay channel (`replay_channel_prefix` + `happened_at`,
//! the GH #1018 switch); `seq` and `at` stay the order and the clock of ingest.
//! A live channel, a switch that is off and a time that does not read all
//! leave `""` -- the last one said on stderr, never swallowed.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const REPLAYED: &str = "2020-01-02T03:04:05Z";
const REPLAYED_WALL: &str = "2020-01-02T03:04:05.000000Z";

/// One round on `channel` and its answer on the tap, `happened_at` in the
/// context where the HTTP door puts the request header.
fn round_on(h: &mut Hive, channel: &str, happened_at: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "channel": channel,
                     "audience_set": ROUND_E, "happened_at": happened_at});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("we adopted a dog last week")]}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "one round, one call: {:?}", h.out);
    h.out.clear();
    h.tap(&calls[0], "stop", json!({}), json!([said("a dog!")]));
}

/// `(kind, turn_at, at)` of every wall row, in ingest order.
fn wall(h: &Hive) -> Vec<(String, Value, String)> {
    h.rows("SELECT kind, turn_at, at FROM wall ORDER BY seq")
        .into_iter()
        .map(|r| {
            (
                r[0].as_str().unwrap_or("").to_string(),
                r[1].clone(),
                r[2].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

#[test]
fn a_replay_channel_stamps_turn_time() {
    if !shipped() {
        return;
    }
    // A replay channel with a time: every row of the round carries it, the
    // person's turn and the answer alike; the ingest clock stays in `at`.
    let mut h = Hive::with(&[("intake", "replay_channel_prefix", json!("replay-"))]);
    round_on(&mut h, "replay-c1-s1", REPLAYED);
    let rows = wall(&h);
    let kinds: Vec<&str> = rows.iter().map(|(k, _, _)| k.as_str()).collect();
    assert_eq!(kinds, ["user", "assistant"], "{rows:?}");
    for (kind, turn_at, at) in &rows {
        assert_eq!(turn_at, &json!(REPLAYED_WALL), "{kind}: {rows:?}");
        assert_ne!(at, REPLAYED_WALL, "`at` keeps the ingest clock: {rows:?}");
    }

    // No prefix (production), a live channel beside a prefix: "" on every row.
    for (prefix, channel) in [("", "replay-c1-s1"), ("replay-", "telegram-42")] {
        let mut h = Hive::with(&[("intake", "replay_channel_prefix", json!(prefix))]);
        round_on(&mut h, channel, REPLAYED);
        let rows = wall(&h);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(
            rows.iter().all(|(_, t, _)| t == &json!("")),
            "prefix {prefix:?} channel {channel:?}: {rows:?}"
        );
        assert!(
            h.stderr.iter().all(|e| !e.contains("happened_at")),
            "{:?}",
            h.stderr
        );
    }

    // A time that does not read: "" -- and said, once per message, without
    // the value (a caller's header can carry anything).
    let mut h = Hive::with(&[("intake", "replay_channel_prefix", json!("replay-"))]);
    round_on(&mut h, "replay-c1-s1", "last week of 1999");
    let rows = wall(&h);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|(_, t, _)| t == &json!("")), "{rows:?}");
    let said: Vec<&String> = h
        .stderr
        .iter()
        .filter(|e| e.contains("happened_at on replay channel is no ISO 8601 time"))
        .collect();
    assert!(
        !said.is_empty(),
        "an unreadable time is said: {:?}",
        h.stderr
    );
    assert!(
        said.iter().all(|e| !e.contains("1999")),
        "the value is not echoed: {said:?}"
    );

    // A row written without the column (an older writer, an import) reads
    // the declared default: "", never NULL.
    let mut h = Hive::new();
    h.row(days_ago(1, 9), "s0", "t0", "user", &user("old row"), 0);
    assert_eq!(
        h.rows("SELECT turn_at FROM wall WHERE session_id = 's0'"),
        vec![vec![json!("")]]
    );
}
