//! GH #1378 -- a turn without the person's words opens no round.
//!
//! Measured (live-2, battery R2 over the phone, 21 of 21 calls with
//! `self_answers >= 1`): the duplex `voice` cell makes the model's greeting a
//! turn of its own with an EMPTY `user` half (`voice/live_turns.rs`, "the model
//! speaking with nothing open is the greeting"), so the memory keeps it. The
//! collector read that arrival like any other turn: `text_of(user)` was empty,
//! so `any_text` took the agent's own sentence as the person's question, the
//! round went `talk`, the brain answered it, the answer went back into the call
//! (`in_speak`), the model spoke it -- and that was again a turn without the
//! caller. The loop pushed the caller's real answer out of the call.
//!
//! The rule is read off the STRUCTURE only, never off the words: an `in_turn`
//! with no text of the person (empty or missing `user`), no event, no peer turn
//! and no consult on its hop, carrying the agent's own text, goes to the
//! curator as the agent's line -- since GH #1567 at once, a curate marked
//! `line_only` that the curator files without a model call (held for the
//! next round, it never rode when the caller hung up first) -- and opens NO
//! round: no turn-open, no recall that would read the agent's sentence as
//! `origin user`.
//!
//! The counter case pins the old road: a turn with the person's words opens its
//! round as before.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const GREETING: &str = "Hello, this is probe, your companion. How are you today?";
const GREET_TURN: &str = "call-1#0";
const NEXT_TURN: &str = "call-1#1";

fn voice_turn(turn: &str, messages: Value) -> Value {
    lane(
        "in_turn",
        json!({"turn_id": turn}),
        json!({"turn_id": turn}),
        messages,
    )
}

/// Every `turns` insert of every `cstore` emission, as `(tool_call_id, row)`.
fn turn_inserts(out: &[Value]) -> Vec<(String, Value)> {
    out.iter()
        .filter(|m| m["header"]["route"].as_str() == Some("cstore"))
        .flat_map(calls_of)
        .filter(|(_, a)| a["operation"] == "insert" && a["table"] == "turns")
        .map(|(id, a)| (id, a["row"].clone()))
        .collect()
}

fn opens_a_round(out: &[Value]) -> bool {
    out.iter().any(|m| {
        let h = &m["header"];
        h["phase"].as_str() == Some("turn-open")
            || matches!(h["route"].as_str(), Some("recall" | "answer"))
    })
}

/// The `curate` emissions of `out`.
fn curates(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| m["header"]["route"].as_str() == Some("curate"))
        .collect()
}

/// GH #1567: the line goes to the curator AT ONCE, as a curate of its own turn
/// marked `line_only` -- held for the next round of the session it never rode
/// when the caller hung up first. No turn-open, no recall, no held row.
#[test]
fn a_greeting_turn_goes_to_the_curator_as_the_agents_line_and_opens_no_round() {
    for messages in [
        // the duplex voice cell's form: an empty `user` half
        json!([
            {"origin": "user", "type": "text", "text": ""},
            {"origin": "assistant", "type": "text", "text": GREETING}
        ]),
        // and the `user` half missing altogether
        json!([{"origin": "assistant", "type": "text", "text": GREETING}]),
    ] {
        // a memory tier on, so a round would ask the recall with the turn's text
        let out = assemble(
            &[("memory_tier", json!("episodic"))],
            voice_turn(GREET_TURN, messages.clone()),
        );
        assert!(
            !opens_a_round(&out),
            "a turn without the person's words opens no round: {out:?}"
        );
        assert!(
            turn_inserts(&out).is_empty(),
            "no held row any more: {out:?}"
        );
        let cur = curates(&out);
        assert_eq!(cur.len(), 1, "one curate, the agent's line: {out:?}");
        let h = &cur[0]["header"];
        assert_eq!(h["line_only"], "1", "{h}");
        assert_eq!(h["iter"], "0", "{h}");
        assert_eq!(h["turn_id"], GREET_TURN, "{h}");
        assert_eq!(h["session_id"], SESSION, "{h}");
        assert_eq!(
            cur[0]["messages"],
            json!([{"origin": "assistant", "type": "text", "text": GREETING}]),
            "the agent's own line and nothing else: {out:?}"
        );
    }
}

