//! GH #1426 -- the curator's `stats` door answers three more kinds: `window`,
//! `rebuild` and `gc`, numbers read off the marks the hive already writes
//! (`curate` once per call, `gc` once per rebuild).
//!
//! The answers are whole numbers, so a charter threshold (an `int` column)
//! compares against them: `window` `{n, p50, p95, max, cached_pct, cold_pct,
//! cost_cents_sum, raw_p95}`, `rebuild` `{n, by_mark, rate_pct}` and `gc`
//! `{n, classes, active_pct, tool_share_pct, recall_rows_p95}`. A ledger that
//! holds none of those marks (one written before they existed) answers zeros
//! and `truncated: false` -- never a refusal. A figure carries numbers only:
//! no sample is drawn from a `curate` or `gc` mark.
//!
//! The hive in one process (`support/curator_hive.rs`): the shipped script
//! under python3, the shipped edges, the ledger an in-memory SQLite.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::{Hive, Msg, shipped};
use meclaw_core::serde_json::{Value, json};

const FROM: &str = "2026-11-01T00:00:00Z";
const TO: &str = "2026-11-02T00:00:00Z";
const SESSION: &str = "+15550100042-2026-11-01T08:00:00.000000Z";

fn at(hour: u32) -> String {
    format!("2026-11-01T{hour:02}:00:00.000000Z")
}

fn call(db: &rusqlite::Connection, id: &str, started_at: &str) {
    db.execute(
        "INSERT INTO calls (call_id, session_id, turn_id, iter, trigger, started_at, model) \
         VALUES (?1, ?2, ?3, 0, 'curate', ?4, '')",
        rusqlite::params![id, SESSION, format!("{SESSION}#{id}"), started_at],
    )
    .expect("a calls row");
}

fn mark(db: &rusqlite::Connection, seq: i64, kind: &str, value: &Value, at: &str) {
    db.execute(
        "INSERT INTO marks (seq, session_id, turn_id, kind, value, at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            seq,
            SESSION,
            format!("{SESSION}#{seq}"),
            kind,
            value.to_string(),
            at
        ],
    )
    .expect("a marks row");
}

/// A `curate` mark the way `./policy` writes it (`said_now`).
fn curate(mark: &str, window: Value, cached: Value, cost: Value, raw: Option<u64>) -> Value {
    let mut v = json!({"call": "c", "curate_mark": mark, "window_tokens": window,
                       "cached_tokens": cached, "cost_cents": cost});
    if let Some(r) = raw {
        v["raw_tokens"] = json!(r);
    }
    v
}

/// The trail: two calls, six `curate` marks (one without a window), two `gc`
/// marks in the form the plan fixes for them, one `gap`, and a `curate` and a
/// `gc` mark outside the window.
fn sow(db: &rusqlite::Connection) {
    call(db, "c1", &at(1));
    call(db, "c2", &at(2));
    let curates = [
        curate("none", json!(10000), json!(9000), json!(1), Some(5000)),
        curate("target", json!(20000), json!(0), json!(2), Some(15000)),
        curate("none", json!(30000), json!(25000), json!(3), Some(70000)),
        curate("none", json!(40000), json!(39000), json!(4), None),
        curate("hard", json!(100000), json!(11000), json!(10), None),
        curate("none", Value::Null, Value::Null, Value::Null, None),
    ];
    for (i, v) in curates.iter().enumerate() {
        let mut v = v.clone();
        if i == 3 {
            v["target_hold"] = json!(28000);
        }
        mark(db, i as i64 + 1, "curate", &v, &at(3 + i as u32));
    }
    let gc = |classes: Value, tool_share: f64, recall_rows: u64| {
        json!({"call": "c", "classes": classes, "tool_share": tool_share,
               "recall_rows": recall_rows})
    };
    mark(
        db,
        20,
        "gc",
        &gc(
            json!({"shrunk_tools": 3, "dup_recall": 0, "transient": 1, "released": 0, "scaffold": 0}),
            0.25,
            4,
        ),
        &at(10),
    );
    mark(
        db,
        21,
        "gc",
        &gc(
            json!({"shrunk_tools": 0, "dup_recall": 0, "transient": 0, "released": 0,
                   "scaffold": 0, "recall_unused": 0}),
            0.75,
            12,
        ),
        &at(11),
    );
    mark(
        db,
        30,
        "gap",
        &json!({"searched": 1, "text": "words"}),
        &at(12),
    );
    // Outside: one second before FROM and exactly on TO.
    mark(
        db,
        40,
        "curate",
        &curate("hard", json!(999999), json!(0), json!(99), None),
        "2026-10-31T23:59:59.000000Z",
    );
    mark(
        db,
        41,
        "gc",
        &gc(json!({"shrunk_tools": 50}), 1.0, 99),
        "2026-11-02T00:00:00.000000Z",
    );
}

fn ask(h: &mut Hive, tag: &str, kinds: &str, samples: i64) -> Msg {
    h.out.clear();
    h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": tag, "stats_from": FROM, "stats_to": TO,
               "stats_kinds": kinds, "stats_samples": samples}),
        json!({"messages": []}),
    );
    assert_eq!(h.out.len(), 1, "one answer per question: {:?}", h.out);
    let m = h.out[0].clone();
    assert_eq!(m.route(), "stats");
    m
}

fn window_figure() -> Value {
    // p50/p95 nearest rank over 10k, 20k, 30k, 40k, 100k; cached 84k of 200k;
    // cold = cached under 0.2 x window: the 20k (0 cached) and the 100k (11k).
    json!({"n": 5, "p50": 30000, "p95": 100000, "max": 100000, "cached_pct": 42,
           "cold_pct": 40, "cost_cents_sum": 20, "raw_p95": 70000})
}

