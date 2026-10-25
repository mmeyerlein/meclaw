//! GH #1298 -- the system part a brain reads stands, byte for byte, over the
//! tool rounds of one turn.
//!
//! Measured in production (talky under a native Anthropic brain): the quick
//! voice answered the turns before with a small window (`input_soft` 20000),
//! the curator kept it in state, and the brain's first call of the next turn
//! was built with it -- the sidecar contract cut to a tenth of 20000 tokens,
//! five optional sections `[dropped ... over budget]`. The brain answered with
//! a tool call; its own answer stamped its package (`input_soft` 90000), and
//! the call after the tool result was built with THAT window: the contract
//! whole, the `instructions` family changed, the provider's `system` no
//! longer the one the brain's signed thinking was made under. The Messages
//! wire refuses such a replay (400 `Invalid signature in thinking block ...
//! The system prompt differs from the one this block was created with`), and
//! the person heard the interim sentence and then nothing.
//!
//! The lock: through the shipped hive, the system tree the model holds after
//! the call of the tool round is the tree it held after the turn's first call
//! -- whatever the window, the pins or the slots did in between; the next
//! turn takes the new window.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Map, Value, json};

/// The collector's contract (`instructions.sidecar`): the frame the policy
/// writes over the sections, one required section, two optional ones of 500
/// characters -- over a tenth of 1000 tokens (300 characters), under a tenth
/// of 90000.
fn contract() -> String {
    let (frame, _) = policy_scope(
        json!({}),
        "sidecar_frame(ARGS[0], ARGS[1])",
        json!([
            [["memory", "{...}"]],
            [["gap", "\"...\""], ["window", "{...}"]]
        ]),
    );
    [
        frame.as_str().expect("the frame").to_string(),
        "## memory (required)\nWrite it.\n{\"memory\":{}}".to_string(),
        format!("## gap (optional)\n{}\n{{\"gap\":\"\"}}", "g".repeat(500)),
        format!(
            "## window (optional)\n{}\n{{\"window\":{{}}}}",
            "w".repeat(500)
        ),
    ]
    .join("\n\n")
}

/// A system write applied to the tree the `llm` cell keeps (GH #264): a node
/// marked `"$replace": true` takes the place of its whole subtree, any other
/// node merges key by key, a leaf (`{"text": ...}`) replaces its leaf.
fn apply(tree: &mut Map<String, Value>, write: &Value) {
    let Some(write) = write.as_object() else {
        return;
    };
    for (k, v) in write {
        if k == "$replace" {
            continue;
        }
        let replaces = v.get("$replace") == Some(&json!(true));
        let leaf = v.get("text").is_some_and(Value::is_string);
        if replaces || leaf || !v.is_object() {
            tree.insert(k.clone(), strip(v));
        } else {
            let entry = tree.entry(k.clone()).or_insert_with(|| json!({}));
            if let Some(sub) = entry.as_object_mut() {
                apply(sub, v);
            }
        }
    }
}

fn strip(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| k.as_str() != "$replace")
                .map(|(k, v)| (k.clone(), strip(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn sidecar(tree: &Map<String, Value>) -> String {
    tree.get("instructions")
        .and_then(|i| i.get("sidecar"))
        .and_then(|s| s.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[test]
fn the_system_of_a_brain_turn_stands_over_its_tool_rounds() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    // The turn before was answered by a model with a small window (in
    // production the quick voice): its stamp stands in state.
    turn(&mut h, "s1", "t1", "q1", "a1", json!({"input_soft": 1000}));
    h.lane(
        "in_slots",
        json!({}),
        json!({}),
        json!({"system": {"instructions": {"sidecar": {"text": contract()}}}}),
    );
    let call0 = h.curate("s1", "t2", 0, json!([user("q2")]), mode("Be brief."));
    let mut held = Map::new();
    apply(&mut held, &call0.body["system"]);
    assert!(
        sidecar(&held).contains("over budget]"),
        "the first call is cut to the small window: {}",
        sidecar(&held)
    );
    // The brain answers with a call and stamps its own, larger package.
    h.tap(
        &call0,
        "tool_calls",
        json!({"input_soft": 90000}),
        json!([tool_call("c1", "look")]),
    );
    assert_eq!(h.state("input_soft"), "90000", "{:?}", h.stderr);
    let call1 = h.curate(
        "s1",
        "t2",
        1,
        json!([
            user("q2"),
            tool_call("c1", "look"),
            tool_result("c1", "rain")
        ]),
        mode("Be brief."),
    );
    let before = held.clone();
    apply(&mut held, &call1.body["system"]);
    assert_eq!(
        Value::Object(held.clone()),
        Value::Object(before),
        "the tool round reads the system its turn began with, byte for byte: {}",
        call1.body["system"]
    );

    // The turn ends; the next turn is built with the new window.
    h.tap(
        &call1,
        "stop",
        json!({"input_soft": 90000}),
        json!([said("rain")]),
    );
    let call2 = h.curate("s1", "t3", 0, json!([user("q3")]), mode("Be brief."));
    apply(&mut held, &call2.body["system"]);
    assert_eq!(
        sidecar(&held),
        contract(),
        "a new turn takes the window of the last answer"
    );
}

// ======================================== fix round F2c (review of F2)

/// A quick call of session `s1` (`path` simple: the quick way of talky).
fn quick(h: &mut Hive, turn_id: &str, ask: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": "",
                     "path": "simple"});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ask)]}));
    let calls = h.routed("quick");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    calls[0].clone()
}

