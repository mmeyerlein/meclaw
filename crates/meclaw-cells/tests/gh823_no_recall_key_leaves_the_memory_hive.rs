//! GH #823 — no recall working key leaves the memory hive.
//!
//! A question reaches the memory hive's `recall` cell as five context keys:
//! `recall_query`, `memory_tier`, `recall_as_of`, `recall_window_from` and
//! `recall_window_to`. They are set twice — by the caller's door (the member's
//! `in_query` promotion, as the hive's contract asks) and by the hive itself on
//! `./tool -> ./recall` — and before 3.4.1 nothing removed them again. Context
//! lives for the whole chain, so the question rode on the answer past the
//! hive's rim, into the asker, and on into whatever the asker did next.
//!
//! The repair is the rule `docs/development-rules.md` § 8c states: the hive
//! deletes them on every one of its exit edges. `delete_context` removes a key
//! whoever set it (`cel_eval.rs`, it runs after `set_*`), so one list per exit
//! covers the ambient road (member-set) and the tool road (hive-set) alike.
//!
//! What this file proves, measured at the receiver through the real router over
//! the shipped files (the `gh562` harness, `apply_edges` from
//! `meclaw_colony::edge_table`):
//!
//! 1. **Every exit deletes the five** — statically, all of them, with a floor so
//!    a graph that lost its exits cannot pass by having none.
//! 2. **An ambient recall** reaches the hive carrying the five (the positive
//!    probe — a key that is never set proves nothing when it is absent) and its
//!    bundle reaches the asker without them.
//! 3. **A tool recall** sets the five inside the hive, and the `tool_result`
//!    leaves without them.
//! 4. **A refusal** leaves the same way an answer does.
//!
//! The five stay in `SHARED` of `gh494_no_interior_marker_leaves_a_hive.rs`:
//! the member sets them on purpose to talk to the hive, so they are shared
//! between the two levels, and this file — not that list — holds the rim.

use std::collections::BTreeSet;

use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{Map, Value};
use meclaw_core::{Headers, Path, Uuid};

const MEMBER: &str = "/m";
const HIVE: &str = "/m/memory-hive";
const BOX: &str = "/m/assistants";
const GEN: &str = "/m/assistants/scribe";

/// The five keys a question is made of (the hive's `in_query` contract).
const RECALL_KEYS: [&str; 5] = [
    "memory_tier",
    "recall_as_of",
    "recall_query",
    "recall_window_from",
    "recall_window_to",
];

// ───────────────────────────────────────────────────────────────── the harness

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(rel: &str) -> Value {
    let p = repo(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn hive_params(rel: &str) -> HiveParams {
    let cfg = read_json(rel);
    let params = cfg
        .get("params")
        .cloned()
        .unwrap_or_else(|| panic!("{rel}: no params"));
    meclaw_core::serde_json::from_value(params).unwrap_or_else(|e| panic!("{rel}: params: {e}"))
}

fn hive_edges(rel: &str) -> Vec<EdgeSpec> {
    hive_params(rel).graph.edges
}

fn recipe_edges(rel: &str) -> Vec<EdgeSpec> {
    let doc = read_json(rel);
    let raw = doc["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{rel}: no diff.add_edges"))
        .clone();
    raw.into_iter()
        .map(|e| {
            meclaw_core::serde_json::from_value(e.clone())
                .unwrap_or_else(|err| panic!("{rel}: add_edges entry {e}: {err}"))
        })
        .collect()
}

fn abs(base: &str, endpoint: &str) -> String {
    match endpoint {
        "." => base.to_string(),
        other => format!("{base}/{}", other.trim_start_matches("./")),
    }
}

fn add_edges(table: &mut EdgeTable, base: &str, specs: &[EdgeSpec], label: &str) {
    for spec in specs {
        let condition = spec.condition.as_ref().map(|src| {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|e| panic!("{label}: condition {src:?}: {e}"))
        });
        let modifier = spec.modifier.as_ref().map(|m| {
            meclaw_colony::cel_eval::parse_modifier(m)
                .unwrap_or_else(|(k, e)| panic!("{label}: modifier {k}: {e}"))
        });
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(base, &spec.from)),
            to: Path::new(&abs(base, &spec.to)),
            condition,
            modifier,
            is_default: spec.is_default,
            lane: spec.lane.clone(),
        });
    }
}

