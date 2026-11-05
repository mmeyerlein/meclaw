//! GH #1427 (D38 K6, R-38-2) -- the budget never outgrows the aim.
//!
//! WHY this file exists: a raw budget above the aim of the running rebuild
//! would hold the window over its target after every `target` rebuild, and
//! the hold (`target_hold`, GH #1272 review I-1) would let it swing up to
//! `window_target` plus the budget -- over the price step of the model
//! (R-38-2: 100k the guide value, 200k the stop). So the budget a rebuild
//! works with is `min(keep_recent_tokens, aim - the rest)`: the aim of the
//! mark that ordered it (`target_aim_of`, `rebuild_to_of`, `cost_aim_of`)
//! less the estimate of what is not raw (the system part, the summary); an
//! aim of 0 leaves the knob alone. talky's presets (OR-38-12/18): the target
//! 100 000, `rebuild_to` 0.45 (the aim 90 000), the budget 80 000.
//!
//! Runs the pure half of the shipped policy and the shipped curator hive
//! (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// 2026-03-01T00:00:00Z and the day after, in seconds.
const DAY_1_S: i64 = 1_772_323_200;
const DAY_2_S: i64 = 1_772_409_600;

fn row_s(i: i64, j: i64, yesterday: i64) -> i64 {
    let day = if i < yesterday { DAY_1_S } else { DAY_2_S };
    day + 3600 + 2 * i + j
}

fn seq_of(i: i64, j: i64, yesterday: i64) -> i64 {
    row_s(i, j, yesterday) * 1_000_000
}

/// `yesterday` rounds on day one, `today` rounds on day two, two rows of
/// `chars` characters each (`chars` / 3 tokens a row).
fn wall(yesterday: i64, today: i64, chars: i64) -> Vec<Value> {
    let mut rows = Vec::new();
    for i in 0..yesterday + today {
        for j in 0..2i64 {
            let t = row_s(i, j, yesterday);
            rows.push(json!({
                "seq": t * 1_000_000, "session_id": "s", "turn_id": format!("r{i}"), "iter": 0,
                "kind": if j == 0 { "user" } else { "assistant" },
                "hash": format!("{:064x}", 2 * i + j + 1), "nth": 0, "final": j,
                "at": chrono::DateTime::from_timestamp(t, 0).unwrap()
                    .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
                "audience_set": ROUND_E, "chars": chars, "type": "text"}));
        }
    }
    rows
}

/// The plan of a rebuild with `aim` and a system part of `system_chars`
/// characters: `{cover, todo, over_aim}`.
fn plan_at(over: Value, rows: &[Value], aim: i64, system_chars: i64) -> Value {
    let probe = "(lambda r: {'cover': r['plan']['cover'], 'todo': len(r['todo']), \
                 'over_aim': r.get('over_aim')})(window_plan(ARGS['rows'], [], [], [], {}, \
                 ARGS['as_of'], [['mode', 'a' * 64, ARGS['system']]], [], 0, None, \
                 aim=ARGS['aim']))";
    let (got, err) = policy_scope(
        over,
        probe,
        json!({"rows": rows, "as_of": (DAY_2_S + 7200) * 1000, "aim": aim,
               "system": system_chars}),
    );
    assert!(!err.contains("Traceback"), "{err}");
    got
}

/// 60 rounds of 2 000 tokens, a budget of 80 000, an aim of 50 000 and a
/// system part of 2 000 tokens: the budget the rebuild works with is 48 000,
/// 24 rounds stay raw and the plan lands on its aim (no `over_aim`). Without
/// an aim the knob alone rules: 40 rounds.
#[test]
fn the_budget_never_outgrows_the_aim() {
    let rows = wall(59, 1, 3000);
    let over = json!({"role": "talky", "keep_recent": 1, "keep_recent_tokens": 80_000});
    let got = plan_at(over.clone(), &rows, 50_000, 6000);
    assert_eq!(
        got["cover"],
        json!(seq_of(36, 0, 59) - 1),
        "24 rounds of 2 000 under an aim of 50 000 less 2 000 of system: {got}"
    );
    assert_eq!(got["todo"], json!(72), "{got}");
    assert_eq!(got["over_aim"], Value::Null, "the plan fits its aim: {got}");

    let got = plan_at(over, &rows, 0, 6000);
    assert_eq!(
        got["cover"],
        json!(seq_of(20, 0, 59) - 1),
        "without an aim the knob alone: {got}"
    );
}

/// The turn's stamp the policy keeps for the round of the call.
fn stamp(h: &Hive) -> Value {
    meclaw_core::serde_json::from_str(&h.state_in("curate", ROUND_E)).unwrap_or(Value::Null)
}

