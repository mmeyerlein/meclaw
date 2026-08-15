//! meclaw-os -- the dispatcher template `dispatcher@1` (start-Egon wave, track D).
//!
//! A brain answers a turn either with a final text or with a BUNDLE of tool
//! calls. Splitting that bundle is routing work, not assembly work (R-OS-2:
//! fan-out is the dispatcher's, fan-in is the collector's), and this file pins
//! the three claims that make the split safe:
//!
//! 1. PLAIN ORDER -- the assistant turn reaches the collector's expectation
//!    lane BEFORE any tool message leaves, so a fast tool can never report a
//!    result for a round the collector has not yet been told about.
//! 2. NO TOPOLOGY IN THE CELL -- a tool call leaves as `hop.tool_name` plus
//!    `hop.tool_call_id` and nothing else. Which cell that name belongs to is
//!    an edge's decision, and the colony half of this file proves it by routing
//!    two calls to two different cells through plain CEL conditions.
//! 3. NO SILENT DROP -- a bundle that exceeds the per-answer call budget, and a
//!    call the cell cannot even parse, are answered with a SYNTHETIC error
//!    `tool_result` that keeps the original `tool_call_id`. The round stays
//!    fan-in-complete, so the brain sees the failure and has to respond to it.
//!
//! The script half runs the shipped `params.script_inline` against real stdin
//! documents; the colony half boots the shipped template file. Nothing is
//! mocked and no provider is called.

use std::io::Write;
use std::process::{Command, Stdio};

#[path = "support_14b.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use support::{boot, recv_bounded};
use tokio::sync::mpsc;

const SPLIT_CONFIG: &str = "../../templates/dispatcher/config.json";

// ======================================================================= SCRIPT

/// `${VAR:-default}` becomes the default (or the override, when the case names
/// one), a bare `${VAR}` becomes the empty string -- the same substitution the
/// colony performs when it instantiates the template.
fn resolve_vars(script: &str, over: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        let inner = &tail[..end];
        let (name, default) = match inner.split_once(":-") {
            Some((n, d)) => (n, d),
            None => (inner, ""),
        };
        let value = over
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
            .unwrap_or(default);
        out.push_str(value);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn split_script(over: &[(&str, &str)]) -> String {
    let raw = std::fs::read_to_string(SPLIT_CONFIG).expect("dispatcher config");
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
        over,
    )
}

/// Runs the real script against a real stdin document and returns the emitted
/// messages.
fn emit_with(over: &[(&str, &str)], doc: Value) -> Vec<Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(split_script(over))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&meclaw_testing::code_stdin_bytes(&doc))
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "split exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn emit(doc: Value) -> Vec<Value> {
    emit_with(&[], doc)
}

/// One OpenAI-form tool call as the `llm` cell emits it: the turn `text` is the
/// stringified `function` object (`{name, arguments}`).
fn call_turn(id: &str, name: &str, args: &str) -> Value {
    json!({"origin": "assistant", "type": "tool_call", "id": id,
           "text": json!({"name": name, "arguments": args}).to_string()})
}

/// A brain answer as the edge from the llm delivers it: `finish_reason` on the
/// hop, the assistant turn(s) in the body.
fn brain_doc(finish: &str, messages: Vec<Value>) -> Value {
    json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0"},
                   "hop": {"finish_reason": finish}},
        "messages": messages
    })
}

fn hop_str(msg: &Value, key: &str) -> String {
    msg["header"][key].as_str().unwrap_or_default().to_string()
}

fn route_of(msg: &Value) -> String {
    hop_str(msg, "route")
}

fn turns_of(msg: &Value) -> &Vec<Value> {
    msg["messages"].as_array().expect("messages array")
}

