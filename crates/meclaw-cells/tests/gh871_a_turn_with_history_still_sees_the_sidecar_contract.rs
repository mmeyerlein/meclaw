//! GH #871 — a turn with history still sees the sidecar contract: every earlier
//! answer in the window carries the block it was written with.
//!
//! Measured before the fix (read-only replay of a running colony's brain turns,
//! `workshop/evals/conversation-guide/replay_871.py`, gpt-6-luna, 23 turns x 5,
//! request reconstruction token-exact against the provider's own count): the
//! model wrote the ```` ```sidecar ```` block in 100 % of the turns WITHOUT an
//! earlier answer in the window and in 38 % of the turns WITH one. The splitter
//! cuts the block out of an answer before the collector files it, so the window
//! showed the model, turn after turn, its own answers without one -- a precedent
//! the contract in the system part lost against. The same window showing each
//! earlier answer with its block: 98 %. Moving the contract behind every other
//! system family (53 %) or adding a sentence to its preamble (38 %) did not.
//!
//! The repair is the hand-over, in three hops and one store column:
//! `talky/splitter` hands the cut block on beside the answer (`sidecar_raw`,
//! contract 1.0.3), `dispatcher` passes it through on the answer lane (1.2.2),
//! and `collector/assemble` keeps it beside the answer (`turns.sidecar`) and
//! renders an earlier answer as text + block (`wire_turn`). No channel ever
//! receives the block.
//!
//! **GH #889.** The window moved to the curator (R-27-1): the collector files no
//! answer and no block any more, and the curator keeps what the model said, raw
//! and with its block, off the brain's tap. Claims 1 and 3 below and the cap on
//! a kept block are the curator's now (`earlier_answer_keeps_its_block_up_to_the_cap`
//! in its own tests); what stays here is the hand-over and the channel.
//!
//! What this file pins, measured at the receiver:
//!
//! 1. (moved to the curator) the system message the PROVIDER receives carries
//!    the contract;
//! 3. (moved to the curator) every earlier `assistant` message the provider
//!    receives carries its block;
//! 4. the answer leaving the collector for the channel carries no fence and no
//!    `sidecar_raw`, and the collector files neither.
//!
//! **Fix round 1 (review I-1, I-2).** The consult return still saw a bare
//! earlier answer: the sentence the model writes BESIDE a consult call leaves
//! through the dispatcher's interim path, and the splitter hands a tool round
//! on untouched, so no block ever rode with it. In three of the four replayed
//! consult turns that sentence was the only earlier answer in view, and the
//! repaired window still showed it bare -- 9 of 20 with a block. The splitter
//! now hands such a sentence the contract's own nothing form as its block
//! (the round itself stays byte-identical, GH #378), the dispatcher passes it
//! on the interim path, and the collector files it like any other block. And
//! an UNFENCED object in the section form, `{"memory": {...}}`, is read and
//! cut whole: the scan used to stop at the inner object and left `{"memory":`
//! in front of the reader (the leak class of GH #534).

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{ASSEMBLE, DISPATCHER, calls_of, lane, run_cell};
use serde_json::{Value, json};

const SPLITTER: &str = "templates/talky/splitter/config.json";
const MEMORY_SCHEMAS: &str = "templates/memory-hive/schemas/config.json";

const ANSWER: &str = "Tea it is.";
const BLOCK: &str = "```sidecar\n{\"memory\": {\"nothing_new\": false, \"facts\": [{\"subject\": \
     \"alex\", \"predicate\": \"likes\", \"object\": \"tea\"}]}}\n```";
const BESIDE_CALL: &str = "Let me ask the core about that.";
const CONSULT_CALL: &str =
    "{\"name\":\"consult_cogny\",\"arguments\":\"{\\\"question\\\":\\\"why\\\"}\"}";

// ─────────────────────────────────────────────────── the answer's three hops

/// The brain's completion, cut by the SHIPPED splitter: the answer half.
fn split(text: &str) -> Value {
    let (out, _) = run_cell(
        SPLITTER,
        &[],
        json!({"header": {"hop": {"finish_reason": "stop"}, "context": {}},
               "messages": [{"origin": "assistant", "type": "text", "text": text}]}),
    );
    out.into_iter()
        .find(|m| m["header"].get("route").is_none())
        .unwrap_or_else(|| panic!("the splitter left no answer half"))
}

/// The answer half through the SHIPPED dispatcher: the `answer` it emits.
fn dispatch(half: &Value) -> Value {
    dispatch_with(half, &[])
}

