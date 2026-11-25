//! d54s nachlauf (GH #1594) -- a call of a cancelled delegation that fails
//! after the cancel does not escalate.
//!
//! WHY this file exists: since GH #1587 a delegation the voice cancels gets
//! no model call from iteration 1 on. The call that was already out when the
//! cancel came is no such round: when it came back without an answer (an
//! error, a timeout, a content filter) the tap took the round on as
//! `<call>-esc` to the brain like any failed quick call (GH #1352) -- a new
//! model call for a superseded errand (review B of #1587, minor 5). A late
//! failure of a cancelled delegation's call now ends silently: nothing
//! reaches the brain, the round is marked `turn_parked {reason: cancelled}`
//! and its wall keeps the one `superseded` line.
//!
//! The hive in one process (`support/curator_hive.rs`), a stand-in for the
//! model (the test answers on the tap); no model.

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
const SESSION: &str = "deleg-session-6";
const LABEL_A: &str = "voice-turn-1";
const ID_A: &str = "item_a";
const ID_B: &str = "item_b";

fn round_a() -> String {
    format!("{LABEL_A}~{OPEN_UNTIL}~aa11aa11")
}

/// The model calls that left the hive.
fn calls(h: &Hive) -> Vec<Msg> {
    let mut c = h.routed("quick");
    c.extend(h.routed("brain"));
    c
}

/// The first curate of delegation A under the handle, a `simple` round (the
/// quick voice, the brain its fallback); returns its call.
fn first_call(h: &mut Hive) -> Msg {
    h.out.clear();
    let round = round_a();
    let mut hop = json!({"session_id": SESSION, "turn_id": round, "iter": "0", "phase": "",
                         "path": "simple", "needs": "", "families": "web",
                         "delegation_id": ID_A});
    let mut ctx = json!({"session_id": SESSION, "turn_id": round, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    for (k, v) in obj(live_keys(LIVE)) {
        hop[k.as_str()] = v.clone();
        ctx[k.as_str()] = v;
    }
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("Book the table at eight.")]}),
    );
    let c = calls(h);
    assert_eq!(
        c.len(),
        1,
        "the delegation makes its call: {:?} {:?}",
        h.out,
        h.stderr
    );
    let esc = format!(
        "pending:{}-esc",
        c[0].hop["curator_call"].as_str().unwrap_or("")
    );
    assert!(
        !h.state(&esc).is_empty(),
        "the call keeps the copy a failure escalates from ({esc}): {:?}",
        h.stderr
    );
    c[0].clone()
}

/// The voice's `delegation_cancel` of A, as talky's edge hands it on.
fn cancel(h: &mut Hive) {
    h.out.clear();
    let hop = json!({"session_id": VOICE_SESSION, "call_id": LIVE, "engine": "duplex",
                     "live": LIVE, "delegation_id": ID_A, "superseded_by": ID_B,
                     "operation_id": format!("{LIVE}:{ID_A}:1")});
    let ctx = json!({"live": LIVE, "audience_set": ROUND_E});
    h.lane("in_delegation_cancel", ctx, hop, json!({"messages": []}));
    let park: Value = sj::from_str(&h.state(&format!("pending:delegation:{LIVE}#{LABEL_A}")))
        .unwrap_or(Value::Null);
    assert!(park.get("cancelled").is_some(), "{park:?} {:?}", h.stderr);
}

fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("")).unwrap_or(Value::Null))
    .collect()
}

fn outcomes(h: &Hive, round: &str) -> Vec<String> {
    h.rows(&format!(
        "SELECT b.body FROM wall w JOIN blocks b ON b.hash = w.hash \
         WHERE w.kind = 'outcome' AND w.turn_id = '{round}' ORDER BY w.seq"
    ))
    .iter()
    .map(|r| {
        sj::from_str::<Value>(r[0].as_str().unwrap_or(""))
            .ok()
            .and_then(|b| b["text"].as_str().map(str::to_string))
            .unwrap_or_default()
    })
    .collect()
}

#[test]
fn gh_d54s_cancel_a_call_that_fails_after_the_cancel_does_not_escalate() {
    if !shipped() {
        return;
    }
    let mut h = live_hive(&[]);
    let call = first_call(&mut h);
    cancel(&mut h);
    // The call that was out comes back with an error.
    h.out.clear();
    h.tap(
        &call,
        "error",
        json!({"error_code": "provider_error"}),
        json!([]),
    );
    assert!(
        calls(&h).is_empty(),
        "a cancelled delegation's failure reaches no model: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(marks(&h, "escalate").is_empty(), "{:?}", h.stderr);
    // Positive: the escalated round was taken and locked as cancelled.
    let parked = marks(&h, "turn_parked");
    assert!(
        parked
            .iter()
            .any(|m| m["delegation_id"] == json!(ID_A) && m["reason"] == json!("cancelled")),
        "{parked:?} {:?}",
        h.stderr
    );
    let oa = outcomes(&h, &round_a());
    assert_eq!(oa.len(), 1, "{oa:?} {:?}", h.stderr);
    assert!(oa[0].contains("superseded"), "{oa:?}");
    let esc = format!(
        "pending:{}-esc",
        call.hop["curator_call"].as_str().unwrap_or("")
    );
    assert!(
        h.state(&esc).is_empty(),
        "the copy is taken: {:?}",
        h.stderr
    );
}

/// The contrast: without the cancel the same failure goes to the brain.
#[test]
fn gh_d54s_cancel_without_it_the_failing_call_escalates() {
    if !shipped() {
        return;
    }
    let mut h = live_hive(&[]);
    let call = first_call(&mut h);
    h.out.clear();
    h.tap(
        &call,
        "error",
        json!({"error_code": "provider_error"}),
        json!([]),
    );
    assert_eq!(h.routed("brain").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(marks(&h, "escalate").len(), 1, "{:?}", h.stderr);
}