fn slot(h: &mut Hive, text: &str) {
    h.lane(
        "in_slots",
        json!({}),
        json!({}),
        json!({"system": {"instructions": {"sidecar": {"text": text}}}}),
    );
}

/// The `what` of every cut on the call's hop that names a contract section.
fn contract_cuts(call: &Msg) -> Vec<Value> {
    call.hop
        .get("cuts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| {
            c["what"]
                .as_str()
                .is_some_and(|w| w.starts_with("sidecar section"))
        })
        .cloned()
        .collect()
}

fn system_rows(h: &Hive) -> Vec<String> {
    h.rows("SELECT key FROM state WHERE key LIKE 'turn_system:%' ORDER BY key")
        .iter()
        .filter_map(|r| r[0].as_str().map(str::to_string))
        .collect()
}

/// Review M1 of F2 -- the measured cause: the quick voice's answer stamped
/// its small window into the state the brain's call read. Its window now
/// stays its own (`input_soft:quick`); the brain's first call after it gets
/// the whole contract and is guarded at its own hard line.
#[test]
fn a_quick_answer_leaves_the_brain_its_own_window() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    slot(&mut h, &contract());
    let first = turn(
        &mut h,
        "s1",
        "t1",
        "q1",
        "a1",
        json!({"input_soft": 90000, "input_hard": 100000}),
    );
    let mut held = Map::new();
    apply(&mut held, &first.body["system"]);
    let q = quick(&mut h, "t2", "q2");
    h.tap(
        &q,
        "stop",
        json!({"input_soft": 1000, "input_hard": 40}),
        json!([said("a2")]),
    );
    assert_eq!(h.state("input_soft"), "90000", "{:?}", h.stderr);
    assert_eq!(h.state("input_hard"), "100000");
    assert_eq!(h.state("input_soft:quick"), "1000");
    assert_eq!(h.state("input_hard:quick"), "40");

    let call = h.curate("s1", "t3", 0, json!([user("q3")]), mode("Be brief."));
    apply(&mut held, &call.body["system"]);
    assert_eq!(
        sidecar(&held),
        contract(),
        "the brain's first call after a quick answer carries the whole contract"
    );
    assert!(
        !call
            .hop
            .get("cuts")
            .and_then(Value::as_array)
            .is_some_and(|c| c.iter().any(|x| x["what"] == "overflow_cut")),
        "the brain is guarded at its own hard line, not at quick's: {:?}",
        call.hop.get("cuts")
    );
}