/// The same, with dispatcher knobs set (a consult is an async tool).
fn dispatch_with(half: &Value, over: &[(&str, Value)]) -> Value {
    let mut doc = json!({"header": {"hop": half["header"].clone(), "context": {}},
                         "messages": half["messages"].clone()});
    if let Some(raw) = half.get("sidecar_raw") {
        doc["sidecar_raw"] = raw.clone();
    }
    let (out, _) = run_cell(DISPATCHER, over, doc);
    out.into_iter()
        .find(|m| m["header"]["route"] == "answer")
        .unwrap_or_else(|| panic!("the dispatcher emitted no answer"))
}

/// The dispatcher's answer arriving on the collector's `in_answer` lane.
fn collect_answer(ans: &Value, over: &[(&str, Value)]) -> Vec<Value> {
    let mut doc = lane(
        "in_answer",
        json!({"finish_reason": "stop"}),
        json!({"turn_id": "t1", "iter": "1"}),
        ans["messages"].clone(),
    );
    if let Some(raw) = ans.get("sidecar_raw") {
        doc["sidecar_raw"] = raw.clone();
    }
    run_cell(ASSEMBLE, over, doc).0
}

/// GH #889: the collector files no answer -- the curator keeps what the model
/// said, raw and with its block, off the brain's tap -- so no store op of the
/// answer's emission writes a `turns` row, and the block rides on none of them.
fn assert_files_nothing(out: &[Value]) {
    let writes: Vec<Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "cstore")
        .flat_map(calls_of)
        .map(|(_, a)| a)
        .filter(|a| a["operation"] == "insert" && a["table"] == "turns")
        .collect();
    assert!(
        writes.is_empty(),
        "the collector files no answer: {writes:?}"
    );
    assert!(
        !serde_json::to_string(out).unwrap().contains("```"),
        "the block rides on no emission of the collector: {out:?}"
    );
}

// ═══════════════════════════════════════════════════════════════ the locks

/// Claim 4, and the hand-over it rests on: the block travels BESIDE the answer
/// through splitter and dispatcher, the collector files neither (GH #889: the
/// curator keeps the raw answer off the tap), and what leaves for the channel
/// carries neither the fence nor the slot.
#[test]
fn the_block_travels_beside_the_answer_and_never_to_the_channel() {
    let half = split(&format!("{ANSWER}\n\n{BLOCK}"));
    assert_eq!(
        half["messages"][0]["text"], ANSWER,
        "the splitter still cuts"
    );
    assert_eq!(
        half["sidecar_raw"], BLOCK,
        "the answer half carries the cut block, byte for byte: {half}"
    );
    let ans = dispatch(&half);
    assert_eq!(
        ans["sidecar_raw"], BLOCK,
        "the dispatcher hands the block on with the answer: {ans}"
    );
    let out = collect_answer(&ans, &[]);
    assert_files_nothing(&out);

    let channel = out
        .iter()
        .find(|m| m["header"]["route"] == "answer")
        .expect("the answer leaves for the channel");
    let wire = channel.to_string();
    assert!(
        !wire.contains("```") && channel.get("sidecar_raw").is_none(),
        "no channel ever receives the block: {channel}"
    );
    assert_eq!(channel["messages"][0]["text"], ANSWER);
}

/// A malformed block leaves the answer (GH #534) and is NOT handed on: a block
/// nobody can read is a precedent nobody should be shown.
#[test]
fn an_unreadable_block_is_cut_and_not_handed_on() {
    let half = split(&format!("{ANSWER}\n\n```sidecar\n{{\"memory\": {{\n```"));
    assert_eq!(half["messages"][0]["text"], ANSWER);
    assert!(half.get("sidecar_raw").is_none(), "{half}");
}

// GH #889: the cap on a kept block and the provider's view of every earlier
// answer with its block moved to the curator with the window (R-27-1); its own
// tests pin them (`earlier_answer_keeps_its_block_up_to_the_cap`).

// ═══════════════════════════════════════════════ fix round 1 (review I-1/I-2)

/// A tool round as the brain completes it: a consult call and, beside it, the
/// sentence the model says while the core works. Cut by the SHIPPED splitter.
fn split_round(text: &str) -> (Value, Value) {
    let input = json!({"header": {"hop": {"finish_reason": "tool_calls"}, "context": {}},
                       "messages": [
                           {"origin": "assistant", "type": "tool_call", "id": "call_c1",
                            "text": CONSULT_CALL},
                           {"origin": "assistant", "type": "text", "text": text}]});
    let (out, _) = run_cell(SPLITTER, &[], input.clone());
    assert_eq!(out.len(), 1, "a tool round leaves as ONE message: {out:?}");
    (input, out.into_iter().next().expect("one message"))
}

