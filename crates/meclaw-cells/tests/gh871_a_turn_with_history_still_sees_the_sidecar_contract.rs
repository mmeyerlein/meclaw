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
//! What this file pins, measured at the receiver:
//!
//! 1. the system message the PROVIDER receives carries the contract;
//! 3. every earlier `assistant` message the provider receives carries its block
//!    -- the answer that came through splitter, dispatcher and collector, and a
//!    second one straight from the store; a row written before the column
//!    existed renders as it always did;
//! 4. the answer leaving the collector for the channel carries no fence and no
//!    `sidecar_raw`.
//!
//! Plus the cap: a block over `sidecar_max_chars` is not kept, never cut.
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
#[path = "mock_openai.rs"]
mod mock_openai;

use assemble_cell::{ASSEMBLE, DISPATCHER, SESSION, bundle_reply, calls_of, lane, run_cell};
use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const SPLITTER: &str = "templates/talky/splitter/config.json";
const MEMORY_SCHEMAS: &str = "templates/memory-hive/schemas/config.json";
const TALKY_BRAIN: &str = "templates/talky/brain/config.json";

const ANSWER: &str = "Tea it is.";
const BLOCK: &str = "```sidecar\n{\"memory\": {\"nothing_new\": false, \"facts\": [{\"subject\": \
     \"alex\", \"predicate\": \"likes\", \"object\": \"tea\"}]}}\n```";
const ANSWER_2: &str = "Noted, no change.";
const BLOCK_2: &str = "```sidecar\n{\"memory\":{\"nothing_new\":true,\"facts\":[],\
     \"topic\":{\"movement\":\"continue\"}}}\n```";
const OLD_ANSWER: &str = "An answer from before the column.";
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

fn stored_answer_row(out: &[Value]) -> Value {
    out.iter()
        .filter(|m| m["header"]["route"] == "cstore")
        .flat_map(calls_of)
        .map(|(_, a)| a)
        .find(|a| a["operation"] == "insert" && a["table"] == "turns")
        .map(|a| a["row"].clone())
        .unwrap_or_else(|| panic!("the answer was not stored: {out:?}"))
}

// ──────────────────────────────────────────────── the window, fan-in, brain

fn row(id: &str, turn: &str, role: &str, content: &str) -> Value {
    json!({"id": id, "session_id": SESSION, "turn_id": turn, "role": role,
           "content": content, "deferred": 0, "consult_id": "", "speaker": "",
           "speaker_ref": ""})
}

/// The next turn's assembly out of the store rows given, newest first as the
/// window read returns them: turn-open parks the window leg, collect fires.
fn assemble_turn(win: Value) -> Value {
    let a = run_cell(
        ASSEMBLE,
        &[],
        bundle_reply(
            "turn-open",
            "t4",
            &[
                ("c-open-turn", Value::Null),
                ("c-open-round", json!([])),
                ("c-open-win", win),
                ("c-open-scope", json!([])),
                ("c-open-roster", json!([])),
            ],
        ),
    )
    .0;
    let mut leg = a
        .iter()
        .filter(|m| m["header"]["route"] == "cstore")
        .flat_map(calls_of)
        .map(|(_, op)| op)
        .find(|op| op["operation"] == "insert" && op["row"]["role"] == "leg-window")
        .map(|op| op["row"].clone())
        .unwrap_or_else(|| panic!("turn-open parked no window leg: {a:?}"));
    leg["fired"] = json!(0);
    run_cell(
        ASSEMBLE,
        &[],
        bundle_reply("collect", "t4", &[("c-collect-read", json!([leg]))]),
    )
    .0
    .into_iter()
    .find(|m| m["header"]["route"] == "brain")
    .expect("a complete round assembles on route `brain`")
}

