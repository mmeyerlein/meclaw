//! GH #1036 -- a sidecar block the model writes BESIDE a tool call never
//! reaches a channel or an app.
//!
//! Measured in a running colony (one talky turn, trace `01a10fc7`): the brain
//! completed a `web_search` call and, beside it, a one-sentence interim answer
//! (that it is looking up the current time in Berlin) followed by a fenced ```sidecar block.
//! The splitter handed the tool round on byte for byte (`splitter` ->
//! `dispatcher` unchanged), the dispatcher sent the sentence out as the interim
//! answer (`interim=1`), and the block travelled `collector` -> `talky` -> the
//! member's channels and apps (display, ambient, voice2vision) with its fence.
//!
//! The repair is in `talky/splitter` (contract 1.0.4): every text turn of a
//! tool round is cut by the same grammar that cuts an answer -- the fence, the
//! legacy fence, a bare fence with a legacy payload, and the unfenced object,
//! the section form `{"memory": {...}}` included (the form GH #871 fix round 1
//! found leaking on the answer half). The round is still never taken apart (GH
//! #378): the calls stay byte for byte and the turns keep their order; the
//! block rides beside the round as `sidecar_raw` when it reads, as an answer's
//! does (GH #871).
//!
//! What this file pins:
//!
//! 1. at the splitter, every form the grammar knows leaves the sentence beside
//!    a call, the call untouched, the readable block beside it as written;
//! 2. a broken block leaves too, and no nothing form is laid over it;
//! 3. a turn that was nothing but the block leaves the round, and the
//!    dispatcher sends no interim answer for it;
//! 4. AT THE RECEIVER: a person's turn through the shipped member road (every
//!    `llm` on a local stub, `support/gh929_member_road.rs`), the brain calling
//!    the member's `memory_recall` with a sentence and a block beside the call
//!    -- fenced, and in the unfenced section form. Every message the assistants
//!    container hands on a lane the member routes to its channels or its apps
//!    (read off `templates/member/config.json`) carries no block, and the
//!    interim answer itself did arrive.
//!
//! Guarded like every template-reading test (GH #49).

#[path = "support/assemble_cell.rs"]
mod assemble_cell;
#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/gh929_member_road.rs"]
mod road;

use assemble_cell::{DISPATCHER, run_cell};
use meclaw_core::serde_json::{Value, json};
use mock_openai::{MockOpenAI, canned_chat_completion, canned_content_and_tool_calls};
use road::Stubs;
use std::collections::{BTreeSet, HashMap};

const SPLITTER: &str = "templates/talky/splitter/config.json";

const BESIDE_CALL: &str = "Let me look that up.";
const FENCED: &str = "```sidecar\n{\"memory\":{\"nothing_new\":true,\"facts\":[],\
     \"topic\":{\"movement\":\"continue\"}}}\n```";
/// The section form written without its fence (GH #871 fix round 1).
const UNFENCED: &str =
    "{\"memory\": {\"nothing_new\": true, \"facts\": [], \"topic\": {\"movement\": \"continue\"}}}";
const CONSULT_CALL: &str =
    "{\"name\":\"consult_cogny\",\"arguments\":\"{\\\"question\\\":\\\"what time\\\"}\"}";

/// What marks a block in a string a reader receives: the fence, the legacy
/// payload's keys, the section key of the unfenced form.
const MARKERS: [&str; 4] = ["```", "nothing_new", "\"facts\"", "\"memory\""];

fn split_round(text: &str) -> (Value, Value) {
    let input = json!({"header": {"hop": {"finish_reason": "tool_calls"}, "context": {}},
                       "messages": [
                           {"origin": "assistant", "type": "tool_call", "id": "call_c1",
                            "text": CONSULT_CALL},
                           {"origin": "assistant", "type": "text", "text": text}]});
    let (out, _) = run_cell(SPLITTER, &[], input.clone());
    assert_eq!(out.len(), 1, "a tool round leaves as ONE message: {out:?}");
    (input, out.into_iter().next().expect("one message"))
}

/// Every string in `v` that carries a block marker, and every `sidecar_raw`.
fn leaks(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            if MARKERS.iter().any(|m| s.contains(m)) {
                out.push(s.clone());
            }
        }
        Value::Array(a) => a.iter().for_each(|x| leaks(x, out)),
        Value::Object(o) => {
            for (k, x) in o {
                if k == "sidecar_raw" {
                    out.push(format!("sidecar_raw: {x}"));
                }
                leaks(x, out);
            }
        }
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────── 1-3: the splitter

