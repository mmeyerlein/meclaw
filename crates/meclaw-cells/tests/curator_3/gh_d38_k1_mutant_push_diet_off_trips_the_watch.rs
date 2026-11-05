//! GH #1426 -- sabotage mutant: a recall push that is no longer kept to its diet
//!
//! The invariant is broken in the curator's test hive (the ledger carries the
//! marks a curator with the broken invariant writes), the hive's own `stats`
//! door counts it, and the argus meter -- the shipped script, fed that very
//! answer -- raises the alert of a role goal whose `alert_at` the broken
//! invariant crosses. The healthy trail beside it raises nothing, so the alert
//! is the invariant's, not the threshold's. Deterministic: no model anywhere.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "gh927/harness.rs"]
#[allow(unused_imports)]
use crate::harness; // harness_fold: mod harness;

use curator_hive::{Hive, shipped};
use harness::{METER, can_run, on_route, run, store_answer};
use meclaw_core::serde_json::{self, Value, json};

const SESSION: &str = "+15550100042-2026-11-01T08:00:00.000000Z";

fn at(i: u32) -> String {
    format!("2026-11-01T{:02}:{:02}:00.000000Z", 1 + i / 60, i % 60)
}

fn call(db: &rusqlite::Connection, i: u32) {
    db.execute(
        "INSERT INTO calls (call_id, session_id, turn_id, iter, trigger, started_at, model) \
         VALUES (?1, ?2, ?3, 0, 'curate', ?4, '')",
        rusqlite::params![format!("c{i}"), SESSION, format!("{SESSION}#{i}"), at(i)],
    )
    .expect("a calls row");
}

fn mark(db: &rusqlite::Connection, i: u32, kind: &str, value: &Value) {
    db.execute(
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

/// The curator's `stats` answer to one kind, handed to the meter as the
/// answer of a `stats` ask of a role goal; what the meter emits.
fn watch(h: &mut Hive, kind: &str, goal: Value) -> Vec<Value> {
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
    let answer = json!({"stats": m.body["stats"].clone(), "error_code": "", "detail": "",
                        "stats_tag": "wait:s1"});
    let carry = json!({"goal": goal, "prices": {}, "rules": [], "cycle_id": "cycle:7",
                       "ask": "baseline"});
    let rows = json!([{"id": "wait:s1", "at": "2026-01-01T00:00:00.000000Z", "kind": "stats",
                       "carry": carry, "status": "open"}]);
    run(METER, store_answer(rows, "wait", answer))
}

fn goal(metric: &str, alert_at: u64) -> Value {
    json!({"id": format!("goal:talky-{metric}"), "metric": metric, "source": "talky",
           "direction": "observe", "window_minutes": 1440, "min_samples": 1,
           "min_delta_pct": 0, "quality_gate": "", "enabled": 1, "alert_at": alert_at})
}

fn alerted(out: &[Value]) -> Option<Value> {
    let alerts = on_route(out, "alert");
    assert!(alerts.len() <= 1, "{out:?}");
    alerts.first().map(|m| {
        serde_json::from_str(m["messages"][0]["text"].as_str().expect("a text")).expect("JSON")
    })
}

/// Ten rebuilds; `diet` says whether the recall push stayed at five entries
/// or less (the invariant) or pushed its whole top-k.
fn trail(h: &Hive, diet: bool) {
    for i in 0..10u32 {
        call(&h.db, i);
        let rows = if diet { 5 } else { 45 };
        mark(
            &h.db,
            i,
            "gc",
            &json!({"call": format!("c{i}"),
                    "classes": {"shrunk_tools": 1, "dup_recall": 0, "transient": 0,
                                "released": 0, "scaffold": 0},
                    "tool_share": 0.1, "recall_rows": rows}),
        );
    }
}

#[test]
fn gh_d38_k1_mutant_push_diet_off_trips_the_watch() {
    if !shipped() || !can_run() {
        return;
    }
    let mut healthy = Hive::new();
    trail(&healthy, true);
    let out = watch(&mut healthy, "gc", goal("recall_rows", 5));
    assert_eq!(alerted(&out), None, "a dieting push stays at five: {out:?}");

    let mut broken = Hive::new();
    trail(&broken, false);
    let out = watch(&mut broken, "gc", goal("recall_rows", 5));
    let alert = alerted(&out).unwrap_or_else(|| panic!("the watch trips: {out:?}"));
    assert_eq!(alert["metric"], "recall_rows", "{alert}");
    assert_eq!(alert["value"], json!(45.0), "{alert}");
}
