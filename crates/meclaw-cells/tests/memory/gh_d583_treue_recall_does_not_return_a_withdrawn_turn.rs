//! d58-3 treue T2 -- recall does not return a withdrawn turn.
//!
//! Measured (d54g diagnosis, class (b)): a made-up turn of the live voice
//! went into the memory as an episode, and from the next call on recall
//! handed it back ("assistant on ...: ...") although the backend had
//! withdrawn it. The withdrawal reaches the memory as part of a message --
//! the answer that withdrew the turn carries `withdraws`, the turn ids of
//! the withdrawn turn -- never as a read of another cell's data. The writer
//! closes the named own episodes of the session with `closure_source`
//! `withdrawn:<turn of the withdrawing answer>` (the row stays, the log is
//! the audit), and recall's one gate of every leg, the same one a forgotten
//! row meets (GH #1039), shows such a row to nobody. Bare script runs, no
//! colony, no model.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, shipped_script};
use support::*;

const AUDIENCE: &str = r#"["agent:a","member:e"]"#;

fn store_ops(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "wstore")
        .flat_map(|m| m["messages"].as_array().cloned().unwrap_or_default())
        .filter_map(|t| {
            t["text"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
        })
        .collect()
}

fn write(body: Value) -> Vec<Value> {
    let mut doc = json!({
        "header": {"context": {"audience_set": AUDIENCE, "channel": "voice",
                               "session_id": "s-1", "turn_id": "s-1#ab12cd34-3",
                               "agent_id": "agent:a"}},
    });
    for (k, v) in body.as_object().cloned().unwrap_or_default() {
        doc[k] = v;
    }
    emit_all(&shipped_script(WRITER), &doc)
}

#[test]
fn gh_d583_treue_recall_does_not_return_a_withdrawn_turn() {
    if !shipped() {
        return;
    }
    let said =
        json!({"origin": "assistant", "type": "text", "text": "I have to take something back."});
    // Control: an answer without `withdraws` closes nothing.
    let plain = store_ops(&write(json!({"messages": [said.clone()]})));
    assert!(
        plain.iter().all(|o| o["operation"] == "insert"),
        "a plain turn only inserts: {plain:?}"
    );
    let out = write(json!({"messages": [said],
                           "withdraws": ["s-1#ab12cd34-1", "t1"]}));
    let ops = store_ops(&out);
    assert!(
        ops.iter()
            .any(|o| o["operation"] == "insert" && o["table"] == "episodes"),
        "the withdrawing answer is an episode as before: {ops:?}"
    );
    let closes: Vec<&Value> = ops
        .iter()
        .filter(|o| o["operation"] == "update" && o["table"] == "episodes")
        .collect();
    assert_eq!(closes.len(), 2, "one close per named turn: {ops:?}");
    for (op, turn) in closes.iter().zip(["s-1#ab12cd34-1", "t1"]) {
        assert_eq!(
            op["set"]["closure_source"], "withdrawn:s-1#ab12cd34-3",
            "{op}"
        );
        assert_eq!(op["where"]["session_id"], "s-1", "{op}");
        assert_eq!(op["where"]["turn_id"], turn, "{op}");
        assert_eq!(op["where"]["sender"], "assistant", "only own turns: {op}");
    }

    // Recall: the gate every leg passes shows the withdrawn row to nobody
    // and the rows beside it as before.
    let script = shipped_script(RECALL);
    let program = format!(
        "import re, json\nP = json.loads({})\nASKED_WORDS = {{'fishing'}}\nphase = 't1-fan'\nctx = {{'recall_query': 'fishing'}}\ndef visible(a, c):\n    return True\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\nprint(json.dumps([visible_row(r) for r in json.loads({})]))",
        serde_json::to_string(&json!({"question_words": read_json(&repo("templates/memory-hive/recall/config.json"))["params"]["question_words"].clone()}).to_string()).expect("params"),
        block_of(&script, "def _knob"),
        block_of(&script, "def _data"),
        block_of(&script, "QUESTION_WORDS"),
        block_of(&script, "def is_question"),
        block_of(&script, "FORGET_SOURCE"),
        block_of(&script, "PUSH_PREFIX"),
        block_of(&script, "def forgotten"),
        block_of(&script, "LIFTED"),
        block_of(&script, "def muted"),
        block_of(&script, "def names_word"),
        block_of(&script, "def asked_for"),
        block_of(&script, "def visible_row"),
        serde_json::to_string(
            &json!([
                {"closure_source": "withdrawn:s-1#ab12cd34-3", "sender": "assistant"},
                {"closure_source": "", "sender": "assistant"},
                {"sender": "user"}
            ])
            .to_string()
        )
        .expect("rows")
    );
    let run = meclaw_testing::run_shipped_script(&program, "{}");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "[false, true, true]"
    );
}