/// The member, its memory hive, the `assistants` container as
/// `examples/organism` wires it, and one generation — the levels a recall
/// round trip crosses.
fn shipped_table() -> EdgeTable {
    let mut t = EdgeTable::new();
    add_edges(
        &mut t,
        MEMBER,
        &hive_edges("templates/member/config.json"),
        "member",
    );
    add_edges(
        &mut t,
        HIVE,
        &hive_edges("templates/memory-hive/config.json"),
        "memory-hive",
    );
    add_edges(
        &mut t,
        GEN,
        &hive_edges("templates/assistant/config.json"),
        "assistant",
    );
    add_edges(
        &mut t,
        BOX,
        &recipe_edges("examples/organism/grow-assistant.json"),
        "grow-assistant",
    );
    t
}

fn headers(context: &[(&str, &str)], hop: &[(&str, &str)]) -> Headers {
    let map = |pairs: &[(&str, &str)]| -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
            .collect()
    };
    Headers::from_parts(map(context), map(hop))
}

fn hop_of(h: &Headers, key: &str) -> String {
    h.hop.get(key).and_then(|v| v.as_str()).unwrap_or("").into()
}

/// Every delivery one message makes until nothing takes it further, as
/// `(receiver, headers it received)`, in order. Follows every branch — a
/// fan-out is not this file's question, but a branch it did not follow would be
/// a receiver it never looked at.
fn deliveries(table: &EdgeTable, from: &str, start: Headers) -> Vec<(String, Headers)> {
    let mut seen = Vec::new();
    let mut frontier = vec![(Path::new(from), start)];
    for _ in 0..24 {
        let mut next = Vec::new();
        for (here, hs) in frontier {
            for d in apply_edges(table, &here, &hs) {
                seen.push((d.target.as_str().to_string(), d.headers_out.clone()));
                next.push((d.target, d.headers_out));
            }
        }
        if next.is_empty() {
            return seen;
        }
        frontier = next;
    }
    panic!("the walk did not settle in 24 hops: {seen:?}");
}

fn recall_keys_in(h: &Headers) -> Vec<&'static str> {
    RECALL_KEYS
        .iter()
        .copied()
        .filter(|k| h.context.contains_key(*k))
        .collect()
}

/// A receiver is inside the hive when it is one of the hive's cells; the hive's
/// own path is its RIM, and what arrives there has already crossed an exit.
fn inside_the_hive(receiver: &str) -> bool {
    receiver.starts_with(&format!("{HIVE}/"))
}

/// The heart of the lock: no receiver outside the hive's interior got a single
/// one of the five, and the walk did leave the hive at all.
fn assert_nothing_leaked(road: &str, walk: &[(String, Headers)], expect_home: &str) {
    assert!(
        walk.iter().any(|(r, _)| r == expect_home),
        "{road}: the answer must reach {expect_home}: {:?}",
        walk.iter().map(|(r, _)| r.as_str()).collect::<Vec<_>>()
    );
    let leaks: Vec<(String, Vec<&str>)> = walk
        .iter()
        .filter(|(r, _)| !inside_the_hive(r))
        .map(|(r, h)| (r.clone(), recall_keys_in(h)))
        .filter(|(_, keys)| !keys.is_empty())
        .collect();
    assert!(
        leaks.is_empty(),
        "{road}: the question rode past the memory hive's rim — receivers outside the hive that \
         still carry recall keys: {leaks:?} (GH #823, docs/development-rules.md § 8c)"
    );
}

/// The request an asker inside the generation raises (the `gh562` shape).
fn recall_request() -> Headers {
    headers(
        &[
            ("assistant", "scribe"),
            ("session_id", "S-42"),
            ("audience_set", "[\"alex\"]"),
            ("channel", "chat:1"),
            ("turn_id", "S-42#7"),
        ],
        &[
            ("route", "recall"),
            ("recall_query", "what did we decide"),
            ("memory_tier", "1"),
            ("recall_window_from", ""),
            ("recall_window_to", ""),
            ("turn_id", "S-42#7"),
        ],
    )
}