fn rebuild_figure() -> Value {
    json!({"n": 6, "by_mark": {"hard": 1, "soft": 0, "day": 0, "target": 1, "cost": 0,
                               "none": 4, "target_hold": 1},
           "rate_pct": 33})
}

fn gc_figure() -> Value {
    json!({"n": 2, "classes": {"shrunk_tools": 3, "dup_recall": 0, "transient": 1,
                               "released": 0, "scaffold": 0, "recall_unused": 0},
           "active_pct": 50, "tool_share_pct": 50, "recall_rows_p95": 12})
}

#[test]
fn gh_d38_k1_stats_answers_window_rebuild_and_gc() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    sow(&h.db);
    let m = ask(&mut h, "k1", r#"["window","rebuild","gc"]"#, 0);
    assert_eq!(m.hop["error_code"], "", "{:?}", h.stderr);
    let counts = &m.body["stats"]["counts"];
    assert_eq!(counts["window"], window_figure(), "{counts}");
    assert_eq!(counts["rebuild"], rebuild_figure(), "{counts}");
    assert_eq!(counts["gc"], gc_figure(), "{counts}");
    assert_eq!(m.body["stats"]["truncated"], false);
    // One ledger round trip, reads only: the marks leg (no `calls` asked).
    let reads: Vec<&Value> = h
        .ledger_ops
        .iter()
        .filter(|(from, _)| from == "stats")
        .map(|(_, a)| a)
        .collect();
    assert_eq!(reads.len(), 1, "{reads:?}");
    assert!(reads.iter().all(|a| a["operation"] == "select"));
}

#[test]
fn gh_d38_k1_the_old_kinds_and_their_samples_stay_as_they_were() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    sow(&h.db);
    let m = ask(&mut h, "mix", r#"["calls","gap","window"]"#, 10);
    let counts = &m.body["stats"]["counts"];
    assert_eq!(counts["calls"], 2, "{counts}");
    assert_eq!(counts["gap"], 1, "{counts}");
    assert_eq!(counts["window"], window_figure(), "{counts}");
    let kinds: Vec<Value> = m.body["stats"]["samples"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| s["kind"].clone())
        .collect();
    assert_eq!(
        kinds,
        [json!("gap")],
        "a figure is numbers only, no `curate` or `gc` mark is sampled"
    );
}

#[test]
fn gh_d38_k1_a_ledger_without_the_marks_answers_zeros() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    call(&h.db, "c1", &at(1));
    let m = ask(&mut h, "old", r#"["window","rebuild","gc"]"#, 5);
    assert_eq!(m.hop["error_code"], "", "never a refusal: {:?}", h.stderr);
    assert_eq!(m.body["stats"]["truncated"], false);
    assert_eq!(m.body["stats"]["samples"], json!([]));
    let counts = &m.body["stats"]["counts"];
    assert_eq!(
        counts["window"],
        json!({"n": 0, "p50": 0, "p95": 0, "max": 0, "cached_pct": 0, "cold_pct": 0,
               "cost_cents_sum": 0, "raw_p95": 0}),
        "{counts}"
    );
    assert_eq!(
        counts["rebuild"],
        json!({"n": 0, "by_mark": {"hard": 0, "soft": 0, "day": 0, "target": 0, "cost": 0,
                                   "none": 0, "target_hold": 0},
               "rate_pct": 0}),
        "{counts}"
    );
    assert_eq!(
        counts["gc"],
        json!({"n": 0, "classes": {}, "active_pct": 0, "tool_share_pct": 0,
               "recall_rows_p95": 0}),
        "{counts}"
    );
}

#[test]
fn gh_d38_k1_the_cold_share_is_a_knob() {
    if !shipped() {
        return;
    }
    // Cold under 0.95 x window: 9k of 10k, 0 of 20k, 25k of 30k, 11k of 100k
    // -- four of five; 39k of 40k stays warm.
    let mut h = Hive::with(&[("stats", "cold_cached_share", json!(0.95))]);
    sow(&h.db);
    let m = ask(&mut h, "knob", r#"["window"]"#, 0);
    assert_eq!(
        m.body["stats"]["counts"]["window"]["cold_pct"], 80,
        "{:?}",
        m.body
    );
}

#[test]
fn gh_d38_k1_an_unmeasured_cache_is_not_cold() {
    if !shipped() {
        return;
    }
    // Review M-1: a call whose usage carried no cache stamp (`./policy`
    // writes `cached_tokens` null then) is unknown, not cold. The shares are
    // taken over the measured call alone: 5k of 10k cached, warm.
    let mut h = Hive::new();
    call(&h.db, "c1", &at(1));
    let marks = [
        curate("none", json!(10000), Value::Null, json!(1), None),
        curate("none", json!(10000), json!(5000), json!(1), None),
    ];
    for (i, v) in marks.iter().enumerate() {
        mark(&h.db, i as i64 + 1, "curate", v, &at(3 + i as u32));
    }
    let m = ask(&mut h, "m1", r#"["window"]"#, 0);
    let w = &m.body["stats"]["counts"]["window"];
    assert_eq!(w["n"], 2, "both calls had a window: {w}");
    assert_eq!(w["cold_pct"], 0, "the unmeasured call is not cold: {w}");
    assert_eq!(w["cached_pct"], 50, "over the measured call only: {w}");
}
