//! W5.7 -- the splitter cuts the sidecar out of an answer (GitHub #379).
//!
//! Per-turn extraction stopped being a tool call. The measured-reliable form is
//! variant 2: the model writes a fenced JSON block into its own answer text and
//! the substrate takes it back out again. This file pins the cell that takes it
//! out -- a `code` cell sitting between the brain and the dispatcher, on the
//! answer path only.
//!
//! **The block became GENERIC in GH #605** and this file moved with it. It is
//! one ```` ```sidecar ```` fence carrying ONE object, one top-level key per
//! SECTION, and the splitter emits one message per section on route `sidecar`
//! with `hop.section` naming it -- knowing no section by name, because the edges
//! downstream distribute on that hop and a cell that knew the names would need
//! changing for every new one. The legacy ```` ```memory ```` fence is still
//! read and becomes the section `memory`, which is what keeps a colony that has
//! not been rewired writing.
//!
//! The output forms, and nothing else:
//!   1. pass-through, byte-identical, when there is no block to cut (and when
//!      the round carries tool calls, which belong to the dispatcher whole);
//!   2. a multi-send when there IS one -- the answer with the fence taken out,
//!      plus one message per readable section on lane `sidecar`; a section that
//!      is not an object is dropped and named in `hop.sidecar_dropped`;
//!   3. pass-through with `header.sidecar == "malformed"` when a block is there
//!      but unreadable.
//!
//! **The third form was RETRACTED in GH #534** and its test moved to
//! `gh534_an_unreadable_block_still_leaves_the_answer.rs`. The pass-through half
//! of it stands -- one message, `sidecar: malformed`, nothing on the lane --
//! but the answer no longer keeps the fence: a model dropped one closing brace
//! in a running colony and the raw JSON reached a person's chat window. Found
//! decides the cut; readable decides the lane.
//!
//! Why a test that runs the SHIPPED script through `python3` rather than a
//! colony: the grammar is a 1:1 port of the harness parser that measured the
//! adoption number (`run_guide.py` `find_annotation`), and a port drifts
//! silently. Same subprocess pattern as
//! `gh299_the_contract_asks_for_both_parts.rs`.

use std::io::Write;
use std::process::{Command, Stdio};

const SPLITTER_CONFIG: &str = "../../templates/talky/splitter/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string -- the same substitution the colony performs on instantiation.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn splitter_config() -> serde_json::Value {
    let raw = std::fs::read_to_string(SPLITTER_CONFIG).unwrap_or_else(|e| {
        panic!(
            "the talky composite ships no splitter cell ({SPLITTER_CONFIG}): {e}. \
             Without it the sidecar travels inside the answer to the person -- \
             GitHub #379."
        )
    });
    serde_json::from_str(&raw).expect("splitter config json")
}

fn splitter_script() -> String {
    let v = splitter_config();
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("params.script_inline"),
    )
}

/// Hand the script to `python3` **on stdin** rather than in argv (GH #279).
fn run_script_on_stdin(script: &str, stdin_doc: &[u8]) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(&String::from_utf8_lossy(stdin_doc).to_string()).unwrap(),
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
    child.wait_with_output().expect("wait")
}

/// Run the shipped splitter over a flatly spelled message and return its stdout
/// as JSON -- an object for a pass-through, an array for a cut.
fn split(doc: serde_json::Value) -> serde_json::Value {
    let out = run_script_on_stdin(&splitter_script(), &meclaw_testing::code_stdin_bytes(&doc));
    assert!(
        out.status.success(),
        "splitter exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "splitter stdout is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// A brain completion the way the llm cell emits one: the assistant turn(s) in
/// `messages`, the finish reason on the hop.
fn completion(finish: &str, turns: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "hop": {"finish_reason": finish, "tokens_completion": 42},
            "context": {"session_id": "s-379", "turn_id": "s-379#1"}
        },
        "messages": turns
    })
}

fn text_turn(text: &str) -> serde_json::Value {
    serde_json::json!({"origin": "assistant", "type": "text", "text": text})
}

const GOOD_BLOCK: &str = "{\"facts\": [{\"subject\": \"alex\", \"predicate\": \"favorite_colour\", \
     \"object\": \"blue\"}], \"topic\": {\"title\": \"colours\", \"movement\": \"start\"}}";

const NOTHING_BLOCK: &str = "{\"nothing_new\": true, \"facts\": [], \"topic\": {\"title\": \"small talk\", \
     \"movement\": \"continue\"}}";

