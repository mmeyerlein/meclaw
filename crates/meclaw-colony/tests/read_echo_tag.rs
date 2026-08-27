//! A `/colony` read answers into a FRESH trace: new `trace_id`,
//! `parent_message_id` null, `correlation_id` null (`docs/roadmap.md`
//! § Trace-Bruch am `/colony`-Reply). A cell that asks twice can therefore not
//! tell the two answers apart — unless it puts something into the question that
//! comes back verbatim. `/colony/ledger` has had exactly that since GH #267;
//! these tests give the same thing to `/colony/graph` and `/colony/registry`,
//! because a builder that reads them needs concurrency more than it needs one
//! read in flight (the GH #361 lease is what the alternative costs).

use meclaw_colony::colony_dispatch::{build_graph_read_reply, build_registry_read_reply};
use meclaw_core::serde_json::{Value, json};
use std::collections::HashMap;

fn empty_graph_reply(body: &Value) -> Value {
    let registry = HashMap::new();
    let edges = meclaw_colony::edge_table::EdgeTable::default();
    build_graph_read_reply(&registry, &edges, body)
}

#[test]
fn a_graph_read_echoes_the_callers_tag() {
    let reply = empty_graph_reply(&json!({"query": {"scope": "/", "tag": "b7#1"}}));
    assert_eq!(
        reply["graph"]["tag"], "b7#1",
        "the tag must come back verbatim — it is the whole memory of the round"
    );
}

#[test]
fn a_graph_read_without_a_tag_carries_no_tag_slot() {
    let reply = empty_graph_reply(&json!({"query": {"scope": "/"}}));
    assert!(
        reply["graph"].get("tag").is_none(),
        "absent stays absent; an empty string is a value somebody can match on"
    );
}

#[test]
fn a_graph_tag_is_truncated_and_never_refused() {
    let long = "x".repeat(200);
    let reply = empty_graph_reply(&json!({"query": {"tag": long}}));
    assert_eq!(
        reply["graph"]["tag"].as_str().expect("tag echoed").len(),
        64,
        "clamped is not dropped — a tag never touches the data, so shortening \
         it cannot change the answer, and an unbounded one is a growth hazard"
    );
}

#[test]
fn a_wrong_typed_graph_tag_is_refused_like_every_other_filter() {
    let reply = empty_graph_reply(&json!({"query": {"tag": 7}}));
    assert_eq!(
        reply["graph"]["error_code"], "invalid_query",
        "GH #341/#359: a filter present but unreadable is refused, never dropped"
    );
}

fn empty_registry_reply(body: &Value) -> Value {
    let registry = HashMap::new();
    build_registry_read_reply(&registry, body)
}

#[test]
fn a_registry_read_echoes_the_callers_tag_beside_the_list() {
    let reply = empty_registry_reply(&json!({"query": {"path_prefix": "/os", "tag": "b7#2"}}));
    assert!(
        reply["registry"].is_array(),
        "the answer slot stays a list — the echo is a sibling, never an entry"
    );
    assert_eq!(reply["tag"], "b7#2");
}

#[test]
fn a_registry_read_without_a_tag_carries_no_tag_slot() {
    let reply = empty_registry_reply(&json!({"query": {"path_prefix": "/os"}}));
    assert!(reply.get("tag").is_none());
}

#[test]
fn a_registry_tag_is_truncated_and_never_refused() {
    let reply = empty_registry_reply(&json!({"query": {"tag": "y".repeat(200)}}));
    assert_eq!(reply["tag"].as_str().expect("tag echoed").len(), 64);
}

#[test]
fn a_wrong_typed_registry_tag_is_refused_like_every_other_filter() {
    let reply = empty_registry_reply(&json!({"query": {"tag": false}}));
    assert_eq!(reply["registry"]["error_code"], "invalid_query");
}
