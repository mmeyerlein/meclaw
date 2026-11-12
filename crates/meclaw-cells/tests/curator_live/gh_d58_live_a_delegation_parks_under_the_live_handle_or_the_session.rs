//! D58 live (GH #1482, plan `u-live.md` contract point 7) -- a delegation
//! parks under the live handle, or else under the session.
//!
//! WHY this file exists: the delegation round of a running session runs
//! under the session the delegation came in on, the turn path of the same
//! session under its own (D38 V3: `pending:delegation:<session>#<turn>` of
//! the two never met, and the voice spoke twice). The knob
//! `delegation_park_by_call` "1" keyed the park by the voice's `call_id`
//! instead; "0" kept the session. The knob falls: every composition parks
//! as "1" did, by the neutral handle -- `pending:delegation:<live>#<label>`
//! with the handle, `pending:delegation:<session>#<label>` without it --
//! and the row always carries the round's audience (a rebuild knows no
//! handle and tells the row by it). The turn path of the live session
//! reads the park, whatever its own session: it sends no call and books
//! `turn_parked`.
//!
//! The hive in one process (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use curator_hive::*;
use curator_live::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const OPEN_UNTIL: u64 = 4_102_444_800_000;
/// The member turn the delegation answers (the label of its round).
const LABEL: &str = "voice-turn-1";
/// The session the delegation round runs under, and the one of the turn path.
const DELEGATION_SESSION: &str = "deleg-session-5";
const TURN_SESSION: &str = "phone-2026-11-07T09:06:50Z";

fn calls(h: &Hive) -> usize {
    h.routed("quick").len() + h.routed("brain").len()
}

fn parked(h: &Hive) -> usize {
    h.rows("SELECT value FROM marks WHERE kind = 'turn_parked'")
        .len()
}

fn parks(h: &Hive) -> Vec<(String, Value)> {
    h.rows("SELECT key, value FROM state WHERE key LIKE 'pending:delegation:%' ORDER BY key")
        .iter()
        .map(|r| {
            (
                r[0].as_str().unwrap_or("").to_string(),
                sj::from_str(r[1].as_str().unwrap_or("")).unwrap_or(Value::Null),
            )
        })
        .collect()
}

/// The delegation round opens under `keys` (its first iteration).
fn delegation(h: &mut Hive, keys: &Value) {
    seam(
        h,
        DELEGATION_SESSION,
        &format!("{LABEL}~{OPEN_UNTIL}~ab12cd34"),
        "",
        keys,
        json!({"path": "", "delegation_id": "item_d1"}),
    );
    assert_eq!(calls(h), 1, "the delegation makes its call: {:?}", h.stderr);
}

#[test]
fn gh_d58_live_a_delegation_parks_under_the_live_handle_or_the_session() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    assert!(
        policy["params"].get("delegation_park_by_call").is_none()
            && policy["contract"]["settings"]
                .get("delegation_park_by_call")
                .is_none(),
        "the knob delegation_park_by_call falls: every composition parks by the handle"
    );

    // With the handle: the park is the handle's, with the round's audience,
    // and the live session's turn path parks beside it.
    let mut h = live_hive(&[]);
    delegation(&mut h, &live_keys(LIVE));
    let rows = parks(&h);
    assert_eq!(
        rows.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        vec![format!("pending:delegation:{LIVE}#{LABEL}")],
        "the park of a live session is keyed by the handle: {rows:?}"
    );
    assert_eq!(rows[0].1["audience_set"], json!(ROUND_E), "{rows:?}");
    seam(
        &mut h,
        TURN_SESSION,
        LABEL,
        "simple",
        &live_keys(LIVE),
        json!({}),
    );
    assert_eq!(
        (calls(&h), parked(&h)),
        (0, 1),
        "the turn path of the live session parks beside its delegation: {:?}",
        h.stderr
    );

    // Without the handle: the session's park, the round's audience all the
    // same.
    let mut h = live_hive(&[]);
    delegation(&mut h, &json!({}));
    let rows = parks(&h);
    assert_eq!(
        rows.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        vec![format!("pending:delegation:{DELEGATION_SESSION}#{LABEL}")],
        "without a handle the park is the session's: {rows:?}"
    );
    assert_eq!(
        rows[0].1["audience_set"],
        json!(ROUND_E),
        "the park always carries the round: {rows:?}"
    );
}
