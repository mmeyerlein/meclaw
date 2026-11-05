//! GH #1423 (M7-C) -- a counselor's report may say how firmly a fact was
//! meant, and the closer sharpens the status, never the claim.
//!
//! `in_correction` (GH #1323) takes an optional `stance` (and `via` beside
//! `reported`): "stored as a fact, but it was a hope", with the quote. The
//! report carries them to the closer's prompt (a stance that is none of the
//! three is not carried); the closer answers `sharpen` with the shown record's
//! id and the stance, and the run writes ONE update of `stance`/`via` on that
//! open record -- no replacement, no closure -- with the receipt `sharpened`.
//! A record outside the window or a stance that is none of the three writes
//! nothing. Bare script runs, no colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{self, Value, json};
use support::*;

const SAID: &str = "2026-11-05T10:00:00Z";
const QUOTE: &str = "The doctor says the mole is harmless.";

fn run(phase: &str, op: Option<&str>, messages: Value) -> Vec<Value> {
    let mut hop = json!({});
    if let Some(op) = op {
        hop["operation"] = json!(op);
    }
    let out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(CLOSE_GLUE),
        &json!({
            "params": read_json(std::path::Path::new(CLOSE_GLUE))["params"].clone(),
            "header": {"context": {"mem_phase": phase, "audience_set": M7A_AUDIENCE,
                                   "channel": "direct:e"},
                       "hop": hop},
            "messages": messages
        }),
    );
    assert_the_declaration_admits(CLOSE_GLUE, &out);
    out
}

fn report(extra: Value) -> Value {
    let mut r = json!({"subject": "member", "predicate": "has_mole", "quote": QUOTE,
                       "claim": "harmless", "kind": "fact", "turn": "", "session_id": "",
                       "note": "stored as settled, the person passed it on", "ref": "r9"});
    for (k, v) in extra.as_object().expect("extra") {
        r[k] = v.clone();
    }
    r
}

fn row(kind: &str, payload: Value) -> Value {
    json!({"key": "correction:k1", "kind": kind, "created_at": SAID,
           "payload": payload.to_string()})
}

/// The apply phase of run k1 over the window {f-said: "harmless"}.
fn apply(verdict: Value) -> Vec<Value> {
    let window = json!({
        "turn": "",
        "axes": [{"subject": "member", "predicate": "has_mole",
                  "statements": [{"id": "f-said", "claim": "harmless",
                                  "since": SAID, "last_asserted": SAID}]}],
        "records": {"f-said": {"episode_id": M7C_SAID, "valid_from": SAID,
                               "fact_kind": "world", "confidence": 70,
                               "audience_set": M7A_AUDIENCE, "claim": "harmless"}},
        "truncated": false
    });
    let mut parked = report(json!({"stance": "reported", "via": "the doctor"}));
    parked["audience"] = json!(["agent:a", "member:e"]);
    let meeting = json!([
        row("verdict", verdict),
        row("window", window),
        row("report", parked)
    ]);
    run(
        "corr-apply|k1|r9",
        Some("bundle"),
        json!([
            {"origin": "tool", "type": "tool_result", "id": "c-park-verdict", "text": "[]"},
            {"origin": "tool", "type": "tool_result", "id": "c-meet", "text": meeting.to_string()}
        ]),
    )
}

fn receipt(out: &[Value]) -> Value {
    let r: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "correction_report")
        .collect();
    assert_eq!(r.len(), 1, "{out:?}");
    r[0]["header"].clone()
}

