//! GH #359 — the other three `/colony` reads must not drop a filter silently.
//!
//! `/colony/graph` got this discipline in GH #341: a filter that is present but
//! unreadable answers a loud error instead of the unfiltered holdings, because
//! an ignored filter and an empty filter must not look alike from the outside.
//!
//! `/colony/registry`, `/colony/templates` and `/colony/trace` still parsed
//! their filters with `.and_then(|v| v.as_str())` / `.as_bool()` / `.as_u64()`
//! chains that end in `None` on a type mismatch — and `None` means "no filter".
//! `/colony/trace` had the sharper case: `Uuid::parse_str(s).ok()` turned a
//! syntactically broken UUID into "no filter", so a caller asking for one trace
//! got the newest 100 entries of every trace.
//!
//! Absent stays absent: a missing `query`, a missing field, or either of them
//! `null`, is the documented default. And a `limit` out of RANGE is still
//! clamped to 1…1000 — clamped is not dropped.
//!
//! Spec: `docs/meclaw-overview.en.md` § `/colony` as a virtual endpoint
//! ("Request body form (cell emissions / EDA)", the reads' half).

use meclaw_colony::colony_dispatch::{
    build_read_error_reply, build_registry_read_reply, parse_read_query_path_filters,
    parse_read_query_templates_filters, parse_read_query_trace_filters,
};
use meclaw_colony::{CellStatus, RegistryEntry};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{ActorHandle, Path, Uuid};
use std::collections::HashMap;

fn entry(path: &Path, cell_type: &str) -> RegistryEntry {
    let (sender, _receiver) = tokio::sync::mpsc::channel::<meclaw_core::Message>(1);
    RegistryEntry {
        handle: ActorHandle::new(path.clone(), sender),
        respawn: Box::new(|| unreachable!("fixture cell is never respawned")),
        wake: None,
        restart_count: 0,
        restart_limit: 5,
        cell_id: Uuid::now_v7(),
        cell_type: cell_type.into(),
        status: CellStatus::Awake,
        eager_on_reconnect: true,
        active: true,
        failed: false,
        stop_tx: None,
        death_ack_rx: None,
    }
}

/// Four cells in two hives, two cell types — so "filtered" and "unfiltered" are
/// distinguishable answers for every filter the endpoint offers.
fn registry_fixture() -> HashMap<Path, RegistryEntry> {
    let mut registry = HashMap::new();
    for (path, cell_type) in [
        ("/a/one", "echo"),
        ("/a/two", "llm"),
        ("/b/one", "echo"),
        ("/b/two", "llm"),
    ] {
        let p = Path::new(path);
        registry.insert(p.clone(), entry(&p, cell_type));
    }
    registry
}

fn entry_paths(reply: &Value, slot: &str) -> Vec<String> {
    let mut v: Vec<String> = reply[slot]
        .as_array()
        .unwrap_or_else(|| panic!("reply carries a {slot} array, got {reply}"))
        .iter()
        .map(|e| e["path"].as_str().unwrap_or_default().to_string())
        .collect();
    v.sort();
    v
}

