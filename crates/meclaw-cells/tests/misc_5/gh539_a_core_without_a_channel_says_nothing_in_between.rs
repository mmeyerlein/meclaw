//! GH #539 -- a brain without a channel must not put a half-answer on its
//! answer lane.
//!
//! A thinking core runs its own tool round, and a tool-using model does what
//! such a model does: it puts a sentence next to the bundle -- "I am checking
//! the official fares now" -- and the dispatcher, correctly, marks it
//! `hop.interim = "1"` and sends it "on its way to the channel while the calls
//! keep running" (GH #378).
//!
//! **A `cogny` has no channel.** It is "structurally a talky without a channel"
//! (`templates/cogny/template.json`), and its answer lane IS the advice lane of
//! the voice that asked: the assistant graph wires `./cogny -> ./talky` on
//! `hop.route == 'answer'` into that surface's `in_advice`. So every interim
//! sentence of the core arrived as an advisor's ANSWER, was recorded as one,
//! re-entered the surface's brain, was verbalised to the user -- and sometimes
//! answered with a fresh consultation, which the core answered with its next
//! interim sentence. Measured on a live colony, one user turn: 11 of the 26
//! answers that core put on the lane carried `interim = 1`, all 11 arrived at
//! the surface as advice, and the exchange produced thirteen messages.
//!
//! The seam is the dispatcher, because it is the only cell that ever sees the
//! whole bundle and therefore the only one that can classify a sentence
//! standing beside it -- the same ground on which the async class (GH #28) and
//! the handoff class (GH #372) are declared there. `params.interim` is the
//! third such declaration; it says whether there is a channel at all.
//!
//! Not an edge, on purpose: a hive out-edge that simply does not match
//! dead-letters the message (`hive_no_route`) and logs a warning per round. The
//! sentence must not be EMITTED.
//!
//! Four claims about the cell and one about the wiring:
//!
//! 1. knob off, a sentence beside a bundle that is waited for -> NOTHING on the
//!    answer lane; the calls travel exactly as before.
//! 2. knob off, the FINAL answer -> exactly one emission on the answer lane. The
//!    core still answers; it just answers once.
//! 3. knob off, a sentence beside an async-non-handoff bundle -> it STILL
//!    leaves, unmarked. Nothing is waited for, so that sentence IS the answer of
//!    the turn (GH #378), and a fix that swallowed it would rebuild #378.
//! 4. the knob absent -> the shipped behaviour of every wiring that exists.
//! 5. `cogny` ships it off and `talky` does not -- the claim is worth nothing if
//!    the composite that has no channel does not make it.
//!
//! No provider is called: the only thing that runs here is the shipped
//! `params.script_inline`.

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{self, Value, json};

const DISPATCHER: &str = "../../templates/dispatcher/config.json";

fn script() -> String {
    let raw = std::fs::read_to_string(DISPATCHER).expect("dispatcher config");
    let v: Value = serde_json::from_str(&raw).expect("config json");
    v["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// The shipped script over a real stdin document. The program travels on stdin
/// (GH #279): a single argv string is capped at 128 KiB and the shipped scripts
/// are within a few KB of that line.
fn emit(doc: Value) -> Vec<Value> {
    let stdin_doc = meclaw_testing::code_stdin(&doc).to_string();
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(&script()).unwrap(),
        serde_json::to_string(&stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "dispatcher exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn call_turn(id: &str, name: &str) -> Value {
    json!({"origin": "assistant", "type": "tool_call", "id": id,
           "text": json!({"name": name, "arguments": "{}"}).to_string()})
}

fn said() -> Value {
    json!({"origin": "assistant", "type": "text",
           "text": "I am checking the official fares now."})
}

/// A brain answer as the edge from the llm delivers it, plus the cell's own
/// `params` -- the surface the knob lives on (GH #138), so what the test
/// configures is what a `ref` marker configures.
fn brain_doc(finish: &str, params: Value, messages: Vec<Value>) -> Value {
    json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0"},
                   "hop": {"finish_reason": finish}},
        "params": params,
        "messages": messages
    })
}

