//! The two eyes that make the measured failure classes visible: the ISLAND
//! (activity is edge-derived, so `graph_read` SHOWS it instead of describing
//! it) and the wrong template choice (`registry_read` says what actually stands
//! where). Both answer into a fresh trace, so both carry a tag -- the whole
//! memory of the round, exactly as the steward's probe carries one.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_one, shipped_script};

const EYES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/eyes/config.json"
);

/// The flat spelling `meclaw_testing::code_stdin` splits into the substrate's
/// three-object document: `header` moves into the envelope, every other key is
/// a body slot. A pre-built `{"envelope": …, "body": …}` would land whole in
/// the body and the script would read an empty envelope -- a green test for the
/// wrong reason.
fn run_eyes(hop: Value, ctx: Value, body: Value) -> Value {
    let mut flat = json!({"header": {"hop": hop, "context": ctx}});
    let slots = flat.as_object_mut().expect("object");
    if let Value::Object(o) = body {
        for (k, v) in o {
            slots.insert(k, v);
        }
    }
    emit_one(&shipped_script(EYES), &flat)
}

#[test]
fn a_graph_read_leaves_with_an_empty_messages_list_and_a_query() {
    let out = run_eyes(
        json!({"tool_name": "graph_read", "tool_call_id": "c-3"}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0"}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "c-3",
                             "text": "{\"scope\": \"/os/orgs/acme\"}"}]}),
    );
    assert_eq!(out["header"]["route"], "graph");
    assert_eq!(
        out["messages"].as_array().expect("messages").len(),
        0,
        "a UBF body needs one of system/messages/attachments, and an empty \
         list is a valid one -- the question travels under `query`"
    );
    assert_eq!(out["query"]["scope"], "/os/orgs/acme");
    assert_eq!(
        out["query"]["tag"], "b7.0.0#c-3",
        "the tag is the whole memory: the answer is a fresh message with no \
         context of ours, so ALL THREE coordinates ride in it -- the fan-in \
         keys its slate on (build_id, iter) and the re-entry edge reads \
         context.iter and context.repairs"
    );
}

#[test]
fn a_registry_read_leaves_on_its_own_route() {
    let out = run_eyes(
        json!({"tool_name": "registry_read", "tool_call_id": "c-4"}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0"}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "c-4",
                             "text": "{\"path_prefix\": \"/os/orgs\", \"active\": true}"}]}),
    );
    assert_eq!(out["header"]["route"], "registry");
    assert_eq!(out["query"]["path_prefix"], "/os/orgs");
    assert_eq!(out["query"]["active"], true);
    assert_eq!(out["query"]["tag"], "b7.0.0#c-4");
}

/// The fix the suite case `A1` forced: the round coordinate comes BACK.
///
/// `weave` reads `build_id`, `iter` and `repairs` out of the context, and the
/// re-entry edge `./weave -> ./compose` reads `context.iter`. A `/colony` reply
/// carries neither, so without these three hop keys the fan-in filed the answer
/// under an empty build at round zero, found no expectation set, and parked --
/// the loop stopped dead after the first look, silently. The keys are always
/// present, empty rather than absent, because the edge that puts them back into
/// the context is a CEL modifier and a missing key makes a modifier SKIP.
#[test]
fn an_answer_carries_the_whole_round_coordinate_back_in_its_hop() {
    let out = run_eyes(
        json!({}),
        json!({}),
        json!({"graph": {"scope": "/", "graph_version": 0, "tag": "b7.3.1#c-9",
                         "nodes": [], "edges": []}}),
    );
    assert_eq!(out["header"]["build_id"], "b7");
    assert_eq!(out["header"]["iter"], "3");
    assert_eq!(out["header"]["repairs"], "1");
    assert_eq!(out["header"]["tool_call_id"], "c-9");
}

/// An ISLAND is a property of a NODE, not of a scope.
///
/// The first spelling said it only when a scope had no edge at all, which is
/// the one shape the finding is worthless in: a real colony read at a real
/// scope has edges, and the unreached cell is then exactly the one a reader
/// would have to find by hand in the dump. `A1` reads scope `/` of a colony
/// with 107 edges and the island has to survive that.
#[test]
fn an_island_is_named_even_in_a_scope_that_has_edges() {
    let out = run_eyes(
        json!({}),
        json!({}),
        json!({"graph": {"scope": "/", "graph_version": 0, "tag": "b7.0.0#c-3",
                         "nodes": [{"path": "/request", "cell_type": "code"},
                                   {"path": "/capture", "cell_type": "code"},
                                   {"path": "/island", "cell_type": "code"}],
                         "edges": [{"from": "/request", "to": "/capture"}]}}),
    );
    let text = out["messages"][0]["text"].as_str().expect("text");
    assert!(
        text.contains("ISLAND") && text.contains("/island"),
        "the unreached node is NAMED, not left for the reader to find: {text}"
    );
    assert!(
        !text.contains("ISLAND and would be born inactive: /request"),
        "a node an edge names is not an island: {text}"
    );
}

#[test]
fn a_graph_answer_becomes_one_tool_result_under_the_echoed_id() {
    let out = run_eyes(
        json!({}),
        json!({}),
        json!({"graph": {"scope": "/os/orgs/acme", "graph_version": 0, "tag": "b7#c-3",
                         "nodes": [{"path": "/os/orgs/acme/collector", "cell_type": "hive"}],
                         "edges": []}}),
    );
    assert_eq!(out["header"]["operation"], "tool_result");
    assert_eq!(
        out["header"]["tool_call_id"], "c-3",
        "the call id is read back out of the echo -- there is nowhere else it \
         could have survived the roundtrip"
    );
    let text = out["messages"][0]["text"].as_str().expect("text");
    assert!(text.contains("/os/orgs/acme/collector"));
    assert!(
        text.contains("edges") || text.contains("no edge"),
        "an empty edge list is the ISLAND, and it has to be legible as one"
    );
}

#[test]
fn a_registry_answer_becomes_one_tool_result_under_the_echoed_id() {
    let out = run_eyes(
        json!({}),
        json!({}),
        json!({"registry": [{"path": "/os/orgs/acme/store", "cell_id": "u-1",
                             "cell_type": "hive", "lifecycle_status": "Awake",
                             "active": false, "failed": false}],
               "tag": "b7#c-4"}),
    );
    assert_eq!(out["header"]["operation"], "tool_result");
    assert_eq!(out["header"]["tool_call_id"], "c-4");
    let text = out["messages"][0]["text"].as_str().expect("text");
    assert!(text.contains("/os/orgs/acme/store"));
    assert!(
        text.contains("false") || text.contains("inactive"),
        "`active: false` is the finding -- a reader must not have to guess it"
    );
}

#[test]
fn a_refused_read_is_named_rather_than_read_as_an_empty_answer() {
    let out = run_eyes(
        json!({}),
        json!({}),
        json!({"graph": {"status": "error", "error_code": "invalid_query",
                         "details": "`query.scope` must be a path string, found number"}}),
    );
    assert_eq!(out["header"]["operation"], "tool_result");
    assert!(
        out["messages"][0]["text"]
            .as_str()
            .expect("text")
            .contains("invalid_query"),
        "NAME the code in the turn -- a refusal the model cannot name is one it \
         cannot correct"
    );
}
