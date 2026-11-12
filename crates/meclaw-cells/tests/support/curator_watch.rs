//! The watch over the curator's figures, for the mutants of the bank
//! (GH #1465, plan part `core-bank`, contract 2).
//!
//! WHY this file exists: four figures of the watch (`rebuild_rate`,
//! `gc_share`, `cold_pct`, `rebuild_cost`) had no mutant. Each mutant writes
//! the trail of marks a curator leaves -- once sound, once with one
//! invariant broken --, lets the hive's own `stats` door count it and hands
//! that very answer to the shipped argus meter as the answer of a role
//! goal. This file is the part the four share: the trail, the ask, the goal
//! and the alert. The role of the goal is a parameter; the trail is the
//! same for every role. Deterministic: no model anywhere.
#![allow(dead_code)]

use crate::curator_hive::Hive;
use crate::harness::{METER, on_route, run, store_answer};
use meclaw_core::serde_json::{self, Value, json};

/// The roles every mutant of the watch is read for.
pub const ROLES: [&str; 2] = ["talky", "coding"];

/// The calls of a trail.
pub const CALLS: u32 = 10;

const SESSION: &str = "+15550100042-2026-11-01T08:00:00.000000Z";

fn at(i: u32) -> String {
    format!("2026-11-01T{:02}:{:02}:00.000000Z", 1 + i / 60, i % 60)
}

/// The call `i` of the trail.
pub fn call(h: &Hive, i: u32) {
    h.db.execute(
        "INSERT INTO calls (call_id, session_id, turn_id, iter, trigger, started_at, model) \
         VALUES (?1, ?2, ?3, 0, 'curate', ?4, '')",
        rusqlite::params![format!("c{i}"), SESSION, format!("{SESSION}#{i}"), at(i)],
    )
    .expect("a calls row");
}

/// A mark of `kind` on the call `i`. A call carries one mark of a trail.
pub fn mark(h: &Hive, i: u32, kind: &str, value: &Value) {
    h.db.execute(
        "INSERT INTO marks (seq, session_id, turn_id, kind, value, at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            i64::from(i) + 1,
            SESSION,
            format!("{SESSION}#{i}"),
            kind,
            value.to_string(),
            at(i)
        ],
    )
    .expect("a marks row");
}

/// The `curate` mark of the call `i` in the form `./policy` writes it: the
/// order of the turn (`none` for no rebuild), the window, the tokens of it
/// the cache held, and the cents of the call.
pub fn curate(i: u32, order: &str, window: u64, cached: u64, cents: u64) -> Value {
    json!({"call": format!("c{i}"), "curate_mark": order, "window_tokens": window,
           "cached_tokens": cached, "cost_cents": cents})
}

/// The `gc` mark of the call `i` in the form a rebuild writes it, with
/// `shrunk` old tool results collected.
pub fn gc(i: u32, shrunk: u64) -> Value {
    json!({"call": format!("c{i}"),
           "classes": {"shrunk_tools": shrunk, "dup_recall": 0, "transient": 0,
                       "released": 0, "scaffold": 0},
           "tool_share": 0.2, "recall_rows": 3})
}

/// The curator's `stats` answer to the figure `kind` over the trail, handed
/// to the meter as the answer of a `stats` ask of `goal`: the figure the
/// door counted, and what the meter emits.
pub fn watch(h: &mut Hive, kind: &str, goal: Value) -> (Value, Vec<Value>) {
    h.out.clear();
    h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": "wait:s1", "stats_from": "2026-11-01T00:00:00Z",
               "stats_to": "2026-11-02T00:00:00Z",
               "stats_kinds": serde_json::to_string(&json!(["calls", kind])).unwrap()}),
        json!({"messages": []}),
    );
    assert_eq!(h.out.len(), 1, "one answer: {:?}", h.out);
    let m = &h.out[0];
    assert_eq!(m.hop["error_code"], "", "{:?}", h.stderr);
    let figure = m.body["stats"]["counts"][kind].clone();
    let answer = json!({"stats": m.body["stats"].clone(), "error_code": "", "detail": "",
                        "stats_tag": "wait:s1"});
    let carry = json!({"goal": goal, "prices": {}, "rules": [], "cycle_id": "cycle:7",
                       "ask": "baseline"});
    let rows = json!([{"id": "wait:s1", "at": "2026-01-01T00:00:00.000000Z", "kind": "stats",
                       "carry": carry, "status": "open"}]);
    (figure, run(METER, store_answer(rows, "wait", answer)))
}

/// The goal of `role` on `metric`, alerting over `alert_at`.
pub fn goal(role: &str, metric: &str, alert_at: u64) -> Value {
    json!({"id": format!("goal:{role}-{metric}"), "metric": metric, "source": role,
           "direction": "observe", "window_minutes": 1440, "min_samples": 1,
           "min_delta_pct": 0, "quality_gate": "", "enabled": 1, "alert_at": alert_at})
}

/// The alert the meter raised, if it raised one.
pub fn alerted(out: &[Value]) -> Option<Value> {
    let alerts = on_route(out, "alert");
    assert!(alerts.len() <= 1, "{out:?}");
    alerts.first().map(|m| {
        serde_json::from_str(m["messages"][0]["text"].as_str().expect("a text")).expect("JSON")
    })
}