/// The ambient question, delivered to the hive's `recall` cell — with the
/// positive probe that it carries all five there.
fn ambient_question_at_the_hive(t: &EdgeTable) -> Headers {
    let walk = deliveries(t, &format!("{GEN}/talky"), recall_request());
    let (receiver, at_hive) = walk
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("the recall went nowhere"));
    assert_eq!(
        receiver,
        format!("{HIVE}/recall"),
        "the ambient recall must reach the hive's recall cell: {:?}",
        walk.iter().map(|(r, _)| r.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(
        recall_keys_in(&at_hive),
        RECALL_KEYS.to_vec(),
        "positive probe: the question arrives at `recall` as all five context keys — a key that \
         is never set proves nothing by being absent later"
    );
    at_hive
}

fn answer(at_hive: &Headers, route: &str) -> Headers {
    Headers::from_parts(
        at_hive.context.clone(),
        [("route", route), ("turn_id", "S-42#7")]
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
            .collect(),
    )
}

// ──────────────────────────────────────────── 1. every exit deletes the five

#[test]
fn every_exit_of_the_memory_hive_deletes_the_five_recall_keys() {
    let edges = hive_edges("templates/memory-hive/config.json");
    let exits: Vec<&EdgeSpec> = edges.iter().filter(|e| e.to == ".").collect();
    assert!(
        exits.len() >= 12,
        "the memory hive ships twelve exit edges (3.4.1); found {} — a graph that lost its exits \
         would pass this file by having none",
        exits.len()
    );

    let mut open = Vec::new();
    for e in &exits {
        let cleared: BTreeSet<&str> = e
            .modifier
            .as_ref()
            .map(|m| m.delete_context.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let missing: Vec<&str> = RECALL_KEYS
            .iter()
            .copied()
            .filter(|k| !cleared.contains(k))
            .collect();
        if !missing.is_empty() {
            open.push(format!(
                "{} -> . [{}] misses {missing:?}",
                e.from,
                e.condition.as_deref().unwrap_or("<no condition>")
            ));
        }
    }
    assert!(
        open.is_empty(),
        "{} of {} exits of the memory hive let the question out (GH #823):\n  {}",
        open.len(),
        exits.len(),
        open.join("\n  ")
    );
}

// ─────────────────────────────────────── 2. the ambient road: member-set keys

#[test]
fn an_ambient_recall_answers_the_asker_without_its_question() {
    let t = shipped_table();
    let at_hive = ambient_question_at_the_hive(&t);
    let walk = deliveries(&t, &format!("{HIVE}/recall"), answer(&at_hive, "bundle"));
    assert_nothing_leaked("ambient bundle", &walk, &format!("{GEN}/talky"));
    let (_, home) = walk
        .iter()
        .find(|(r, _)| *r == format!("{GEN}/talky"))
        .cloned()
        .expect("checked above");
    assert_eq!(hop_of(&home, "route"), "in_bundle");
}

// ──────────────────────────────────────────── 3. the tool road: hive-set keys

/// The tool result leaves the member hive's rim for the generation's container
/// (`/m/assistants`), which is where this table ends; the road on to the brain
/// inside the generation is the assistant's and not measured here.
#[test]
fn a_tool_recall_leaves_for_the_generation_without_its_question() {
    let t = shipped_table();
    let ask = headers(
        &[
            ("memory_call_id", "call-7"),
            ("session_id", "S-42"),
            ("audience_now", "[\"alex\"]"),
            ("channel", "chat:1"),
        ],
        &[
            ("route", "ask"),
            ("recall_query", "what did we decide"),
            ("memory_tier", "1"),
            ("recall_window_from", ""),
            ("recall_window_to", ""),
        ],
    );
    let inward = deliveries(&t, &format!("{HIVE}/tool"), ask);
    let (receiver, at_recall) = inward.last().cloned().expect("the ask went nowhere");
    assert_eq!(receiver, format!("{HIVE}/recall"));
    assert_eq!(
        recall_keys_in(&at_recall),
        RECALL_KEYS.to_vec(),
        "positive probe: `./tool -> ./recall` sets all five inside the hive"
    );

    // The bundle goes back to `tool` — inside the hive, where the keys may stand.
    let back = deliveries(&t, &format!("{HIVE}/recall"), answer(&at_recall, "bundle"));
    let (receiver, at_tool) = back.last().cloned().expect("the bundle went nowhere");
    assert_eq!(
        receiver,
        format!("{HIVE}/tool"),
        "a tool recall's bundle comes back to the tool cell, not out of the rim: {back:?}"
    );
    assert_nothing_leaked("tool bundle", &back, &format!("{HIVE}/tool"));

    // And the tool cell's answer leaves the hive without the question.
    let out = deliveries(&t, &format!("{HIVE}/tool"), answer(&at_tool, "tool_result"));
    assert_nothing_leaked("tool_result", &out, BOX);
}

// ─────────────────────────────────────────────── 4. a refusal leaves the same

#[test]
fn a_refused_recall_leaves_without_its_question_too() {
    let t = shipped_table();
    let at_hive = ambient_question_at_the_hive(&t);
    let walk = deliveries(&t, &format!("{HIVE}/recall"), answer(&at_hive, "reject"));
    assert_nothing_leaked("ambient reject", &walk, &format!("{GEN}/talky"));
}
