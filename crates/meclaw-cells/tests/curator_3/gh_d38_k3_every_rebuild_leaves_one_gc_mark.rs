//! GH #1430 (D38 K3) -- every rebuild leaves one `gc` mark, and `stats` counts it.
//!
//! WHY this file exists: the garbage collection of a rebuild (R-38-2) is only
//! worth what a watcher can see of it. So every rebuild -- with the
//! collection on or off -- writes one mark `gc` beside its plan: `{call,
//! classes: {shrunk_tools, dup_recall, transient, released, scaffold},
//! tool_share, recall_rows}`, every class a whole number (0 when nothing of
//! it happened), `tool_share` the share of tool-result characters in the
//! window the plan leaves, `recall_rows` the rows of the newest
//! `memory_recall` result that stays in it. With `gc` "1" the collection runs
//! at every rebuild, not only when the plan is over its aim. The form is the
//! contract with the `stats` door (GH #1426, K1): the third lock here asks
//! the hive's own `stats` for kind `gc` and expects exactly the figure of
//! the marks `./policy` wrote -- the writer and the reader coupled on the
//! real ledger, no mark set by hand (review K1 I-1).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const DAY_2_S: i64 = 1_772_409_600;
const AS_OF_MS: i64 = (DAY_2_S + 7200) * 1000;
const CLASSES: [&str; 5] = [
    "shrunk_tools",
    "dup_recall",
    "transient",
    "released",
    "scaffold",
];

fn row(i: i64, j: i64, turn: &str, kind: &str, chars: i64, typ: &str, extra: Value) -> Value {
    let t = DAY_2_S + 3600 + 10 * i + j;
    let mut r = json!({
        "seq": t * 1_000_000, "session_id": "s", "turn_id": turn, "iter": 0, "kind": kind,
        "hash": sha256_hex(&format!("{turn}/{i}/{j}")), "nth": 0,
        "final": if kind == "assistant" { 1 } else { 0 },
        "at": chrono::DateTime::from_timestamp(t, 0).unwrap()
            .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
        "audience_set": ROUND_E, "chars": chars, "type": typ});
    if let Some(o) = extra.as_object() {
        for (k, v) in o {
            r[k] = v.clone();
        }
    }
    r
}

fn tool_round(i: i64, turn: &str, result: Value) -> Vec<Value> {
    vec![
        row(i, 0, turn, "user", 3000, "text", json!({})),
        row(
            i,
            1,
            turn,
            "tool_call",
            100,
            "tool_call",
            json!({"name": "lookup"}),
        ),
        row(i, 2, turn, "tool_result", 3000, "tool_result", result),
        row(i, 3, turn, "assistant", 3000, "text", json!({})),
    ]
}

fn id_of(r: &Value) -> String {
    r["hash"].as_str().unwrap()[..12].to_string()
}

/// Three tool rounds, no aim at all (`cw` 0): with `gc` "1" the result of r1
/// shrinks all the same (r0 is an error, r2 the newest), and the plan
/// carries the mark's value with every class, 0 where nothing happened.
/// With `gc` "0" nothing shrinks, the value stands all the same.
#[test]
fn gc_on_collects_at_every_rebuild_and_the_value_names_every_class() {
    let wall: Vec<Value> = [
        tool_round(0, "r0", json!({"error": true})),
        tool_round(1, "r1", json!({"error": false})),
        tool_round(2, "r2", json!({"error": false})),
    ]
    .concat();
    let probe = "(lambda r: {'shrunk': r['plan']['shrunk'], 'gc': r.get('gc')})(\
                 window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], [], 0, None))";
    for (gc, want_shrunk) in [("1", json!([id_of(&wall[6])])), ("0", json!([]))] {
        let (got, err) = policy_scope(
            json!({"role": "talky", "gc": gc, "keep_rounds": 1}),
            probe,
            json!({"rows": wall, "as_of": AS_OF_MS}),
        );
        assert!(!err.contains("Traceback"), "{err}");
        assert_eq!(got["shrunk"], want_shrunk, "gc {gc}: {got}");
        let value = &got["gc"];
        let classes = value["classes"].as_object().expect("classes");
        for c in CLASSES {
            assert!(
                classes.get(c).and_then(Value::as_u64).is_some(),
                "class {c} is a whole number: {value}"
            );
        }
        assert_eq!(
            value["classes"]["shrunk_tools"],
            json!(if gc == "1" { 1 } else { 0 }),
            "{value}"
        );
        let share = value["tool_share"].as_f64().expect("tool_share");
        assert!(
            share > 0.0 && share <= 1.0,
            "a share of the window: {value}"
        );
        assert_eq!(value["recall_rows"], json!(0), "no recall in it: {value}");
    }
}