#[test]
fn a_two_call_bundle_splits_into_three_emissions_in_plain_order() {
    let out = emit(brain_doc(
        "tool_calls",
        vec![
            call_turn("c1", "web_search", "{\"q\":\"alpha\"}"),
            call_turn("c2", "web_fetch", "{\"url\":\"beta\"}"),
        ],
    ));

    assert_eq!(
        out.len(),
        3,
        "one expectation set plus one message per call"
    );
    // PLAIN order: the collector learns the expectation set FIRST. A tool that
    // answers instantly must not arrive before the round exists.
    assert_eq!(
        route_of(&out[0]),
        "calls",
        "the assistant turn leads: {out:?}"
    );
    assert_eq!(turns_of(&out[0]).len(), 2, "both calls travel together");

    assert_eq!(route_of(&out[1]), "tool");
    assert_eq!(hop_str(&out[1], "tool_name"), "web_search");
    assert_eq!(hop_str(&out[1], "tool_call_id"), "c1");
    assert_eq!(
        turns_of(&out[1])[0]["text"],
        "{\"q\":\"alpha\"}",
        "the tool gets the RAW arguments, not the OpenAI wrapper"
    );
    assert_eq!(
        turns_of(&out[1])[0]["id"],
        "c1",
        "the id survives the split"
    );

    assert_eq!(route_of(&out[2]), "tool");
    assert_eq!(hop_str(&out[2], "tool_name"), "web_fetch");
    assert_eq!(hop_str(&out[2], "tool_call_id"), "c2");
    assert_eq!(turns_of(&out[2])[0]["text"], "{\"url\":\"beta\"}");
}

#[test]
fn the_calls_lane_carries_the_assistant_turn_verbatim() {
    // A brain may put a sentence next to its calls. That turn is part of the
    // assistant turn and travels with it; it is NOT a call and never becomes a
    // tool message.
    let said = json!({"origin": "assistant", "type": "text", "text": "looking it up"});
    let out = emit(brain_doc(
        "tool_calls",
        vec![call_turn("c1", "web_search", "{}"), said.clone()],
    ));

    // Three since R-CG-3: the calls lane, the interim answer the sentence
    // becomes, and the call itself.
    assert_eq!(out.len(), 3, "{out:?}");
    assert_eq!(route_of(&out[0]), "calls");
    assert_eq!(
        turns_of(&out[0])[1],
        said,
        "the text turn reaches the collector unchanged"
    );
    assert_eq!(route_of(&out[2]), "tool");
}

// ────────────────────────────────────────────── the advisor split (GH #28, R-CG-3)

#[test]
fn a_sentence_next_to_a_bundle_also_leaves_on_the_answer_lane_at_once() {
    // Delta 1 of R-CG-3: ONE brain response carries the interim answer AND the
    // calls. The text must reach the channel in the same turn that asks the
    // tool -- a second inference for "one moment" is exactly the extra turn the
    // advisor split abolishes.
    let said = json!({"origin": "assistant", "type": "text", "text": "one moment, asking"});
    let out = emit(brain_doc(
        "tool_calls",
        vec![call_turn("c1", "consult_cogny", "{}"), said.clone()],
    ));

    assert_eq!(out.len(), 3, "calls, the interim answer, the call: {out:?}");
    assert_eq!(route_of(&out[0]), "calls", "PLAIN order is unchanged");
    assert_eq!(route_of(&out[1]), "answer", "the sentence travels at once");
    assert_eq!(
        hop_str(&out[1], "interim"),
        "1",
        "an interim answer says so, so a channel can tell it from a final one"
    );
    assert_eq!(
        turns_of(&out[1]),
        &vec![said],
        "the interim answer carries the TEXT only, never the calls"
    );
    assert_eq!(route_of(&out[2]), "tool", "the call runs regardless");
}

#[test]
fn a_bundle_without_a_sentence_says_nothing_to_the_channel() {
    let out = emit(brain_doc("tool_calls", vec![call_turn("c1", "t", "{}")]));
    assert_eq!(out.len(), 2, "no interim answer is invented: {out:?}");
    assert!(out.iter().all(|m| route_of(m) != "answer"), "{out:?}");
}

#[test]
fn an_async_call_opens_no_fan_in_expectation_and_carries_its_consult_id() {
    // Delta 2 of R-CG-3: the dispatcher is the only cell that sees the whole
    // bundle, so it is the only cell that can classify one. It does not swallow
    // the call -- it NAMES it on the expectation lane, and the collector reads
    // that name instead of keeping its own list.
    let out = emit_with(
        &[("DISPATCHER_ASYNC_TOOLS", "consult_cogny")],
        brain_doc(
            "tool_calls",
            vec![
                call_turn(
                    "c1",
                    "consult_cogny",
                    r#"{"question":"weather?","eta":"seconds"}"#,
                ),
                call_turn("c2", "web_search", "{}"),
            ],
        ),
    );

    assert_eq!(out.len(), 3, "{out:?}");
    assert_eq!(route_of(&out[0]), "calls");
    assert_eq!(
        hop_str(&out[0], "async_calls"),
        "c1",
        "the fan-in is told WHICH ids it must not wait for: {out:?}"
    );

    assert_eq!(hop_str(&out[1], "tool_name"), "consult_cogny");
    assert_eq!(hop_str(&out[1], "async"), "1");
    assert_eq!(
        hop_str(&out[1], "consult_id"),
        "c1",
        "a fresh consult is correlated by the call that opened it"
    );
    // GH #123, observe-only: the estimate the model produced in the SAME turn
    // travels on the hop and nothing reads it.
    assert_eq!(hop_str(&out[1], "consult_eta"), "seconds");

    assert_eq!(hop_str(&out[2], "tool_name"), "web_search");
    assert_eq!(hop_str(&out[2], "async"), "", "a sync tool is untouched");
    assert_eq!(hop_str(&out[2], "consult_id"), "");
}

