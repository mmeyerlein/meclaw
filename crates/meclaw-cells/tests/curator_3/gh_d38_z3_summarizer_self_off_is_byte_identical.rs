//! GH #1440 (D38 Z3, OR-38-42 A1) -- `summarizer_self`: "0" is the rebuild
//! of before; "1" has the role's own model condense, after its answer, on
//! the prefix it already holds in its cache.
//!
//! WHY this file exists: R-27-2 Nr. 5 gives the summary to `./summarizer`, a
//! small model that reads a transcript cold. The review of arXiv 2611.05306
//! (§ 4 A1) found the answering model itself, asked at the end of its own
//! cached prefix (same system, same wall, the instruction last), condenses
//! better at a fraction of the input price. The default stays "0" until M2
//! (S10) has measured it. Pinned:
//!
//! 1. "0" (shipped) and unset: one request to `./summarizer`, nothing on the
//!    role's way after the answer, no `summarize` hop anywhere;
//! 2. "1": no summarizer request; one request on the way `brain` with
//!    `hop.summarize` "1", the system of the round's last call byte for byte,
//!    that call's messages as its prefix and the instruction as the last,
//!    user message; its answer comes home on `in_summary_self` and is kept as
//!    the summary, with a `marks` row `summary_self`;
//! 3. talky's wiring: an answer of such a call is never spoken (no partial,
//!    no splitter) and reaches the curator on `in_summary_self` only;
//! 4. review r1: the mark carries `why` and the token counts (M2), the
//!    answer clears `system_hash_sent` (I3), and a rebuild for an expired
//!    cache (M1) or after a tool round (M3) goes to the summarizer.
//!
//! Stub summarizer and stub model (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};

fn rebuild(over: &[(&str, &str, Value)]) -> (Hive, Msg) {
    rebuild_as(over, "cache", false)
}

/// A rebuild for the reason `why` (the strike's `reason`, as the clock
/// carries it): `cache`, or `compress`/a mark for a warm prefix. With
/// `tool_round` the round's last call is a tool round (`finish tool_calls`),
/// so the newest final answer on the wall is the one of the turn before.
fn rebuild_as(over: &[(&str, &str, Value)], why: &str, tool_round: bool) -> (Hive, Msg) {
    let mut o = vec![("policy", "keep_recent", json!(1))];
    o.extend(over.iter().cloned());
    let mut h = Hive::with(&o);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    let last = h.curate("s1", "t3", 0, json!([user("q3")]), mode("Be brief."));
    let (finish, said_now) = if tool_round {
        ("tool_calls", json!([tool_call("c1", "search")]))
    } else {
        ("stop", json!([said("a3")]))
    };
    h.tap(
        &last,
        finish,
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        said_now,
    );
    h.out.clear();
    let mut order = last_add(&h);
    order.body["emit_body"]["reason"] = json!(why);
    h.fire(&order);
    (h, last)
}

fn summarize_hops(h: &Hive) -> usize {
    h.out
        .iter()
        .filter(|m| m.hop.contains_key("summarize"))
        .count()
}

#[test]
fn summarizer_self_off_is_byte_identical() {
    if !shipped() {
        return;
    }
    let policy = cell_config("policy");
    assert_eq!(policy["params"]["summarizer_self"], json!("0"), "ships off");
    assert_eq!(
        policy["contract"]["settings"]["summarizer_self"]["default"],
        json!("0")
    );
    let (mut base, _) = rebuild(&[]);
    assert_eq!(base.summ.len(), 1, "one summarizer request");
    assert!(base.routed("brain").is_empty(), "nothing on the role's way");
    assert_eq!(summarize_hops(&base), 0);
    let want = base.answer("S1.", "stop");
    for v in [json!("0"), json!(false)] {
        let (mut h, _) = rebuild(&[("policy", "summarizer_self", v.clone())]);
        assert_eq!(h.summ.len(), 1, "{v}");
        assert!(h.routed("brain").is_empty(), "{v}");
        assert_eq!(summarize_hops(&h), 0, "{v}");
        let got = h.answer("S1.", "stop");
        assert_eq!(got.body, want.body, "{v}: the request of before");
        assert_eq!(got.hop, want.hop, "{v}");
        assert_eq!(
            h.rows("SELECT COUNT(*) FROM marks WHERE kind = 'summary_self'")[0][0],
            json!(0)
        );
    }
}