/// One round on the replay channel, said at `at`, answered on the tap with a
/// window far under the target (as in `gh1272_tool_results_of_an_old_day_shrink`).
fn round_at(h: &mut Hive, turn: &str, at: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s", "turn_id": turn, "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s", "turn_id": turn, "iter": "0", "channel": "replay-1",
                     "audience_set": ROUND_E, "happened_at": at});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(&format!("tell me about {turn}"))]}),
    );
    let calls = h.routed("brain");
    assert_eq!(
        calls.len(),
        1,
        "one round, one call: {:?} {:?}",
        h.out,
        h.stderr
    );
    let call = calls[0].clone();
    h.out.clear();
    h.tap(
        &call,
        "stop",
        json!({"tokens_prompt": 2_000, "input_soft": 250_000, "input_hard": 1_000_000}),
        json!([said(&format!("answered {turn}"))]),
    );
    call
}

/// A recall result in the form the memory hive renders it (document head,
/// section head, the rows at an indent of two): only the rows count
/// (review I-1).
fn recall_text(lines: &[&str]) -> String {
    let mut out = vec![
        "WHAT THIS MEMORY HOLDS (as of 2026-03-02)".to_string(),
        "FACTS (extracted, canonical, dated)".to_string(),
    ];
    out.extend(lines.iter().map(|l| format!("  {l}")));
    out.join("\n")
}

/// A lookup round written the way `./intake` writes it: the person asks, one
/// `memory_recall` pair whose result carries the rows `lines`, the answer.
fn lookup_round(h: &mut Hive, turn: &str, at: &str, n: u64, lines: &[&str]) {
    let base = chrono::DateTime::parse_from_rfc3339(at)
        .unwrap()
        .with_timezone(&chrono::Utc);
    h.row(base, "s", turn, "user", &user("what do you remember?"), 0);
    let id = format!("call_recall_{n:016x}");
    h.row(
        base + chrono::Duration::seconds(1),
        "s",
        turn,
        "tool_call",
        &tool_call(&id, "memory_recall"),
        0,
    );
    h.row(
        base + chrono::Duration::seconds(2),
        "s",
        turn,
        "tool_result",
        &tool_result(&id, &recall_text(lines)),
        0,
    );
    h.row(
        base + chrono::Duration::seconds(3),
        "s",
        turn,
        "assistant",
        &said("that is what I remember"),
        1,
    );
}

/// The day mark rebuilds on the first turn of a new day. Within talky's raw
/// token budget the day before stays raw and nothing is condensed (GH #1427,
/// K6) -- the rebuild then ends without a summarizer call; one that asks for
/// a summary is answered (OR-d38-1.k3.9).
fn day_rebuild(h: &mut Hive, turn: &str, at: &str) {
    round_at(h, turn, at);
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the day orders a rebuild");
    assert_eq!(order.body["emit_body"]["reason"], "day", "{:?}", h.stderr);
    h.fire(&order);
    if !h.summ.is_empty() {
        h.answer("The day before, in short.", "stop");
    }
}

/// Three days, two rebuilds: a lookup on day one; day two rebuilds; two
/// lookups on day two that share a row; day three rebuilds. Answers the
/// `(value, at)` of every `gc` mark the ledger holds, oldest first.
fn two_rebuilds(h: &mut Hive) -> Vec<(Value, String)> {
    round_at(h, "d1-open", "2026-03-01T08:00:00Z");
    lookup_round(
        h,
        "d1-look",
        "2026-03-01T09:00:00Z",
        1,
        &["fact A", "fact B"],
    );
    day_rebuild(h, "d2-a", "2026-03-02T08:00:00Z");
    lookup_round(
        h,
        "d2-look1",
        "2026-03-02T09:00:00Z",
        2,
        &["fact A", "fact C"],
    );
    lookup_round(
        h,
        "d2-look2",
        "2026-03-02T10:00:00Z",
        3,
        &["fact A", "fact D"],
    );
    day_rebuild(h, "d3-a", "2026-03-03T08:00:00Z");
    h.rows("SELECT value, at FROM marks WHERE kind = 'gc' ORDER BY seq")
        .into_iter()
        .map(|r| {
            let v: Value = meclaw_core::serde_json::from_str(r[0].as_str().unwrap()).unwrap();
            (v, r[1].as_str().unwrap().to_string())
        })
        .collect()
}