#[test]
fn without_the_knob_no_call_is_async() {
    let out = emit(brain_doc(
        "tool_calls",
        vec![call_turn("c1", "consult_cogny", "{}")],
    ));
    assert_eq!(hop_str(&out[0], "async_calls"), "", "default is empty");
    assert_eq!(hop_str(&out[1], "async"), "");
}

#[test]
fn a_reply_to_an_open_consult_keeps_the_correlation_id_it_was_given() {
    // The bilateral half: the advisor asked back, the user answered, and the
    // model passes the id it was shown. Without this the reply would open a
    // SECOND consult and the advisor would have to guess the thread.
    let out = emit_with(
        &[("DISPATCHER_ASYNC_TOOLS", "consult_cogny")],
        brain_doc(
            "tool_calls",
            vec![call_turn(
                "c9",
                "consult_cogny",
                r#"{"consult_id":"k-7","answer":"Berlin"}"#,
            )],
        ),
    );
    assert_eq!(hop_str(&out[1], "consult_id"), "k-7");
    assert_eq!(
        hop_str(&out[1], "tool_call_id"),
        "c9",
        "the call id stays the call id -- the two are different keys"
    );
    assert_eq!(hop_str(&out[0], "async_calls"), "c9");
}

#[test]
fn a_stop_turn_passes_through_on_the_answer_lane() {
    let turn = json!({"origin": "assistant", "type": "text", "text": "42"});
    let out = emit(brain_doc("stop", vec![turn.clone()]));

    assert_eq!(out.len(), 1, "exactly one answer emission: {out:?}");
    assert_eq!(route_of(&out[0]), "answer");
    assert_eq!(hop_str(&out[0], "finish_reason"), "stop");
    assert_eq!(turns_of(&out[0])[0], turn, "the answer is passed through");
}

#[test]
fn a_bundle_over_the_call_budget_answers_every_call_with_a_synthetic_error() {
    let calls: Vec<Value> = (0..17)
        .map(|i| call_turn(&format!("c{i}"), "web_search", "{}"))
        .collect();
    let out = emit(brain_doc("tool_calls", calls));

    // Default budget is 16, the bundle asks for 17: nothing runs, but every
    // expected id is answered, so the round stays fan-in-complete.
    assert_eq!(out.len(), 18, "the assistant turn plus 17 results: {out:?}");
    assert_eq!(route_of(&out[0]), "calls");
    assert!(
        out.iter().all(|m| hop_str(m, "tool_name").is_empty()),
        "no tool message leaves at all: {out:?}"
    );
    for (i, m) in out[1..].iter().enumerate() {
        assert_eq!(route_of(m), "result");
        assert_eq!(hop_str(m, "tool_call_id"), format!("c{i}"));
        assert_eq!(hop_str(m, "error_code"), "call_budget_exceeded");
        let turn = &turns_of(m)[0];
        assert_eq!(turn["origin"], "tool");
        assert_eq!(turn["type"], "tool_result");
        assert_eq!(turn["id"], format!("c{i}"), "the call id is preserved");
        assert!(
            turn["text"]
                .as_str()
                .unwrap_or_default()
                .contains("call budget exceeded"),
            "the brain must be able to read WHY: {turn}"
        );
    }
}

#[test]
fn the_budget_is_a_knob_and_a_bundle_at_the_cap_still_runs() {
    let two = || {
        vec![
            call_turn("c1", "web_search", "{}"),
            call_turn("c2", "web_fetch", "{}"),
        ]
    };
    let at_cap = emit_with(
        &[("DISPATCHER_MAX_CALLS", "2")],
        brain_doc("tool_calls", two()),
    );
    assert_eq!(at_cap.len(), 3, "the cap is a ceiling, not a barrier");
    assert_eq!(route_of(&at_cap[1]), "tool");

    let mut three = two();
    three.push(call_turn("c3", "web_search", "{}"));
    let over = emit_with(
        &[("DISPATCHER_MAX_CALLS", "2")],
        brain_doc("tool_calls", three),
    );
    assert_eq!(
        over.len(),
        4,
        "one over the cap: the whole bundle is refused"
    );
    assert!(
        over[1..].iter().all(|m| route_of(m) == "result"),
        "{over:?}"
    );
}