/// The half of a cut that carries a section, by name.
fn section<'a>(out: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    out.as_array()
        .unwrap_or_else(|| panic!("a cut is an array: {out}"))
        .iter()
        .find(|m| m["header"]["section"] == name)
        .unwrap_or_else(|| panic!("no `{name}` section left the splitter: {out}"))
}

#[test]
fn an_answer_without_a_sidecar_travels_on_untouched() {
    // (a) The pass-through IS the contract with every deployment that never
    // installs the block contract: without a block the cell is a wire.
    let input = completion("stop", serde_json::json!([text_turn("Blau, klar.")]));
    let out = split(input.clone());
    assert!(
        out.is_object(),
        "no block, no cut -- one message leaves: {out}"
    );
    assert_eq!(
        out["messages"], input["messages"],
        "and its turns are the ones that arrived, byte for byte: {out}"
    );
    assert_eq!(
        out["header"]["finish_reason"], "stop",
        "with the hop it arrived on: {out}"
    );
    assert!(
        out["header"].get("route").is_none(),
        "nothing is routed anywhere: {out}"
    );
    assert!(
        out["header"].get("sidecar").is_none(),
        "and nothing is flagged: {out}"
    );
}

#[test]
fn a_fenced_sidecar_leaves_as_its_own_message() {
    // (b) The whole point. Two messages: the answer WITHOUT the instrument, and
    // the instrument on its own lane, as a section.
    let answer = format!("Blue is your colour.\n\n```sidecar\n{{\"memory\": {GOOD_BLOCK}}}\n```");
    let out = split(completion("stop", serde_json::json!([text_turn(&answer)])));
    let arr = out
        .as_array()
        .unwrap_or_else(|| panic!("a cut is an array of two: {out}"));
    assert_eq!(arr.len(), 2, "exactly two: {out}");

    assert_eq!(arr[0]["header"]["finish_reason"], "stop");
    assert_eq!(
        arr[0]["messages"][0]["text"], "Blue is your colour.",
        "the person gets the prose and never the instrument: {out}"
    );
    assert!(
        arr[0]["header"].get("route").is_none(),
        "the answer half is not routed anywhere: {out}"
    );

    let carried = section(&out, "memory");
    assert_eq!(
        carried["header"]["route"], "sidecar",
        "the section rides the one lane: {out}"
    );
    assert_eq!(
        carried["section"], "memory",
        "and names itself in the body as well as on the hop, because the body \
         is what a receiver reads: {out}"
    );
    assert_eq!(
        carried["messages"],
        serde_json::json!([]),
        "a section is a payload and not a turn: {out}"
    );
    assert_eq!(
        carried["payload"]["facts"][0]["predicate"],
        "favorite_colour"
    );
    assert_eq!(carried["payload"]["topic"]["movement"], "start");
}

#[test]
fn the_legacy_memory_fence_is_read_as_the_memory_section() {
    // (b2) GH #605. The single-section fence this cell shipped first is still
    // the one every running colony writes, and it becomes the section `memory`
    // with the whole block as its payload. Without this the rewire would have to
    // land in the models and in the tree in the same second.
    let answer = format!("Blue is your colour.\n\n```memory\n{GOOD_BLOCK}\n```");
    let out = split(completion("stop", serde_json::json!([text_turn(&answer)])));
    assert_eq!(out.as_array().map(Vec::len), Some(2), "{out}");
    assert_eq!(out[0]["messages"][0]["text"], "Blue is your colour.");
    let carried = section(&out, "memory");
    assert_eq!(carried["header"]["route"], "sidecar", "{out}");
    assert_eq!(carried["payload"]["topic"]["movement"], "start", "{out}");
}

