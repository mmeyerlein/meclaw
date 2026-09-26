//! GH #842 — a completion cut on `length` without a tool call still answers.
//!
//! Measured before the fix (`dispatcher@1.2.0`, script lines 160-162):
//! `if not calls: if finish == "stop": emit(answer); emit([])`. A completion
//! that stopped on `length` — reasoning ate `max_tokens`, for example — and
//! asked for no tool left NOTHING: no answer, no error, no stderr line, and the
//! partial text was dropped with it. From the outside that is a hang.
//!
//! The ruling (OR-SN-3): `length` without a call answers with the partial text
//! and keeps its reason on the hop, so whatever reads the answer can mark it as
//! cut; every other finish without a call stays terminal — `content_filter` and
//! `error` are carried by the brain's own error edge, and an echoed prompt must
//! never be filed as the agent's words — but no longer in silence.
//!
//! The shipped wiring keeps `length` away from the dispatcher (talky, cogny and
//! builder route it elsewhere, finding F1), so this is the cell's own defect,
//! pinned on the cell: the shipped script over stdin.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{DISPATCHER, run_cell};
use serde_json::{Value, json};

fn shipped() -> bool {
    assemble_cell::repo(DISPATCHER).is_file()
}

fn brain_doc(finish: &str, messages: Value) -> Value {
    json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0"},
                   "hop": {"finish_reason": finish}},
        "messages": messages
    })
}

fn partial() -> Value {
    json!([{"origin": "assistant", "type": "text", "text": "The three points are: first"}])
}

#[test]
fn a_length_finish_without_a_call_answers_with_its_reason() {
    if !shipped() {
        return;
    }
    let (out, _) = run_cell(DISPATCHER, &[], brain_doc("length", partial()));
    assert_eq!(out.len(), 1, "one answer, not silence: {out:?}");
    assert_eq!(out[0]["header"]["route"], "answer");
    assert_eq!(
        out[0]["header"]["finish_reason"], "length",
        "the reason travels with the partial text, so a reader can tell it was cut"
    );
    assert_eq!(
        out[0]["messages"],
        partial(),
        "the partial text is the answer, unchanged"
    );
}

#[test]
fn a_stop_finish_still_answers_unchanged() {
    if !shipped() {
        return;
    }
    let (out, stderr) = run_cell(DISPATCHER, &[], brain_doc("stop", partial()));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "answer");
    assert_eq!(out[0]["header"]["finish_reason"], "stop");
    assert!(
        stderr.is_empty(),
        "a normal answer says nothing: {stderr:?}"
    );
}

#[test]
fn every_other_finish_without_a_call_is_terminal_and_says_so() {
    if !shipped() {
        return;
    }
    for finish in ["tool_calls", "content_filter", "error", ""] {
        let (out, stderr) = run_cell(DISPATCHER, &[], brain_doc(finish, partial()));
        assert!(
            out.is_empty(),
            "`{finish}` without a call stays terminal (OR-SN-3): {out:?}"
        );
        assert!(
            stderr.contains(&format!(
                "dispatcher: finish '{finish}' without a tool call -- nothing dispatched"
            )),
            "`{finish}`: the stop must be said on stderr; got {stderr:?}"
        );
    }
}