#[test]
fn a_malformed_tool_call_becomes_a_synthetic_error_result() {
    let broken = json!({"origin": "assistant", "type": "tool_call", "id": "c2",
                        "text": "not json at all"});
    let out = emit(brain_doc(
        "tool_calls",
        vec![call_turn("c1", "web_search", "{}"), broken],
    ));

    assert_eq!(out.len(), 3, "{out:?}");
    assert_eq!(route_of(&out[1]), "tool", "the sound call still runs");
    assert_eq!(hop_str(&out[1], "tool_call_id"), "c1");
    // An unroutable call would stall the fan-in forever; it is answered instead.
    assert_eq!(route_of(&out[2]), "result");
    assert_eq!(hop_str(&out[2], "tool_call_id"), "c2");
    assert_eq!(hop_str(&out[2], "error_code"), "malformed_tool_call");
    assert_eq!(turns_of(&out[2])[0]["id"], "c2");
}

#[test]
fn a_turn_that_is_neither_a_bundle_nor_an_answer_is_terminal() {
    // The llm error path echoes the INPUT conversation back with
    // finish_reason "error". Forwarding that onto the answer lane would write
    // the whole prompt into the window as if the agent had said it, so the
    // dispatcher stops instead -- an error lane belongs on the brain's edge.
    let out = emit(brain_doc(
        "error",
        vec![json!({"origin": "user", "type": "text", "text": "hello"})],
    ));
    assert!(
        out.is_empty(),
        "empty multi-send, terminal by design: {out:?}"
    );
}

// ======================================================================= COLONY

/// The shipped template, copied cell by cell: only `config.json` files travel,
/// so the cell under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

fn template_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/dispatcher")
}

/// A brain that asks for two tools on the first word and answers otherwise.
/// A `code` cell rather than a model: the round has to be measurable.
const BRAIN: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
txt = " ".join(str(m.get("text", "")) for m in d.get("messages", []))
def call(i, name, arg):
    return {"origin": "assistant", "type": "tool_call", "id": "c%d" % i,
            "text": json.dumps({"name": name, "arguments": json.dumps({"q": arg})})}
if txt.startswith("tools"):
    out = {"header": {"finish_reason": "tool_calls"},
           "messages": [call(1, "web_search", "alpha"), call(2, "web_fetch", "beta")]}
else:
    out = {"header": {"finish_reason": "stop"},
           "messages": [{"origin": "assistant", "type": "text", "text": "answer:" + txt}]}
sys.stdout.write(json.dumps(out))
"#;

/// A fake tool: echoes its own name and the raw arguments it was handed, so a
/// mis-routed call is visible in the result rather than merely absent.
fn tool_script(name: &str) -> String {
    format!(
        r#"
import sys, json
d = json.load(sys.stdin)["body"]
m = (d.get("messages") or [{{}}])[0]
sys.stdout.write(json.dumps({{"header": {{"route": "res"}},
    "messages": [{{"origin": "tool", "type": "tool_result", "id": m.get("id", ""),
                  "text": "{name}:" + str(m.get("text", ""))}}]}}))
"#
    )
}