#[test]
fn a_turn_with_the_persons_words_opens_its_round_as_before() {
    let out = assemble(
        &[("memory_tier", json!("episodic"))],
        voice_turn(
            NEXT_TURN,
            json!([
                {"origin": "user", "type": "text", "text": "Fine, thanks."},
                {"origin": "assistant", "type": "text", "text": "Glad to hear."}
            ]),
        ),
    );
    let rows: Vec<(String, String, String)> = calls_of(in_phase(&out, "turn-open"))
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert" && a["table"] == "turns")
        .map(|(id, a)| {
            (
                id,
                a["row"]["role"].as_str().unwrap_or("").to_string(),
                a["row"]["content"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "c-open-turn".to_string(),
                "user".to_string(),
                "Fine, thanks.".to_string()
            ),
            (
                "c-open-pair".to_string(),
                "assistant".to_string(),
                "Glad to hear.".to_string()
            ),
        ],
        "{out:?}"
    );
    assert_eq!(
        hop_str(on_route(&out, "recall"), "recall_query"),
        "Fine, thanks.",
        "the recall asks with the person's words: {out:?}"
    );
}

/// Review M1 of #1378: a held line waits for the NEXT turn of its session, and
/// the session id is new with every call, so the model's words after the
/// person's last turn never ride. The hold itself drops the held agent lines
/// past the departure clock (`DEPART_CUT`, seven days), in the same bundle as
/// its insert. The store compares `recorded_at` as a string (`lt`), so the
/// test does exactly that: a line eight days old falls, one an hour old stays.
#[test]
fn holding_a_line_drops_the_held_lines_past_the_departure_clock() {
    let out = assemble(
        &[],
        voice_turn(
            GREET_TURN,
            json!([{"origin": "assistant", "type": "text", "text": GREETING}]),
        ),
    );
    let calls: Vec<(String, Value)> = out
        .iter()
        .filter(|m| m["header"]["route"].as_str() == Some("cstore"))
        .flat_map(calls_of)
        .collect();
    let drop = calls
        .iter()
        .map(|(_, a)| a)
        .find(|a| a["operation"] == "delete" && a["table"] == "turns")
        .unwrap_or_else(|| panic!("the line drops no old held line: {out:?}"));
    assert_eq!(drop["where"]["role"], "assistant", "{drop}");
    assert_eq!(drop["where"]["deferred"], 1, "only held rows: {drop}");
    let cut = drop["where"]["recorded_at"]["lt"]
        .as_str()
        .unwrap_or_else(|| panic!("a recorded_at cut: {drop}"))
        .to_string();
    // GH #1567: no row is filed any more, so the anchor is the clock the
    // test ran the cell at -- the claim is the cut's place (seven days back),
    // with a day and an hour of room on either side, not elapsed time.
    let anchor = chrono::Utc::now().naive_utc();
    let at = |days: i64, hours: i64| {
        (anchor - chrono::Duration::days(days) - chrono::Duration::hours(hours))
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string()
    };
    let old = at(8, 0);
    let young = at(0, 1);
    assert!(old < cut, "a line eight days old falls: {old} vs {cut}");
    assert!(young >= cut, "a line an hour old stays: {young} vs {cut}");
    assert_eq!(curates(&out).len(), 1, "the line itself goes on: {out:?}");
}

/// Review M1, counter case: a turn with the person's words drops nothing --
/// the text channels' road stays as it was.
#[test]
fn a_turn_with_the_persons_words_drops_no_held_line() {
    let out = assemble(
        &[],
        voice_turn(
            NEXT_TURN,
            json!([{"origin": "user", "type": "text", "text": "Fine, thanks."}]),
        ),
    );
    let drops = out
        .iter()
        .filter(|m| m["header"]["route"].as_str() == Some("cstore"))
        .flat_map(calls_of)
        .filter(|(_, a)| a["operation"] == "delete" && a["table"] == "turns")
        .count();
    assert_eq!(drops, 0, "{out:?}");
}

/// Review M2 of #1378: nothing waits on the store's answer to a held line,
/// but a refusal loses it -- so the refusal is said on stderr, like the
/// refusal of a departure, and nothing else happens.
#[test]
fn a_refused_held_line_is_said_on_stderr() {
    let refusal = json!({
        "header": {"context": {"session_id": SESSION, "turn_id": GREET_TURN, "iter": "0",
                               "col_phase": "line", "store_origin": "collector"},
                   "hop": {"operation": "bundle", "error_code": "store_error"}},
        "messages": [{"origin": "tool", "type": "text", "text": "disk full"}]
    });
    let (out, stderr) = run_cell(ASSEMBLE, &[], refusal);
    assert!(
        stderr.contains("held line of the agent was refused: store_error"),
        "the lost line is said: {stderr}"
    );
    assert!(out.is_empty(), "nothing else happens: {out:?}");
}