#[test]
fn one_block_three_sections_leave_as_three_messages() {
    // (b3) The generic cut (GH #605). ONE object, one top-level key per section,
    // one message per section -- and the splitter recognises none of them by
    // name: `hop.section` is what the edges downstream distribute on, so a
    // section this tree has never heard of travels without a line of code here.
    //
    // A key whose value is NOT an object is the one thing that cannot travel:
    // the body slot it would ride in is an object slot, and inventing a wrapper
    // would be this cell's guess rather than the model's statement. It is
    // dropped BY NAME, so a close pass can tell "the model wrote past the form"
    // from "the model wrote nothing".
    let block = format!(
        "{{\"memory\": {GOOD_BLOCK}, \
         \"display\": {{\"kind\": \"fact\", \"title\": \"Colour\", \"data\": \"blue\"}}, \
         \"notes\": \"a section that is not an object\"}}"
    );
    let answer = format!("Blue is your colour.\n\n```sidecar\n{block}\n```");
    let out = split(completion("stop", serde_json::json!([text_turn(&answer)])));
    let arr = out
        .as_array()
        .unwrap_or_else(|| panic!("a cut is an array: {out}"));
    assert_eq!(
        arr.len(),
        3,
        "the answer plus one message per READABLE section: {out}"
    );

    assert_eq!(
        arr[0]["messages"][0]["text"], "Blue is your colour.",
        "the prose is the prose, three sections or one: {out}"
    );
    assert_eq!(
        arr[0]["header"]["sidecar_dropped"], "notes",
        "and the section that could not travel is named rather than silent: {out}"
    );

    let names: Vec<&str> = arr[1..]
        .iter()
        .map(|m| m["header"]["section"].as_str().expect("section on the hop"))
        .collect();
    assert_eq!(
        names,
        ["memory", "display"],
        "in the order the model wrote them: {out}"
    );
    for m in &arr[1..] {
        assert_eq!(
            m["header"]["route"], "sidecar",
            "one lane for all of them: {out}"
        );
        assert_eq!(m["messages"], serde_json::json!([]), "{out}");
        assert_eq!(m["section"], m["header"]["section"], "{out}");
    }
    assert_eq!(
        section(&out, "memory")["payload"]["topic"]["movement"],
        "start",
        "{out}"
    );
    assert_eq!(
        section(&out, "display")["payload"]["kind"],
        "fact",
        "a section the splitter has no idea about travels the same way: {out}"
    );
}

#[test]
fn the_nothing_form_travels_too() {
    // (c) An explicit nothing is a VERDICT, not an absence: it is what books the
    // turn as annotated-and-empty in the queue. Swallowing it here would leave
    // the close pass re-reading turns the model already answered for.
    let answer = format!("Understood.\n\n```sidecar\n{{\"memory\": {NOTHING_BLOCK}}}\n```");
    let out = split(completion("stop", serde_json::json!([text_turn(&answer)])));
    let arr = out.as_array().expect("a nothing is still a cut");
    assert_eq!(arr.len(), 2, "{out}");
    assert_eq!(
        section(&out, "memory")["payload"]["nothing_new"],
        true,
        "the verdict reaches the lane: {out}"
    );
    assert_eq!(arr[0]["messages"][0]["text"], "Understood.");
}

#[test]
fn an_unreadable_block_is_flagged_and_leaves_the_answer_all_the_same() {
    // (d) RETRACTED and rewritten (GH #534). This test used to assert
    // `out["messages"][0]["text"] == answer` -- the fence stayed in, on the
    // reasoning that half-cutting a block nobody can read would corrupt the
    // answer for the sake of a write that cannot happen anyway. It was measured
    // wrong: there is no half cut, the span is the one the parser already found,
    // and a model one closing brace short put raw JSON in front of a reader.
    //
    // What survives from the old decision is the half that was right: nothing
    // unreadable is repaired and nothing unreadable travels. One message, the
    // flag on the hop, no section. The forms of the cut live in
    // `gh534_an_unreadable_block_still_leaves_the_answer.rs`.
    let answer = "Bitte sehr.\n\n```sidecar\n{\"memory\": {\"facts\": [oops\n```";
    let out = split(completion("stop", serde_json::json!([text_turn(answer)])));
    assert!(out.is_object(), "nothing readable to route: {out}");
    assert_eq!(
        out["header"]["sidecar"], "malformed",
        "but the miss is on the record: {out}"
    );
    assert_eq!(
        out["messages"][0]["text"], "Bitte sehr.",
        "and the reader never sees a fence this cell found: {out}"
    );
}

#[test]
fn a_block_whose_top_level_is_not_an_object_is_a_miss_and_not_a_section() {
    // (d2) GH #605. The block contract is ONE OBJECT whose keys are sections.
    // A list parses as JSON and carries no section names, so there is nothing to
    // name a lane with -- it is the same miss as a broken payload, and it earns
    // the same treatment rather than a guessed-at wrapper.
    let out = split(completion(
        "stop",
        serde_json::json!([text_turn("Here.\n\n```sidecar\n[1, 2]\n```")]),
    ));
    assert!(out.is_object(), "nothing to route: {out}");
    assert_eq!(out["header"]["sidecar"], "malformed", "{out}");
    assert_eq!(out["messages"][0]["text"], "Here.", "{out}");
}

