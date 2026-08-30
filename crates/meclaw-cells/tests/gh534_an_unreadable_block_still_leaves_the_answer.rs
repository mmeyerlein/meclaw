//! The fence always leaves the answer, readable or not (GitHub #534).
//!
//! [`gh379`](./gh379_the_splitter_cuts_the_sidecar.rs) built the splitter with a
//! deliberate exception: a block that does not parse was left INSIDE the answer
//! and only flagged, on the reasoning that half-cutting a block nobody can read
//! would corrupt the answer for the sake of a write that cannot happen anyway.
//!
//! **That exception is retracted.** It was measured in a running colony: a model
//! that had annotated the turn before it correctly dropped one closing brace,
//! and the fenced JSON travelled through the dispatcher and out to the channel
//! verbatim. The trade the exception assumed does not exist -- `find_annotation`
//! computes the cut out of the same `start`/`end` span it found the block with,
//! so the answer either side of the fence is untouched whether or not the JSON
//! parses. Cutting costs nothing; leaving it in costs the whole point of the
//! cell.
//!
//! What does NOT change is the refusal to repair: an unreadable block is not
//! re-serialised, not fixed and not handed to the memory. It leaves the answer,
//! `hop.sidecar == "malformed"` records that a block was seen, and the
//! `extraction` lane carries nothing -- there is nothing readable to carry.
//!
//! This file also pins the CONTRACT SURFACE the cut has to hold across, because
//! the two happy forms `gh379` pinned were not enough to catch the shape that
//! escaped: every movement, an empty `facts` beside a topic, a trailing newline
//! and none, a block that is not last, and CRLF.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const SPLITTER_CONFIG: &str = "../../templates/talky/splitter/config.json";

fn splitter() -> String {
    shipped_script(SPLITTER_CONFIG)
}

/// A brain completion the way the `llm` cell emits one on the answer path.
fn answer(text: &str) -> Value {
    json!({
        "header": {
            "hop": {"finish_reason": "stop", "model": "a/model"},
            "context": {"session_id": "s-534", "turn_id": "s-534#1"}
        },
        "messages": [{"origin": "assistant", "type": "text", "text": text}]
    })
}

/// The prose half of a cut, whichever form the splitter chose.
fn prose(out: &[Value]) -> String {
    out[0]["messages"][0]["text"]
        .as_str()
        .expect("the answer turn carries text")
        .to_string()
}

/// The raw block on lane `extraction`, or `None` when nothing was routed there.
fn extraction(out: &[Value]) -> Option<String> {
    out.iter()
        .find(|m| m["header"]["route"] == "extraction")
        .map(|m| {
            m["messages"][0]["text"]
                .as_str()
                .expect("the sidecar half carries text")
                .to_string()
        })
}

/// The completion that reached a person's chat window, in the shape it had: the
/// answer, a well-formed-looking fence, and a payload one closing brace short.
/// The measured original was German prose; the words are translated here and
/// nothing else about the string moves, because the property under test is the
/// fence and the missing brace, not the language of the sentence beside them
/// (export rule R8, which reads string literals of exported tests).
const THE_ESCAPE: &str = concat!(
    "You are right in the middle of the second arc. ",
    "I will stay spoiler-free: how are you finding the season so far?\n",
    "\n",
    "```memory\n",
    "{\"facts\":[],\"topic\":{\"movement\":\"continue\",\"name\":\"Andor\"}\n",
    "```"
);

#[test]
fn the_block_that_escaped_does_not_escape_again() {
    let out = emit_all(&splitter(), &answer(THE_ESCAPE));
    assert_eq!(out.len(), 1, "nothing readable to route: {out:?}");
    let prose = prose(&out);
    assert!(
        !prose.contains("```") && !prose.contains("\"facts\""),
        "the reader gets the sentence and never the instrument: {prose:?}"
    );
    assert_eq!(
        prose,
        "You are right in the middle of the second arc. \
         I will stay spoiler-free: how are you finding the season so far?",
        "and the sentence either side of the fence is untouched: {prose:?}"
    );
    assert_eq!(
        out[0]["header"]["sidecar"], "malformed",
        "the miss stays on the record, so a close pass can tell a model that \
         missed the form from one that never annotated: {out:?}"
    );
    assert_eq!(
        extraction(&out),
        None,
        "and nothing unreadable is handed to the memory: {out:?}"
    );
}

#[test]
fn the_hop_the_completion_arrived_on_survives_the_flag() {
    // The malformed path is a pass-through in every respect but the text: the
    // dispatcher still needs the finish reason, and the accounting keys the llm
    // cell wrote are the round's only record of what the call cost.
    let out = emit_all(&splitter(), &answer(THE_ESCAPE));
    assert_eq!(out[0]["header"]["finish_reason"], "stop", "{out:?}");
    assert_eq!(out[0]["header"]["model"], "a/model", "{out:?}");
    assert!(
        out[0]["header"].get("route").is_none(),
        "a malformed sidecar routes nowhere: {out:?}"
    );
}

