//! GH #485, third half — the model's own sentence went to the dead-letter
//! queue, every round, and nothing said so.
//!
//! `dispatcher@1.1.1` splits ONE model answer into two emissions when it
//! carries content AND tool calls: the bundle on `hop.route == 'calls'`, and
//! the sentence beside it on `hop.route == 'answer'` with `hop.interim` set
//! (GH #378). The builder hive drew edges out of `./dispatch` for `calls`, for
//! `tool_name`, for `result` and a default on `hop.route == 'tool'` — and none
//! for `answer`. A hosted model narrates while it calls, so every narrating
//! round produced a `no_route` dead letter, silently, beside a round that
//! otherwise worked.
//!
//! Two things follow and only one of them is tidiness. A dead letter per round
//! is noise that hides real ones; and the sentence is the only part of the
//! answer that could have carried the manifest, so dropping it unread is the
//! one loss this hive must not take quietly.
//!
//! It is parked, not replayed. An interim sentence and its own bundle are ONE
//! provider message, and re-entering the thread as a second `assistant` turn
//! between the bundle and its results is a message order no OpenAI-shaped
//! endpoint accepts. So `weave` writes it into the round table under a role of
//! its own — the transcript keeps what the model said — and `rebuild` skips it
//! for the same reason it skips `caller` and `system`.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const WEAVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/weave/config.json"
);
const BUILD: &str = "b485i";

fn hive() -> Value {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/builder/config.json");
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).expect("builder config"))
        .expect("parses")
}

#[test]
fn the_hive_carries_the_interim_answer_off_the_dispatcher() {
    let edges = hive()["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    let carried: Vec<&Value> = edges
        .iter()
        .filter(|e| {
            e["from"] == "./dispatch"
                && e["condition"]
                    .as_str()
                    .unwrap_or("")
                    .contains("hop.route == 'answer'")
        })
        .collect();
    assert_eq!(
        carried.len(),
        1,
        "the dispatcher emits the model's sentence on `answer` whenever it \
         narrates beside a call, and no edge takes it: every narrating round is \
         a no_route dead letter. Edges out of ./dispatch: {:?}",
        edges
            .iter()
            .filter(|e| e["from"] == "./dispatch")
            .map(|e| e["condition"].clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        carried[0]["to"], "./weave",
        "and it belongs at the fan-in, which is the only cell of this hive that \
         writes a round table row"
    );
}

/// What `weave` does with the sentence that arrived beside a bundle.
fn weave_on_interim() -> Vec<Value> {
    emit_all(
        &shipped_script(WEAVE),
        &json!({
            "header": {"hop": {"route": "answer", "interim": "1",
                               "finish_reason": "tool_calls"},
                       "context": {"build_id": BUILD, "iter": "2", "repairs": "0"}},
            "params": {},
            "messages": [{"origin": "assistant", "type": "text", "id": "",
                          "text": "Let me check the firewall's hold lane first."}],
        }),
    )
}

#[test]
fn the_sentence_is_written_down_under_a_role_of_its_own() {
    let out = weave_on_interim();
    assert_eq!(
        out.len(),
        1,
        "one emission: the row, and nothing that re-enters the composer -- \
         a narrating round is still the SAME round: {out:?}"
    );
    let legs = out[0]["messages"].as_array().expect("bundle legs");
    assert_eq!(
        legs.len(),
        1,
        "an insert, and no read-back: this leg must not join the round \
         election, or a sentence could close a round its results have not \
         answered: {legs:?}"
    );
    let op: Value =
        meclaw_core::serde_json::from_str(legs[0]["text"].as_str().unwrap_or("{}")).expect("op");
    assert_eq!(op["operation"], "insert");
    assert_eq!(op["table"], "thread");
    assert_eq!(op["row"]["build_id"], BUILD);
    assert_eq!(op["row"]["iter"], 2, "under the round it was said in");
    assert_eq!(
        op["row"]["role"], "interim",
        "a role of its own -- `assistant` would put it between a bundle and its \
         results, which is a message order no provider accepts: {op}"
    );
}

#[test]
fn the_sentence_never_reaches_the_provider_again() {
    // `rebuild` skips `caller` and `system` because neither is a turn. An
    // `interim` row is not a turn either: it is one half of a provider message
    // whose other half is already in the thread.
    let rows = json!([
        {"build_id": BUILD, "iter": 0, "role": "user",
         "turn": "{\"origin\":\"user\",\"type\":\"text\",\"text\":\"a wish\"}",
         "fired": 0, "recorded_at": "2999-01-01T09:00:00.000000Z"},
        {"build_id": BUILD, "iter": 0, "role": "assistant",
         "turn": "[{\"origin\":\"assistant\",\"type\":\"tool_call\",\"id\":\"c0\",\"text\":\"{}\"}]",
         "fired": 0, "recorded_at": "2999-01-01T10:00:00.000000Z"},
        {"build_id": BUILD, "iter": 0, "role": "interim",
         "turn": "{\"origin\":\"assistant\",\"type\":\"text\",\"text\":\"NARRATION\"}",
         "fired": 0, "recorded_at": "2999-01-01T10:00:00.500000Z"},
        {"build_id": BUILD, "iter": 0, "role": "tool",
         "turn": "{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"c0\",\"text\":\"a row\"}",
         "fired": 0, "recorded_at": "2999-01-01T10:00:01.000000Z"},
    ]);
    let out = emit_all(
        &shipped_script(WEAVE),
        &json!({
            "header": {"hop": {"route": "cstore"},
                       "context": {"build_id": BUILD, "iter": "0", "repairs": "0"}},
            "params": {},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "w-round-read",
                          "text": meclaw_core::serde_json::to_string(&rows).expect("rows")}],
        }),
    );
    let fired = out
        .iter()
        .find(|m| m["header"]["route"] == "fire")
        .unwrap_or_else(|| panic!("no round closed: {out:?}"));
    let thread = meclaw_core::serde_json::to_string(&fired["messages"]).expect("thread");
    assert!(
        !thread.contains("NARRATION"),
        "the interim sentence re-entered the thread as a turn of its own, \
         between a bundle and its results: {thread}"
    );
}
