//! GH #1282 (collector half) -- the window of a consult ends on its request.
//!
//! Measured before this lock (soul2me rollout R025, read-only `message_log` of
//! one colony, rows 15938 -> 15982): a `complex` turn leaves the speaking
//! surface through the curator on `consult`, and the reasoning core's collector
//! receives the surface's WHOLE window on `in_turn` -- earlier questions and
//! answers, the request, the recall round behind it. The collector read the
//! request as the question (`text_of(.., "user")`, the last user text) and the
//! LAST assistant text of the arrival as the answer half of a duplex pair
//! (R-25-9), filed under the question's id plus `-a`. That text was the
//! surface's answer to the turn BEFORE the request ("Andor season 2 ..."), so
//! the core's window came out as `[user <request>, assistant <old answer>]`:
//! an old answer behind the request, a window ending on `assistant`. A model
//! without prefill support rejects that with 400 (R025: 2 of 2 consults on a
//! Claude model), and every other model read an answer to a question nobody
//! asked as the last word of the conversation.
//!
//! The answer half of a duplex arrival is the agent text that FOLLOWS the
//! person's block -- "a user block plus the agent fragments that follow it",
//! as the collector's own comment says. Assistant text in front of the last
//! user text is earlier conversation and never the answer half.
//!
//! What is pinned here, over the SHIPPED `script_inline` on stdin:
//!
//! 1. the curator's consult (the surface's window ending on the request and its
//!    recall round) files ONE row, the request, and no pair row -- the
//!    surface's last texts in front of the request travel framed INSIDE that
//!    row as context, never as turns of their own;
//! 2. the round of that consult the fan-in hands the curator is that one user
//!    row and ends on it (the history in front of it is the curator's,
//!    GH #889, and its ledger keeps the rounds this cell passes);
//! 3. a follow-up request ("plan that thoroughly") keeps what "that" points
//!    at: the answer it refers to is in the frame of the request row, and the
//!    round still ends on the request;
//! 4. the speaking model's own `consult_cogny` errand, on the hop it really
//!    arrives with (`async`, `consult_id`; `consult_class` is a CONTEXT key the
//!    `talky -> cogny` edge sets), files its arguments -- `context` and
//!    `question` the model wrote itself -- as the request and no pair row;
//! 5. a consult (`consult_class` on the hop) files no pair row at all, not even
//!    for a sentence of the surface behind the request;
//! 6. a plain arrival whose assistant text stands before the last user text
//!    files no pair row; one whose assistant text follows it still does
//!    (R-25-9 kept); one with no user text at all files none either.
//! 7. the frame carries spoken text only: no sidecar block or its dropped mark,
//!    no pin, no block id or released/expired form of the surface's wall;
//! 8. nothing the core's ledger already holds -- an earlier request, the advice
//!    row with the core's answer, the surface passing it on -- is framed again;
//! 9. the fence rule and the curator's forms are the curator's text, byte for byte.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "keeper:01a1-consult:2026-10-20T15:09:35.344573Z";
const REQUEST: &str = "[#cf1a39d6e38c] Plan me a thorough three-day trip to Athens on 800 euros.";
const OLD_ANSWER: &str = "Andor season 2 is out on 23 April 2025.";

/// The surface's window as the curator hands it to the core on `consult`
/// (R025 row 15938, shortened): earlier turns, the request, its recall round.
fn consult_window() -> Value {
    json!([
        {"origin": "user", "type": "text", "text": "[#e4a96efc71c5] Say hello."},
        {"origin": "assistant", "type": "text", "text": "Hello, good to see you."},
        {"origin": "user", "type": "text", "text": "[#4c65e203bec4] When did Andor season 2 come out?"},
        {"origin": "assistant", "type": "text", "text": "Let me look that up."},
        {"origin": "assistant", "type": "tool_call", "id": "call_web_1",
         "text": "{\"name\":\"web_search\",\"arguments\":\"{}\"}"},
        {"origin": "tool", "type": "tool_result", "id": "call_web_1", "text": "{\"results\": []}"},
        {"origin": "assistant", "type": "text", "text": OLD_ANSWER},
        {"origin": "user", "type": "text", "text": REQUEST},
        {"origin": "assistant", "type": "tool_call", "id": "call_recall_1",
         "text": "{\"name\":\"memory_recall\",\"arguments\":\"{}\"}"},
        {"origin": "tool", "type": "tool_result", "id": "call_recall_1",
         "text": "[#b45f8d2c3e5d] Nothing in this memory answers this question."}
    ])
}

fn consult_arrival(messages: Value) -> Value {
    lane(
        "in_turn",
        json!({"turn_id": TURN, "consult_class": "consult",
               "consult_id": "k-a0686b6f"}),
        json!({"turn_id": TURN, "consult_class": "consult",
               "consult_id": "k-a0686b6f"}),
        messages,
    )
}

