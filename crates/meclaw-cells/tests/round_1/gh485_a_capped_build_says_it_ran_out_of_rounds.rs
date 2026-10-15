//! GH #485, second half — when the cap fires, the refusal named the wrong
//! problem.
//!
//! A capped build sends the collected thread to `./normalise` on `draft`,
//! un-briefed, and `normalise` reads a manifest out of the LAST turn. The last
//! turn of a capped build is whatever the model happened to say — two
//! `tool_call` turns, in the measured run — so the caller got
//!
//! ```text
//! normalise -> builder   route=error   error_code=declarations_not_a_list
//! ```
//!
//! and, on a run whose last answer was prose, `no_manifest_in_answer`. Both are
//! true sentences about the last turn and neither is the reason the build
//! ended. "Your answer was not a list" sends a reader to look at an answer;
//! "you ran out of rounds" sends them to the wish, the corpus or the cap —
//! opposite repairs, which is the argument `wish_incomplete` was created on.
//!
//! `hop.round_capped` is already on that very message. So the branch is one
//! read, and it costs the honest codes nothing: a cap that DID produce a
//! manifest still ships one, and a composer that ASKED still comes back as
//! `wish_incomplete`, because a question is an answer and not a budget.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const NORMALISE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/normalise/config.json"
);

/// What `normalise` emits for one last turn, with or without the cap on the hop.
fn normalised(capped: bool, last: Value) -> Vec<Value> {
    let mut hop = json!({"route": "draft", "build_id": "b485"});
    if capped {
        hop["round_capped"] = json!("1");
        hop["rounds_done"] = json!("7");
    }
    emit_all(
        &shipped_script(NORMALISE),
        &json!({
            "header": {"hop": hop, "context": {"build_id": "b485", "iter": "6"}},
            "params": {},
            "messages": [last],
        }),
    )
}

fn tool_call_turn() -> Value {
    json!({"origin": "assistant", "type": "tool_call", "id": "c6",
           "text": "{\"query\": \"firewall hold lane release\"}"})
}

fn prose_turn() -> Value {
    json!({"origin": "assistant", "type": "text", "id": "",
           "text": "Let me look at the firewall template once more."})
}

fn code_of(all: &[Value]) -> String {
    all[0]["header"]["error_code"]
        .as_str()
        .unwrap_or("<none>")
        .to_string()
}

#[test]
fn a_round_capped_build_that_wrote_nothing_names_the_budget() {
    for last in [tool_call_turn(), prose_turn()] {
        let out = normalised(true, last.clone());
        assert_eq!(
            code_of(&out),
            "design_budget_exhausted",
            "a build that ran out of rounds is reported as the answer it \
             happened to end on ({last:?}), which sends a reader to the wrong \
             repair"
        );
        let said = out[0]["messages"][0]["text"].as_str().unwrap_or("");
        assert!(
            said.contains('7'),
            "the refusal has to carry the rounds it spent, or it is a name \
             without a measurement: {said:?}"
        );
    }
}

#[test]
fn the_cap_does_not_swallow_a_manifest_that_did_arrive() {
    // The last round is a WRITING round (GH #485, first half), so the ordinary
    // way for a capped thread to end is WITH a manifest. The budget code must
    // not eat it.
    let out = normalised(
        true,
        json!({"origin": "assistant", "type": "text", "id": "",
               "text": "{\"declarations\": [{\"scope\": \"/a\", \"diff\": \
                        {\"add_edges\": [{\"from\": \"./x\", \"to\": \"./y\"}]}}]}"}),
    );
    assert_eq!(
        out[0]["header"]["manifest_class"], "composed",
        "a capped round that answered with a manifest still ships it: {:?}",
        out[0]["header"]
    );
}

#[test]
fn a_question_is_still_a_question_under_the_cap() {
    let out = normalised(
        true,
        json!({"origin": "assistant", "type": "text", "id": "",
               "text": "{\"question\": \"which model should the brain use?\"}"}),
    );
    assert_eq!(
        code_of(&out),
        "wish_incomplete",
        "the composer declined to invent a fact it was never given, and that is \
         a RESULT -- the cap did not cause it and must not rename it"
    );
}

#[test]
fn without_the_cap_the_old_codes_are_unchanged() {
    assert_eq!(
        code_of(&normalised(false, tool_call_turn())),
        "declarations_not_a_list"
    );
    assert_eq!(
        code_of(&normalised(false, prose_turn())),
        "no_manifest_in_answer"
    );
}
