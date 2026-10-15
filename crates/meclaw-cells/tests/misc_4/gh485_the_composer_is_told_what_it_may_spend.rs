//! GH #485, first half — a loop the model cannot see the end of is a loop it
//! will not leave.
//!
//! `BUILDER_MAX_ITER` lives on the condition of `./weave -> ./compose` and in
//! `weave`'s `params`. `weave` even computes the numbers — it stamps
//! `hop.round_capped` and `hop.rounds_done` on every emission that closes a
//! round — and the RE-BRIEFING carried neither. `briefed()` re-attached the
//! parked instructions and the parked question, and nothing in the prompt said
//! how many rounds had gone or how many were left.
//!
//! Measured against a hosted Sonnet-class model, one wish that is two edges of
//! work: seven rounds, `iter=0` through `iter=6`, every single one
//! `finish_reason: tool_calls`, thirteen tool calls, not one text turn, and the
//! build ended without a manifest. The briefing's own TOOLS block argues for
//! exactly that behaviour — *"Answering without a single call is allowed and is
//! almost always wrong"* — and named no criterion for when looking is done.
//!
//! So the repair is two mechanisms, not a sentence:
//!
//! * the re-briefing carries the round the composer is in and the rounds that
//!   are left, recomputed by the cell that already holds both numbers;
//! * the LAST round of a build is a WRITING round — the tool menu is withheld
//!   from it, so answering is the only move the wire admits. The research
//!   budget and the writing budget are two budgets (`write_rounds`), which is
//!   what stops a model from spending the whole of one on the other.
//!
//! This file asserts the PROMPT, never an answer, for the reason
//! `gh477_the_second_round_still_knows_the_question.rs` gives: every scenario
//! case of the design lane drives a stub that answers by position and never
//! reads what it was asked.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const WEAVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/weave/config.json"
);
const BUILD: &str = "b485";

fn weave_config() -> Value {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/builder/weave/config.json");
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).expect("weave config"))
        .expect("parses")
}

fn max_iter() -> i64 {
    weave_config()["params"]["max_iter"]
        .as_i64()
        .expect("max_iter in params")
}

fn write_rounds() -> i64 {
    weave_config()["params"]["write_rounds"]
        .as_i64()
        .expect("write_rounds in params -- the writing budget is a budget of its own")
}

fn row(iter: i64, role: &str, turn: &str, at: &str) -> Value {
    json!({"build_id": BUILD, "iter": iter, "role": role, "turn": turn,
           "fired": 0, "recorded_at": at})
}

/// The parked prompt, exactly as `brief` writes it: instructions plus the tool
/// menu. `weave` reads this row back and hands it on.
fn system_row(iter: i64) -> Value {
    let tree = json!({
        "instructions": {"text": "GRAMMAR -- the part that gets refused."},
        "tools": {"librarian_search": {"text": "{\"type\":\"function\"}"}},
    });
    row(
        iter,
        "system",
        &meclaw_core::serde_json::to_string(&tree).expect("tree"),
        "2999-01-01T09:00:00.000000Z",
    )
}

fn user_row() -> Value {
    row(
        0,
        "user",
        "{\"origin\":\"user\",\"type\":\"text\",\"text\":\"a wish\"}",
        "2999-01-01T09:00:01.000000Z",
    )
}

/// One closed tool round: the assistant turn that opened it and the result.
fn closed_round(iter: i64) -> Vec<Value> {
    let call = format!("c{iter}");
    vec![
        row(
            iter,
            "assistant",
            &format!(
                "[{{\"origin\":\"assistant\",\"type\":\"tool_call\",\"id\":\"{call}\",\
                 \"text\":\"{{}}\"}}]"
            ),
            "2999-01-01T10:00:00.000000Z",
        ),
        row(
            iter,
            "tool",
            &format!(
                "{{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"{call}\",\
                 \"text\":\"a catalogue row\"}}"
            ),
            "2999-01-01T10:00:01.000000Z",
        ),
    ]
}

/// What `weave` emits when the store hands back the slate of round `it`.
fn round_closed_at(it: i64) -> Vec<Value> {
    let mut rows = vec![system_row(0), user_row()];
    for i in 0..=it {
        rows.extend(closed_round(i));
    }
    let slate = meclaw_core::serde_json::to_string(&rows).expect("rows");
    emit_all(
        &shipped_script(WEAVE),
        &json!({
            "header": {"hop": {"route": "cstore"},
                       "context": {"build_id": BUILD, "iter": it.to_string(),
                                   "repairs": "0"}},
            "params": {},
            "messages": [{"origin": "tool", "type": "tool_result",
                          "id": "w-round-read", "text": slate}],
        }),
    )
}