/// Review M1 of F2 (2): the earlier answers of a turn's window read alike
/// over its rounds -- a sidecar block the first call showed as dropped stays
/// dropped after the brain's answer stamped a larger window, so the
/// `messages` before a replayed thinking block are the ones it was made
/// under.
#[test]
fn the_earlier_answers_read_alike_over_the_rounds_of_a_turn() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let big = format!("a1.\n\n```sidecar\n{{\"a\": \"{}\"}}\n```", "x".repeat(600));
    let c = h.curate("s1", "t1", 0, json!([user("q1")]), mode("Be brief."));
    h.tap(&c, "stop", json!({"input_soft": 1000}), json!([said(&big)]));
    let call0 = h.curate("s1", "t2", 0, json!([user("q2")]), mode("Be brief."));
    let m0 = call0.messages();
    assert!(
        m0.iter().any(|m| m["text"]
            .as_str()
            .is_some_and(|t| t.contains("[dropped: sidecar"))),
        "the small window drops the earlier block: {m0:?}"
    );
    h.tap(
        &call0,
        "tool_calls",
        json!({"input_soft": 90000}),
        json!([tool_call("c1", "look")]),
    );
    let call1 = h.curate(
        "s1",
        "t2",
        1,
        json!([
            user("q2"),
            tool_call("c1", "look"),
            tool_result("c1", "rain")
        ]),
        mode("Be brief."),
    );
    let m1 = call1.messages();
    assert!(m1.len() > m0.len(), "{m1:?}");
    assert_eq!(
        m1[..m0.len()],
        m0[..],
        "the messages before the tool round are those of the turn's first call"
    );
}

/// Review m1 of F2: a call that sends the kept system says the contract
/// cuts of what it sends, not of a contract it does not send.
#[test]
fn a_kept_system_says_the_cuts_it_sends() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({"input_soft": 1000}));
    slot(&mut h, &contract());
    let call0 = h.curate("s1", "t2", 0, json!([user("q2")]), mode("Be brief."));
    let cut0 = contract_cuts(&call0);
    assert!(!cut0.is_empty(), "{:?}", call0.hop.get("cuts"));
    h.tap(
        &call0,
        "tool_calls",
        json!({"input_soft": 1000}),
        json!([tool_call("c1", "look")]),
    );
    // A new contract lands within the turn: the turn keeps its own, and says
    // the cuts of the one it sends.
    slot(&mut h, "## memory (required)\nWrite it.\n{\"memory\":{}}");
    let call1 = h.curate(
        "s1",
        "t2",
        1,
        json!([
            user("q2"),
            tool_call("c1", "look"),
            tool_result("c1", "rain")
        ]),
        mode("Be brief."),
    );
    assert_eq!(contract_cuts(&call1), cut0, "{:?}", call1.hop.get("cuts"));
}

