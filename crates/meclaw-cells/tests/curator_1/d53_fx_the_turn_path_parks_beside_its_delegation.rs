//! #1407 -- the turn path parks while a delegation of its turn is open.
//!
//! WHY this file exists: on the phone (d53 battery round 1, call 00-P2) the
//! voice model delegated the person's turn at +248 ms and the delegation
//! round answered it -- and beside it the turn's own path went on: a second
//! weather call at +4 071 ms, then `talky/quick` ran its second round 20 s
//! into its timeout (+24 398 ms). Two tool rounds for one question, and the
//! quick cell busy for 20 s after every delegation. A delegation round
//! (`<turn>~<deadline_ms>~<hex8>`, `delegation_id` beside it) marks its turn
//! open until its deadline; a curate of that turn's own path in the meantime
//! sends no call (`turn_parked {delegation_id, iter}`).
//!
//! Virtual time: the deadline rides in the round key, so "open" and "late"
//! are a key far in the future and one in the past -- nothing waits.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const ASK: &str = "What is the weather tomorrow?";
/// A deadline no run of this test reaches (2100-01-01).
const OPEN_UNTIL: u64 = 4_102_444_800_000;
/// A deadline long past (2001-09-09).
const LATE_SINCE: u64 = 1_000_000_000_000;

fn hive() -> Hive {
    let mut h = Hive::with(&[]).reporting_composite("talky");
    h.check_emits = true;
    h
}

fn seam(h: &mut Hive, turn: &str, iter: &str, hop_extra: Value, body: Value) {
    h.out.clear();
    let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": iter, "phase": "",
                         "path": "", "needs": "", "families": ""});
    for (k, v) in obj(hop_extra) {
        hop[k] = v;
    }
    let path = hop["path"].clone();
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": iter,
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane("in_curate", ctx, hop, body);
}

fn duplex(path: &str, needs: &str, families: &str) -> Value {
    json!({"live": "call-9", "path": path,
           "needs": needs, "families": families})
}

fn calls(h: &Hive) -> usize {
    h.routed("quick").len() + h.routed("brain").len()
}

fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the mark is JSON"))
    .collect()
}

/// Turn `turn` to its tool call: the stamp of a running session's turn that
/// needs `weather`, the quick tool round, the model's call on the tap.
fn tool_round(h: &mut Hive, turn: &str) {
    seam(
        h,
        turn,
        "0",
        duplex("simple", "tool", "weather"),
        json!({"messages": [user(ASK)]}),
    );
    let q: Vec<Msg> = h
        .routed("quick")
        .into_iter()
        .filter(|q| {
            !q.hop["curator_call"]
                .as_str()
                .unwrap_or("")
                .ends_with("-bridge")
        })
        .collect();
    assert_eq!(q.len(), 1, "one tool round: {:?} {:?}", h.out, h.stderr);
    h.out.clear();
    h.tap(
        &q[0],
        "tool_calls",
        json!({}),
        json!([tool_call("c1", "weather")]),
    );
}

/// The tool result of turn `turn` on iteration 1: the curate of round 2.
fn round_two(h: &mut Hive, turn: &str) {
    seam(
        h,
        turn,
        "1",
        duplex("simple", "", "weather"),
        json!({"messages": [tool_call("c1", "weather"),
                            tool_result("c1", "{\"summary\": \"cloudy, 4 to 9\"}")]}),
    );
}

/// The voice model's delegation of `label`: its own round, keyed by the
/// collector as `<label>~<deadline_ms>~<hex8>`, answered on the brain's way.
fn delegation(h: &mut Hive, label: &str, until: u64, id: &str) {
    let key = format!("{label}~{until}~ab12cd34");
    let mut hop = duplex("", "", "");
    hop["delegation_id"] = json!(id);
    seam(
        h,
        &key,
        "0",
        hop,
        json!({"messages": [user(&format!("[delegation {id} from the voice channel] {ASK}"))]}),
    );
    assert_eq!(
        calls(h),
        1,
        "the delegation round makes its call: {:?} {:?}",
        h.out,
        h.stderr
    );
}