#[test]
fn an_opener_with_no_closer_takes_its_fence_with_it() {
    // The completion stopped inside the block. Cutting only the JSON -- which is
    // what the naked-object probe would do -- leaves the bare ```memory line
    // standing in the answer, which is the same leak one character smaller.
    let out = emit_all(
        &splitter(),
        &answer("Notiert.\n\n```memory\n{\"facts\":[],\"topic\":"),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(prose(&out), "Notiert.", "{out:?}");
    assert_eq!(out[0]["header"]["sidecar"], "malformed", "{out:?}");
}

/// Every form the inline contract admits, and the ways a model spells them.
///
/// `(name, block, movement)` -- the block goes into a `memory` fence, the
/// movement is what the extraction half must carry through unchanged.
const CONTRACT_FORMS: &[(&str, &str, &str)] = &[
    (
        "an explicit nothing",
        "{\"nothing_new\": true, \"facts\": [], \"topic\": {\"movement\": \"continue\"}}",
        "continue",
    ),
    (
        "empty facts beside a topic that continues",
        "{\"facts\": [], \"topic\": {\"movement\": \"continue\", \"name\": \"Andor\"}}",
        "continue",
    ),
    (
        "a fact beside a topic that starts",
        "{\"facts\": [{\"subject\": \"alex\", \"predicate\": \"favorite_colour\", \
         \"claim\": \"blue\", \"fact_kind\": \"world\"}], \
         \"topic\": {\"movement\": \"start\", \"name\": \"colours\"}}",
        "start",
    ),
    (
        "a topic that ends",
        "{\"facts\": [], \"topic\": {\"movement\": \"end\", \"name\": \"colours\"}}",
        "end",
    ),
];

#[test]
fn every_form_of_the_contract_is_cut_and_travels() {
    // The escape got through because only two of these were ever pinned. An
    // empty `facts` list is a VERDICT like `nothing_new` is -- it books the turn
    // as annotated-and-empty -- so it has to reach the lane, not be judged here.
    for (name, block, movement) in CONTRACT_FORMS {
        let out = emit_all(
            &splitter(),
            &answer(&format!("Fine.\n\n```memory\n{block}\n```")),
        );
        assert_eq!(out.len(), 2, "{name}: a valid block is a cut: {out:?}");
        assert_eq!(prose(&out), "Fine.", "{name}: {out:?}");
        let raw = extraction(&out).unwrap_or_else(|| panic!("{name}: no extraction half: {out:?}"));
        let carried: Value = meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| {
            panic!("{name}: the lane carries what the model wrote ({e}): {raw}")
        });
        assert_eq!(carried["topic"]["movement"], *movement, "{name}: {raw}");
        assert!(
            out[0]["header"].get("sidecar").is_none(),
            "{name}: a readable block is not a miss: {out:?}"
        );
    }
}

/// The same well-formed block, spelled the ways a model actually spells it.
const BLOCK: &str = "{\"facts\": [], \"topic\": {\"movement\": \"continue\", \"name\": \"Andor\"}}";

#[test]
fn the_spelling_of_the_fence_does_not_decide_whether_the_reader_sees_it() {
    let cases: &[(&str, String, &str)] = &[
        (
            "with a trailing newline before the closing fence",
            format!("Fine.\n\n```memory\n{BLOCK}\n```"),
            "Fine.",
        ),
        (
            "without one",
            format!("Fine.\n\n```memory\n{BLOCK}```"),
            "Fine.",
        ),
        (
            "with the answer continuing after the block",
            format!("Before.\n\n```memory\n{BLOCK}\n```\n\nAfter."),
            "Before.\n\n\n\nAfter.",
        ),
        (
            "with CRLF line endings throughout",
            format!("Fine.\r\n\r\n```memory\r\n{BLOCK}\r\n```"),
            "Fine.",
        ),
        (
            "with trailing whitespace after the fence word",
            format!("Fine.\n\n```memory   \n{BLOCK}\n```"),
            "Fine.",
        ),
    ];
    for (name, text, expected) in cases {
        let out = emit_all(&splitter(), &answer(text));
        assert_eq!(out.len(), 2, "{name}: {out:?}");
        assert_eq!(prose(&out), *expected, "{name}: {out:?}");
        assert!(
            extraction(&out).is_some_and(|raw| raw.contains("Andor")),
            "{name}: the block reaches the lane: {out:?}"
        );
    }
}

#[test]
fn a_tool_round_is_still_never_taken_apart() {
    // The one exception that stands: a completion carrying calls belongs to the
    // dispatcher whole, fence or no fence (GH #378). Cutting here would build
    // the mixed form that strands a round, which is a worse failure than a
    // block a reader sees.
    let text = format!("Moment.\n\n```memory\n{BLOCK}\n```");
    let input = json!({
        "header": {"hop": {"finish_reason": "tool_calls"}},
        "messages": [
            {"origin": "assistant", "type": "tool_call", "id": "c1",
             "text": "{\"name\":\"weather\",\"arguments\":\"{}\"}"},
            {"origin": "assistant", "type": "text", "text": text}
        ]
    });
    let out = emit_all(&splitter(), &input);
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(
        out[0]["messages"], input["messages"],
        "byte-identical, fence included: {out:?}"
    );
    assert!(out[0]["header"].get("sidecar").is_none(), "{out:?}");
}
