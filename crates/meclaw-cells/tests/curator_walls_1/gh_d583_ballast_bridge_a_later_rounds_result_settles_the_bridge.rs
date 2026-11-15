//! GH #1528 (d58-3 ballast, R-39-5, OR-583-52; review of the bridges M5) --
//! the result of a look-up that comes in a later round settles its bridge.
//!
//! WHY this file exists: a bridge ("one moment, I am looking it up") is no
//! content, only a consequence of the latency of the look-up, and the plan
//! leaves it out once the look-up's result stands on the wall. It knew the
//! result only in the bridge's own round: an errand whose answer came back
//! as a round of its own (the advice under the errand's id, `consult_id`)
//! left the bridge as the last word of its round, raw for the rest of the
//! wall. The errand's id was only in the head of the call. Now both rounds
//! keep it in a mark (`errand_link`): `from` on the round whose call sent
//! the errand beside its bridge, `answer` on the round that answers it; the
//! plan reads both with the bridges' marks. A bridge of the form that keeps
//! the target's window (`later`, its text keeps the talk going and may ask
//! the person something, review M7) writes no `from` and stays raw.
//!
//! Two halves: the shipped policy in a hive with talky's wiring writes the
//! marks (`support/curator_hive.rs`), and the pure plan of the shipped
//! policy settles the bridge on made-up rows.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// 2026-03-01T00:00:00Z in seconds; the plan is made a day later.
const DAY_S: i64 = 1_772_323_200;
const AS_OF_MS: i64 = (DAY_S + 86_400) * 1000;
/// The short id of the bridge.
const BRIDGE: &str = "bbbbbbbbbbbb";
/// The errand's id, as the policy names it ("k-<call>").
const ERRAND: &str = "k-c-1";

fn curate_with(h: &mut Hive, turn_id: &str, path: &str, extra_ctx: Value, ask: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": path});
    let mut ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": path});
    for (k, v) in extra_ctx.as_object().cloned().unwrap_or_default() {
        ctx[k] = v;
    }
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": mode("Be brief.")}),
    );
}

/// The `errand_link` marks of the ledger: `(turn_id, value)`, in order.
fn links(h: &Hive) -> Vec<(String, Value)> {
    h.rows("SELECT turn_id, value FROM marks WHERE kind = 'errand_link' ORDER BY seq")
        .into_iter()
        .map(|r| {
            let v = meclaw_core::serde_json::from_str(r[1].as_str().unwrap_or_default())
                .unwrap_or(Value::Null);
            (r[0].as_str().unwrap_or_default().to_string(), v)
        })
        .collect()
}

#[test]
fn gh_d583_ballast_bridge_the_errand_id_is_kept_on_both_rounds() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[]).reporting_composite("talky");
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));

    // t1: a consult leaves beside a bridge that sees the turn alone (`soon`).
    curate_with(&mut h, "t1", "complex", json!({}), "plan my week");
    let errand = h.routed("consult");
    assert_eq!(errand.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let cid = errand[0].hop["consult_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !cid.is_empty(),
        "the errand carries its id: {:?}",
        errand[0].hop
    );
    assert_eq!(
        h.routed("quick").len(),
        1,
        "the bridge beside it: {:?}",
        h.out
    );
    assert_eq!(links(&h), vec![("t1".to_string(), json!({"from": cid}))]);

    // t2: research bridges with the target's window (`later`): no `from`.
    curate_with(&mut h, "t2", "research", json!({}), "find everything on it");
    assert_eq!(h.routed("research").len(), 1, "{:?}", h.out);
    assert_eq!(h.routed("quick").len(), 1, "a later bridge: {:?}", h.out);
    assert_eq!(
        links(&h).len(),
        1,
        "a later bridge keeps no link: {:?}",
        links(&h)
    );

    // t3: the advice of t1 comes back under its id -- the round keeps it.
    curate_with(
        &mut h,
        "t3",
        "research",
        json!({"consult_id": cid.clone()}),
        "the advice",
    );
    assert_eq!(
        h.routed("brain").len(),
        1,
        "the advice is spoken: {:?}",
        h.out
    );
    assert_eq!(
        links(&h),
        vec![
            ("t1".to_string(), json!({"from": cid.clone()})),
            ("t3".to_string(), json!({"answer": cid})),
        ]
    );
}

/// Piece `j` of round `i` of the wall.
fn row(i: i64, j: i64, kind: &str, short: &str, chars: i64) -> Value {
    let t = DAY_S + 3600 + 10 * i + j;
    json!({"seq": t * 1_000_000, "session_id": "s", "turn_id": format!("r{i}"), "iter": 0,
           "kind": kind, "hash": format!("{short}{}", "0".repeat(52)), "nth": 0,
           "final": i64::from(kind == "assistant"), "at": "2026-03-01T01:00:00.000000Z",
           "audience_set": ROUND_E, "chars": chars, "type": "text"})
}

fn mark(i: i64, kind: &str, value: Value) -> Value {
    let t = DAY_S + 3600 + 10 * i + 5;
    json!({"seq": t * 1_000_000, "session_id": "s", "turn_id": format!("r{i}"),
           "kind": kind, "value": value.to_string()})
}

/// Five rounds, the newest kept raw: r1 the person's words and a bridge
/// (no answer of its own), r2 the advice -- a final answer --, the others
/// words and an answer. `links` are the `errand_link` marks.
fn bridged_with(links: &[Value]) -> Value {
    let mut rows = Vec::new();
    for i in 0..5 {
        rows.push(row(i, 0, "user", &format!("{:012x}", 100 + i), 300));
        let (short, chars) = if i == 1 {
            (BRIDGE.to_string(), 70)
        } else {
            (format!("{:012x}", 200 + i), 400)
        };
        rows.push(row(i, 1, "assistant", &short, chars));
    }
    let mut marks = vec![mark(
        1,
        "side_answer",
        json!({"call": "c-1-bridge", "ids": [BRIDGE]}),
    )];
    marks.extend(links.iter().cloned());
    let probe = "window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], [], 0, None, \
                 collect_only=True, bridges=ARGS['marks'])['plan'].get('bridged') or []";
    let (out, err) = policy_scope(
        json!({"keep_recent": 1, "keep_recent_tokens": null}),
        probe,
        json!({"rows": rows, "as_of": AS_OF_MS, "marks": marks}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    out
}

#[test]
fn gh_d583_ballast_bridge_a_later_rounds_result_settles_the_bridge() {
    if !shipped() {
        return;
    }
    let from = mark(1, "errand_link", json!({"from": ERRAND}));
    let answer = mark(2, "errand_link", json!({"answer": ERRAND}));
    // Without the link the bridge is the last word of its round and stays.
    assert_eq!(bridged_with(&[]), json!([]), "no link, the bridge stays");
    // The errand sent beside it is answered in r2: the bridge goes.
    assert_eq!(
        bridged_with(&[from.clone(), answer.clone()]),
        json!([{"s": "s", "t": "r1", "ids": [BRIDGE]}]),
        "the result of a later round settles the bridge"
    );
    // Half a link settles nothing: a bridge without `from` (a later bridge)
    // or an errand never answered, or answered under another id.
    assert_eq!(
        bridged_with(&[answer]),
        json!([]),
        "no `from`, the bridge stays"
    );
    assert_eq!(
        bridged_with(std::slice::from_ref(&from)),
        json!([]),
        "no answer, it stays"
    );
    let other = mark(2, "errand_link", json!({"answer": "k-c-9"}));
    assert_eq!(
        bridged_with(&[from, other]),
        json!([]),
        "another errand's answer"
    );
}