/// Claim 1: every form the grammar knows leaves the sentence beside the call;
/// the call is untouched, the turns keep their order, no section reaches a
/// lane, and the readable block rides beside the round exactly as written.
#[test]
fn every_form_of_the_block_leaves_the_sentence_beside_a_call() {
    let forms: &[(&str, &str)] = &[
        ("fenced sidecar", FENCED),
        (
            "legacy fence",
            "```memory\n{\"nothing_new\": true, \"facts\": []}\n```",
        ),
        (
            "bare json fence",
            "```json\n{\"nothing_new\": true, \"facts\": []}\n```",
        ),
        ("unfenced section form", UNFENCED),
        (
            "naked legacy object",
            "{\"nothing_new\": true, \"facts\": []}",
        ),
    ];
    for (name, block) in forms {
        let (input, half) = split_round(&format!("{BESIDE_CALL}\n\n{block}"));
        let turns = half["messages"].as_array().expect("messages");
        assert_eq!(turns.len(), 2, "{name}: the round keeps its turns: {half}");
        assert_eq!(
            turns[0], input["messages"][0],
            "{name}: the call is untouched: {half}"
        );
        assert_eq!(
            turns[1]["text"], BESIDE_CALL,
            "{name}: the sentence leaves bare: {half}"
        );
        assert_eq!(
            half["sidecar_raw"], *block,
            "{name}: the model's own block rides beside it: {half}"
        );
        assert_eq!(half["header"]["finish_reason"], "tool_calls", "{half}");
        assert!(
            half["header"].get("route").is_none() && half["header"].get("sidecar").is_none(),
            "{name}: no section of a tool round reaches a lane: {half}"
        );
    }
}

/// Claim 2: a block that does not read leaves the sentence as well (the leak
/// class of GH #534), and no nothing form is laid over the model's attempt.
#[test]
fn a_broken_block_beside_a_call_leaves_too_and_gets_no_second_one() {
    let broken = "```sidecar\n{\"memory\": {\"nothing_new\": true\n```";
    let (input, half) = split_round(&format!("{BESIDE_CALL}\n\n{broken}"));
    assert_eq!(half["messages"][0], input["messages"][0], "{half}");
    assert_eq!(half["messages"][1]["text"], BESIDE_CALL, "{half}");
    assert!(half.get("sidecar_raw").is_none(), "{half}");
}

/// Claim 3: a text turn that was nothing but the block leaves the round, so
/// the dispatcher has no sentence to send and no interim answer goes out.
#[test]
fn a_turn_that_was_only_the_block_sends_no_interim() {
    let (input, half) = split_round(FENCED);
    assert_eq!(
        half["messages"],
        json!([input["messages"][0].clone()]),
        "only the call is left: {half}"
    );
    let mut doc = json!({"header": {"hop": half["header"].clone(), "context": {}},
                         "messages": half["messages"].clone()});
    if let Some(raw) = half.get("sidecar_raw") {
        doc["sidecar_raw"] = raw.clone();
    }
    let (out, _) = run_cell(
        DISPATCHER,
        &[("async_tools", json!(["consult_cogny"]))],
        doc,
    );
    assert!(
        out.iter().all(|m| m["header"]["route"] != "answer"),
        "no interim answer for a block alone: {out:?}"
    );
}

// ─────────────────────────────────────────────────── 4: at the receiver

const BRAIN: &str = "assistants/scribe/talky/brain";
const CALL_ID: &str = "call_1036";
const RECALL_ARGS: &str = r#"{"query":"the probe"}"#;

/// The lanes the member hands from its assistants to a channel or an app,
/// read off the shipped member's own edges.
fn reader_lanes() -> BTreeSet<String> {
    let member = road::read_json(&road::repo("templates/member/config.json"));
    let mut lanes = BTreeSet::new();
    for e in member["params"]["graph"]["edges"]
        .as_array()
        .expect("member edges")
    {
        let (from, to) = (
            e["from"].as_str().unwrap_or_default(),
            e["to"].as_str().unwrap_or_default(),
        );
        if from == "./assistants" && (to.starts_with("./channels") || to.starts_with("./apps")) {
            let cond = e["condition"].as_str().unwrap_or_default();
            for part in cond.split("hop.route == '").skip(1) {
                if let Some(end) = part.find('\'') {
                    lanes.insert(part[..end].to_string());
                }
            }
        }
    }
    lanes
}

