//! GH #425 — the two new occupants of the tool surface, and the one thing they
//! both have to get right: the round they opened is the round they close.
//!
//! The fan-in (`collector`) waits for exactly the `tool_call_id` the brain
//! issued. `hop` is a ONE-HOP compartment and there are eight hops between the
//! tool surface and the builder, so the id cannot ride there. It travels in the
//! CONTEXT — the same solution the librarian uses for `orig_request` and canvy
//! for its own correlation — stamped by the door of this hive on the way out and
//! read by the return edge.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};

const BUILD: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/tools/build/config.json"
);
const APPLY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/tools/apply/config.json"
);

fn phase_a(script: &str, tool_name: &str, call_id: &str, args: &str) -> Value {
    emit_one(
        script,
        &json!({
            "target": "/os/orgs/acme/members/alex/assistants/scribe/tools/build",
            "header": {"hop": {"route": "tool_call", "tool_name": tool_name,
                               "tool_call_id": call_id},
                       "context": {}},
            "ttl": 64,
            "messages": [{"origin": "assistant", "type": "tool_call", "id": call_id,
                          "text": args}],
        }),
    )
}

fn phase_b(script: &str, call_id: &str, hop: Value, body: Value) -> Value {
    let mut flat = json!({
        "target": "/os/orgs/acme/members/alex/assistants/scribe/tools/build",
        "header": {"hop": hop, "context": {"build_call_id": call_id}},
        "ttl": 64,
    });
    for (k, v) in body.as_object().expect("body object") {
        flat[k] = v.clone();
    }
    emit_one(script, &flat)
}

#[test]
fn the_tool_call_id_travels_in_context_and_comes_back_on_the_result() {
    let script = shipped_script(BUILD);
    let a = phase_a(
        &script,
        "build_topology",
        "call-7",
        r#"{"request":"build me a digest"}"#,
    );
    assert_eq!(a["header"]["operation"], json!("build_request"));
    assert_eq!(a["header"]["build_op"], json!("draft"));
    assert_eq!(
        a["messages"][0]["text"],
        json!(r#"{"request":"build me a digest"}"#),
        "the arguments travel verbatim — this cell does not read them"
    );

    let b = phase_b(
        &script,
        "call-7",
        json!({"route": "in_build_result", "build_op": "draft",
               "manifest_sha256": "abc123", "manifest_class": "fast"}),
        json!({"manifest": [{"scope": "/os", "ctx": {}, "diff": {}}], "messages": []}),
    );
    assert_eq!(b["header"]["operation"], json!("build"));
    assert_eq!(
        b["messages"][0]["id"],
        json!("call-7"),
        "a tool_result under the wrong id is a round the fan-in waits out"
    );
    let text = b["messages"][0]["text"].as_str().expect("a turn");
    assert!(
        text.contains("This is a DRAFT"),
        "the model has to be told, in the only place it looks, that nothing has \
         been applied: {text}"
    );
    assert!(
        text.contains("abc123"),
        "the digest travels down with the draft"
    );
}

#[test]
fn a_build_that_came_back_as_an_error_is_still_a_tool_result() {
    let b = phase_b(
        &shipped_script(BUILD),
        "call-7",
        json!({"route": "in_build_result", "build_op": "draft",
               "error_code": "no_manifest_in_answer"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                             "text": "{\"reason\": \"the answer carried no balanced json object\"}"}]}),
    );
    assert_eq!(b["header"]["operation"], json!("build"));
    assert_eq!(b["header"]["error_code"], json!("no_manifest_in_answer"));
    let text = b["messages"][0]["text"].as_str().expect("a turn");
    assert!(
        text.contains("no_manifest_in_answer"),
        "the model has to be able to tell the human WHY, and it only sees the turn"
    );
    assert_eq!(b["messages"][0]["id"], json!("call-7"));
}

#[test]
fn the_apply_tool_carries_the_manifest_out_without_touching_it() {
    let manifest = json!([{"scope": "/os", "ctx": {},
                           "diff": {"add_edges": [{"from": "./a", "to": "./b"}]}}]);
    let args = json!({"manifest": manifest, "manifest_sha256": "deadbeef"});
    let a = phase_a(
        &shipped_script(APPLY),
        "apply_manifest",
        "c2",
        &args.to_string(),
    );
    assert_eq!(a["header"]["operation"], json!("apply_request"));
    assert_eq!(a["header"]["build_op"], json!("apply"));
    assert_eq!(a["header"]["manifest_sha256"], json!("deadbeef"));
    assert_eq!(
        a["manifest"], manifest,
        "any re-render, re-order or re-serialise changes the bytes, and the \
         submitter would refuse its own colony's draft"
    );
}

#[test]
fn an_apply_without_a_manifest_is_answered_here_and_not_sent_upstairs() {
    let a = phase_a(
        &shipped_script(APPLY),
        "apply_manifest",
        "c2",
        r#"{"note":"do it"}"#,
    );
    assert_eq!(a["header"]["operation"], json!("apply"));
    assert_eq!(a["header"]["error_code"], json!("manifest_missing"));
    assert_eq!(a["messages"][0]["id"], json!("c2"));
    assert!(
        a.get("manifest").is_none(),
        "a call without a manifest has nothing to do at the OS level, and the \
         answer belongs in the same round"
    );
}

#[test]
fn the_apply_receipt_comes_back_under_the_id_of_the_second_call() {
    let b = phase_b(
        &shipped_script(APPLY),
        "c2",
        json!({"route": "in_build_result", "build_op": "apply", "applied": 2,
               "failed_at": 3, "remaining": 2, "error_code": "scope_containment"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                             "text": "manifest refused at position 3: scope_containment (2 applied, 2 untouched)"}]}),
    );
    assert_eq!(b["header"]["operation"], json!("apply"));
    assert_eq!(b["header"]["applied"], json!(2));
    assert_eq!(b["header"]["failed_at"], json!(3));
    assert_eq!(b["messages"][0]["id"], json!("c2"));
    assert!(
        b["messages"][0]["text"]
            .as_str()
            .expect("a turn")
            .contains("2 applied"),
        "the partial state survives the last hop too — it is the only thing the \
         model can tell the human about what is live now"
    );
}
