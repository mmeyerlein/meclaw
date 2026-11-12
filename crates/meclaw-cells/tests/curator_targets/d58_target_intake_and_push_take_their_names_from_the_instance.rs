//! D58 target (GH #1483, contract points 4 and 11) -- the intake and the
//! push's ask take their names and their bounds from the instance and the
//! targets, not from a composite's cell names.
//!
//! WHY this file exists: `./intake` knew the errand tool (`consult_cogny`),
//! the agent behind an advice (`cogny`) and a bridge (the call id ending
//! `-bridge`) by fixed names; `./policy` knew the slot of the open errands
//! (`consult`) by name and stamped the push's ask and the model call with
//! the values of its own role and the bare bound of the last answer. Each
//! name is now a knob of the instance (`errand_tool`, `agent_sources`,
//! `errand_slot`), a bridge says so in its context (`context.bridge`, lifted
//! by the composite from `hop.bridge`), and the push's ask and the model
//! call read the target that holds the wall (OR-d58-2.target.16).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

const TOOL: &str = "ask_helper";
const SLOT: &str = "errand";

fn usage(input_soft: i64) -> Value {
    json!({"tokens_prompt": 2000, "tokens_cached": 1000,
           "input_soft": input_soft, "input_hard": 10_000_000,
           "cost_in": 10, "cost_cached_in": 0,
           "cache_expires_at": "2099-01-01T00:00:00Z"})
}

/// A tool call the model wrote itself, as the llm cell hands it on the tap.
fn call_of(id: &str, name: &str) -> Value {
    json!({"origin": "assistant", "type": "tool_call", "id": id,
           "text": json!({"name": name, "arguments": "{\"goal\":\"plan\"}"}).to_string()})
}