#[test]
fn with_summarizer_self_the_role_condenses_after_its_answer() {
    if !shipped() {
        return;
    }
    let (mut h, last) = rebuild_as(
        &[("policy", "summarizer_self", json!("1"))],
        "compress",
        false,
    );
    assert!(h.summ.is_empty(), "no request to ./summarizer");
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "one call on the role's way: {:?}", h.stderr);
    let call = &calls[0];
    assert_eq!(call.hop["summarize"], json!("1"));
    assert_eq!(
        call.body["system"], last.body["system"],
        "the system of the round's last call, byte for byte (the cached prefix)"
    );
    let (prefix, sent) = (last.messages(), call.messages());
    assert!(
        sent.len() > prefix.len() && sent[..prefix.len()] == prefix[..],
        "the last call's messages are the prefix: {sent:?}"
    );
    let tail = sent.last().expect("an instruction");
    assert_eq!(tail["origin"], json!("user"), "the instruction comes last");
    assert!(
        !tail["text"].as_str().unwrap_or_default().is_empty(),
        "{tail}"
    );
    let (sum_call, token) = (
        call.hop["summary_call"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        call.hop["summary_token"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    );
    assert!(!sum_call.is_empty() && !token.is_empty(), "{:?}", call.hop);

    // The answer comes home the way talky routes it (see the wiring test).
    h.lane(
        "in_summary_self",
        json!({}),
        json!({"finish_reason": "stop", "model": "role-model", "summarize": "1",
               "summary_call": sum_call, "summary_token": token, "tokens_prompt": 1000,
               "tokens_cached": 900, "tokens_completion": 50}),
        json!({"messages": [said("S-self: q1 a1 q2 a2.")]}),
    );
    // Review r1 M2: the mark carries what the self summary cost and why it
    // ran -- S10/M2 compare it with the summarizer's.
    let mark = h.rows("SELECT value FROM marks WHERE kind = 'summary_self'");
    let mark: Value = meclaw_core::serde_json::from_str(mark[0][0].as_str().unwrap_or("{}"))
        .expect("a json mark");
    for (k, v) in [
        ("model", json!("role-model")),
        ("why", json!("compress")),
        ("tokens_prompt", json!(1000)),
        ("tokens_cached", json!(900)),
        ("tokens_completion", json!(50)),
    ] {
        assert_eq!(mark[k], v, "{k}: {mark}");
    }
    let rows = h.rows("SELECT model FROM summaries");
    assert_eq!(rows.len(), 1, "the answer is the summary: {:?}", h.stderr);
    assert_eq!(rows[0][0], json!("role-model"));
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM marks WHERE kind = 'summary_self'")[0][0],
        json!(1),
        "one ledger mark per self summary"
    );
    assert_eq!(
        h.rows(
            "SELECT COUNT(*) FROM wall w JOIN blocks b ON b.hash = w.hash \
             WHERE b.body LIKE '%S-self%'"
        )[0][0],
        json!(0),
        "the answer is no turn: it never lands on the wall"
    );
    assert_eq!(
        h.state_in("rebuild_running", ROUND_E),
        "",
        "the rebuild is over"
    );
}

// ---- 3. talky's wiring ------------------------------------------------------

fn table(base: &str, rel: &str) -> EdgeTable {
    let params = read_json(&repo(rel))["params"].clone();
    let hp: HiveParams =
        meclaw_core::serde_json::from_value(params).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let spec: &EdgeSpec = spec;
        let abs = |ep: &str| match ep {
            "." => base.to_string(),
            other => format!("{base}/{}", other.trim_start_matches("./")),
        };
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(&spec.from)),
            to: Path::new(&abs(&spec.to)),
            condition: spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("{rel}: condition {src:?}: {e}"))
            }),
            modifier: spec.modifier.as_ref().map(|m| {
                meclaw_colony::cel_eval::parse_modifier(m)
                    .unwrap_or_else(|(k, e)| panic!("{rel}: modifier {k}: {e}"))
            }),
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        });
    }
    t
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("object")
}

/// One delivery of the edge table: target, context out, hop out.
type Delivery = (String, Map<String, Value>, Map<String, Value>);

fn deliveries(t: &EdgeTable, from: &str, context: Value, hop: Value) -> Vec<Delivery> {
    let hs = Headers::from_parts(map(context), map(hop));
    apply_edges(t, &Path::new(from), &hs)
        .into_iter()
        .map(|d| {
            (
                d.target.as_str().to_string(),
                d.headers_out.context.clone(),
                d.headers_out.hop.clone(),
            )
        })
        .collect()
}