fn emission_on<'a>(all: &'a [Value], route: &str) -> &'a Value {
    all.iter()
        .find(|m| m["header"]["route"] == route)
        .unwrap_or_else(|| {
            panic!(
                "no emission on route {route:?}: {:?}",
                all.iter()
                    .map(|m| m["header"]["route"].clone())
                    .collect::<Vec<_>>()
            )
        })
}

fn budget_text(fired: &Value) -> String {
    fired["system"]["budget"]["text"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "the re-briefing carries no `system.budget` slot -- from the \
                 model's side the loop is unbounded, and it behaves \
                 accordingly: {:?}",
                fired["system"]
            )
        })
        .to_string()
}

#[test]
fn every_re_briefed_round_carries_the_round_it_is_in() {
    let total = max_iter() + 1;
    for it in 0..(max_iter() - write_rounds()) {
        let all = round_closed_at(it);
        let fired = emission_on(&all, "fire");
        let text = budget_text(fired);
        // The round the composer is ABOUT to run, counted from one: it closed
        // round `it`, so the re-entry is round `it + 2` of `total`.
        let expected = format!("round {} of {}", it + 2, total);
        assert!(
            text.contains(&expected),
            "round {it} re-briefs with {text:?} -- it has to name {expected:?}, \
             which is the number the emitting cell already holds"
        );
    }
}

#[test]
fn the_round_that_is_left_is_on_the_hop_as_well() {
    let all = round_closed_at(1);
    let fired = emission_on(&all, "fire");
    assert_eq!(
        fired["header"]["rounds_left"], "4",
        "the loop stamps what it spends: `rounds_done` counts backwards, \
         `rounds_left` counts forwards, and an onlooker needs the second one: \
         {:?}",
        fired["header"]
    );
}

#[test]
fn a_looking_round_still_gets_the_tool_menu() {
    let last_looking = max_iter() - write_rounds() - 1;
    let all = round_closed_at(last_looking);
    let fired = emission_on(&all, "fire");
    assert!(
        fired["system"]["tools"]["librarian_search"]["text"].is_string(),
        "a round that may still look is asked with the tools it may call: {:?}",
        fired["system"]
    );
}

#[test]
fn the_last_round_of_a_build_is_a_writing_round() {
    // The mechanism, and the reason this is not a prompt trick: on the last
    // round the tool menu is not offered, so a tool call is not a move the wire
    // admits. A model that cannot call cannot spend the round looking.
    let last = max_iter() - write_rounds();
    let all = round_closed_at(last);
    let fired = emission_on(&all, "fire");
    assert!(
        fired["system"]["tools"].is_null(),
        "the writing round still publishes the tool menu, so the model may \
         spend it on another lookup and the build ends paid-for and empty: {:?}",
        fired["system"]
    );
    // Leaving the slot out is not enough and the first S13 walk measured it: an
    // `llm` cell UPSERTS the system tree per slot path into its own cell.db, and
    // this composer is ONE cell for every build in the colony -- so the menu of
    // the round before was still standing and the model answered
    // `finish_reason: tool_calls` in a round whose body carried no tools at all.
    // `$replace` at the root (GH #264) is what makes the message authoritative.
    assert_eq!(
        fired["system"]["$replace"], true,
        "without the root replace marker the withheld menu is merely absent \
         from this message and still remembered from the last: {:?}",
        fired["system"]
    );
    let text = budget_text(fired);
    assert!(
        text.contains("no tools") || text.contains("not offered"),
        "and it has to SAY the menu is gone, or the model reads its absence as \
         an outage: {text:?}"
    );
}

#[test]
fn the_briefing_names_the_writing_round_from_the_first_round_on() {
    // Round 0 is briefed by `brief`, not by the loop, so the rule has to be in
    // the head: a budget a model learns about in the last round is a budget it
    // could not plan against.
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/builder/brief/config.json");
    let script = shipped_script(p.to_str().expect("path"));
    assert!(
        script.contains("writing round") || script.contains("WRITING round"),
        "the briefing head never mentions the round that offers no tools"
    );
}