/// One logged delivery with its hop and body.
struct Row {
    from: String,
    to: String,
    hop: Value,
    body: Value,
}

fn rows(root: &std::path::Path) -> Vec<Row> {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut st = conn
        .prepare("SELECT from_path, to_path, headers, body_payload FROM message_log ORDER BY rowid")
        .expect("message_log");
    st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .map(|(from, to, headers, body)| {
        let h: Value = meclaw_core::serde_json::from_str(&headers).unwrap_or(Value::Null);
        let body = body
            .map(|b| meclaw_core::serde_json::from_str(&b).unwrap_or(Value::String(b)))
            .unwrap_or(Value::Null);
        Row {
            from,
            to,
            hop: h["hop"].clone(),
            body,
        }
    })
    .collect()
}

/// A person's turn through the member road whose brain calls the member's
/// memory with `beside` next to the call, then answers plainly.
async fn a_round_with(beside: &str) -> Vec<Row> {
    let brain = MockOpenAI::start(vec![
        canned_content_and_tool_calls(beside, vec![(CALL_ID, "memory_recall", RECALL_ARGS)]),
        canned_chat_completion(road::REPLY, "stop"),
    ])
    .await;
    let background =
        MockOpenAI::start(vec![canned_chat_completion(road::BACKGROUND_REPLY, "stop")]).await;
    let stubs = Stubs {
        scripted: HashMap::from([(BRAIN.to_string(), brain.base_url.clone())]),
        background: background.base_url.clone(),
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    let pointed = road::build_member(&td, &stubs);
    assert!(
        pointed.iter().any(|c| c == BRAIN),
        "{BRAIN} is an llm cell of the road: {pointed:?}"
    );
    let (h, mut ports) = road::boot(&td).await;
    h.send(road::person(
        "talky",
        "What did I say about the probe?",
        true,
        0,
    ))
    .await;
    // The interim answer and the final one; a missing one is read below.
    let mut answered = 0;
    while answered < 2 {
        match tokio::time::timeout(road::DEADLINE, ports.sink.recv()).await {
            Ok(Some(_)) => answered += 1,
            _ => break,
        }
    }
    h.shutdown().await;
    rows(td.path())
}

async fn assert_no_block_reaches_a_reader(form: &str, block: &str) {
    if !road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let lanes = reader_lanes();
    assert!(
        lanes.contains("answer"),
        "the member hands answers to its channels: {lanes:?}"
    );
    let log = a_round_with(&format!("{BESIDE_CALL}\n\n{block}")).await;
    let handed: Vec<&Row> = log
        .iter()
        .filter(|r| {
            r.from == "/assistants" && r.hop["route"].as_str().is_some_and(|l| lanes.contains(l))
        })
        .collect();
    let say = |r: &Row| format!("{} -> {} {} {}", r.from, r.to, r.hop, r.body);
    assert!(
        handed.iter().any(|r| r.hop["route"] == "answer"
            && r.hop["interim"] == "1"
            && r.body.to_string().contains(BESIDE_CALL)),
        "{form}: the interim answer reached the member's door:\n{}",
        handed.iter().map(|r| say(r)).collect::<Vec<_>>().join("\n")
    );
    for r in &handed {
        let mut found = Vec::new();
        leaks(&r.body, &mut found);
        assert!(
            found.is_empty(),
            "{form}: a message for a channel or an app carries the block: {found:?}\n{}",
            say(r)
        );
    }
}

/// Claim 4, the fenced form: the trace of the issue, at the receiver.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fenced_block_beside_a_call_never_reaches_a_channel_or_an_app() {
    assert_no_block_reaches_a_reader("fenced", FENCED).await;
}

/// Claim 4, the unfenced section form (GH #871 fix round 1), at the receiver.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unfenced_block_beside_a_call_never_reaches_a_channel_or_an_app() {
    assert_no_block_reaches_a_reader("unfenced", UNFENCED).await;
}
