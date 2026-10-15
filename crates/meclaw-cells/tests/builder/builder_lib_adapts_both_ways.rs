//! The librarian is REFERENCED, not rewritten. This adapter turns a tool_call
//! into the body shape `builder-librarian` already accepts, and turns its brief
//! back into ONE tool_result under the id that asked. Both directions are
//! recognised POSITIVELY -- the librarian's own comment records what the naive
//! "anything that is not a fresh request" reading cost: a reply-to-fallback
//! loop that spun until the TTL killed it.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};

const LIB: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/lib/config.json"
);

fn run_lib(hop: Value, messages: Value) -> Value {
    emit_one(
        &shipped_script(LIB),
        &json!({
            "target": "/os/builder/lib",
            "header": {"hop": hop, "context": {}},
            "ttl": 64,
            "messages": messages,
        }),
    )
}

#[test]
fn a_search_call_leaves_as_the_shape_the_librarian_accepts() {
    let out = run_lib(
        json!({"tool_name": "librarian_search", "tool_call_id": "c-1"}),
        json!([{"origin": "assistant", "type": "tool_call", "id": "c-1",
                "text": "{\"query\": \"a timer that writes a note\"}"}]),
    );
    assert_eq!(out["header"]["route"], "lsearch_out");
    assert_eq!(out["header"]["lib_call_id"], "c-1");
    assert_eq!(
        out["messages"][0]["origin"], "user",
        "the librarian tokenises the first turn carrying text for its BM25 \
         query -- a json blob there is the query turned into noise"
    );
    assert_eq!(out["messages"][0]["text"], "a timer that writes a note");
}

#[test]
fn a_catalogue_call_asks_the_same_store_for_template_rows_only() {
    let out = run_lib(
        json!({"tool_name": "catalogue_lookup", "tool_call_id": "c-2"}),
        json!([{"origin": "assistant", "type": "tool_call", "id": "c-2",
                "text": "{\"query\": \"summarizer\"}"}]),
    );
    assert_eq!(out["header"]["route"], "lsearch_out");
    assert_eq!(
        out["header"]["lib_kind"], "template",
        "the catalogue is the same corpus filtered to template rows -- the row \
         that opens with CONTRACT is what stops requirement_missing at source"
    );
}

#[test]
fn a_brief_comes_back_as_one_tool_result_under_the_id_that_asked() {
    let out = run_lib(
        json!({"route": "brief", "stage": "briefed", "hits": 3}),
        json!([
            {"origin": "user", "type": "text", "id": "", "text": "a timer"},
            {"origin": "tool", "type": "tool_result", "id": "",
             "text": "### docs -- timer (spec) [d-1]\nthe timer cell fires"}]),
    );
    assert_eq!(out["header"]["operation"], "tool_result");
    assert_eq!(out["messages"].as_array().expect("messages").len(), 1);
    assert_eq!(out["messages"][0]["type"], "tool_result");
    assert!(
        out["messages"][0]["text"]
            .as_str()
            .expect("text")
            .contains("timer cell"),
        "the corpus rows are the result the model asked for"
    );
}

#[test]
fn a_degraded_brief_is_an_observation_and_still_answers() {
    let out = run_lib(
        json!({"route": "brief", "stage": "briefed", "hits": 0, "degraded": true}),
        json!([
            {"origin": "user", "type": "text", "id": "", "text": "a timer"},
            {"origin": "tool", "type": "tool_result", "id": "",
             "text": "(retrieval unavailable: store_error)"}]),
    );
    assert_eq!(out["header"]["operation"], "tool_result");
    assert_eq!(
        out["header"]["degraded"], true,
        "a corpus outage is now a property of ONE ROUND, not of the build -- \
         and the round says so rather than passing it off as zero hits"
    );
    assert!(!out["messages"].as_array().expect("messages").is_empty());
}