/// Review m3 of F2: one system row per running turn -- two turns of one
/// session keep their own, the brain's last answer takes its row down, and
/// a row a round left behind goes with a later turn's first call.
#[test]
fn a_turn_keeps_its_own_system_row_and_takes_it_down() {
    if !shipped() {
        return;
    }
    // The shortest sweep (60 s), so the row left behind can be placed past it
    // inside the lookback the midnight wait holds (`backdated`).
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "pending_sweep_s", json!(60)),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({"input_soft": 1000}));
    slot(&mut h, &contract());
    let a0 = h.curate("s1", "tA", 0, json!([user("qa")]), mode("Be brief."));
    let b0 = h.curate("s1", "tB", 0, json!([user("qb")]), mode("Be brief."));
    assert_eq!(
        system_rows(&h),
        vec!["turn_system:s1:tA", "turn_system:s1:tB"],
        "{:?}",
        h.stderr
    );
    assert!(!contract_cuts(&a0).is_empty(), "{:?}", a0.hop.get("cuts"));
    // tA's round goes on after tB began, and the brain's window grew: tA
    // still sends the system it began with (a fresh one would carry the
    // contract whole and say no cut).
    h.tap(
        &a0,
        "tool_calls",
        json!({"input_soft": 90000}),
        json!([tool_call("ca", "look")]),
    );
    let a1 = h.curate(
        "s1",
        "tA",
        1,
        json!([
            user("qa"),
            tool_call("ca", "look"),
            tool_result("ca", "rain")
        ]),
        mode("Be brief."),
    );
    assert_eq!(contract_cuts(&a1), contract_cuts(&a0), "{:?}", a1.hop);
    h.tap(
        &a1,
        "stop",
        json!({"input_soft": 90000}),
        json!([said("ra")]),
    );
    assert_eq!(system_rows(&h), vec!["turn_system:s1:tB"]);
    h.tap(
        &b0,
        "stop",
        json!({"input_soft": 90000}),
        json!([said("rb")]),
    );
    assert!(system_rows(&h).is_empty(), "{:?}", system_rows(&h));

    // A round that ends on a hand-over (nothing re-enters the brain) leaves
    // its row; once it is older than `pending_sweep_s`, the next turn's first
    // call takes it down.
    let c0 = h.curate("s1", "tC", 0, json!([user("qc")]), mode("Be brief."));
    h.tap(
        &c0,
        "tool_calls",
        json!({"input_soft": 90000}),
        json!([tool_call("cc", "look")]),
    );
    assert_eq!(system_rows(&h), vec!["turn_system:s1:tC"]);
    let old = backdated(LOOKBACK_S).to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    h.rows(&format!(
        "UPDATE state SET value = '[[\"tC\",\"{old}\"]]' \
         WHERE key = 'turn_system_last:s1' RETURNING key"
    ));
    let d0 = h.curate("s1", "tD", 0, json!([user("qd")]), mode("Be brief."));
    assert_eq!(system_rows(&h), vec!["turn_system:s1:tD"], "{:?}", h.stderr);
    h.tap(
        &d0,
        "stop",
        json!({"input_soft": 90000}),
        json!([said("rd")]),
    );
    assert!(system_rows(&h).is_empty(), "{:?}", system_rows(&h));

    // Delta review m5: the brain's first call of a turn the quick voice
    // gave up after a tool result (`speaker`: an error result is
    // unspeakable, the brain call runs at iter 1) lists its row as well, so
    // a round that ends there on a hand-over leaves nothing a later turn
    // does not take down.
    let seam_e = |h: &mut Hive, iter: &str, messages: Value| {
        h.out.clear();
        let hop = json!({"session_id": "s1", "turn_id": "tE", "iter": iter, "phase": "",
                         "path": "simple", "families": "web", "needs": "tool"});
        let ctx = json!({"session_id": "s1", "turn_id": "tE", "iter": iter,
                         "channel": "test", "audience_set": ROUND_E, "path": "simple"});
        h.lane("in_curate", ctx, hop, json!({"messages": messages}));
    };
    seam_e(&mut h, "0", json!([user("rain tomorrow?")]));
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        2,
        "bridge and call: {:?} {:?}",
        h.out,
        h.stderr
    );
    h.tap(
        &quick[1],
        "tool_calls",
        json!({}),
        json!([tool_call("ce", "look")]),
    );
    assert!(system_rows(&h).is_empty(), "{:?}", system_rows(&h));
    seam_e(
        &mut h,
        "1",
        json!([
            user("rain tomorrow?"),
            tool_call("ce", "look"),
            tool_result("ce", "{\"error\": \"down\"}")
        ]),
    );
    let brain = h.routed("brain");
    assert_eq!(brain.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(brain[0].hop["iter"], json!("1"), "{:?}", brain[0].hop);
    h.tap(
        &brain[0],
        "tool_calls",
        json!({"input_soft": 90000}),
        json!([tool_call("cf", "look")]),
    );
    assert_eq!(system_rows(&h), vec!["turn_system:s1:tE"]);
    let listed = h.rows("SELECT value FROM state WHERE key = 'turn_system_last:s1'");
    let listed: Vec<Value> = listed
        .first()
        .and_then(|r| r[0].as_str())
        .and_then(|v| meclaw_core::serde_json::from_str(v).ok())
        .unwrap_or_default();
    assert!(
        listed.iter().any(|x| x[0] == "tE"),
        "the escalated turn's row is listed: {listed:?}"
    );
    let aged: Vec<Value> = listed.iter().map(|x| json!([x[0], old])).collect();
    h.rows(&format!(
        "UPDATE state SET value = '{}' WHERE key = 'turn_system_last:s1' RETURNING key",
        Value::Array(aged)
    ));
    h.curate("s1", "tF", 0, json!([user("qf")]), mode("Be brief."));
    assert_eq!(system_rows(&h), vec!["turn_system:s1:tF"], "{:?}", h.stderr);
}