/// The `memory` offer of the SHIPPED memory hive, merged by the SHIPPED
/// collector into the `menu` message that carries the contract (GH #606).
fn contract_menu() -> Value {
    let (offer, _) = run_cell(MEMORY_SCHEMAS, &[], json!({"tools": ["*"], "messages": []}));
    let offers = offer[0]["sidecar"].clone();
    let recorded = run_cell(
        ASSEMBLE,
        &[("sidecar", json!("1"))],
        json!({"target": "/main/collector",
               "header": {"hop": {"route": "in_menu"}, "context": {}},
               "messages": [], "unknown": [], "sidecar": offers,
               // One declaration, or the lane writes no menu at all (gh525).
               "schemas": [{"name": "web_search", "description": "search",
                            "parameters": {"type": "object", "properties": {}}}]}),
    )
    .0;
    let op: Value = serde_json::from_str(
        recorded[0]["messages"]
            .as_array()
            .expect("a store bundle")
            .iter()
            .find(|m| m["id"] == "c-menu-put")
            .expect("the answer is recorded")["text"]
            .as_str()
            .expect("op text"),
    )
    .expect("op json");
    run_cell(
        ASSEMBLE,
        &[("sidecar", json!("1"))],
        json!({"target": "/main/collector",
               "header": {"hop": {"route": "cstore", "operation": "bundle"},
                          "context": {"col_phase": "menu-merge"}},
               "messages": [{"id": "c-menu-all", "type": "tool_result",
                             "text": json!([op["row"].clone()]).to_string()}],
               "results": [{"tool_call_id": "c-menu-all", "operation": "select"}]}),
    )
    .0
    .into_iter()
    .next()
    .expect("the menu lane writes one message")
}

fn brain(td: &tempfile::TempDir, base_url: &str) -> (LlmCell, DbConn) {
    let order = assemble_cell::config_of(TALKY_BRAIN)["params"]["system_order"].clone();
    let params = LlmParams::parse(&json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{base_url}/v1"), "system_order": order,
    }))
    .expect("params must parse");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (
        LlmCell::new(params, reqwest::Client::builder().build().unwrap()),
        DbConn::wrap(conn, None),
    )
}

