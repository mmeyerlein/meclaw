//! #1407 -- the fact of a tool turn comes straight from the tool.
//!
//! WHY this file exists: on the phone (d53 battery round 1) the fact of a
//! tool turn came out of the SECOND model round -- tool_result to the
//! splitter's `sidecar section fact` took 2.8 to 6.7 s, and once the model
//! wrote `context` instead and no fact came at all -- although the weather
//! tool hands back its finished sentence as `summary`. With
//! `fact_from_tool` naming a tool, the first result of that tool on a running
//! session's turn (its handle `live`, D58 live) sends ONE `live_feed` section `fact` (`hop.fact_source`
//! "tool", the summary as `payload`) at once; round 2 still runs for the
//! answer. `""` (the default) moves no byte. Since D58 feed (GH #1481,
//! contract point 6) the fact takes the curator's one road of orders: its
//! claim is `pending:feed:<live>#<turn>#tool_fact`, its mark `feed` with the
//! section `fact` and the tool as `mark.tool` (was `fact_from_tool {tool}`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const ASK: &str = "What is the weather tomorrow?";
const SUMMARY: &str = "Tomorrow in Berlin: cloudy, 4 to 9 degrees.";

fn hive(tools: &str) -> Hive {
    let mut h =
        Hive::with(&[("policy", "fact_from_tool", json!(tools))]).reporting_composite("talky");
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

/// The tool stamp of a running session's turn: `simple`, needs a tool.
fn tool_stamp(family: &str) -> Value {
    json!({"live": "call-9", "path": "simple",
           "needs": "tool", "families": family})
}

fn facts(h: &Hive) -> Vec<Msg> {
    h.routed("live_feed")
        .into_iter()
        .filter(|m| m.hop.get("section") == Some(&json!("fact")))
        .collect()
}

fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("{}")).expect("the mark is JSON"))
    .collect()
}

/// The tool of every mark `feed` of a tool fact (section `fact`, the tool as
/// `mark.tool`), in order -- D58 feed: the tool fact's mark is the one road's.
fn tool_marks(h: &Hive) -> Vec<Value> {
    marks(h, "feed")
        .into_iter()
        .filter(|v| v["section"] == json!("fact"))
        .map(|v| v["mark"].clone())
        .collect()
}

/// The claim of the tool fact of `turn` on the handle `call-9`.
fn claim(turn: &str) -> String {
    format!("pending:feed:call-9#{turn}#tool_fact")
}

fn result_text() -> String {
    json!({"summary": SUMMARY, "days": [{"min": 4, "max": 9}]}).to_string()
}

/// Turn `t1` to its tool result: the stamp, the required call asks for
/// `tool`, the result comes back on iteration 1.
fn to_the_result(h: &mut Hive, tool: &str, family: &str, text: &str) {
    seam(
        h,
        "t1",
        "0",
        tool_stamp(family),
        json!({"messages": [user(ASK)]}),
    );
    let calls: Vec<Msg> = h
        .routed("quick")
        .into_iter()
        .filter(|q| {
            !q.hop["curator_call"]
                .as_str()
                .unwrap_or("")
                .ends_with("-bridge")
        })
        .collect();
    assert_eq!(calls.len(), 1, "one tool round: {:?} {:?}", h.out, h.stderr);
    h.out.clear();
    h.tap(
        &calls[0],
        "tool_calls",
        json!({}),
        json!([tool_call("c1", tool)]),
    );
    let mut hop = tool_stamp(family);
    hop["needs"] = json!("");
    seam(
        h,
        "t1",
        "1",
        hop,
        json!({"messages": [tool_call("c1", tool), tool_result("c1", text)]}),
    );
}