/// Review m2 of F2 (OR-EF-9): a consult the brain starts within a turn takes
/// the consult tool away from the next call at once (`tools` is not frozen;
/// the llm cell's `drop_block` keeps the turn), while the system the turn
/// began with stands byte for byte -- within the turn the tool result
/// carries the consult; its `consult.pending` note joins the system from the
/// next turn on.
#[test]
fn a_consult_started_in_a_turn_denies_the_tool_and_keeps_the_system() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    let call0 = h.curate("s1", "t2", 0, json!([user("plan")]), mode("Be brief."));
    let mut held = Map::new();
    apply(&mut held, &call0.body["system"]);
    let consult = json!({"origin": "assistant", "type": "tool_call", "id": "k1",
        "text": json!({"name": "consult_cogny",
                       "arguments": json!({"goal": "plan"}).to_string()}).to_string()});
    h.tap(&call0, "tool_calls", json!({}), json!([consult.clone()]));
    let call1 = h.curate(
        "s1",
        "t2",
        1,
        json!([user("plan"), consult, tool_result("k1", "asked")]),
        mode("Be brief."),
    );
    let denied: Vec<&str> = call1
        .body
        .get("tool_scope")
        .and_then(|t| t.get("deny"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    assert!(denied.contains(&"consult_cogny"), "{:?}", call1.body);
    let before = held.clone();
    apply(&mut held, &call1.body["system"]);
    assert_eq!(
        Value::Object(held.clone()),
        Value::Object(before),
        "the system of the turn stands, without the note: {}",
        call1.body["system"]
    );
    h.tap(&call1, "stop", json!({}), json!([said("asked")]));
    let call2 = h.curate("s1", "t3", 0, json!([user("and?")]), mode("Be brief."));
    apply(&mut held, &call2.body["system"]);
    let note = held
        .get("consult")
        .and_then(|c| c.get("pending"))
        .and_then(|p| p.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        note.contains("A consult is pending (id "),
        "the next turn carries the note: {}",
        call2.body["system"]
    );
}

// ======================================== agent-2 A2, review I1 (OR-AG-50)

/// A call of session `s1` from a parent with stages (simple-agent 2): the
/// collector's context carries `sa_stage`, the call leaves on `brain`.
fn staged(h: &mut Hive, turn_id: &str, stage: &str, ask: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "sa_stage": stage});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(ask)], "system": mode("Be brief.")}),
    );
    let calls = h.routed("brain");
    assert_eq!(calls.len(), 1, "{:?} {:?}", h.out, h.stderr);
    calls[0].clone()
}

#[test]
fn each_stage_is_sized_by_the_package_of_its_own_model() {
    if !shipped() {
        return;
    }
    // fast answers with a large package, strong with a small one: the
    // contract fits a tenth of fast's window and not a tenth of strong's.
    let fast = json!({"input_soft": 90000, "input_hard": 180000, "tokens_prompt": 100});
    let strong = json!({"input_soft": 1000, "input_hard": 400000, "tokens_prompt": 100});
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    slot(&mut h, &contract());
    let first = staged(&mut h, "t0", "strong", "q0");
    h.tap(&first, "stop", strong.clone(), json!([said("a0")]));
    for i in 1..=5 {
        let call = staged(&mut h, &format!("t{i}"), "fast", &format!("q{i}"));
        // The first fast call knows no fast package yet: it takes the last
        // answer's, as a way without a window of its own does.
        assert!(
            i == 1 || contract_cuts(&call).is_empty(),
            "fast is sized by fast's package: {:?}",
            call.hop
        );
        h.tap(&call, "stop", fast.clone(), json!([said("a")]));
    }
    // A strong turn after five fast ones: strong's window, not the last
    // answer's -- and no hard rebuild.
    let call = staged(&mut h, "t6", "strong", "q6");
    assert!(
        !contract_cuts(&call).is_empty(),
        "strong is sized by strong's package, not by fast's last answer: {:?}",
        call.hop
    );
    h.tap(&call, "stop", strong.clone(), json!([said("a6")]));
    let mark = |h: &Hive| {
        meclaw_core::serde_json::from_str::<Value>(&h.state_in("curate", ROUND_E))
            .unwrap_or(Value::Null)["curate_mark"]
            .clone()
    };
    assert_ne!(mark(&h), json!("hard"), "{:?}", h.stderr);
    // And back: a fast turn after strong takes fast's window again.
    let call = staged(&mut h, "t7", "fast", "q7");
    assert!(
        contract_cuts(&call).is_empty(),
        "fast after strong is sized by fast's package: {:?}",
        call.hop
    );
    h.tap(&call, "stop", fast, json!([said("a7")]));
    assert_ne!(mark(&h), json!("hard"), "{:?}", h.stderr);
    // The bare key stays the last answer's: the memory ask is sized by it.
    assert_eq!(h.state("input_soft"), "90000");
}
