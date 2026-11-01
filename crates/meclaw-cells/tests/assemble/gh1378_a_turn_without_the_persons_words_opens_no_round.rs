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
//! and no consult on its hop, carrying the agent's own text, is held as the
//! agent's line -- one `assistant` row, filed DEFERRED, so the next round of
//! the session carries it to the curator exactly like the answer half of a
//! deferred duplex pair (GH #103 / #889) -- and opens NO round: no turn-open,
//! no curate, no recall that would read the agent's sentence as `origin user`.
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
            || matches!(h["route"].as_str(), Some("recall" | "curate" | "answer"))
    })
}

#[test]
fn a_greeting_turn_is_held_as_the_agents_line_and_opens_no_round() {
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
        let rows = turn_inserts(&out);
        assert_eq!(rows.len(), 1, "one row, the agent's line: {rows:?}");
        let row = &rows[0].1;
        assert_eq!(row["role"], "assistant", "{row}");
        assert_eq!(row["content"], GREETING, "{row}");
        assert_eq!(row["turn_id"], GREET_TURN, "{row}");
        assert_eq!(
            row["deferred"], 1,
            "held for the next round of the session: {row}"
        );
    }
}

#[test]
fn the_held_line_rides_with_the_next_round_in_front_of_the_persons_words() {
    let held = turn_inserts(&assemble(
        &[],
        voice_turn(
            GREET_TURN,
            json!([
                {"origin": "user", "type": "text", "text": ""},
                {"origin": "assistant", "type": "text", "text": GREETING}
            ]),
        ),
    ));
    let opened_by = assemble(
        &[],
        voice_turn(
            NEXT_TURN,
            json!([{"origin": "user", "type": "text", "text": "Fine, thanks."}]),
        ),
    );
    let mut win: Vec<Value> = held.into_iter().map(|(_, r)| r).collect();
    win.extend(
        calls_of(in_phase(&opened_by, "turn-open"))
            .into_iter()
            .filter(|(_, a)| a["operation"] == "insert" && a["table"] == "turns")
            .map(|(_, a)| a["row"].clone()),
    );
    let opened = assemble(
        &[],
        bundle_reply(
            "turn-open",
            NEXT_TURN,
            &[
                ("c-open-turn", Value::Null),
                ("c-open-round", json!([])),
                ("c-open-win", json!(win)),
                ("c-open-scope", json!([])),
                ("c-open-roster", json!([])),
            ],
        ),
    );
    let mut leg = calls_of(in_phase(&opened, "collect"))
        .into_iter()
        .map(|(_, a)| a)
        .find(|a| a["operation"] == "insert" && a["row"]["role"] == "leg-window")
        .map(|a| a["row"].clone())
        .unwrap_or_else(|| panic!("no window leg parked: {opened:?}"));
    leg["fired"] = json!(0);
    let fired = assemble(
        &[],
        bundle_reply("collect", NEXT_TURN, &[("c-collect-read", json!([leg]))]),
    );
    let round: Vec<(String, String)> = on_route(&fired, "curate")["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| {
            (
                m["origin"].as_str().unwrap_or("").to_string(),
                m["text"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(
        round,
        vec![
            ("assistant".to_string(), GREETING.to_string()),
            ("user".to_string(), "Fine, thanks.".to_string())
        ],
        "the greeting reaches the curator as the agent's own line: {round:?}"
    );
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
        .unwrap_or_else(|| panic!("the hold drops no old held line: {out:?}"));
    assert_eq!(drop["where"]["role"], "assistant", "{drop}");
    assert_eq!(drop["where"]["deferred"], 1, "only held rows: {drop}");
    let cut = drop["where"]["recorded_at"]["lt"]
        .as_str()
        .unwrap_or_else(|| panic!("a recorded_at cut: {drop}"))
        .to_string();
    // Anchored on the cell's own clock (the line's `recorded_at`), never on a
    // clock of the test: the claim is the cut's place, not elapsed time.
    let rows = turn_inserts(&out);
    let filed = rows[0].1["recorded_at"]
        .as_str()
        .unwrap_or_else(|| panic!("a recorded_at: {rows:?}"))
        .to_string();
    let anchor = chrono::NaiveDateTime::parse_from_str(&filed, "%Y-%m-%dT%H:%M:%S%.fZ")
        .unwrap_or_else(|e| panic!("{filed}: {e}"));
    let at = |days: i64, hours: i64| {
        (anchor - chrono::Duration::days(days) - chrono::Duration::hours(hours))
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string()
    };
    let old = at(8, 0);
    let young = at(0, 1);
    assert!(old < cut, "a line eight days old falls: {old} vs {cut}");
    assert!(young >= cut, "a line an hour old stays: {young} vs {cut}");
    assert_eq!(rows.len(), 1, "the line itself is still filed: {out:?}");
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