/// Pin 1: a listed tool's summary is the turn's fact at once -- beside the
/// second round, not after it -- and only once per turn.
#[test]
fn a_listed_tool_summary_becomes_a_fact_at_once() {
    if !shipped() {
        return;
    }
    let mut h = hive("weather");
    to_the_result(&mut h, "weather", "weather", &result_text());
    let f = facts(&h);
    assert_eq!(f.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let f = &f[0];
    assert_eq!(f.hop["fact_source"], json!("tool"));
    assert_eq!(f.hop["live"], json!("call-9"));
    assert_eq!(f.hop["turn_id"], json!("t1"));
    assert_eq!(f.hop["session_id"], json!("s1"));
    assert!(f.hop.get("at_turn_end").is_none(), "{:?}", f.hop);
    assert_eq!(f.body["payload"], json!(SUMMARY));
    assert_eq!(f.body["section"], json!("fact"));
    // Round 2 still runs for the answer.
    assert_eq!(h.routed("quick").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(tool_marks(&h), vec![json!({"tool": "weather"})]);
    assert!(
        marks(&h, "fact_from_tool").is_empty(),
        "the old mark is gone"
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
    assert!(!h.state(&claim("t1")).is_empty(), "the turn's claim");

    // The same iteration once more (a re-read, a second iteration): no
    // second fact of the turn.
    let mut hop = tool_stamp("weather");
    hop["needs"] = json!("");
    seam(
        &mut h,
        "t1",
        "1",
        hop,
        json!({"messages": [tool_call("c1", "weather"), tool_result("c1", &result_text())]}),
    );
    assert!(facts(&h).is_empty(), "one fact per turn: {:?}", h.out);
    assert_eq!(tool_marks(&h).len(), 1);
}

/// Pin 2: an unlisted tool, a result without a summary and an error result
/// wait for round 2 -- no fact from the curator.
#[test]
fn an_unlisted_tool_waits_for_round_two() {
    if !shipped() {
        return;
    }
    for (tool, text) in [
        ("web_search", result_text()),
        ("weather", json!({"days": []}).to_string()),
        (
            "weather",
            json!({"error": "upstream", "summary": SUMMARY}).to_string(),
        ),
    ] {
        let mut h = hive("weather");
        to_the_result(&mut h, tool, tool.split('_').next().unwrap_or(tool), &text);
        assert!(facts(&h).is_empty(), "{tool} {text}: {:?}", h.out);
        assert!(tool_marks(&h).is_empty());
        assert_eq!(
            h.routed("quick").len() + h.routed("brain").len(),
            1,
            "{:?}",
            h.out
        );
    }
    // A turn without a handle gets none either.
    let mut h = hive("weather");
    seam(
        &mut h,
        "t1",
        "1",
        json!({"path": "simple", "families": "weather"}),
        json!({"messages": [user(ASK), tool_call("c1", "weather"),
                            tool_result("c1", &result_text())]}),
    );
    assert!(facts(&h).is_empty(), "a text turn: {:?}", h.out);
}

/// Default byte-identical: `fact_from_tool` "" sends, writes and reads
/// exactly what the knob left out does. Both are one code path -- the guard
/// against a byte change to the template before the knob are the standing
/// pins of the policy (review M-1); this pin keeps "" from reading as a name.
#[test]
fn default_empty_is_byte_identical() {
    if !shipped() {
        return;
    }
    let run = |over: &[(&str, &str, Value)]| {
        let mut h = Hive::with(over).reporting_composite("talky");
        to_the_result(&mut h, "weather", "weather", &result_text());
        // Routes and sections: the calls carry fresh ids and times.
        let out: Vec<(String, Value)> = h
            .out
            .iter()
            .map(|m| {
                (
                    m.route().to_string(),
                    m.hop.get("section").cloned().unwrap_or(Value::Null),
                )
            })
            .collect();
        let state = h.rows("SELECT key FROM state WHERE key LIKE 'pending:feed:%'");
        let ops = h.ledger_ops.len();
        (out, state, ops, facts(&h).len())
    };
    let plain = run(&[]);
    let off = run(&[("policy", "fact_from_tool", json!(""))]);
    assert_eq!(plain.0, off.0);
    assert_eq!(plain.1, off.1);
    assert!(off.1.is_empty());
    assert_eq!(plain.2, off.2, "the same ledger traffic");
    assert_eq!(off.3, 0);
}

/// Lock (review I-1): a delegation round's tool result is no fact of the
/// curator. The delegation's own answer is what the voice speaks for the
/// turn (`settle_delegation` counts one answer per delegation); a tool fact
/// in the round would take that place or come twice -- round 2 of the
/// delegation answers, the fact comes with it.
#[test]
fn a_delegation_round_sends_no_tool_fact() {
    if !shipped() {
        return;
    }
    let mut h = hive("weather");
    // The collector keys a delegation round `<turn>~<deadline_ms>~<hex8>`.
    let key = "t1~4102444800000~ab12cd34";
    let mut hop = tool_stamp("weather");
    hop["delegation_id"] = json!("d1");
    seam(
        &mut h,
        key,
        "0",
        hop.clone(),
        json!({"messages": [user(ASK)]}),
    );
    let calls: Vec<Msg> = h
        .routed("quick")
        .into_iter()
        .chain(h.routed("brain"))
        .filter(|q| {
            !q.hop["curator_call"]
                .as_str()
                .unwrap_or("")
                .ends_with("-bridge")
        })
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "the delegation's call: {:?} {:?}",
        h.out,
        h.stderr
    );
    h.out.clear();
    h.tap(
        &calls[0],
        "tool_calls",
        json!({}),
        json!([tool_call("c1", "weather")]),
    );
    hop["needs"] = json!("");
    seam(
        &mut h,
        key,
        "1",
        hop,
        json!({"messages": [tool_call("c1", "weather"), tool_result("c1", &result_text())]}),
    );
    assert!(
        facts(&h).is_empty(),
        "no fact in a delegation round: {:?}",
        h.out
    );
    assert!(tool_marks(&h).is_empty());
    assert_eq!(
        h.routed("quick").len() + h.routed("brain").len(),
        1,
        "the delegation's round 2 answers: {:?} {:?}",
        h.out,
        h.stderr
    );
}

/// Lock (fix round 1): the gap's section passes the policy on to `./push`
/// with its sidecar payload as an object (`{"gap": ...}`). A body `payload`
/// declared `string` in the policy's `emits` (for the tool fact) made the
/// colony dead-letter it -- the gap's find never came (assemble gh895:
/// no `addendum` within 30 s). The declaration stays off; the tool fact's
/// string and the gap's object both pass.
#[test]
fn a_gaps_object_payload_passes_the_policy() {
    if !shipped() {
        return;
    }
    let mut h = hive("weather");
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "section": "gap"});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "channel": "test",
                     "audience_set": ROUND_E});
    h.lane(
        "in_section",
        ctx,
        hop,
        json!({"section": "gap", "payload": {"gap": "Hannah ferry Porto"}}),
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
    assert!(
        h.stderr.iter().any(|l| l.contains("curator/push: gap")),
        "the gap reaches the push: {:?}",
        h.stderr
    );
}
