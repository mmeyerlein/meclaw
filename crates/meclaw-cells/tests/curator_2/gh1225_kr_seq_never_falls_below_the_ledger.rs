//! GH #1225 (A10) -- a wall sequence number never falls below the ledger.
//!
//! WHY this file exists: `next_seq` of `./intake` made the clock strictly
//! monotonic only against what the child itself had minted (`_MEM["seq"]`).
//! A new child after a clock set back (an NTP step, a host moved) minted
//! numbers below rows that stand, and the wall's order -- the order of
//! arrival every reader sorts by -- put the new turn before the old one. Now
//! step A of every wall lane reads `max(seq)` and B mints above it. The clock
//! is injected: the shipped script runs behind a prefix that answers
//! `datetime.datetime.now` from `context.test_clock`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const CLOCK: &str = r#"import datetime as _kr_dt, io as _kr_io, json as _kr_json, sys as _kr_sys
_kr_raw = _kr_sys.stdin.read()
_kr_sys.stdin = _kr_io.StringIO(_kr_raw)
try:
    _kr_at = _kr_json.loads(_kr_raw)["envelope"]["header"]["context"].get("test_clock")
except Exception:
    _kr_at = None
if not hasattr(_kr_dt, "_kr_real"):
    _kr_dt._kr_real = _kr_dt.datetime

    class _KrClock(_kr_dt._kr_real):
        at = None

        @classmethod
        def now(cls, tz=None):
            if not cls.at:
                return _kr_dt._kr_real.now(tz)
            t = _kr_dt._kr_real.fromisoformat(cls.at)
            return t.astimezone(tz) if tz is not None else t.replace(tzinfo=None)

    _kr_dt.datetime = _KrClock
_kr_dt.datetime.at = _kr_at
"#;

fn round_at(h: &mut Hive, turn: &str, clock: &str, words: &str) {
    h.lane(
        "in_curate",
        json!({"session_id": "s-a10", "turn_id": turn, "iter": "0", "channel": "test",
               "audience_set": ROUND_E, "test_clock": clock}),
        json!({"session_id": "s-a10", "turn_id": turn, "iter": "0", "phase": ""}),
        json!({"messages": [user(words)], "system": mode("Be brief.")}),
    );
}

fn seqs(h: &Hive, turn: &str) -> Vec<i64> {
    h.rows(&format!(
        "SELECT seq FROM wall WHERE turn_id = '{turn}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| r[0].as_i64().expect("an integer seq"))
    .collect()
}

#[test]
fn seq_never_falls_below_the_ledger() {
    if !shipped() {
        return;
    }
    let script = format!("{CLOCK}{}", script_of("intake"));
    let mut h = Hive::with(&[("intake", "script_inline", json!(script))]);
    round_at(&mut h, "t1", "2026-10-16T12:00:00+00:00", "The first turn.");
    let first = seqs(&h, "t1");
    assert!(!first.is_empty(), "{:?}", h.stderr);
    // A new child, and the clock one day back.
    h.restart("intake");
    round_at(
        &mut h,
        "t2",
        "2026-10-15T12:00:00+00:00",
        "The second turn.",
    );
    let second = seqs(&h, "t2");
    assert!(!second.is_empty(), "{:?}", h.stderr);
    assert!(
        second[0] > *first.last().unwrap(),
        "the later turn sorts after the earlier: {first:?} {second:?}"
    );
}