#[test]
fn the_report_takes_its_stance_to_the_closer() {
    if !shipped() {
        return;
    }
    // Arrival: the report is parked with its stance and its `via`.
    let body = report(json!({"stance": "Reported", "via": "the doctor"}));
    let out = run(
        "correct",
        None,
        json!([{"origin": "user", "type": "text", "id": "c", "text": body.to_string()}]),
    );
    let parked = out
        .iter()
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .filter_map(|t| {
            t["text"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
        })
        .find(|a| a["row"]["kind"] == "report")
        .unwrap_or_else(|| panic!("no parked report: {out:?}"));
    let parked: Value =
        serde_json::from_str(parked["row"]["payload"].as_str().expect("payload")).expect("json");
    assert_eq!(parked["stance"], "reported", "{parked}");
    assert_eq!(parked["via"], "the doctor", "{parked}");

    // A stance that is none of the three is not carried.
    let body = report(json!({"stance": "certain", "via": "the doctor"}));
    let out = run(
        "correct",
        None,
        json!([{"origin": "user", "type": "text", "id": "c", "text": body.to_string()}]),
    );
    let text = serde_json::to_string(&out).expect("out");
    assert!(!text.contains("certain"), "{text}");

    // The window phase renders it into the closer's prompt.
    let rows = json!({"rows": [m7c_fact("f-said", M7C_SAID, "harmless", "")],
                      "truncated": false, "turn": ""});
    let meeting = json!([row("rows", rows), row("report", parked)]);
    let out = run(
        "corr-window|k1|r9",
        Some("bundle"),
        json!([
            {"origin": "tool", "type": "tool_result", "id": "c-park-rows", "text": "[]"},
            {"origin": "tool", "type": "tool_result", "id": "c-meet", "text": meeting.to_string()}
        ]),
    );
    let ask = out
        .iter()
        .find(|m| m["header"]["route"] == "correct")
        .unwrap_or_else(|| panic!("no closer call: {out:?}"));
    let doc: Value =
        serde_json::from_str(ask["messages"][0]["text"].as_str().expect("text")).expect("json");
    assert_eq!(doc["report"]["stance"], "reported", "{doc}");
    assert_eq!(doc["report"]["via"], "the doctor", "{doc}");
    let rules = ask["system"]["instructions"]["text"]
        .as_str()
        .expect("rules");
    assert!(rules.contains("sharpen"), "{rules}");
}

#[test]
fn a_sharpen_verdict_changes_only_the_stance() {
    if !shipped() {
        return;
    }
    let out = apply(
        json!({"decision": "sharpen", "fact_id": "f-said", "stance": "reported",
                           "via": "the doctor", "claim": "certainly harmless",
                           "reason": "the person passed the doctor's word on"}),
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "close_write"),
        "no replacement: {out:?}"
    );
    let ops = m7c_fact_ops(&out);
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0]["operation"], "update");
    assert_eq!(
        ops[0]["set"],
        json!({"stance": "reported", "via": "the doctor"})
    );
    assert_eq!(ops[0]["where"]["id"], "f-said");
    assert!(ops[0]["where"].get("expired_at").is_some(), "{ops:?}");
    let head = receipt(&out);
    assert_eq!(head["result"], "sharpened", "{head}");
    assert_eq!(head["reason"], "sharpen", "{head}");
    assert_eq!(head["fact_id"], "f-said", "{head}");
    assert_eq!(head["stance"], "reported", "{head}");
    assert_eq!(head["correction_ref"], "r9", "{head}");
}

#[test]
fn a_sharpen_outside_the_window_or_without_a_stance_writes_nothing() {
    if !shipped() {
        return;
    }
    for (verdict, reason) in [
        (
            json!({"decision": "sharpen", "fact_id": "f-elsewhere", "stance": "believed"}),
            "unseen",
        ),
        (
            json!({"decision": "sharpen", "fact_id": "f-said", "stance": "certain"}),
            "no_stance",
        ),
        (
            json!({"decision": "sharpen", "fact_id": "f-said"}),
            "no_stance",
        ),
    ] {
        let out = apply(verdict.clone());
        assert!(m7c_fact_ops(&out).is_empty(), "{verdict}: {out:?}");
        assert!(
            !out.iter().any(|m| m["header"]["route"] == "close_write"),
            "{verdict}: {out:?}"
        );
        let head = receipt(&out);
        assert_eq!(head["result"], "refused", "{verdict}: {head}");
        assert_eq!(head["reason"], reason, "{verdict}: {head}");
    }
}