/// The nothing form the splitter shows beside a call, fenced as the contract asks.
fn nothing_frame() -> String {
    let knob = assemble_cell::config_of(SPLITTER)["params"]["nothing_block"]
        .as_str()
        .unwrap_or_else(|| panic!("{SPLITTER} ships the knob `nothing_block`"))
        .to_string();
    format!("```sidecar\n{knob}\n```")
}

/// The frame is the contract's OWN nothing form, never a second text: the knob
/// stands as one line of the offer the shipped memory hive makes.
#[test]
fn the_frame_beside_a_call_is_the_contract_s_own_nothing_form() {
    let knob = assemble_cell::config_of(SPLITTER)["params"]["nothing_block"].clone();
    let knob = knob
        .as_str()
        .unwrap_or_else(|| panic!("{SPLITTER} ships the knob `nothing_block`: {knob}"));
    let (offer, _) = run_cell(MEMORY_SCHEMAS, &[], json!({"tools": ["*"], "messages": []}));
    let instruction = offer[0]["sidecar"][0]["instruction"]
        .as_str()
        .expect("the memory offer carries its instruction");
    assert!(
        instruction.lines().any(|l| l == knob),
        "the knob is a line of the memory contract, byte for byte: {knob}"
    );
}

/// Review I-1, the hand-over half: the sentence said beside a consult call is
/// handed the nothing form as its block, because the model wrote none. The
/// round stays byte-identical (GH #378) and the channel still gets no fence.
/// GH #889: the consult's return -- the window that shows the sentence with
/// its block -- is the curator's now.
#[test]
fn a_sentence_beside_a_consult_call_is_handed_the_nothing_form() {
    let frame = nothing_frame();
    let (input, half) = split_round(BESIDE_CALL);
    assert_eq!(
        half["messages"], input["messages"],
        "the round itself is never taken apart: {half}"
    );
    assert_eq!(
        half["sidecar_raw"], frame,
        "the sentence beside the call gets the nothing form: {half}"
    );
    let ans = dispatch_with(&half, &[("async_tools", json!(["consult_cogny"]))]);
    assert_eq!(ans["messages"][0]["text"], BESIDE_CALL, "{ans}");
    assert_eq!(
        ans["sidecar_raw"], frame,
        "the dispatcher hands it on with the sentence: {ans}"
    );
    let out = collect_answer(&ans, &[]);
    assert_files_nothing(&out);
    let channel = out
        .iter()
        .find(|m| m["header"]["route"] == "answer")
        .expect("the sentence leaves for the channel");
    assert!(
        !channel.to_string().contains("```") && channel.get("sidecar_raw").is_none(),
        "no channel ever receives the block: {channel}"
    );
}

/// A block the model DID write beside a call stays its own: the round is
/// untouched and no nothing form is laid over it.
#[test]
fn a_block_written_beside_a_call_gets_no_second_one() {
    let (input, half) = split_round(&format!("{BESIDE_CALL}\n\n{BLOCK}"));
    assert_eq!(half["messages"], input["messages"], "{half}");
    assert!(half.get("sidecar_raw").is_none(), "{half}");
}

/// Review I-2: an unfenced object in the section form is one attempt, read and
/// cut whole -- no `{"memory":` is left for the reader.
#[test]
fn an_unfenced_section_object_leaves_the_answer_whole() {
    let inner = "{\"facts\": [], \"topic\": {\"movement\": \"continue\", \"name\": \"tea\"}}";
    let (out, _) = run_cell(
        SPLITTER,
        &[],
        json!({"header": {"hop": {"finish_reason": "stop"}, "context": {}},
               "messages": [{"origin": "assistant", "type": "text",
                             "text": format!("{ANSWER}\n\n{{\"memory\": {inner}}}")}]}),
    );
    let half = out
        .iter()
        .find(|m| m["header"].get("route").is_none())
        .expect("an answer half");
    assert_eq!(half["messages"][0]["text"], ANSWER, "{out:?}");
    assert!(
        half["header"].get("sidecar").is_none(),
        "not malformed: {out:?}"
    );
    assert!(
        out.iter()
            .any(|m| m["header"]["route"] == "sidecar" && m["section"] == "memory"),
        "the section reaches its lane: {out:?}"
    );
    let channel = dispatch(half);
    assert!(
        !channel.to_string().contains("\"memory\""),
        "the reader sees no fragment: {channel}"
    );
}
