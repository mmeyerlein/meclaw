//! #1279 -- a failed tool result keeps its code, off the wire.
//!
//! WHY this file exists: a tool that failed says so in the header of its
//! result (`error_code`), its text is often plain prose. The code cannot ride
//! on the turn: `$defs/TurnObject` in `crates/meclaw-core/schemas/ubf-body.json`
//! is closed, and an emission carrying it dead-letters as `invalid_ubf_body`
//! (`gh893 a_call_without_a_round_is_refused_with_missing_audience`); the text
//! cannot carry it either (T5-08, `gh529`: an error is parked as it came). So
//! the code stays on the PARKED row only, the wire's turn is the turn as it
//! came, and the curate of the same iteration names it in the body slot
//! `tool_errors` (`{tool_call_id: error_code}`), which the curator's policy
//! reads -- a failure in prose alone is still a failure there.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "wk3-channel#0001";
const PROSE: &str = "malformed tool_call: `web` names no `op`";

/// The parked `tool` row of the in_tool bundle, as the element it holds.
fn parked_tool(err: &str) -> Value {
    let out = assemble(
        &[],
        lane(
            "in_tool",
            json!({"error_code": err}),
            json!({"turn_id": TURN, "iter": "0"}),
            json!([{"origin": "tool", "type": "tool_result", "id": "call-1",
                    "text": PROSE}]),
        ),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    let row = calls_of(&out[0])
        .into_iter()
        .map(|(_, a)| a)
        .find(|a| a["table"] == "round" && a["row"]["role"] == "tool")
        .unwrap_or_else(|| panic!("no parked tool row: {out:?}"));
    serde_json::from_str(row["row"]["turn"].as_str().expect("turn text")).expect("turn json")
}

/// The curate of a round whose tool rows are `tool_rows` (iteration 0).
fn curate_of(tool_rows: &[Value]) -> Value {
    let window = json!({"turns": [{"role": "user", "turn_id": TURN, "text": "weather?"}]});
    let call = json!([{"origin": "assistant", "type": "tool_call", "id": "call-1",
                       "text": "{\"name\":\"web_search\",\"arguments\":\"{}\"}"}]);
    let mut rows = vec![
        json!({"turn_id": TURN, "iter": 0, "role": "leg-window",
               "turn": window.to_string(), "fired": 1,
               "recorded_at": "2026-09-25T10:00:00.000000Z"}),
        json!({"turn_id": TURN, "iter": 0, "role": "assistant", "turn": call.to_string(),
               "fired": 0, "recorded_at": "2026-09-25T10:00:00.000000Z"}),
    ];
    for t in tool_rows {
        rows.push(
            json!({"turn_id": TURN, "iter": 0, "role": "tool", "turn": t.to_string(),
                         "fired": 0, "recorded_at": "2026-09-25T10:00:01.000000Z"}),
        );
    }
    let back = assemble(
        &[],
        bundle_reply(
            "round-check",
            TURN,
            &[("c-round-check-read", Value::Array(rows))],
        ),
    );
    on_route(&back, "curate").clone()
}

#[test]
fn a_failed_result_keeps_its_code_on_the_parked_row_only() {
    let parked = parked_tool("timeout");
    assert_eq!(parked["error_code"], json!("timeout"), "{parked}");
    assert_eq!(parked["text"], json!(PROSE), "parked as it came: {parked}");
    let healthy = parked_tool("");
    assert!(healthy.get("error_code").is_none(), "{healthy}");

    // The curate of the iteration: the turn on the wire is a closed UBF turn,
    // the code travels in the slot.
    let c = curate_of(&[parked]);
    let wire = c["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["type"] == "tool_result" && m["id"] == "call-1")
        .cloned()
        .unwrap_or_else(|| panic!("no result on the wire: {c}"));
    assert!(wire.get("error_code").is_none(), "{wire}");
    assert_eq!(wire["text"], json!(PROSE));
    assert_eq!(c["tool_errors"], json!({"call-1": "timeout"}), "{c}");
}

#[test]
fn a_round_without_a_failed_result_carries_no_slot() {
    let c = curate_of(&[parked_tool("")]);
    assert!(c.get("tool_errors").is_none(), "{c}");
}

/// A call that never reached a tool is no failed tool (GH #1260, #1341): the
/// dispatcher answers a call without an op with `malformed_tool_call`, the
/// tools hive answers a name no answerer carries with `unknown_tool`, and the
/// collector hangs the family's ops or the op's schema on that answer so the
/// model can mend its own call. Such a result stays a result of the round;
/// only a call that ran and failed names the slot.
#[test]
fn a_call_that_never_ran_is_no_failed_tool() {
    for code in ["malformed_tool_call", "unknown_tool"] {
        let c = curate_of(&[parked_tool(code)]);
        assert!(c.get("tool_errors").is_none(), "{code}: {c}");
    }
    let c = curate_of(&[parked_tool("unknown_tool"), parked_tool("timeout")]);
    assert_eq!(c["tool_errors"], json!({"call-1": "timeout"}), "{c}");
}