/// `(tool_call_id, row)` of every `turns` insert of the turn-open bundle.
fn turn_rows(out: &[Value]) -> Vec<(String, Value)> {
    calls_of(in_phase(out, "turn-open"))
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert" && a["table"] == "turns")
        .map(|(id, a)| (id, a["row"].clone()))
        .collect()
}

fn texts(msgs: &Value) -> Vec<(String, String)> {
    msgs.as_array()
        .expect("messages")
        .iter()
        .map(|m| {
            (
                m["origin"].as_str().unwrap_or("").to_string(),
                m["text"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// The round the fan-in hands the curator for `arrival`, as `(origin, text)`.
///
/// The rows the collector files at the turn-open are the rows the store hands
/// back as the window (`c-open-win`, the same bundle, GH #419); the round the
/// fan-in passes to the curator is that window (GH #889: the history in front
/// of it is the curator's, off its ledger -- and the ledger keeps the rounds
/// this cell passes, so an old answer filed here was also history in every
/// later consult of the session, R025 row 16300).
fn curated_round(arrival: Value) -> Vec<(String, String)> {
    let out = assemble(&[], arrival);
    let filed: Vec<Value> = turn_rows(&out).into_iter().map(|(_, r)| r).collect();

    let opened = assemble(
        &[],
        bundle_reply(
            "turn-open",
            TURN,
            &[
                ("c-open-turn", Value::Null),
                ("c-open-round", json!([])),
                ("c-open-win", json!(filed)),
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
        bundle_reply("collect", TURN, &[("c-collect-read", json!([leg]))]),
    );
    let round = on_route(&fired, "curate");
    texts(&round["messages"])
}

/// The request row's content: a frame naming it context, then the request.
fn assert_framed(content: &str, request: &str, context: &[&str]) {
    assert!(
        content.starts_with(CONSULT_FRAME_HEAD),
        "the context is framed as context: {content:?}"
    );
    assert!(
        content.ends_with(&format!("]\n{request}")),
        "the request is the last line of the row: {content:?}"
    );
    for c in context {
        assert!(content.contains(c), "{c:?} missing from {content:?}");
    }
}

const CONSULT_FRAME_HEAD: &str = "[consult from the speaking surface";

#[test]
fn a_curator_consult_files_the_request_and_no_old_answer() {
    let rows = turn_rows(&assemble(&[], consult_arrival(consult_window())));
    assert_eq!(
        rows.len(),
        1,
        "one row, the request -- the surface's earlier answer is not the core's \
         answer half (R025 row 15946 filed it as `c-open-pair`): {rows:?}"
    );
    assert_eq!(rows[0].0, "c-open-turn");
    assert_eq!(rows[0].1["role"], "user");
    // The last four texts in front of the request, the old answer among them,
    // ride in the request row as context; the first exchange ("Say hello.")
    // is older than that and stays out.
    let content = rows[0].1["content"].as_str().unwrap_or("");
    assert_framed(
        content,
        REQUEST,
        &[
            "Hello, good to see you.",
            "When did Andor season 2 come out?",
            "Let me look that up.",
            OLD_ANSWER,
        ],
    );
    assert!(!content.contains("Say hello."), "{content:?}");
    assert!(
        rows.iter().all(|(id, r)| id != "c-open-pair"
            && r["role"] != "assistant"
            && r["content"] != OLD_ANSWER),
        "the old answer is filed as no turn of its own: {rows:?}"
    );
}

#[test]
fn the_consult_window_is_the_request_and_ends_on_it() {
    let got = curated_round(consult_arrival(consult_window()));
    assert_eq!(
        got.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(),
        vec!["user"],
        "the consult's round is its request row, and it ends on it -- R025 row \
         15952 was `[user <request>, assistant <old answer>]`: {got:?}"
    );
    assert_framed(&got[0].1, REQUEST, &[OLD_ANSWER]);
}

#[test]
fn a_follow_up_consult_keeps_what_it_refers_to() {
    // "Plan that thoroughly" is a request only together with the answer in
    // front of it. Without the frame the core's window was the bare sentence:
    // its ledger holds only its own consults, and recall rarely sees a turn
    // this fresh (review of GH #1282, I1).
    let suggestion = "How about three days in Athens? Flights are cheap in May.";
    let got = curated_round(consult_arrival(json!([
        {"origin": "user", "type": "text", "text": "Where could I go for a long weekend?"},
        {"origin": "assistant", "type": "text", "text": suggestion},
        {"origin": "user", "type": "text", "text": "Plan that thoroughly."},
        {"origin": "assistant", "type": "tool_call", "id": "call_recall_2",
         "text": "{\"name\":\"memory_recall\",\"arguments\":\"{}\"}"},
        {"origin": "tool", "type": "tool_result", "id": "call_recall_2", "text": "[]"}
    ])));
    assert_eq!(
        got.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(),
        vec!["user"],
        "context, not turns: the window still ends on the request: {got:?}"
    );
    assert_framed(
        &got[0].1,
        "Plan that thoroughly.",
        &["Where could I go for a long weekend?", suggestion],
    );
}

#[test]
fn the_speaking_models_own_consult_files_its_errand_and_no_pair() {
    let errand = "{\"consult_id\":\"\",\"context\":\"The person wants a trip plan.\",\
                  \"question\":\"Plan three days in Athens on 800 euros.\"}";
    // The hop it really arrives with: the dispatcher stamps `async` and the
    // consult id; `consult_class` is set by the `talky -> cogny` edge as a
    // CONTEXT key (templates/assistant), so the hop guard does not see this
    // arrival -- it stays clean because its one turn is a `tool_call`, which
    // is no answer half, and its context travels in the arguments.
    let rows = turn_rows(&assemble(
        &[],
        lane(
            "in_turn",
            json!({"async": "1", "consult_id": "k-a0686b6f",
                   "tool_call_id": "call_consult_1"}),
            json!({"consult_id": "k-a0686b6f", "consult_class": "consult"}),
            json!([
                {"origin": "assistant", "type": "tool_call", "id": "call_consult_1",
                 "text": errand}
            ]),
        ),
    ));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1["role"], "user");
    assert_eq!(rows[0].1["content"], errand);
}

#[test]
fn a_consult_never_files_an_answer_half() {
    // Not even a sentence of the surface behind the request (a bridge): what
    // the surface said is the surface's, not an answer the core gave.
    let rows = turn_rows(&assemble(
        &[],
        consult_arrival(json!([
            {"origin": "user", "type": "text", "text": REQUEST},
            {"origin": "assistant", "type": "text", "text": "One moment, I am looking into it."}
        ])),
    ));
    assert_eq!(
        rows.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        vec!["c-open-turn"],
        "{rows:?}"
    );
}

/// A plain `in_turn` arrival without a consult on its hop (a duplex turn).
fn plain_arrival(messages: Value) -> Value {
    lane(
        "in_turn",
        json!({"turn_id": "call-1#4"}),
        json!({"turn_id": "call-1#4"}),
        messages,
    )
}

#[test]
fn only_text_after_the_last_question_is_the_answer_half() {
    let before = turn_rows(&assemble(
        &[],
        plain_arrival(json!([
            {"origin": "user", "type": "text", "text": "first question"},
            {"origin": "assistant", "type": "text", "text": OLD_ANSWER},
            {"origin": "user", "type": "text", "text": "how is the weather?"}
        ])),
    ));
    assert_eq!(
        before.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        vec!["c-open-turn"],
        "assistant text before the last question is earlier conversation: {before:?}"
    );

    let after = turn_rows(&assemble(
        &[],
        plain_arrival(json!([
            {"origin": "assistant", "type": "text", "text": OLD_ANSWER},
            {"origin": "user", "type": "text", "text": "how is the weather?"},
            {"origin": "assistant", "type": "text", "text": "Sunny."}
        ])),
    ));
    assert_eq!(
        after
            .iter()
            .map(|(id, r)| (id.as_str(), r["content"].as_str().unwrap_or("")))
            .collect::<Vec<_>>(),
        vec![
            ("c-open-turn", "how is the weather?"),
            ("c-open-pair", "Sunny.")
        ],
        "a duplex pair keeps its answer half (R-25-9): {after:?}"
    );
}

#[test]
fn an_arrival_without_a_question_files_no_answer_half() {
    // Nothing the person said, so nothing follows it: the answer half is the
    // agent text AFTER the person's last turn, and there is none (review of
    // GH #1282, M2 -- the scan used to keep the last assistant text).
    let rows = turn_rows(&assemble(
        &[],
        plain_arrival(json!([
            {"origin": "assistant", "type": "text", "text": "Sunny."}
        ])),
    ));
    assert!(rows.iter().all(|(id, _)| id != "c-open-pair"), "{rows:?}");
}

/// The surface's window is its RENDERED curator window (curator/policy
/// `render`), so beside the spoken texts it carries the curator's artifacts:
/// an answer with the sidecar block it was written with (GH #871) or the mark
/// of a dropped one, a pin broadcast in the person's role, block ids and the
/// released/expired forms of the surface's wall (review of GH #1282, F1).
#[test]
fn the_frame_carries_spoken_text_only() {
    let sidecar_answer =
        "The museum opens at nine.\n\n```sidecar\n{\"fact\": \"museum 9:00\"}\n```";
    let rows = turn_rows(&assemble(
        &[],
        consult_arrival(json!([
            {"origin": "user", "type": "text", "text": "[#0a1b2c3d4e5f] When does the museum open?"},
            {"origin": "assistant", "type": "text", "text": sidecar_answer},
            {"origin": "user", "type": "text",
             "text": "[#1b2c3d4e5f6a] [pinned by ./reminder] Dentist on Friday."},
            {"origin": "user", "type": "text",
             "text": "[#2c3d4e5f6a7b released \u{2014} history_read(\"#2c3d4e5f6a7b\")]"},
            {"origin": "user", "type": "text", "text": "[#3d4e5f6a7b8c expired]"},
            {"origin": "assistant", "type": "text",
             "text": "Tickets are 15 euros.\n\n...[dropped: sidecar (812 chars) over budget]"},
            {"origin": "user", "type": "text", "text": "Plan the visit thoroughly."}
        ])),
    ));
    assert_eq!(rows.len(), 1, "{rows:?}");
    let content = rows[0].1["content"].as_str().unwrap_or("");
    assert_framed(
        content,
        "Plan the visit thoroughly.",
        &[
            "person: When does the museum open?",
            "surface: The museum opens at nine.",
            "surface: Tickets are 15 euros.",
        ],
    );
    for leak in [
        "```sidecar",
        "museum 9:00",
        "dropped: sidecar",
        "[pinned by",
        "Dentist",
        "[#",
        "history_read",
        "expired",
    ] {
        assert!(!content.contains(leak), "{leak:?} leaked into {content:?}");
    }
}

/// Consult n+1 right after consult n: the core's ledger already holds request n
/// and its own answer (GH #889), and the window shows both again -- the
/// request, the advice row with the core's answer, the surface passing it on.
/// None of it is framed a second time (review of GH #1282, F3); what the person
/// said after it is.
#[test]
fn a_second_consult_frames_nothing_the_core_already_holds() {
    let earlier = json!([
        {"origin": "user", "type": "text", "text": "[#4e5f6a7b8c9d] Plan me three days in Athens."},
        {"origin": "assistant", "type": "text", "text": "One moment, I am working on it."},
        {"origin": "user", "type": "text",
         "text": "[#5f6a7b8c9d0e] [advice from your reasoning core, consult k-1]\nDay 1: Acropolis."},
        {"origin": "assistant", "type": "text", "text": "Here is the plan: day 1, the Acropolis."}
    ]);
    let mut direct = earlier.as_array().cloned().unwrap_or_default();
    direct.push(json!({"origin": "user", "type": "text", "text": "Make that four days."}));
    let rows = turn_rows(&assemble(&[], consult_arrival(json!(direct))));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0].1["content"], "Make that four days.",
        "everything in front of the request is in the core's ledger: {rows:?}"
    );

    let mut later = earlier.as_array().cloned().unwrap_or_default();
    later.extend([
        json!({"origin": "user", "type": "text", "text": "Is it hot there in May?"}),
        json!({"origin": "assistant", "type": "text", "text": "Warm, around 25 degrees."}),
        json!({"origin": "user", "type": "text", "text": "Make that four days."}),
    ]);
    let rows = turn_rows(&assemble(&[], consult_arrival(json!(later))));
    let content = rows[0].1["content"].as_str().unwrap_or("");
    assert_framed(
        content,
        "Make that four days.",
        &["Is it hot there in May?", "Warm, around 25 degrees."],
    );
    for held in [
        "Plan me three days",
        "One moment",
        "advice from",
        "Here is the plan",
    ] {
        assert!(
            !content.contains(held),
            "{held:?} framed again: {content:?}"
        );
    }
}

/// The fence rule and the two curator forms are one text with the curator's:
/// two readings of where a block stands would let a block through one door.
#[test]
fn the_window_readers_are_the_curators_text() {
    let script = |rel: &str| -> String {
        config_of(rel)["params"]["script_inline"]
            .as_str()
            .unwrap_or_else(|| panic!("{rel}: no script_inline"))
            .to_string()
    };
    let def = |s: &str, name: &str| -> String {
        let at = s
            .find(&format!("\ndef {name}("))
            .unwrap_or_else(|| panic!("no {name}"));
        let rest = &s[at + 1..];
        let end = rest[1..].find("\ndef ").map_or(rest.len(), |e| e + 1);
        rest[..end].trim_end().to_string()
    };
    let line = |s: &str, start: &str| -> String {
        s.lines()
            .find(|l| l.starts_with(start))
            .unwrap_or_else(|| panic!("no {start}"))
            .to_string()
    };
    let (c, p) = (
        script(ASSEMBLE),
        script("templates/curator/policy/config.json"),
    );
    for name in ["block_span", "stripped_text"] {
        assert_eq!(def(&c, name), def(&p, name), "{name}");
    }
    for start in ["SIDECAR_DROPPED = ", "PIN_FRAME = "] {
        assert_eq!(line(&c, start), line(&p, start), "{start}");
    }
}