fn denied(call: &Msg) -> Vec<String> {
    call.body
        .get("tool_scope")
        .and_then(|s| s.get("deny"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn errand_hive() -> Hive {
    Hive::with(&[
        ("lead", "errand_tool", json!(TOOL)),
        ("intake", "errand_tool", json!(TOOL)),
        ("intake", "errand_slot", json!(SLOT)),
        ("policy", "errand_slot", json!(SLOT)),
    ])
}

#[test]
fn d58_target_the_intake_knows_the_errand_tool_by_its_knob() {
    if !shipped() {
        return;
    }
    let mut h = errand_hive();
    announce_profile(&mut h, "main", json!({"system": "delta"}));
    let c = say(&mut h, Some("main"), "t1", "plan my week");
    answer(
        &mut h,
        &c,
        "tool_calls",
        usage(5_000_000),
        json!([call_of("c1", TOOL)]),
    );
    let mark: Value = meclaw_core::serde_json::from_str(&h.lead_state("consult_pending:s1"))
        .unwrap_or(Value::Null);
    assert_eq!(mark["consult_id"], json!("c1"), "{mark} {:?}", h.stderr);

    // The next call: the tool is denied, the open errand stands in the slot
    // the instance names.
    let next = say(&mut h, Some("main"), "t2", "and tomorrow?");
    assert!(
        denied(&next).contains(&TOOL.to_string()),
        "{:?}",
        next.body.get("tool_scope")
    );
    assert!(
        next.body["system"][SLOT]["pending"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("c1")),
        "{:?}",
        next.body["system"]
    );
    assert!(
        next.body["system"].get("consult").is_none(),
        "{:?}",
        next.body["system"]
    );
}

#[test]
fn d58_target_a_tool_the_knob_does_not_name_starts_no_errand() {
    if !shipped() {
        return;
    }
    // The shipped intake names no errand tool: the name of before is a tool
    // like any other.
    let mut h = Hive::with(&[("intake", "errand_tool", json!(""))]);
    announce_profile(&mut h, "main", json!({"system": "delta"}));
    let c = say(&mut h, Some("main"), "t1", "plan my week");
    answer(
        &mut h,
        &c,
        "tool_calls",
        usage(5_000_000),
        json!([call_of("c1", "consult_cogny")]),
    );
    assert_eq!(h.lead_state("consult_pending:s1"), "", "{:?}", h.stderr);
}

const RULE: &str = r#"{"items":"facts","quote":"source"}"#;
const ADVICE: &str = "[advice from your reasoning core, consult c1]\nThe person might enjoy a trip to the Baltic Sea this summer.";

fn fact(quote: &str, source: &str) -> Value {
    json!({"predicate": "likes", "value": "x", "quote": quote, "source": source})
}

#[test]
fn d58_target_an_advice_is_filed_under_the_agent_the_instance_names() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        (
            "intake",
            "sections",
            json!({"memory": {"lane": "sidecar", "mark": "topic"}}),
        ),
        (
            "intake",
            "agent_sources",
            json!("advice:core,delegation:voice"),
        ),
    ]);
    turn(&mut h, "s1", "r1", "Hello there.", "Hi.", json!({}));
    let call = h.curate("s1", "r2", 0, json!([user(ADVICE)]), mode("Be brief."));
    let mut ctx = call.context.clone();
    for k in [
        "curator_call",
        "turn_id",
        "session_id",
        "iter",
        "episode_turn_id",
    ] {
        ctx.insert(
            k.into(),
            call.hop.get(k).cloned().unwrap_or_else(|| json!("")),
        );
    }
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(ctx),
        json!({"section": "memory", "turn_id": call.hop["turn_id"], "sidecar_verify": RULE}),
        json!({"messages": [], "section": "memory", "verify_answer": "Sounds good.",
        "payload": {"facts": [
            fact("enjoy a trip to the Baltic Sea", "agent:core"),
            fact("enjoy a trip to the Baltic Sea", "agent:cogny"),
        ]}}),
    );
    let sides = h.routed("sidecar");
    assert_eq!(sides.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let quotes: Vec<String> = sides[0].body["payload"]["facts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|f| f["source"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        quotes,
        ["agent:core"],
        "the knob names the agent of an advice"
    );
    let reasons: Vec<String> = h
        .routed("reject")
        .iter()
        .map(|m| m.hop["reject_reason"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        reasons,
        ["source_unknown"],
        "a name the knob does not give is no source"
    );
}

/// The recall ask of a turn, as the collector sends it.
fn recall_ask(h: &mut Hive, turn: &str, text: &str) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": "s1", "channel": "test", "audience_set": ROUND_E}),
        json!({"phase": "recall", "turn_id": turn, "session_id": "s1", "iter": "0",
               "recall_query": text, "memory_tier": "1",
               "recall_window_from": "", "recall_window_to": ""}),
        json!({"messages": [user(text)]}),
    );
    h.routed("recall")
}

/// Whether `./push` handed the last ask on as it came: its one read of the
/// usable window (phase `pass`) instead of the reads of a pushed question.
fn passed_through(h: &Hive) -> bool {
    h.ledger_steps
        .iter()
        .any(|(cell, phase)| cell.ends_with("push") && phase == "pass")
}

