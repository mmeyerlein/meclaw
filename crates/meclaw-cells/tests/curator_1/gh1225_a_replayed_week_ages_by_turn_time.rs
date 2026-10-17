//! GH #1225 (K-1, OR-KF-5) -- a replayed conversation ages by the time of its
//! turns, not by when it was written.
//!
//! A replay writes a week of turns in minutes: by `at`, the ingest clock, the
//! whole week was one UTC day, and the curator's `day` horizon kept all of it
//! raw. `./intake` stamps `wall.turn_at` on a replay channel; the plan ages a
//! row by `turn_at` where it is set, and the clock of age is the newest turn
//! time among its rows. Expiry is not age: a `valid_until` is a wall-clock
//! stamp of its maker and runs out on the plan's wall clock (`as_of_ms`).
//! Without `turn_at` the plan is the plan of curator 1.11.5, byte for byte.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// 2026-10-12T09:00:00Z: the ingest minute every row is written in.
const BASE_S: i64 = 1_791_795_600;
/// The plan's wall clock: two minutes after the first row.
const AS_OF_MS: i64 = BASE_S * 1000 + 120_000;

/// Six rounds of two rows each, written ten seconds apart; with `turn_at`,
/// rounds 0-1 were said on 10 October, 2-3 on the 11th and 4-5 on the 12th.
fn rows(stamped: bool) -> Vec<Value> {
    let days = [
        "2026-10-10T12:00:00Z",
        "2026-10-11T12:00:00Z",
        "2026-10-12T08:00:00Z",
    ];
    let mut out = Vec::new();
    for i in 0..6i64 {
        for j in 0..2i64 {
            let at = chrono::DateTime::from_timestamp(BASE_S + 10 * i + j, 0).unwrap();
            let mut r = json!({
                "seq": (BASE_S + 10 * i + j) * 1_000_000, "session_id": "s",
                "turn_id": format!("t{i}"), "iter": 0,
                "kind": if j == 0 { "user" } else { "assistant" },
                "hash": format!("{:064x}", 2 * i + j + 1), "nth": 0, "final": j,
                "at": at.format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
                "audience_set": ROUND_E, "chars": 100, "type": "text"});
            if stamped {
                r["turn_at"] = json!(days[(i / 2) as usize]);
            }
            out.push(r);
        }
    }
    out
}