#[test]
fn every_rebuild_leaves_one_gc_mark() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("intake", "replay_channel_prefix", json!("replay-")),
    ]);
    let marks = two_rebuilds(&mut h);
    assert_eq!(
        marks.len(),
        2,
        "one mark per rebuild: {marks:?} {:?}",
        h.stderr
    );
    for (v, _) in &marks {
        assert!(
            !v["call"].as_str().unwrap_or("").is_empty(),
            "the mark names its call: {v}"
        );
        for c in CLASSES {
            assert!(
                v["classes"][c].as_u64().is_some(),
                "class {c} is a whole number: {v}"
            );
        }
        let share = v["tool_share"].as_f64().expect("tool_share");
        assert!((0.0..=1.0).contains(&share), "{v}");
        assert!(v["recall_rows"].as_u64().is_some(), "{v}");
    }
    // Day two stays raw at the rebuild of day three (talky's budget): the
    // older lookup of day two shares `fact A` with the younger one and
    // shrinks as a duplicate; the younger result (two rows) is the newest
    // recall in the window.
    let second = &marks[1].0;
    assert!(
        second["classes"]["dup_recall"].as_u64().unwrap() >= 1,
        "the older lookup of day two shrinks as a duplicate: {second}"
    );
    assert_eq!(second["recall_rows"], json!(2), "{second}");
}

/// `whole` and `rank` of `./stats`: halves rounded up, nearest-rank percentile.
fn whole(x: f64) -> u64 {
    if x > 0.0 { (x + 0.5) as u64 } else { 0 }
}

fn rank(values: &[u64], p: usize) -> u64 {
    let mut vs = values.to_vec();
    vs.sort_unstable();
    if vs.is_empty() {
        return 0;
    }
    let i = (p * vs.len()).div_ceil(100).max(1) - 1;
    vs[i.min(vs.len() - 1)]
}

/// The `gc` figure `./stats` answers, read off the marks `./policy` wrote.
#[test]
fn the_stats_lane_counts_the_mark_the_policy_wrote() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("intake", "replay_channel_prefix", json!("replay-")),
    ]);
    let marks = two_rebuilds(&mut h);
    assert_eq!(marks.len(), 2, "{marks:?}");
    let mut classes = meclaw_core::serde_json::Map::new();
    let (mut active, mut shares, mut rows) = (0u64, Vec::new(), Vec::new());
    for (v, _) in &marks {
        let mut hit = false;
        for (k, c) in v["classes"].as_object().unwrap() {
            let c = c.as_u64().unwrap();
            let sum = classes.get(k).and_then(Value::as_u64).unwrap_or(0) + c;
            classes.insert(k.clone(), json!(sum));
            hit |= c > 0;
        }
        active += u64::from(hit);
        shares.push(v["tool_share"].as_f64().unwrap());
        rows.push(v["recall_rows"].as_u64().unwrap());
    }
    let n = marks.len() as u64;
    let want = json!({
        "n": n, "classes": classes,
        "active_pct": whole(active as f64 * 100.0 / n as f64),
        "tool_share_pct": whole(shares.iter().sum::<f64>() * 100.0 / shares.len() as f64),
        "recall_rows_p95": rank(&rows, 95)});

    // The window of the question: the days the marks were written on.
    let first = &marks[0].1[..10];
    let last = chrono::NaiveDate::parse_from_str(&marks[marks.len() - 1].1[..10], "%Y-%m-%d")
        .unwrap()
        + chrono::Duration::days(1);
    h.out.clear();
    h.lane(
        "in_stats",
        json!({}),
        json!({"stats_tag": "k3", "stats_from": format!("{first}T00:00:00Z"),
               "stats_to": format!("{}T00:00:00Z", last.format("%Y-%m-%d")),
               "stats_kinds": "[\"gc\"]", "stats_samples": 0}),
        json!({"messages": []}),
    );
    assert_eq!(h.out.len(), 1, "one answer per question: {:?}", h.out);
    let m = h.out[0].clone();
    assert_eq!(m.route(), "stats");
    assert_eq!(
        m.body["stats"]["counts"]["gc"], want,
        "the figure is the marks, nothing lost: {marks:?}"
    );
}
