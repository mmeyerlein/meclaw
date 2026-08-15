//! GH #80: a fan-out's missing key is not an operator's problem, a typo is.
//!
//! Every edge condition that reads an optional `hop` key errors on every
//! message that does not carry it -- which, in a fan-out, is most messages. The
//! routing outcome is right (skip), but the substrate logged one WARN per
//! non-matching edge per message, and the reported colony ran at 94 percent
//! warnings. An operator cannot see a real edge typo in that, which is the exact
//! condition the warning exists to catch.
//!
//! The split pinned here: a missing key on an otherwise valid expression is the
//! steady state of a fan-out and logs at DEBUG; a genuine eval error (type
//! mismatch, non-bool result, a reference to a variable that is not bound at
//! all) stays at WARN. Routing is unchanged in both cases -- the edge is
//! skipped, which is what spec F3 says.
//!
//! The subscriber below is hand-rolled on `tracing` alone; no crate is added to
//! read three log lines.

use meclaw_colony::cel_eval::{CondErrorKind, evaluate_condition, parse_condition};
use meclaw_colony::edge_table::{Edge, evaluate_edge};
use meclaw_core::{Headers, Path, Uuid, serde_json::json};
use std::sync::{Arc, Mutex};

// ---- a minimal event-level recorder -----------------------------------------

#[derive(Default)]
struct Recorded {
    events: Vec<(tracing::Level, String)>,
}

#[derive(Clone, Default)]
struct Recorder {
    log: Arc<Mutex<Recorded>>,
}

impl Recorder {
    fn levels(&self, level: tracing::Level) -> Vec<String> {
        self.log
            .lock()
            .expect("log mutex")
            .events
            .iter()
            .filter(|(l, _)| *l == level)
            .map(|(_, m)| m.clone())
            .collect()
    }
}

/// Pulls the `message` field out of an event; everything else is ignored.
struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl tracing::Subscriber for Recorder {
    fn enabled(&self, _meta: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut v = MessageVisitor(String::new());
        event.record(&mut v);
        self.log
            .lock()
            .expect("log mutex")
            .events
            .push((*event.metadata().level(), v.0));
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

fn edge(condition: &str) -> Edge {
    Edge {
        id: Uuid::now_v7(),
        from: Path::new("/dispatch"),
        to: Path::new("/searcher"),
        condition: Some(parse_condition(condition).expect("parse")),
        modifier: None,
    }
}

// ---- the two classes ---------------------------------------------------------

/// The reported case: a `c_asst` emission carries no `tool_name` at all, and the
/// dispatcher has one lane per tool.
#[test]
fn a_missing_hop_key_skips_the_edge_and_logs_at_debug() {
    let rec = Recorder::default();
    let headers = Headers::new(); // no `tool_name` anywhere

    let taken = tracing::subscriber::with_default(rec.clone(), || {
        evaluate_edge(&edge("hop.tool_name == 'web_search'"), &headers)
    });

    assert!(taken.is_none(), "routing is unchanged: the edge is skipped");
    assert!(
        rec.levels(tracing::Level::WARN).is_empty(),
        "a fan-out's normal steady state must not warn, got: {:?}",
        rec.levels(tracing::Level::WARN)
    );
    let debugs = rec.levels(tracing::Level::DEBUG);
    assert_eq!(debugs.len(), 1, "exactly one debug line, got: {debugs:?}");
}

/// A type mismatch is a builder error, and the operator has to see it.
#[test]
fn a_type_error_still_warns() {
    let rec = Recorder::default();
    let mut headers = Headers::new();
    headers.hop.insert("tool_name".into(), json!(7));

    let taken = tracing::subscriber::with_default(rec.clone(), || {
        // `7 > 'x'` is not comparable: an eval error, not an absent key.
        evaluate_edge(&edge("hop.tool_name > 'web_search'"), &headers)
    });

    assert!(taken.is_none(), "routing is unchanged: the edge is skipped");
    let warns = rec.levels(tracing::Level::WARN);
    assert_eq!(warns.len(), 1, "a real eval error warns, got: {warns:?}");
}

/// A condition that names a compartment nobody binds is the edge typo the
/// warning exists for.
#[test]
fn an_unbound_variable_still_warns() {
    let rec = Recorder::default();
    let headers = Headers::new();

    let taken = tracing::subscriber::with_default(rec.clone(), || {
        evaluate_edge(&edge("hopp.tool_name == 'web_search'"), &headers)
    });

    assert!(taken.is_none());
    assert_eq!(
        rec.levels(tracing::Level::WARN).len(),
        1,
        "a reference to an unbound variable is not a missing key"
    );
}

/// A condition that evaluates to a non-bool is a builder error too.
#[test]
fn a_non_bool_result_still_warns() {
    let rec = Recorder::default();
    let mut headers = Headers::new();
    headers.hop.insert("tool_name".into(), json!("web_search"));

    let taken = tracing::subscriber::with_default(rec.clone(), || {
        evaluate_edge(&edge("hop.tool_name"), &headers)
    });

    assert!(taken.is_none());
    assert_eq!(rec.levels(tracing::Level::WARN).len(), 1);
}

// ---- the classification itself ----------------------------------------------

#[test]
fn evaluate_condition_classifies_a_missing_key() {
    let c = parse_condition("hop.tool_name == 'web_search'").expect("parse");
    let err = evaluate_condition(&c, &Default::default(), &Default::default()).unwrap_err();
    assert_eq!(err.kind, CondErrorKind::MissingKey, "{err}");
}

#[test]
fn evaluate_condition_classifies_a_real_eval_error() {
    let c = parse_condition("hop.n > 'x'").expect("parse");
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("n".into(), json!(7));
    let err = evaluate_condition(&c, &Default::default(), &hop).unwrap_err();
    assert_eq!(err.kind, CondErrorKind::Eval, "{err}");
}

/// The documented guard is what topologies are supposed to use: it routes the
/// same way and produces no line at all.
#[test]
fn the_has_guard_is_silent_and_routes_the_same() {
    let rec = Recorder::default();
    let mut carrying = Headers::new();
    carrying.hop.insert("tool_name".into(), json!("web_search"));

    let (absent, present) = tracing::subscriber::with_default(rec.clone(), || {
        let guarded = "has(hop.tool_name) && hop.tool_name == 'web_search'";
        (
            evaluate_edge(&edge(guarded), &Headers::new()),
            evaluate_edge(&edge(guarded), &carrying),
        )
    });

    assert!(absent.is_none(), "no key, no route");
    assert!(present.is_some(), "key matches, edge is taken");
    assert!(
        rec.log.lock().expect("log mutex").events.is_empty(),
        "the guarded form emits nothing at all: {:?}",
        rec.log.lock().expect("log mutex").events
    );
}
