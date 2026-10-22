//! GH #1292 (NF) -- a model that wraps its JSON verdict in a Markdown code
//! fence is read like one that does not.
//!
//! The close pass's closer and the night's dreamer answer with one JSON
//! object. A model that packs it into ```` ```json ```` (or a bare ```` ``` ````)
//! made the close pass end in `closer_failed` and the night in
//! `verdict_not_json`, although the verdict inside was whole. The locks:
//! - a fenced verdict (```` ```json ````, bare ```` ``` ````, one line) is
//!   parked exactly like the same verdict unfenced;
//! - an unfenced verdict is parked as before;
//! - what is no JSON inside a fence or out of one stays a failure
//!   (`salvage|json` in the close pass, `verdict_not_json` at night).

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, shipped_script};
use support::assert_the_declaration_admits;

const CLOSE: &str = "../../templates/memory-hive/close-glue/config.json";
const DREAM: &str = "../../templates/memory-hive/dream-glue/config.json";

fn config(path: &str) -> Value {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path);
    serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json")
}

/// The glue at `path` handed the model answer `text`; what it emits must fit
/// the glue's own declaration whatever the answer was.
fn answer(path: &str, context: Value, text: &str) -> Vec<Value> {
    let out = emit_all(
        &shipped_script(path),
        &json!({
            "params": config(path)["params"].clone(),
            "header": {"context": context, "hop": {"finish_reason": "stop"}},
            "messages": [{"origin": "assistant", "type": "text", "text": text}]
        }),
    );
    assert_the_declaration_admits(path, &out);
    out
}

/// Every tool call the script emitted, with the phase of its message.
fn tool_calls(out: &[Value]) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    for m in out {
        let phase = m["header"]["phase"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        for x in m["messages"].as_array().into_iter().flatten() {
            if x["type"] == "tool_call"
                && let Some(op) = x["text"]
                    .as_str()
                    .and_then(|t| serde_json::from_str::<Value>(t).ok())
            {
                found.push((phase.clone(), op));
            }
        }
    }
    found
}

fn fenced(verdict: &str) -> Vec<(&'static str, String)> {
    vec![
        ("json fence", format!("```json\n{verdict}\n```")),
        ("bare fence", format!("```\n{verdict}\n```")),
        (
            "fence with blanks",
            format!("\n  ```json\n{verdict}\n```  \n"),
        ),
        ("one-line fence", format!("```json {verdict} ```")),
    ]
}

// ──────────────────────────────────────────────── the close pass

fn close_verdict(text: &str) -> Vec<Value> {
    answer(
        CLOSE,
        json!({"mem_phase": "verdict", "session_id": "s-2", "close_group": "0"}),
        text,
    )
}

/// The verdict the close pass parked, or `None` with the phase it took instead.
fn parked_close(out: &[Value]) -> Result<Value, String> {
    let calls = tool_calls(out);
    match calls.as_slice() {
        [(phase, op)] if phase == "verdict-parked" => {
            assert_eq!(op["table"], "scratch", "{out:?}");
            assert_eq!(op["row"]["kind"], "verdict", "{out:?}");
            Ok(
                serde_json::from_str(op["row"]["payload"].as_str().expect("payload"))
                    .expect("payload json"),
            )
        }
        other => Err(format!("{other:?}")),
    }
}

#[test]
fn a_fenced_closer_verdict_is_parked_like_a_bare_one() {
    let verdict = json!({"nothing_to_add": true, "forget": [], "add": []}).to_string();
    let bare = parked_close(&close_verdict(&verdict)).expect("the bare verdict is parked");
    assert_eq!(bare["nothing_to_add"], true);
    assert_eq!(bare["close_group"], 0);
    for (why, text) in fenced(&verdict) {
        assert_eq!(
            parked_close(&close_verdict(&text)),
            Ok(bare.clone()),
            "{why}: {text:?}"
        );
    }
}

#[test]
fn what_is_no_json_stays_a_failed_closer_fence_or_not() {
    for text in [
        "I can't help with that.",
        "```json\nI can't help with that.\n```",
        "```\n{\"forget\": [\n```",
        "```",
    ] {
        let out = close_verdict(text);
        let calls = tool_calls(&out);
        assert_eq!(calls.len(), 1, "{text:?}: {out:?}");
        assert_eq!(calls[0].0, "salvage|json", "{text:?}: {out:?}");
    }
}

// ──────────────────────────────────────────────── the night

fn dream_verdict(text: &str) -> Vec<Value> {
    answer(
        DREAM,
        json!({"store_origin": "dream", "mem_phase": "verdict",
               "dream_run": "r1", "dream_to": "2026-08-12T03:00:00Z"}),
        text,
    )
}

/// The verdicts the night parked, or what it did instead.
fn parked_dream(out: &[Value]) -> Result<Value, String> {
    let calls = tool_calls(out);
    let parked: Vec<&Value> = calls
        .iter()
        .filter(|(_, op)| op["table"] == "scratch" && op["row"]["kind"] == "verdicts")
        .map(|(_, op)| op)
        .collect();
    match parked.as_slice() {
        [op] => Ok(
            serde_json::from_str(op["row"]["payload"].as_str().expect("payload"))
                .expect("payload json"),
        ),
        _ => Err(format!("{calls:?}")),
    }
}

#[test]
fn a_fenced_dreamer_verdict_is_parked_like_a_bare_one() {
    let verdict = json!({"supersede": [], "beliefs": [{"text": "x"}]}).to_string();
    let bare = parked_dream(&dream_verdict(&verdict)).expect("the bare verdict is parked");
    assert_eq!(bare["beliefs"][0]["text"], "x");
    for (why, text) in fenced(&verdict) {
        assert_eq!(
            parked_dream(&dream_verdict(&text)),
            Ok(bare.clone()),
            "{why}: {text:?}"
        );
    }
}

#[test]
fn what_is_no_json_stays_verdict_not_json_at_night() {
    for text in [
        "not json at all",
        "```json\nnot json at all\n```",
        "```\n{\"supersede\": [\n```",
    ] {
        let out = dream_verdict(text);
        assert!(parked_dream(&out).is_err(), "{text:?}: {out:?}");
        assert!(
            tool_calls(&out)
                .iter()
                .any(|(_, op)| op.to_string().contains("verdict_not_json")),
            "{text:?}: {out:?}"
        );
    }
}