async fn deliver(cell: &mut LlmCell, db: &mut DbConn, body: Value) {
    let (tx, mut rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/brain"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new("/brain"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    while rx.recv().await.is_some() {}
}

// ═══════════════════════════════════════════════════════════════ the locks

/// Claim 4, and the hand-over it rests on: the block travels BESIDE the answer
/// through splitter and dispatcher, the collector keeps it beside the answer,
/// and what leaves for the channel carries neither the fence nor the slot.
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
    let stored = stored_answer_row(&out);
    assert_eq!(
        stored["content"], ANSWER,
        "content stays the bare answer: {stored}"
    );
    assert_eq!(
        stored["sidecar"], BLOCK,
        "the block is kept beside it: {stored}"
    );

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

/// The cap: a block over `sidecar_max_chars` is not kept -- never cut to fit.
#[test]
fn a_block_over_the_cap_is_not_kept() {
    let ans = json!({"messages": [{"origin": "assistant", "type": "text", "text": ANSWER}],
                     "sidecar_raw": BLOCK});
    let stored = stored_answer_row(&collect_answer(&ans, &[("sidecar_max_chars", json!(20))]));
    assert_eq!(stored["sidecar"], "", "{stored}");
    let mut oversize = row("0002", "t1", "assistant", ANSWER);
    oversize["sidecar"] = json!("x".repeat(7000));
    let brain_msg = assemble_turn(json!([
        row("0003", "t4", "user", "and now?"),
        oversize,
        row("0001", "t1", "user", "I like tea"),
    ]));
    let shown: Vec<&Value> = brain_msg["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["origin"] == "assistant" && m["type"] == "text")
        .collect();
    assert_eq!(
        shown[0]["text"], ANSWER,
        "an oversize stored block is not shown"
    );
}

/// Claims 1 and 3, at the provider: a third turn with two earlier answers and
/// an open consult. The contract is in the system message, and every earlier
/// answer the provider receives carries its own block.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_provider_sees_every_earlier_answer_with_its_block() {
    // Turn 1's answer, through the real three hops into a stored row.
    let stored = stored_answer_row(&collect_answer(
        &dispatch(&split(&format!("{ANSWER}\n\n{BLOCK}"))),
        &[],
    ));
    let mut first = row("0002", "t1", "assistant", ANSWER);
    first["sidecar"] = stored["sidecar"].clone();
    let mut second = row("0004", "t2", "assistant", ANSWER_2);
    second["sidecar"] = json!(BLOCK_2);
    let mut advice = row("0005", "t3", "advice", "the core says 21C");
    advice["consult_id"] = json!("call_c1");
    // Newest first, as the window read returns them.
    let win = json!([
        row("0006", "t4", "user", "and now?"),
        advice,
        second,
        row("0003", "t2", "user", "anything new?"),
        first,
        row("0001", "t1", "user", "I like tea"),
        row("0000", "t0", "assistant", OLD_ANSWER),
    ]);
    let turn = assemble_turn(win);
    assert_eq!(
        turn["system"]["consult"]["open"],
        json!(["call_c1"]),
        "the fixture has an open consult: {turn}"
    );

    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    let (mut cell, mut db) = brain(&td, &mock.base_url);
    let menu = contract_menu();
    deliver(
        &mut cell,
        &mut db,
        json!({"system": menu["system"].clone()}),
    )
    .await;
    deliver(&mut cell, &mut db, turn).await;

    let reqs = mock.recorded_requests().await;
    let req = reqs.first().expect("the brain called the provider");
    let msgs = req.messages().expect("messages[]");

    // 1. The contract is in the system message the provider received.
    let system: String = msgs
        .iter()
        .filter(|m| m["role"] == "system")
        .filter_map(|m| m["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    assert!(
        system.contains("ANNOTATE EVERY TURN") && system.contains("```sidecar"),
        "the provider must receive the contract: {system}"
    );

    // 3. Every earlier answer carries its block; the old row renders as before.
    let answers: Vec<&str> = msgs
        .iter()
        .filter(|m| m["role"] == "assistant")
        .filter_map(|m| m["content"].as_str())
        .collect();
    assert_eq!(
        answers,
        vec![
            OLD_ANSWER,
            format!("{ANSWER}\n\n{BLOCK}").as_str(),
            format!("{ANSWER_2}\n\n{BLOCK_2}").as_str(),
        ],
        "each earlier answer reaches the model as it was written, block included; \
         a row from before the column is unchanged"
    );
}

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

/// Review I-1, at the provider: the sentence said beside a consult call is an
/// earlier answer on the consult's return, and it carries a block like every
/// other one -- the nothing form, because the model wrote none. The round
/// stays byte-identical (GH #378) and the channel still gets no fence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sentence_beside_a_consult_call_carries_a_block_on_the_return() {
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
    let stored = stored_answer_row(&out);
    assert_eq!(stored["content"], BESIDE_CALL, "{stored}");
    assert_eq!(stored["sidecar"], frame, "{stored}");
    let channel = out
        .iter()
        .find(|m| m["header"]["route"] == "answer")
        .expect("the sentence leaves for the channel");
    assert!(
        !channel.to_string().contains("```") && channel.get("sidecar_raw").is_none(),
        "no channel ever receives the block: {channel}"
    );

    // The consult comes back: the window holds the question, the sentence
    // beside the call and the advice.
    let mut said = row("0002", "t1", "assistant", BESIDE_CALL);
    said["sidecar"] = stored["sidecar"].clone();
    let mut advice = row("0003", "t2", "advice", "the core says 21C");
    advice["consult_id"] = json!("call_c1");
    let turn = assemble_turn(json!([
        advice,
        said,
        row("0001", "t1", "user", "why is it so warm?"),
    ]));
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = tempfile::TempDir::new().unwrap();
    let (mut cell, mut db) = brain(&td, &mock.base_url);
    deliver(&mut cell, &mut db, turn).await;
    let reqs = mock.recorded_requests().await;
    let req = reqs.first().expect("the brain called the provider");
    let answers: Vec<&str> = req
        .messages()
        .expect("messages[]")
        .iter()
        .filter(|m| m["role"] == "assistant")
        .filter_map(|m| m["content"].as_str())
        .collect();
    assert_eq!(
        answers,
        vec![format!("{BESIDE_CALL}\n\n{frame}").as_str()],
        "the provider sees the sentence with a block"
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
