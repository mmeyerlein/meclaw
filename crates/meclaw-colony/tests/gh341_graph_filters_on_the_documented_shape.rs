//! GH #341 — `/colony/graph` filters on the documented request shape.
//!
//! The handler parsed its filter from `body.scope`, while the spec's documented
//! request envelope for every `/colony/*` read — and the one shipped consumer,
//! `templates/canvy` — send it as `{"query": {"scope": …}}`. Neither side
//! errored: the handler found no `scope`, applied no filter, and answered the
//! unfiltered topology. An ignored filter and an empty filter looked alike from
//! the outside.
//!
//! Ruling K-1: the documented `body.query` shape is what the handler reads;
//! `body.scope` survives as a deprecated alias for exactly one release; and a
//! filter that is present but unparseable is a hard error, never an unfiltered
//! answer.
//!
//! **The alias round is over.** It ran through 0.18.0 and the removal is paid
//! here: a top-level `body.scope` is now refused as `invalid_query`, by the
//! same loudness rule that governs an unparseable filter.
//!
//! Spec: `docs/meclaw-overview.en.md` § `/colony` as a virtual endpoint
//! ("Request body form (cell emissions / EDA)", the reads' half).

use meclaw_colony::colony_dispatch::build_graph_read_reply;
use meclaw_colony::edge_table::{Edge, EdgeTable};
use meclaw_colony::{CellStatus, RegistryEntry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{ActorHandle, Path, Uuid};

fn entry(path: &Path) -> RegistryEntry {
    let (sender, _receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
    RegistryEntry {
        handle: ActorHandle::new(path.clone(), sender),
        respawn: Box::new(|| unreachable!("fixture cell is never respawned")),
        wake: None,
        restart_count: 0,
        restart_limit: 5,
        cell_id: Uuid::now_v7(),
        cell_type: "echo".into(),
        status: CellStatus::Awake,
        eager_on_reconnect: true,
        active: true,
        failed: false,
        dormant: false,
        stop_tx: None,
        death_ack_rx: None,
    }
}

/// Two hives, one cell each, one edge inside each hive. Filtered by `/a` the
/// answer is strictly smaller than the unfiltered one — so "the filter applied"
/// and "the filter was dropped" are distinguishable.
fn two_hive_fixture() -> (std::collections::HashMap<Path, RegistryEntry>, EdgeTable) {
    let mut registry = std::collections::HashMap::new();
    let mut edges = EdgeTable::new();
    for hive in ["/a", "/b"] {
        for cell in ["one", "two"] {
            let p = Path::new(&format!("{hive}/{cell}"));
            registry.insert(p.clone(), entry(&p));
        }
        edges.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&format!("{hive}/one")),
            to: Path::new(&format!("{hive}/two")),
            condition: None,
            modifier: None,
            is_default: false,
        });
    }
    (registry, edges)
}

fn node_paths(reply: &Value) -> Vec<String> {
    let mut v: Vec<String> = reply["graph"]["nodes"]
        .as_array()
        .expect("reply carries a nodes array")
        .iter()
        .map(|n| n["path"].as_str().unwrap_or_default().to_string())
        .collect();
    v.sort();
    v
}

fn edge_count(reply: &Value) -> usize {
    reply["graph"]["edges"]
        .as_array()
        .expect("reply carries an edges array")
        .len()
}

/// (a) The documented shape — and the one canvy ships — actually filters.
#[test]
fn the_documented_query_shape_filters_the_topology() {
    let (registry, edges) = two_hive_fixture();

    let unfiltered = build_graph_read_reply(&registry, &edges, &json!({}));
    assert_eq!(
        node_paths(&unfiltered),
        vec!["/a/one", "/a/two", "/b/one", "/b/two"],
        "no filter answers the whole topology"
    );
    assert_eq!(edge_count(&unfiltered), 2);

    let filtered = build_graph_read_reply(&registry, &edges, &json!({"query": {"scope": "/a"}}));
    assert_eq!(
        node_paths(&filtered),
        vec!["/a/one", "/a/two"],
        "body.query.scope is the documented filter and must apply"
    );
    assert_eq!(
        edge_count(&filtered),
        1,
        "edges are filtered with the nodes"
    );
    assert_eq!(
        filtered["graph"]["scope"], "/a",
        "the reply echoes the scope it actually applied"
    );
}

/// (b) The alias is gone. Its one release round was the 0.18.0 round; from now on
/// a top-level `scope` is a refused filter, not a second way to say the same
/// thing. It is refused rather than ignored because of K-1's loudness rule: a
/// caller who still sends the old shape must learn that its filter no longer
/// applies, instead of receiving an unfiltered graph that looks like an answer.
#[test]
fn a_top_level_scope_is_refused_as_invalid_query() {
    let (registry, edges) = two_hive_fixture();

    for body in [
        json!({"scope": "/a"}),                           // the old shape, alone
        json!({"query": {"scope": "/a"}, "scope": "/b"}), // old and new together
    ] {
        let reply = build_graph_read_reply(&registry, &edges, &body);
        assert!(
            reply["graph"]["nodes"].is_null(),
            "a removed filter must not answer a node list: {body} -> {reply}"
        );
        assert_eq!(
            reply["graph"]["status"], "error",
            "the removed top-level scope answers a loud error: {body}"
        );
        assert_eq!(
            reply["graph"]["error_code"], "invalid_query",
            "the error carries the canonical read code: {body}"
        );
        let details = reply["graph"]["details"]
            .as_str()
            .expect("the error names what it refused");
        assert!(
            details.contains("query"),
            "the error names the migration target: {details}"
        );
    }
}

/// A `query` object without a `scope` field is the documented default, not an
/// error: "if `query` or a single field is missing, the defaults apply".
#[test]
fn a_query_without_a_scope_field_is_the_root_default() {
    let (registry, edges) = two_hive_fixture();

    let reply = build_graph_read_reply(&registry, &edges, &json!({"query": {"limit": 5}}));
    assert_eq!(reply["graph"]["scope"], "/");
    assert_eq!(node_paths(&reply).len(), 4);
}

/// (c) A filter that is present but unparseable is a hard error — never the
/// unfiltered graph. This is the loudness rule of K-1: a caller must not be
/// able to mistake "your filter was thrown away" for "here is your answer".
#[test]
fn an_unparseable_filter_is_an_error_not_an_unfiltered_answer() {
    let (registry, edges) = two_hive_fixture();

    for body in [
        json!({"query": "/a"}),          // query as a string
        json!({"query": ["/a"]}),        // query as an array
        json!({"query": {"scope": 7}}),  // scope not a string
        json!({"query": {"scope": []}}), // scope not a string
        json!({"scope": 7}),             // removed top-level alias
    ] {
        let reply = build_graph_read_reply(&registry, &edges, &body);
        assert!(
            reply["graph"]["nodes"].is_null(),
            "an unparseable filter must not answer a node list: {body} -> {reply}"
        );
        assert_eq!(
            reply["graph"]["status"], "error",
            "an unparseable filter answers a loud error: {body}"
        );
        assert_eq!(
            reply["graph"]["error_code"], "invalid_query",
            "the error carries the canonical code: {body}"
        );
        assert!(
            reply["graph"]["details"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "the error names what it could not parse: {body}"
        );
    }
}
