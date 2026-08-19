//! A recall request is a request, whatever chain state its caller carries
//! (GitHub #152).
//!
//! `mem_phase` and `recall_id` are the hive's OWN bookkeeping, and they are
//! *persistent* context: once a consumer has asked memory something, they ride
//! along in everything that consumer emits afterwards. In a tree with two
//! recall consumers — a channel voice and an agent core, both with
//! `memory_tier` set — the first consumer's phase travels into the consult it
//! hands to the second, whose collector then asks this hive with a phase it
//! never set.
//!
//! The echo guard read that as "mid-chain" and parked. No error, no dead
//! letter, no log line: the caller waited for a bundle that would never come.
//! Measured in production as a four-minute silent stall, and invisible in every
//! shipped example, because one recall consumer never produces the constellation.
//!
//! The discriminator is the HOP, not the context: the port edge stamps
//! `phase: "recall"` on a request, and nothing inside the hive ever does. The
//! echo guard keeps working for everything else — which is the second half of
//! this file.

use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string — the substitution the colony performs at instantiation.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn recall_script() -> String {
    let raw = std::fs::read_to_string(RECALL_CONFIG).expect("recall config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run the real script against a real stdin document and return its emission.
fn emit(script: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
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
        "recall exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// A tier-1 request exactly as the port edge delivers it: the query and the
/// tier in both compartments, `phase: "recall"` on the hop, and the asking
/// round the read path is fail-closed on since #244 (`audience_now`,
/// `channel`). The round is the one this file's scenario has -- one person
/// asking, one agent answering, in the room they are in. Not `["*"]`: this
/// file's control test would otherwise pass against a read path with no gate.
fn request(context_extra: serde_json::Value) -> serde_json::Value {
    let mut context = serde_json::json!({
        "recall_query": "radiator valve",
        "memory_tier": "1",
        "session_id": "s1",
        "audience_now": r#"["member:user","agent:assistant"]"#,
        "channel": "c-152"
    });
    for (k, v) in context_extra.as_object().expect("object") {
        context[k] = v.clone();
    }
    serde_json::json!({
        "header": {
            "context": context,
            "hop": {"phase": "recall", "recall_query": "radiator valve", "memory_tier": "1"}
        },
        "messages": [{"origin": "user", "type": "text", "text": ""}]
    })
}

fn phases(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| m["header"]["phase"].as_str().map(str::to_string))
        .collect()
}

// -------------------------------------------------------------------- tests

#[test]
fn a_clean_request_fans_out_as_before() {
    // The control: nothing about this changed, and the file would be worthless
    // without it — a fix that answers every message equally is not a fix.
    let script = recall_script();
    let out = emit(&script, request(serde_json::json!({})));
    assert!(
        !out.is_empty(),
        "a tier-1 request has to open the leg fan, got nothing"
    );
    let ph = phases(&out);
    assert!(
        ph.iter().any(|p| p.starts_with("t1-")),
        "the fan should start with tier-1 legs, got {ph:?}"
    );
}

#[test]
fn a_request_carrying_a_foreign_chain_phase_is_still_answered() {
    // The whole of #152 in one assertion. Same request, plus the bookkeeping a
    // *different* consumer left in the shared context. Before the fix this
    // parked silently and the caller hung.
    let script = recall_script();
    let out = emit(
        &script,
        request(serde_json::json!({
            "mem_phase": "t1-emit",
            "recall_id": "a-chain-that-belongs-to-somebody-else"
        })),
    );
    assert!(
        !out.is_empty(),
        "a request with a foreign mem_phase must still be answered — this is \
         the silent stall of #152"
    );
    let ph = phases(&out);
    assert!(
        ph.iter().any(|p| p.starts_with("t1-")),
        "and it must open its OWN chain, got {ph:?}"
    );
}

#[test]
fn the_foreign_recall_id_is_not_adopted() {
    // Answering is half of it: the new chain must not write into the scratch
    // rows of the chain it inherited the id from, or two consumers fuse into
    // one bundle.
    let script = recall_script();
    let foreign = "a-chain-that-belongs-to-somebody-else";
    let out = emit(
        &script,
        request(serde_json::json!({"mem_phase": "t1-emit", "recall_id": foreign})),
    );
    for m in &out {
        assert_ne!(
            m["header"]["recall_id"].as_str().unwrap_or_default(),
            foreign,
            "the request adopted the caller's recall_id: {m}"
        );
    }
}

#[test]
fn an_echo_without_the_request_hop_still_parks() {
    // The guard this fix had to leave standing. A message that carries a chain
    // phase but did NOT come in through the port is our own emission coming
    // back; answering it would restart the fan on every hop.
    let script = recall_script();
    let echo = serde_json::json!({
        "header": {
            "context": {
                "recall_query": "radiator valve", "memory_tier": "1",
                "mem_phase": "t1-emit", "recall_id": "our-own-chain"
            },
            "hop": {}
        },
        "messages": [{"origin": "user", "type": "text", "text": ""}]
    });
    assert!(
        emit(&script, echo).is_empty(),
        "an echo without the request hop must park, or the fan restarts forever"
    );
}