fn plan_of(rows: Vec<Value>) -> Value {
    let (got, err) = policy_scope(
        json!({"role": "talky", "keep_recent": 1}),
        "window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], [], 0, '[\"member:e\"]')",
        json!({"rows": rows, "as_of": AS_OF_MS}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    got
}

fn turns(rows: &[Value]) -> Vec<String> {
    let mut t: Vec<String> = rows
        .iter()
        .map(|r| r["turn_id"].as_str().unwrap().to_string())
        .collect();
    t.dedup();
    t
}

#[test]
fn a_replayed_week_ages_by_turn_time() {
    let wall = rows(true);
    let got = plan_of(wall.clone());
    let cover = got["plan"]["cover"].as_i64().expect("a cover");
    let raw: Vec<Value> = wall
        .iter()
        .filter(|r| r["seq"].as_i64().unwrap() > cover)
        .cloned()
        .collect();
    // The day of the newest turn stays raw ...
    assert_eq!(turns(&raw), ["t4", "t5"], "{got}");
    // ... the day before goes to the summary ...
    assert_eq!(
        turns(got["todo"].as_array().unwrap()),
        ["t2", "t3"],
        "{got}"
    );
    assert_eq!(got["plan"]["summary"], json!("pending"));
    // ... and two days back is in no tier: neither raw nor condensed.
    assert!(
        cover >= wall[3]["seq"].as_i64().unwrap(),
        "the rows of 10 October are below the cover: {got}"
    );
    // The clock of age is the newest turn time; the plan keeps both clocks.
    assert_eq!(got["plan"]["as_of_ms"], json!(AS_OF_MS));
    assert_eq!(
        got["plan"]["age_ms"],
        json!(1_791_792_000_000i64),
        "2026-10-12T08:00:00Z: {got}"
    );
}

#[test]
fn an_unstamped_wall_keeps_the_plan_of_before() {
    // The plan curator 1.11.5 made of the same rows without `turn_at`: one
    // ingest day, everything raw, nothing to condense -- byte for byte.
    let golden = json!({"chain": false, "notes": [], "pin_add": [], "pin_audience": [],
        "pin_drop": [], "pin_drop_audience": [],
        "plan": {"as_of_ms": 1_791_795_720_000i64, "cover": 1_791_795_599_999_999i64,
                 "keep": [], "marks_on": {}, "marks_to": 0,
                 "marks_to_by": {"[\"member:e\"]": 0}, "released": [], "role": "talky",
                 "round": "[\"member:e\"]", "shrunk": [], "stubs": [], "sum_from": 0,
                 "sum_to": 0, "summary": "", "v": 1},
        "todo": []});
    assert_eq!(plan_of(rows(false)), golden);
}

#[test]
fn a_replay_ahead_of_the_wall_clock_expires_nothing_early() {
    // A replay instance whose simulated clock runs three days ahead: a block
    // valid for an hour of the wall clock is still valid when the plan is
    // made two minutes later -- age moves to the turn time, expiry does not.
    let mut wall = rows(true);
    for r in wall.iter_mut() {
        r["turn_at"] = json!("2026-10-15T09:00:00Z");
    }
    let until = chrono::DateTime::from_timestamp(AS_OF_MS / 1000 + 3600, 0).unwrap();
    wall[11]["element"] = json!({"origin": "assistant", "type": "text",
                                 "text": "the offer holds for an hour",
                                 "valid_until": until.to_rfc3339()});
    for r in wall.iter_mut().take(11) {
        r["element"] = json!({"origin": "user", "type": "text", "text": "x"});
    }
    let (shown, err) = policy_scope(
        json!({"role": "talky", "keep_recent": 1}),
        "[el['text'] for _, el, _, _ in render(ARGS['rows'], {}, window_plan(ARGS['rows'], [], \
         [], [], {}, ARGS['as_of'], [], [], 0, '[\"member:e\"]')['plan'], None, None, \
         '[\"member:e\"]')]",
        json!({"rows": wall, "as_of": AS_OF_MS}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    let last = shown.as_array().unwrap().last().unwrap().as_str().unwrap();
    assert!(
        last.ends_with("the offer holds for an hour"),
        "not expired on the wall clock: {shown}"
    );
}

/// Rounds of two rows each, ten seconds apart in the ingest minute, two
/// rounds said on each of `days` (`turn_at`).
fn replay(days: &[&str]) -> Vec<Value> {
    let mut out = Vec::new();
    for (i, day) in (0i64..).zip(days.iter().flat_map(|d| [*d, *d])) {
        for j in 0..2i64 {
            let at = chrono::DateTime::from_timestamp(BASE_S + 10 * i + j, 0).unwrap();
            out.push(json!({
                "seq": (BASE_S + 10 * i + j) * 1_000_000, "session_id": "s",
                "turn_id": format!("t{i}"), "iter": 0,
                "kind": if j == 0 { "user" } else { "assistant" },
                "hash": format!("{:064x}", 2 * i + j + 1), "nth": 0, "final": j,
                "at": at.format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
                "turn_at": day, "audience_set": ROUND_E, "chars": 100, "type": "text"}));
        }
    }
    out
}

/// The plan of `rows` after the plan `prev` (its summary made: id, hash and
/// audience as `prev_of` hands them on).
fn plan_after(rows: Vec<Value>, prev: &Value) -> Value {
    let (got, err) = policy_scope(
        json!({"role": "talky", "keep_recent": 1}),
        "window_plan(ARGS['rows'], [], [], [], ARGS['prev'], ARGS['as_of'], [], [], 0, \
         '[\"member:e\"]')",
        json!({"rows": rows, "prev": prev, "as_of": AS_OF_MS}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// A chained summary is dated by the turn of its first row
/// (`sum_from_turn_ms`), never by its ingest `seq` (KD § 1 K-1, review M-4):
/// a replay writes the whole week in one ingest minute.
#[test]
fn a_chained_summary_is_dated_by_its_first_turn() {
    const D10: &str = "2026-10-10T12:00:00Z";
    const D11: &str = "2026-10-11T12:00:00Z";
    const D12: &str = "2026-10-12T08:00:00Z";
    const D13: &str = "2026-10-13T08:00:00Z";
    // The first rebuild: the summary takes the rounds of the 11th.
    let first = plan_of(replay(&[D10, D11, D12]));
    assert_eq!(first["plan"]["summary"], json!("pending"), "{first}");
    let d11_ms = 1_791_720_000_000i64;
    assert_eq!(first["plan"]["sum_from_turn_ms"], json!(d11_ms), "{first}");
    let mut prev = first["plan"].clone();
    prev["summary"] = json!("sum-1");
    prev["hash"] = json!("e".repeat(64));
    prev["sum_audience"] = json!(ROUND_E);

    // The next rebuild the same day: the summary of the 11th is one day back,
    // still in the summary tier -- carried on, dated by its first turn.
    let same_day = plan_after(replay(&[D10, D11, D12, D12]), &prev);
    assert_eq!(same_day["plan"]["summary"], json!("sum-1"), "{same_day}");
    assert_eq!(
        same_day["plan"]["sum_from_turn_ms"],
        json!(d11_ms),
        "{same_day}"
    );

    // A day later the 11th is two days back, in no tier: the chain breaks
    // and the summary is made anew from the 12th. By the ingest `seq` (one
    // minute of the 12th) it would hold and chain on.
    let next_day = plan_after(replay(&[D10, D11, D12, D13]), &prev);
    assert_eq!(next_day["chain"], json!(false), "{next_day}");
    assert_eq!(next_day["plan"]["summary"], json!("pending"), "{next_day}");
    assert_eq!(
        turns(next_day["todo"].as_array().unwrap()),
        ["t4", "t5"],
        "{next_day}"
    );
    assert_eq!(
        next_day["plan"]["sum_from_turn_ms"],
        json!(1_791_792_000_000i64),
        "2026-10-12T08:00:00Z: {next_day}"
    );
}

/// The span the summary leaf names is the time of the turns that went in
/// (`row_time`), not the replay's ingest minute (KD § 1 K-1, review M-5).
#[test]
fn the_summary_leaf_names_the_turn_time_of_its_rows() {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent", json!(1)),
    ]);
    let base = chrono::Utc::now() - chrono::Duration::seconds(50);
    for i in 0..4i64 {
        let at = base + chrono::Duration::milliseconds(100 * i);
        h.row(
            at,
            "s",
            &format!("r{i}"),
            "user",
            &user(&"z".repeat(1000)),
            0,
        );
    }
    let pkg = json!({"input_soft": 300, "input_hard": 100000, "cost_in": 10.0,
                     "cost_cached_in": 0.0, "cache_expires_at": "2099-01-01T00:00:00Z",
                     "tokens_prompt": 5000});
    turn(&mut h, "s", "t1", &"y".repeat(1200), "a", pkg);
    // A replay channel stamped every turn with its own time: one day, a
    // minute per round.
    for (i, t) in ["r0", "r1", "r2", "r3", "t1"].iter().enumerate() {
        h.db.execute(
            "UPDATE wall SET turn_at = ?1 WHERE turn_id = ?2",
            [format!("2026-10-10T12:0{i}:00Z"), t.to_string()],
        )
        .unwrap();
    }
    h.fire(&last_add(&h));
    assert_eq!(h.summ.len(), 1, "{:?}", h.stderr);
    h.answer("Earlier, briefly.", "stop");
    let leaf = h.rows(&format!(
        "SELECT b.body FROM slots s JOIN blocks b ON b.hash = s.hash WHERE s.path = '{}'",
        summary_slot(ROUND_E)
    ));
    let leaf: Value = meclaw_core::serde_json::from_str(leaf[0][0].as_str().unwrap()).unwrap();
    let head = leaf["text"]
        .as_str()
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    assert!(
        head.contains(", 2026-10-10T12:00:00Z .. 2026-10-10T12:0"),
        "the span is the turn time: {head}"
    );
    // Not the ingest stamp of the first row (the minute of the replay).
    let ingest = h.rows("SELECT at FROM wall WHERE turn_id = 'r0'");
    let ingest = ingest[0][0].as_str().unwrap();
    assert!(!head.contains(ingest), "not the ingest minute: {head}");
}