fn count(h: &Hive, kind: &str) -> i64 {
    h.rows(&format!("SELECT COUNT(*) FROM marks WHERE kind = '{kind}'"))[0][0]
        .as_i64()
        .unwrap()
}

/// A round whose user turn holds `chars` characters, answered with `usage`.
fn turn_of(h: &mut Hive, turn: &str, chars: usize, usage: Value) {
    let call = h.curate(
        "s",
        turn,
        0,
        json!([user(&"x".repeat(chars))]),
        mode("Be brief."),
    );
    h.tap(&call, "stop", usage, json!([said("a")]));
}

fn usage(tokens_prompt: i64) -> Value {
    json!({"tokens_prompt": tokens_prompt, "input_soft": 250_000, "input_hard": 1_000_000})
}

/// talky as it ships: 90 rounds of today, about 1 800 tokens each (162 000 in
/// all), a window of 101 000 over the target of 100 000 orders a `target`
/// rebuild to the aim of 90 000 (100 000 x 0.45 / 0.5). After it the raw
/// part is under the aim -- the budget's 44 rounds at least (80 000 / 1 800;
/// the floor of 40 rounds is 72 000, under the aim, so the budget decides),
/// the overflow moves the rest to the summary until the plan fits -- and the
/// plan is not over its aim; the next turn under the target is marked `none` without a
/// hold, and its stamp names the raw part.
#[test]
fn after_a_target_rebuild_the_raw_part_is_under_the_aim() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
    let base = chrono::Utc::now() - chrono::Duration::seconds(300);
    for i in 0..90i64 {
        let at = base + chrono::Duration::milliseconds(100 * i);
        let text = format!("{i:02} {}", "x".repeat(2697));
        h.row(at, "s", &format!("r{i}"), "user", &user(&text), 0);
        h.row(
            at + chrono::Duration::milliseconds(10),
            "s",
            &format!("r{i}"),
            "assistant",
            &said(&text),
            1,
        );
    }
    turn_of(&mut h, "t1", 30, usage(101_000));
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "target", "{s} {:?}", h.stderr);
    assert_eq!(
        h.state_in("aim", ROUND_E),
        "90000",
        "100 000 x rebuild_to 0.45 / compress_at 0.5"
    );
    let order = h
        .clock_order_for(ROUND_E)
        .expect("the target orders a rebuild");
    h.fire(&order);
    assert_eq!(
        h.summ.len(),
        1,
        "the oldest rounds condense: {:?}",
        h.stderr
    );
    h.answer("Earlier: many rounds of x.", "stop");
    let plan = h.plan_in(ROUND_E);
    let cover = plan["cover"].as_i64().expect("a cover");
    // The rows above the cover with their blocks' size: the raw part, by the
    // plan's own rule (`chars` / 3).
    let raw: Vec<(i64, String)> = h
        .rows(&format!(
            "SELECT b.chars, w.turn_id FROM wall w JOIN blocks b ON b.hash = w.hash \
             WHERE w.seq > {cover} ORDER BY w.seq"
        ))
        .into_iter()
        .map(|r| (r[0].as_i64().unwrap(), r[1].as_str().unwrap().to_string()))
        .collect();
    let raw_rounds = raw
        .iter()
        .map(|(_, t)| t.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len() as i64;
    let raw_est: i64 = raw.iter().map(|(c, _)| c).sum::<i64>() / 3;
    assert!(
        raw_est <= 90_000,
        "the raw part is under the aim: {raw_est} tokens in {raw_rounds} rounds, {plan}"
    );
    assert!(
        raw_rounds >= 44,
        "the budget's own rounds stay (80 000 / 1 800): {raw_rounds} {plan}"
    );
    assert_eq!(count(&h, "over_aim"), 0, "{:?}", h.stderr);

    // The next turn, under the target: no hold, and the raw part in the stamp.
    turn_of(&mut h, "t2", 30, usage(92_000));
    let s = stamp(&h);
    assert_eq!(s["curate_mark"], "none", "{s} {:?}", h.stderr);
    assert!(s.get("target_hold").is_none(), "no hold: {s}");
    let raw_tokens = s["raw_tokens"]
        .as_i64()
        .unwrap_or_else(|| panic!("raw_tokens in the stamp: {s}"));
    assert!(raw_tokens > 0 && raw_tokens <= 90_000, "{s}");
    let stamped_rounds = s["raw_rounds"]
        .as_i64()
        .unwrap_or_else(|| panic!("raw_rounds in the stamp: {s}"));
    assert!(
        stamped_rounds >= raw_rounds && stamped_rounds <= raw_rounds + 1,
        "the rounds above the cover, the running one among them or not: {s} {raw_rounds}"
    );
}