#[test]
fn d58_target_the_push_asks_with_the_values_of_the_tree_holding_target() {
    if !shipped() {
        return;
    }
    // The instance does not push; the target that holds the wall does.
    let mut h = Hive::new();
    h.ledger_steps.clear();
    let off = recall_ask(&mut h, "t1", "where again?");
    assert_eq!(off.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        passed_through(&h),
        "the instance does not push: {:?}",
        h.ledger_steps
    );
    announce_profile(
        &mut h,
        "main",
        json!({"system": "delta", "recall_push": "1"}),
    );
    announce_profile(
        &mut h,
        "side",
        json!({"system": "full", "recall_push": "0"}),
    );
    h.ledger_steps.clear();
    let on = recall_ask(&mut h, "t2", "where again?");
    assert_eq!(on.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        !passed_through(&h),
        "the tree-holding target pushes: {:?}",
        h.ledger_steps
    );

    // The instance pushes; the target that holds the wall says no.
    let mut h = Hive::with(&[("policy", "recall_push", json!("1"))]);
    h.ledger_steps.clear();
    let on = recall_ask(&mut h, "t0", "where again?");
    assert_eq!(on.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        !passed_through(&h),
        "the instance pushes: {:?}",
        h.ledger_steps
    );
    announce_profile(
        &mut h,
        "main",
        json!({"system": "delta", "recall_push": "0"}),
    );
    h.ledger_steps.clear();
    let off = recall_ask(&mut h, "t1", "where again?");
    assert_eq!(off.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        passed_through(&h),
        "the target says no: {:?}",
        h.ledger_steps
    );
}

#[test]
fn d58_target_the_call_sizes_the_memory_by_the_bound_of_its_wall_target() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    announce_profile(&mut h, "first", json!({"system": "delta"}));
    announce_profile(&mut h, "second", json!({"system": "delta"}));
    let a = say(&mut h, Some("first"), "t1", TALK[0].0);
    answer(
        &mut h,
        &a,
        "stop",
        usage(2_000_000),
        json!([said(TALK[0].1)]),
    );
    let b = say(&mut h, Some("second"), "t2", TALK[1].0);
    answer(
        &mut h,
        &b,
        "stop",
        usage(1_000_000),
        json!([said(TALK[1].1)]),
    );
    // The last answer was the second target's; the call to the first holds
    // its own wall and sizes the memory by its own bound.
    let c = say(&mut h, Some("first"), "t3", TALK[2].0);
    assert_eq!(c.hop["recall_input_soft"], json!("2000000"), "{:?}", c.hop);
    let d = say(&mut h, Some("second"), "t4", TALK[3].0);
    assert_eq!(d.hop["recall_input_soft"], json!("1000000"), "{:?}", d.hop);
}

/// The answer texts the wall holds for turn `turn`, in `seq` order.
fn wall_answers(h: &Hive, turn: &str) -> Vec<String> {
    h.rows(&format!(
        "SELECT b.body FROM wall w JOIN blocks b ON b.hash = w.hash \
         WHERE w.turn_id = '{turn}' AND w.kind = 'assistant' ORDER BY w.seq"
    ))
    .into_iter()
    .map(|r| {
        let el: Value = meclaw_core::serde_json::from_str(r[0].as_str().unwrap_or_default())
            .unwrap_or(Value::Null);
        el["text"].as_str().unwrap_or_default().to_string()
    })
    .collect()
}

#[test]
fn d58_target_the_intake_knows_a_bridge_by_its_context_not_its_call_id() {
    if !shipped() {
        return;
    }
    const SHOWN: &str = "Let me see.";
    const REST: &str = "It rains tomorrow.";
    for (bridge, want) in [
        (None, format!("{SHOWN} {REST}")),
        (Some("tool"), REST.to_string()),
    ] {
        let mut h = Hive::new();
        announce_profile(&mut h, "main", json!({"system": "delta"}));
        let mut c = say(&mut h, Some("main"), "t1", "will it rain?");
        h.db.execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)",
            [
                "spec_shown:s1#t1".to_string(),
                json!({"call": "earlier", "text": SHOWN}).to_string(),
            ],
        )
        .expect("the shown sentence");
        // D58 moveout (contract point 3): the side call's kind, as the
        // composite lifts it (`call_kind`); the value is the curator's.
        if bridge.is_some() {
            c.hop.insert("call_kind".into(), json!("side"));
        }
        answer(&mut h, &c, "stop", usage(5_000_000), json!([said(REST)]));
        assert_eq!(
            wall_answers(&h, "t1"),
            vec![want],
            "{bridge:?} {:?}",
            h.stderr
        );
    }
}
