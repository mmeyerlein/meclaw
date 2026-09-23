//! What every GH #728 lock needs from `collector/assemble`: the SHIPPED script, run
//! once per message the way a `code` cell runs it, with the shipped params merged
//! under the case's overrides, and the few readers that turn an emission back into
//! the store ops and hop keys a lock measures.
//!
//! One file instead of one copy per lock, the precedent `support/duplex_cell.rs`
//! set. The script goes to python3 on stdin, never in argv (GH #279: a single argv
//! string is capped at 128 KiB and the assembler is past it).
#![allow(dead_code)]

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

pub const ASSEMBLE: &str = "templates/collector/assemble/config.json";
pub const DISPATCHER: &str = "templates/dispatcher/config.json";
pub const ASSISTANT: &str = "templates/assistant/config.json";
pub const TALKY_REF: &str = "templates/assistant/talky/config.json";
pub const TALKY_CHAT_REF: &str = "templates/assistant/talky-chat/config.json";

/// The session every fixture below speaks in.
pub const SESSION: &str = "s1";

pub fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

pub fn config_of(rel: &str) -> Value {
    let p = repo(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The shipped params of a code cell, minus its source, with `over` merged on top.
/// A knob the config does not carry is a failure, so a typo cannot pass as a
/// silently ignored override.
pub fn params_of(rel: &str, over: &[(&str, Value)]) -> Value {
    let mut p = config_of(rel)["params"]
        .as_object()
        .cloned()
        .expect("params object");
    p.remove("script_inline");
    for (k, v) in over {
        assert!(p.contains_key(*k), "{rel}: no such param: {k}");
        p.insert((*k).to_string(), v.clone());
    }
    Value::Object(p)
}

/// Run a shipped code cell over one message. Returns the emissions and stderr.
pub fn run_cell(rel: &str, over: &[(&str, Value)], doc: Value) -> (Vec<Value>, String) {
    let script = config_of(rel)["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let mut doc = doc;
    doc["params"] = params_of(rel, over);
    let stdin_doc = meclaw_testing::code_stdin(&doc).to_string();
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(&script).unwrap(),
        serde_json::to_string(&stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{rel} exited non-zero: {stderr}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{rel}: output is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    let msgs = match v {
        Value::Array(a) => a,
        one => vec![one],
    };
    (msgs, stderr)
}

pub fn assemble(over: &[(&str, Value)], doc: Value) -> Vec<Value> {
    run_cell(ASSEMBLE, over, doc).0
}

/// A lane arrival as the hive's door hands it to `./assemble`.
pub fn lane(route: &str, hop: Value, ctx: Value, messages: Value) -> Value {
    let mut h = json!({"route": route});
    for (k, v) in hop.as_object().expect("hop object") {
        h[k] = v.clone();
    }
    let mut c = json!({"session_id": SESSION});
    for (k, v) in ctx.as_object().expect("ctx object") {
        c[k] = v.clone();
    }
    json!({"header": {"context": c, "hop": h}, "messages": messages})
}

/// A store BUNDLE reply as `./window` hands it back: the phase and the round key
/// in context (the cstore edge promoted both), one leg per `(tool_call_id, rows)`.
pub fn bundle_reply(phase: &str, turn_id: &str, legs: &[(&str, Value)]) -> Value {
    json!({
        "header": {"context": {"session_id": SESSION, "turn_id": turn_id, "iter": "0",
                               "col_phase": phase, "store_origin": "collector"},
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": legs.iter().map(|(id, rows)| json!(
            {"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()}))
            .collect::<Vec<_>>(),
        "results": legs.iter().map(|(id, _)| json!(
            {"tool_call_id": id, "operation": "select", "rows_affected": 1,
             "duration_ms": 0})).collect::<Vec<_>>()
    })
}

/// Every `(tool_call_id, store args)` of one `cstore` emission, in order.
pub fn calls_of(msg: &Value) -> Vec<(String, Value)> {
    msg["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| {
            let id = m["id"].as_str().unwrap_or_default().to_string();
            let args: Value =
                serde_json::from_str(m["text"].as_str().expect("op text")).expect("op json");
            (id, args)
        })
        .collect()
}

/// The store args under one `tool_call_id` of a `cstore` emission.
pub fn call(msg: &Value, id: &str) -> Value {
    calls_of(msg)
        .into_iter()
        .find(|(cid, _)| cid == id)
        .map(|(_, a)| a)
        .unwrap_or_else(|| panic!("no call `{id}` in {msg}"))
}

/// The one emission on `route`.
pub fn on_route<'a>(out: &'a [Value], route: &str) -> &'a Value {
    out.iter()
        .find(|m| m["header"]["route"].as_str() == Some(route))
        .unwrap_or_else(|| panic!("nothing on route `{route}`: {out:?}"))
}

/// The one `cstore` emission of this phase.
pub fn in_phase<'a>(out: &'a [Value], phase: &str) -> &'a Value {
    out.iter()
        .find(|m| {
            m["header"]["route"].as_str() == Some("cstore")
                && m["header"]["phase"].as_str() == Some(phase)
        })
        .unwrap_or_else(|| panic!("no cstore in phase `{phase}`: {out:?}"))
}

pub fn hop_str(msg: &Value, key: &str) -> String {
    msg["header"][key]
        .as_str()
        .unwrap_or_else(|| panic!("the header carries no `{key}`: {msg}"))
        .to_string()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64
}

/// An advisor's answer coming home on `in_advice`, under its consult id.
pub fn advice(consult: &str, text: &str) -> Value {
    lane(
        "in_advice",
        json!({"turn_id": "cogny-round-1"}),
        json!({"consult_id": consult, "turn_id": "cogny-round-1"}),
        json!([{"origin": "assistant", "type": "text", "text": text}]),
    )
}

/// Stage B of an advice: the lookup came back. `departs` is what the depart select
/// found; `rid` and `text` are the advice's own row, read back.
pub fn advice_looked(prov: &str, departs: Value, rid: &str, text: &str, consult: &str) -> Value {
    bundle_reply(
        "advice-look",
        prov,
        &[
            ("c-look-depart", departs),
            ("c-look-turn", Value::Null),
            (
                "c-look-self",
                json!([{"id": rid, "content": text, "consult_id": consult}]),
            ),
        ],
    )
}

/// The brain's answer reaching the collector under a round key.
pub fn answer_in(round_key: &str, text: &str) -> Value {
    lane(
        "in_answer",
        json!({}),
        json!({"turn_id": round_key, "iter": "1"}),
        json!([{"origin": "assistant", "type": "text", "text": text}]),
    )
}

/// Run an advice through both stages and return the round key it opened.
pub fn open_advice(consult: &str, departs: Value) -> (String, Vec<Value>) {
    let a = assemble(&[], advice(consult, "berlin: 21C"));
    let look = in_phase(&a, "advice-look");
    let prov = hop_str(look, "turn_id");
    let row = call(look, "c-look-turn");
    let rid = row["row"]["id"].as_str().expect("row id").to_string();
    let b = assemble(
        &[],
        advice_looked(&prov, departs, &rid, "berlin: 21C", consult),
    );
    let key = hop_str(in_phase(&b, "turn-open"), "turn_id");
    (key, b)
}