#[test]
fn a_round_with_tool_calls_belongs_to_the_dispatcher_whole() {
    // (e) The mixed form -- text beside an async call in ONE message -- is the
    // shape that strands a round (GH #378). The splitter never builds it and
    // never takes one apart: a completion carrying calls passes untouched.
    let input = completion(
        "tool_calls",
        serde_json::json!([
            {"origin": "assistant", "type": "tool_call", "id": "c1",
             "text": "{\"name\":\"weather\",\"arguments\":\"{}\"}"},
            text_turn(&format!("Moment.\n\n```sidecar\n{{\"memory\": {GOOD_BLOCK}}}\n```"))
        ]),
    );
    let out = split(input.clone());
    assert!(out.is_object(), "no cut on a tool round: {out}");
    assert_eq!(
        out["messages"], input["messages"],
        "byte-identical, fence included: {out}"
    );
    assert!(out["header"].get("route").is_none(), "{out}");
    assert!(out["header"].get("sidecar").is_none(), "{out}");
}

#[test]
fn the_fence_tolerance_is_the_harness_tolerance() {
    // (f) A ```json fence carrying the legacy payload is an attempt that missed
    // the label, and the harness grades it as one -- so the lane cuts it too,
    // as the `memory` section. A bare fence with no marker in it is a code block
    // in an answer and stays put.
    let labelled = format!("Da.\n\n```json\n{GOOD_BLOCK}\n```");
    let out = split(completion(
        "stop",
        serde_json::json!([text_turn(&labelled)]),
    ));
    assert!(
        out.is_array(),
        "a payload-shaped ```json block is a sidecar that missed its label: {out}"
    );
    assert_eq!(
        section(&out, "memory")["payload"]["topic"]["movement"],
        "start"
    );

    let code = "Here is your snippet:\n\n```\nprint(1)\n```";
    let out = split(completion("stop", serde_json::json!([text_turn(code)])));
    assert!(
        out.is_object(),
        "an ordinary code block is not an annotation: {out}"
    );
    assert_eq!(out["messages"][0]["text"], code, "{out}");
}

#[test]
fn a_naked_trailing_object_is_an_attempt_too() {
    // (g) Same reasoning one step further: the model produced the payload and
    // dropped the wrapper entirely. Cutting it is what keeps the write and keeps
    // the JSON out of the person's answer.
    let answer = format!("Notiert.\n\n{GOOD_BLOCK}");
    let out = split(completion("stop", serde_json::json!([text_turn(&answer)])));
    let arr = out
        .as_array()
        .unwrap_or_else(|| panic!("a naked object is cut too: {out}"));
    assert_eq!(arr.len(), 2, "{out}");
    assert_eq!(arr[0]["messages"][0]["text"], "Notiert.", "{out}");
    assert_eq!(arr[1]["header"]["route"], "sidecar", "{out}");
    assert_eq!(arr[1]["header"]["section"], "memory", "{out}");
}

#[test]
fn the_declared_lane_is_the_one_the_script_writes() {
    // The contract is validated always-on for `code`: an undeclared value is a
    // `contract_violation` at runtime, which is a boot-time mistake found in
    // production. So the declaration is read here beside the behaviour.
    let cfg = splitter_config();
    assert_eq!(cfg["cell"]["type"], "code");
    assert_eq!(
        cfg["contract"]["multi_send_capable"], true,
        "the cut is a multi-send; without the declaration it is \
         `multi_send_not_declared`: {cfg}"
    );
    let hop = &cfg["contract"]["emits"]["hop"];
    assert_eq!(hop["route"]["values"], serde_json::json!(["sidecar"]));
    assert_eq!(
        hop["route"]["required"], false,
        "a pass-through routes nothing"
    );
    assert_eq!(
        hop["section"]["type"], "string",
        "the section name is DECLARED and OPEN: an enum here would make every \
         new section a change to this cell, which is the one thing GH #605 \
         removed: {cfg}"
    );
    assert!(
        hop["section"].get("values").is_none(),
        "and it carries no enum: {cfg}"
    );
    assert_eq!(
        hop["finish_reason"]["values"],
        serde_json::json!(["stop", "tool_calls"]),
        "the two the answer path carries"
    );
    assert_eq!(hop["sidecar"]["values"], serde_json::json!(["malformed"]));
    assert_eq!(
        hop["sidecar_dropped"]["type"], "string",
        "the sections that could not travel, by name: {cfg}"
    );
    let body = &cfg["contract"]["emits"]["body"];
    assert_eq!(body["messages"]["required"], true);
    assert_eq!(
        body["payload"]["type"], "object",
        "a section is an object, and the slot that carries it says so: {cfg}"
    );
    assert_eq!(body["section"]["type"], "string", "{cfg}");
    assert!(
        cfg["description"]["purpose"]
            .as_str()
            .expect("purpose")
            .contains("pass-through"),
        "the cell says what it is without a block contract: {cfg}"
    );
}