#[test]
fn the_self_summary_is_never_spoken() {
    if !shipped() {
        return;
    }
    let t = table("/t", "templates/talky/config.json");
    // Out: the curator's self call reaches the brain, its marks in the context.
    let out = deliveries(
        &t,
        "/t/curator",
        json!({"session_id": "s1", "turn_id": "t3"}),
        json!({"route": "brain", "iter": "0", "curator_call": "k", "session_id": "s1",
               "turn_id": "t3", "summarize": "1", "summary_call": "rb", "summary_token": "tok"}),
    );
    let to_brain: Vec<_> = out.iter().filter(|(to, _, _)| to == "/t/brain").collect();
    assert_eq!(to_brain.len(), 1, "{out:?}");
    let ctx = to_brain[0].1.clone();
    assert_eq!(ctx["summarize"], json!("1"), "{ctx:?}");
    let back_ctx = json!({"session_id": "s1", "turn_id": "t3", "curator_call": "k",
                         "summarize": "1", "summary_call": "rb", "summary_token": "tok"});
    // A partial of that call goes nowhere.
    let partial = deliveries(
        &t,
        "/t/brain",
        back_ctx.clone(),
        json!({"route": "partial"}),
    );
    assert!(partial.is_empty(), "never spoken: {partial:?}");
    // The final answer reaches the curator on `in_summary_self` only.
    for finish in ["stop", "length", "error"] {
        let d = deliveries(
            &t,
            "/t/brain",
            back_ctx.clone(),
            json!({"finish_reason": finish}),
        );
        assert_eq!(
            d.iter().map(|(to, _, _)| to.clone()).collect::<Vec<_>>(),
            vec!["/t/curator".to_string()],
            "{finish}: {d:?}"
        );
        assert_eq!(d[0].2["route"], json!("in_summary_self"), "{finish}");
    }
    // An ordinary answer still goes the old way (no `summarize` in context).
    let plain = deliveries(
        &t,
        "/t/brain",
        json!({"session_id": "s1", "turn_id": "t3", "curator_call": "k"}),
        json!({"finish_reason": "stop"}),
    );
    assert!(
        plain
            .iter()
            .any(|(to, _, hop)| to == "/t/curator" && hop["route"] == json!("in_llm")),
        "{plain:?}"
    );
    // Inside the hive: `in_summary_self` reaches `./policy` and nothing else.
    let c = table("/c", "templates/curator/config.json");
    let d = deliveries(
        &c,
        "/c",
        json!({}),
        json!({"route": "in_summary_self", "summarize": "1", "finish_reason": "stop"}),
    );
    assert_eq!(
        d.iter().map(|(to, _, _)| to.clone()).collect::<Vec<_>>(),
        vec!["/c/policy".to_string()],
        "{d:?}"
    );
}

/// Review r1 I3 (class GH #925/#1247): the self call sends the role's `llm`
/// the whole system of the last call -- the cell keeps ONE tree, so a family
/// another call changed since is set back. The hashes the policy diffs the
/// next call against are cleared as for `quick`, and the next call sends
/// every family whole.
#[test]
fn the_self_call_clears_the_sent_hashes() {
    if !shipped() {
        return;
    }
    let (mut h, _) = rebuild_as(
        &[("policy", "summarizer_self", json!("1"))],
        "compress",
        false,
    );
    let call = h.routed("brain");
    assert_eq!(call.len(), 1, "{:?}", h.stderr);
    let (sum_call, token) = (
        call[0].hop["summary_call"].clone(),
        call[0].hop["summary_token"].clone(),
    );
    h.lane(
        "in_summary_self",
        json!({}),
        json!({"finish_reason": "stop", "model": "role-model", "summarize": "1",
               "summary_call": sum_call, "summary_token": token}),
        json!({"messages": [said("S-self.")]}),
    );
    assert_eq!(
        h.state("system_hash_sent"),
        "{}",
        "after the self call nothing counts as sent: {:?}",
        h.stderr
    );
}

/// Review z3 r2 m1 (OR-d38-2.o3.7): the hashes are cleared when the self
/// call LEAVES, not only when its answer comes home -- a call built while the
/// role condenses diffs against the tree the self call is about to set back.
/// Through the summarizer nothing is sent to the role, nothing is cleared.
#[test]
fn the_self_call_clears_the_sent_hashes_when_it_leaves() {
    if !shipped() {
        return;
    }
    let (h, _) = rebuild_as(&[], "compress", false);
    assert_eq!(h.summ.len(), 1, "{:?}", h.stderr);
    let sent = h.state("system_hash_sent");
    assert!(
        !sent.is_empty() && sent != "{}",
        "the hot path noted what it sent: {sent:?}"
    );
    let (h, _) = rebuild_as(
        &[("policy", "summarizer_self", json!("1"))],
        "compress",
        false,
    );
    assert_eq!(h.routed("brain").len(), 1, "{:?}", h.stderr);
    assert_eq!(
        h.state("system_hash_sent"),
        "{}",
        "the self call left, nothing counts as sent: {:?}",
        h.stderr
    );
}

/// Review r1 M1: a rebuild for an expired cache finds the prefix cold --
/// the role would pay the full price over the whole window for a prefix
/// that is replaced right after. The summarizer condenses.
#[test]
fn a_cache_rebuild_goes_to_the_summarizer() {
    if !shipped() {
        return;
    }
    let (h, _) = rebuild_as(&[("policy", "summarizer_self", json!("1"))], "cache", false);
    assert_eq!(h.summ.len(), 1, "the summarizer condenses: {:?}", h.stderr);
    assert!(h.routed("brain").is_empty(), "nothing on the role's way");
}

/// Review r1 M3: after a tool round the newest final answer is the one of
/// the turn before -- prefix and answer would not belong together. The
/// summarizer condenses.
#[test]
fn after_a_tool_round_the_summarizer_condenses() {
    if !shipped() {
        return;
    }
    let (h, _) = rebuild_as(
        &[("policy", "summarizer_self", json!("1"))],
        "compress",
        true,
    );
    assert_eq!(h.summ.len(), 1, "the summarizer condenses: {:?}", h.stderr);
    assert!(h.routed("brain").is_empty(), "nothing on the role's way");
}