/// The shared shape every refused read answers with: the endpoint's own
/// top-level slot, discriminated by `status`, carrying no result list.
fn assert_refused(reply: &Value, slot: &str, ctx: &str) {
    assert!(
        reply[slot].as_array().is_none(),
        "a refused filter must not answer a {slot} list: {ctx} -> {reply}"
    );
    assert_eq!(
        reply[slot]["status"], "error",
        "a refused filter answers a loud error: {ctx}"
    );
    assert_eq!(
        reply[slot]["error_code"], "invalid_query",
        "the error carries the canonical code: {ctx}"
    );
    assert!(
        reply[slot]["details"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the error names what it could not parse: {ctx}"
    );
}

// ------------------------------- /colony/registry ---------------------------

/// Absent is the documented default: no `query`, no field, or `null`, answers
/// everything.
#[test]
fn registry_without_a_filter_answers_everything() {
    let registry = registry_fixture();

    for body in [
        json!({}),
        json!({ "query": null }),
        json!({ "query": {} }),
        json!({ "query": { "cell_type": null, "active": null } }),
    ] {
        let reply = build_registry_read_reply(&registry, &body);
        assert_eq!(
            entry_paths(&reply, "registry"),
            vec!["/a/one", "/a/two", "/b/one", "/b/two"],
            "an absent filter is the documented default: {body}"
        );
    }
}

/// A readable filter actually filters — otherwise the error path would be the
/// only thing this file proves.
#[test]
fn registry_with_a_readable_filter_filters() {
    let registry = registry_fixture();

    let reply = build_registry_read_reply(&registry, &json!({"query": {"path_prefix": "/a"}}));
    assert_eq!(entry_paths(&reply, "registry"), vec!["/a/one", "/a/two"]);

    let reply = build_registry_read_reply(&registry, &json!({"query": {"cell_type": "llm"}}));
    assert_eq!(entry_paths(&reply, "registry"), vec!["/a/two", "/b/two"]);

    let reply = build_registry_read_reply(&registry, &json!({"query": {"active": true}}));
    assert_eq!(entry_paths(&reply, "registry").len(), 4);

    let reply = build_registry_read_reply(&registry, &json!({"query": {"active": false}}));
    assert!(entry_paths(&reply, "registry").is_empty());
}

/// The defect: a wrong-typed filter used to be thrown away and the caller got
/// the whole registry back with no way to tell.
#[test]
fn registry_refuses_a_wrong_typed_filter_instead_of_answering_unfiltered() {
    let registry = registry_fixture();

    for body in [
        json!({"query": "/a"}),                     // query not an object
        json!({"query": ["/a"]}),                   // query not an object
        json!({"query": {"path": 7}}),              // path not a string
        json!({"query": {"path_prefix": []}}),      // path_prefix not a string
        json!({"query": {"cell_type": 7}}),         // cell_type not a string
        json!({"query": {"active": "true"}}),       // active not a bool
        json!({"query": {"limit": "50"}}),          // limit not a number
        json!({"query": {"limit": -1}}),            // negative is not a count
        json!({"query": {"limit": 1.5}}),           // fractional is not a count
        json!({"query": {"active": 1, "path": 2}}), // several at once
    ] {
        let reply = build_registry_read_reply(&registry, &body);
        assert_refused(&reply, "registry", &body.to_string());
    }
}

/// A limit out of RANGE is clamped, not refused — documented behaviour that
/// this issue explicitly leaves alone.
#[test]
fn registry_limit_stays_clamped_not_refused() {
    let registry = registry_fixture();

    let huge = parse_read_query_path_filters(&json!({"query": {"limit": 10_000}}))
        .expect("a numeric limit is readable");
    assert_eq!(huge.limit, 1000, "over the cap is clamped to 1000");

    let zero =
        parse_read_query_path_filters(&json!({"query": {"limit": 0}})).expect("0 is readable");
    assert_eq!(zero.limit, 1, "under the floor is clamped to 1");

    let default = parse_read_query_path_filters(&json!({})).expect("no limit is readable");
    assert_eq!(default.limit, 100, "the documented default stays 100");

    let reply = build_registry_read_reply(&registry, &json!({"query": {"limit": 2}}));
    assert_eq!(
        entry_paths(&reply, "registry").len(),
        2,
        "an in-range limit still applies"
    );
}

// ------------------------------- /colony/templates --------------------------

#[test]
fn templates_without_a_filter_answers_everything() {
    for body in [json!({}), json!({"query": null}), json!({"query": {}})] {
        let f = parse_read_query_templates_filters(&body)
            .unwrap_or_else(|e| panic!("absent is the default: {body} -> {e:?}"));
        assert_eq!(f.cell_type, None);
        assert_eq!(f.name, None);
        assert_eq!(f.limit, 100);
    }
}

#[test]
fn templates_with_a_readable_filter_filters() {
    let f = parse_read_query_templates_filters(&json!({
        "query": { "cell_type": "store", "name": "memory-hive", "limit": 7 }
    }))
    .expect("a readable filter parses");
    assert_eq!(f.cell_type.as_deref(), Some("store"));
    assert_eq!(f.name.as_deref(), Some("memory-hive"));
    assert_eq!(f.limit, 7);
}

#[test]
fn templates_refuses_a_wrong_typed_filter_instead_of_answering_unfiltered() {
    for body in [
        json!({"query": 7}),
        json!({"query": {"cell_type": 7}}),
        json!({"query": {"name": []}}),
        json!({"query": {"limit": "50"}}),
    ] {
        let err = parse_read_query_templates_filters(&body)
            .expect_err(&format!("a wrong-typed filter is refused: {body}"));
        assert_refused(
            &build_read_error_reply("templates", &err),
            "templates",
            &body.to_string(),
        );
    }
}

// ------------------------------- /colony/trace ------------------------------

#[test]
fn trace_without_a_filter_answers_everything() {
    for body in [json!({}), json!({"query": null}), json!({"query": {}})] {
        let f = parse_read_query_trace_filters(&body)
            .unwrap_or_else(|e| panic!("absent is the default: {body} -> {e:?}"));
        assert_eq!(f.trace_id, None);
        assert_eq!(f.correlation_id, None);
        assert_eq!(f.path_prefix, None);
        assert!(!f.only_error);
        assert_eq!(f.since, None);
        assert_eq!(f.limit, 100);
    }
}

#[test]
fn trace_with_a_readable_filter_filters() {
    let id = Uuid::now_v7();
    let corr = Uuid::now_v7();
    let f = parse_read_query_trace_filters(&json!({
        "query": {
            "trace_id": id.to_string(),
            "correlation_id": corr.to_string(),
            "path_prefix": "/a",
            "only_error": true,
            "since": 1_700_000_000_i64,
            "limit": 5,
        }
    }))
    .expect("a readable filter parses");
    assert_eq!(f.trace_id, Some(id));
    assert_eq!(f.correlation_id, Some(corr));
    assert_eq!(f.path_prefix.as_ref().map(|p| p.as_str()), Some("/a"));
    assert!(f.only_error);
    assert_eq!(f.since, Some(1_700_000_000));
    assert_eq!(f.limit, 5);
}

#[test]
fn trace_refuses_a_wrong_typed_filter_instead_of_answering_unfiltered() {
    for body in [
        json!({"query": "abc"}),
        json!({"query": {"trace_id": 7}}),
        json!({"query": {"correlation_id": []}}),
        json!({"query": {"path_prefix": 7}}),
        json!({"query": {"only_error": "true"}}),
        json!({"query": {"since": "yesterday"}}),
        json!({"query": {"since": 1.5}}), // a fractional second is not a timestamp
        json!({"query": {"limit": "50"}}),
        json!({"query": {"limit": 1.5}}),
    ] {
        let err = parse_read_query_trace_filters(&body)
            .expect_err(&format!("a wrong-typed filter is refused: {body}"));
        assert_refused(
            &build_read_error_reply("trace", &err),
            "trace",
            &body.to_string(),
        );
    }
}

/// The sharper case this issue names: a syntactically broken UUID has the right
/// JSON type and used to become "no filter" — a caller asking for ONE trace got
/// the newest 100 entries of EVERY trace back.
#[test]
fn a_broken_uuid_is_refused_not_turned_into_no_filter() {
    for body in [
        json!({"query": {"trace_id": "not-a-uuid"}}),
        json!({"query": {"trace_id": ""}}),
        json!({"query": {"trace_id": "0198e5aa-0000-7000-8000"}}), // truncated
        json!({"query": {"correlation_id": "not-a-uuid"}}),
    ] {
        let err = parse_read_query_trace_filters(&body)
            .expect_err(&format!("a broken UUID is refused: {body}"));
        assert!(
            err.details.contains("UUID") || err.details.contains("uuid"),
            "the error says the UUID is the problem: {body} -> {}",
            err.details
        );
        assert_refused(
            &build_read_error_reply("trace", &err),
            "trace",
            &body.to_string(),
        );
    }
}
