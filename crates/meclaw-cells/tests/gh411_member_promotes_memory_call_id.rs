//! GH #411 — the member's recall edges promote `memory_call_id` into context.
//!
//! `collector/assemble` correlates a returning bundle over
//! `context.memory_call_id`: set means "this is the tool_result of a
//! `memory_recall` call", empty means "this is the turn's ambient memory leg"
//! (`templates/collector/README.md`, the correlation contract). The caller
//! puts the id in the HOP — and a hop does not survive the memory-hive,
//! because the `recall` cell forms its own hop. Only context travels.
//!
//! So the promotion has to happen on the member's edges into
//! `./memory-hive`, next to `recall_query` and the window pair. Before this
//! test, both edges promoted everything the recall shape names EXCEPT the
//! call id: the bundle came back correct and uncorrelatable, the collector
//! filed it as the ambient leg, and the brain saw "tool result lost" — the
//! shipped four-level composition had a working memory it could not use in
//! conversation.
//!
//! Facts about the FILE, checked with no colony and no runtime, in the
//! `gh302_org_is_a_namespace` style: read the shipped template off the tree,
//! so that dropping the promotion goes red here instead of losing a tool
//! result two minutes later in a live round.

use serde_json::Value;

const MEMBER_CONFIG: &str = "../../templates/member/config.json";

fn member_config() -> Value {
    let raw = std::fs::read_to_string(MEMBER_CONFIG).expect("read templates/member/config.json");
    serde_json::from_str(&raw).expect("member config is JSON")
}

/// The edges into `./memory-hive` that rewrite a recall into the hive's own
/// `in_query` — the two doors #411 found without the promotion.
fn recall_edges(config: &Value) -> Vec<&Value> {
    config["params"]["graph"]["edges"]
        .as_array()
        .expect("member declares params.graph.edges")
        .iter()
        .filter(|e| {
            e["to"] == "./memory-hive"
                && e["modifier"]["set_hop"]["route"]
                    .as_str()
                    .is_some_and(|r| r.contains("in_query"))
        })
        .collect()
}

#[test]
fn both_recall_doors_promote_the_call_id_into_context() {
    let config = member_config();
    let edges = recall_edges(&config);
    assert_eq!(
        edges.len(),
        2,
        "the member has exactly two recall doors into ./memory-hive \
         (from ./assistants on 'recall', from . on 'in_recall'): {edges:#?}"
    );
    for edge in edges {
        let promoted = edge["modifier"]["set_context"]["memory_call_id"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "edge {} -> ./memory-hive must promote memory_call_id \
                     into set_context (GH #411): {edge:#}",
                    edge["from"]
                )
            });
        assert!(
            promoted.contains("hop.memory_call_id"),
            "the promotion reads the id off the hop, where the caller put it \
             (got {promoted:?})"
        );
        assert!(
            promoted.contains("has(hop.memory_call_id)"),
            "an absent id must promote as empty — empty is the ambient leg, \
             and a CEL error on a missing key would kill the lane instead \
             (got {promoted:?})"
        );
    }
}