/// Pin: a delegation of the turn is open -- the turn's round 2 sends no
/// call, the park is booked once with the delegation's id.
#[test]
fn a_turn_path_parks_beside_an_open_delegation_of_the_same_turn() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    tool_round(&mut h, "t1");
    delegation(&mut h, "t1", OPEN_UNTIL, "d1");
    assert!(
        !h.state("pending:delegation:call-9#t1").is_empty(),
        "the delegation marks its turn open (the handle's scope, D58 live): {:?}",
        h.stderr
    );
    round_two(&mut h, "t1");
    assert_eq!(
        calls(&h),
        0,
        "the turn path parks: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert_eq!(
        marks(&h, "turn_parked"),
        vec![json!({"delegation_id": "d1", "iter": 1})]
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

/// Lock: without a delegation, beside a late one and beside one of another
/// turn, round 2 goes as before and nothing is parked.
#[test]
fn a_turn_without_a_delegation_runs_as_before() {
    if !shipped() {
        return;
    }
    // (label of the delegation, its deadline); None = no delegation.
    for case in [None, Some(("t1", LATE_SINCE)), Some(("t0", OPEN_UNTIL))] {
        let mut h = hive();
        tool_round(&mut h, "t1");
        if let Some((label, until)) = case {
            delegation(&mut h, label, until, "d1");
        }
        round_two(&mut h, "t1");
        assert_eq!(
            calls(&h),
            1,
            "{case:?}: round 2 goes: {:?} {:?}",
            h.out,
            h.stderr
        );
        assert!(marks(&h, "turn_parked").is_empty(), "{case:?}");
    }
    // A turn without a handle reads no delegation row at all.
    let mut h = hive();
    seam(
        &mut h,
        "t1",
        "0",
        json!({"path": "simple"}),
        json!({"messages": [user(ASK)]}),
    );
    assert_eq!(calls(&h), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        h.state("pending:delegation:s1#t1").is_empty(),
        "a text turn writes no delegation row"
    );
}

/// Pin (review I-2): the turn's normal curate (iteration 0) parks beside the
/// open delegation while the turn's speculation holds an answer -- the
/// speculation ends with the park (rows `spec:`/`spec_held:` gone, mark
/// `quick_spec` dropped), as at any drop. Before, the rows stayed: no normal
/// curate came any more to decide them, and the A13 sweep reaps only
/// `pending:` rows.
#[test]
fn a_parked_turn_ends_its_speculation() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("lead", "quick_speculative", json!("1")),
        ("lead", "spec_tools", json!("")),
    ])
    .reporting_composite("talky");
    h.check_emits = true;
    // The collector's early curate of the turn: `spec` "1", no path.
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ASK)]}));
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "one spec call: {:?} {:?}", h.out, h.stderr);
    let call = quick[0].clone();
    // Its answer is in and held.
    h.out.clear();
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert("spec".into(), json!("1"));
    h.lane(
        "in_spec_llm",
        Value::Object(ctx),
        json!({"finish_reason": "stop", "model": "test-model", "tokens_prompt": 10}),
        json!({"messages": [said("Cloudy tomorrow.")]}),
    );
    assert!(
        // The spec rows live in the lead's store since GH #1447.
        !h.lead_state("spec_held:s1#t1").is_empty(),
        "the answer is held: {:?}",
        h.stderr
    );
    delegation(&mut h, "t1", OPEN_UNTIL, "d1");
    seam(
        &mut h,
        "t1",
        "0",
        duplex("simple", "", ""),
        json!({"messages": [user(ASK)]}),
    );
    assert_eq!(
        calls(&h),
        0,
        "the turn path parks: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(h.routed("spec_release").is_empty(), "{:?}", h.out);
    assert_eq!(
        marks(&h, "turn_parked"),
        vec![json!({"delegation_id": "d1", "iter": 0})]
    );
    assert!(
        h.lead_state("spec:s1#t1").is_empty(),
        "the spec row goes with the park"
    );
    assert!(
        h.lead_state("spec_held:s1#t1").is_empty(),
        "the held answer goes with it"
    );
    let spec = marks(&h, "spec");
    assert_eq!(spec.len(), 1, "{spec:?}");
    assert_eq!(spec[0]["verdict"], json!("dropped"));
    assert_eq!(spec[0]["reason"], json!("parked:delegation"));
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}