fn route_of(msg: &Value) -> &str {
    msg["header"]["route"].as_str().unwrap_or_default()
}

const OFF: fn() -> Value = || json!({"interim": ""});

#[test]
fn a_core_without_a_channel_emits_no_interim_answer() {
    let out = emit(brain_doc(
        "tool_calls",
        OFF(),
        vec![call_turn("c1", "web_search"), said()],
    ));

    assert!(
        out.iter().all(|m| route_of(m) != "answer"),
        "the sentence has nowhere to go and must not leave: {out:?}"
    );
    // The round is untouched: the expectation set still goes first, the call
    // still runs. A fix that bought silence by dropping the bundle would be
    // worse than the bug.
    assert_eq!(out.len(), 2, "calls and the call itself: {out:?}");
    assert_eq!(route_of(&out[0]), "calls", "PLAIN order is unchanged");
    assert_eq!(route_of(&out[1]), "tool");
}

#[test]
fn the_one_answer_the_core_owes_still_leaves() {
    let answer = json!({"origin": "assistant", "type": "text",
                        "text": "Three days in Athens, with the fares."});
    let out = emit(brain_doc("stop", OFF(), vec![answer.clone()]));

    assert_eq!(out.len(), 1, "exactly one advice per consultation: {out:?}");
    assert_eq!(route_of(&out[0]), "answer");
    assert!(
        out[0]["header"].get("interim").is_none(),
        "a final answer never claims to be an interim one: {out:?}"
    );
    assert_eq!(out[0]["messages"].as_array().unwrap(), &vec![answer]);
}

#[test]
fn a_final_sentence_beside_an_async_bundle_leaves_even_with_the_knob_off() {
    // GH #378 read from the other side: when every call is async and none is a
    // handoff, nothing is waited for and the sentence IS this turn's answer.
    // The knob withholds the PROMISE of a later answer, never an answer.
    // The async class rides on the same `params` object as the knob under test
    // -- it was a substitution token this case patched into the script until
    // `dispatcher@1.2.0` (GH #138), and a patch like that is exactly the harness
    // that keeps working after the surface it imitated is gone.
    let doc = brain_doc(
        "tool_calls",
        json!({"interim": "", "async_tools": ["remember"]}),
        vec![call_turn("c1", "remember"), said()],
    );
    let out = emit(doc);

    let answers: Vec<&Value> = out.iter().filter(|m| route_of(m) == "answer").collect();
    assert_eq!(answers.len(), 1, "the sentence still leaves: {out:?}");
    assert!(
        answers[0]["header"].get("interim").is_none(),
        "and it leaves as the FINAL answer it is: {out:?}"
    );
    assert!(
        out.iter().any(|m| route_of(m) == "tool"),
        "the fire-and-forget call still travels: {out:?}"
    );
}

#[test]
fn the_knob_is_on_by_default_so_a_voice_keeps_its_interim() {
    // Every wiring that existed before this issue passes NO params at all.
    let out = emit(brain_doc(
        "tool_calls",
        json!({}),
        vec![call_turn("c1", "web_search"), said()],
    ));

    assert_eq!(out.len(), 3, "calls, the interim answer, the call: {out:?}");
    assert_eq!(route_of(&out[1]), "answer");
    assert_eq!(
        out[1]["header"]["interim"].as_str(),
        Some("1"),
        "an interim answer still says so: {out:?}"
    );
}

#[test]
fn the_composite_without_a_channel_is_the_one_that_turns_it_off() {
    let read = |rel: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(format!("../../{rel}")).unwrap()).unwrap()
    };
    let cogny = read("templates/cogny/dispatcher/config.json");
    assert_eq!(
        cogny["override_params"][""]["interim"].as_str(),
        Some(""),
        "the core has no channel and its ref marker has to say so: {cogny}"
    );
    let talky = read("templates/talky/dispatcher/config.json");
    assert!(
        talky.get("override_params").is_none() || talky["override_params"][""]["interim"].is_null(),
        "the conversation surface HAS a channel and keeps the default: {talky}"
    );
}