/// A `code` cell config with the contract the substrate validates against.
fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({});
    if !routes.is_empty() {
        hop["route"] = json!({"type": "string", "values": routes, "required": false});
    }
    if let Some(obj) = extra_hop.as_object() {
        for (k, v) in obj {
            hop[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for a colony that exercises the dispatcher ports.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// The wiring a parent draws around the dispatcher. `/sink` stands in for the
/// collector: it drains the expectation lane, the synthetic results and the
/// answer, exactly as the collector's `in_calls` / `in_tool` / `in_answer`
/// ports would. The tool lanes are plain CEL on `hop.tool_name` -- the cell
/// itself knows neither cell.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./brain", "to": "./split", "condition": "has(hop.finish_reason)"},
        {"from": "./split", "to": "/sink", "condition": "has(hop.route) && hop.route == 'calls'"},
        {"from": "./split", "to": "/sink", "condition": "has(hop.route) && hop.route == 'result'"},
        {"from": "./split", "to": "/sink", "condition": "has(hop.route) && hop.route == 'answer'"},
        {"from": "./split", "to": "./search",
         "condition": "has(hop.tool_name) && hop.tool_name == 'web_search'"},
        {"from": "./split", "to": "./fetch",
         "condition": "has(hop.tool_name) && hop.tool_name == 'web_fetch'"},
        {"from": "./search", "to": "/sink", "condition": "has(hop.route) && hop.route == 'res'"},
        {"from": "./fetch", "to": "/sink", "condition": "has(hop.route) && hop.route == 'res'"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, env: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), env).unwrap();
    write(root, "main/config.json", &main_config());
    copy_cells(&template_dir(), &root.join("main/split"));
    let finish = json!({"finish_reason": {"type": "string",
                                          "values": ["stop", "tool_calls"], "required": true}});
    write(
        root,
        "main/brain/config.json",
        &code_cell(BRAIN, &[], finish),
    );
    write(
        root,
        "main/search/config.json",
        &code_cell(&tool_script("search"), &["res"], json!({})),
    );
    write(
        root,
        "main/fetch/config.json",
        &code_cell(&tool_script("fetch"), &["res"], json!({})),
    );
}

fn turn(text: &str) -> Message {
    MessageBuilder::new(Path::new("/brain"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .ttl(64)
        .build()
}

/// Collects `n` messages from the collector stub as (route, first turn text).
async fn drain(rx: &mut mpsc::Receiver<Message>, n: usize) -> Vec<(String, String)> {
    let mut got = Vec::new();
    for _ in 0..n {
        let m = recv_bounded(rx)
            .await
            .unwrap_or_else(|| panic!("only {} of {n} messages arrived: {got:?}", got.len()));
        let route = m
            .headers
            .hop
            .get("route")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let text = match &m.body {
            Body::Inline(v) => v["messages"][0]["text"].as_str().unwrap_or("").to_string(),
            Body::Blob(_) => panic!("inline expected"),
        };
        got.push((route, text));
    }
    got
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bundle_fans_out_by_name_and_the_round_comes_back_complete() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "");
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(turn("tools please")).await;
    let got = drain(&mut sink_rx, 3).await;

    // One expectation set and two results. The two calls went to two DIFFERENT
    // cells, and each one got its own arguments -- the routing decision lives
    // in the edge conditions, not in the cell.
    let routes: Vec<&str> = got.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(
        routes.iter().filter(|r| **r == "calls").count(),
        1,
        "exactly one expectation set: {got:?}"
    );
    let texts: Vec<&str> = got.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("search:") && t.contains("alpha")),
        "the search call reached the search cell with its own arguments: {got:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("fetch:") && t.contains("beta")),
        "the fetch call reached the fetch cell with its own arguments: {got:?}"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_turn_reaches_the_answer_lane_and_runs_no_tool() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "");
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(turn("just answer")).await;
    let got = drain(&mut sink_rx, 1).await;

    assert_eq!(got[0].0, "answer", "{got:?}");
    assert_eq!(got[0].1, "answer:just answer", "{got:?}");
    assert!(
        recv_bounded_short(&mut sink_rx).await.is_none(),
        "no tool ran and nothing else was emitted"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bundle_over_the_budget_keeps_the_round_complete_without_running_a_tool() {
    let td = tempfile::TempDir::new().unwrap();
    // One call per answer: the two-call bundle of this brain is over budget.
    build_tree(&td, "DISPATCHER_MAX_CALLS=1\n");
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(turn("tools please")).await;
    let got = drain(&mut sink_rx, 3).await;

    assert_eq!(
        got.iter().filter(|(r, _)| r == "calls").count(),
        1,
        "the round is still announced: {got:?}"
    );
    assert_eq!(
        got.iter().filter(|(r, _)| r == "result").count(),
        2,
        "every expected call is answered: {got:?}"
    );
    assert!(
        got.iter()
            .all(|(_, t)| !t.starts_with("search:") && !t.starts_with("fetch:")),
        "no tool ran: {got:?}"
    );

    h.shutdown().await;
}

/// A short bounded receive for "nothing more arrives" assertions. The 30 s of
/// `recv_bounded` are a failure marker; this one is a semantic discriminator
/// and stays tight on purpose (the tool leg of this tree is two hops of local
/// python, an order of magnitude below it).
async fn recv_bounded_short(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
        .await
        .ok()
        .flatten()
}
