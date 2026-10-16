//! GH #1225 (K-2, OR-KF-6) -- a segment has no ceiling any more.
//!
//! The cover of the curator's window moved only at a segment's edge (the UTC
//! day of talky, the task of consult/coding/research): a day longer than the
//! window stayed raw whole, the hard line of the model was never guarded, and
//! a rebuild whose summary failed was ordered again on every turn. Now the
//! plan moves the oldest rounds beyond `keep_recent` into the summary until it
//! fits its aim, the call itself is cut under `HARD_HEADROOM` of `input_hard`
//! (and says so), and a failed rebuild backs off.
//!
//! Runs the shipped curator hive and its pure half (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// 2026-10-12T09:00:00Z.
const BASE_S: i64 = 1_791_795_600;

fn count(h: &Hive, kind: &str) -> i64 {
    h.rows(&format!("SELECT COUNT(*) FROM marks WHERE kind = '{kind}'"))[0][0]
        .as_i64()
        .unwrap()
}

#[test]
fn a_day_longer_than_the_window_still_gets_cut() {
    // 300 rounds of 4 000 characters on one day (the review's
    // `probe_raw_unbounded.py`): 400 000 tokens at three characters a token,
    // against an aim out of luna's 250 000 soft.
    let mut wall = Vec::new();
    for i in 0..300i64 {
        for j in 0..2i64 {
            wall.push(json!({
                "seq": (BASE_S + 2 * i + j) * 1_000_000, "session_id": "s",
                "turn_id": format!("t{i}"), "iter": 0,
                "kind": if j == 0 { "user" } else { "assistant" },
                "hash": format!("{:064x}", 2 * i + j + 1), "nth": 0, "final": j,
                "at": chrono::DateTime::from_timestamp(BASE_S + 2 * i + j, 0).unwrap()
                    .format("%Y-%m-%dT%H:%M:%S.000000Z").to_string(),
                "audience_set": ROUND_E, "chars": 2000, "type": "text"}));
        }
    }
    let probe = "(lambda r, a: {'todo': len(r['todo']), 'cover': r['plan']['cover'], 'aim': a, \
                 'est': sum(x['chars'] for x in ARGS['rows'] if x['seq'] > r['plan']['cover']) \
                 / CHARS_PER_TOKEN})(window_plan(ARGS['rows'], [], [], [], {}, ARGS['as_of'], [], \
                 [], 0, '[\"member:e\"]', aim=rebuild_to_of({'input_soft': 250000})), \
                 rebuild_to_of({'input_soft': 250000}))";
    for role in ["talky", "consult", "coding"] {
        let (got, err) = policy_scope(
            json!({"role": role}),
            probe,
            json!({"rows": wall, "as_of": (BASE_S + 3600) * 1000}),
        );
        assert!(!err.contains("Traceback"), "{role}: {err}");
        assert!(got["todo"].as_i64().unwrap() > 0, "{role}: {got}");
        assert!(
            got["cover"].as_i64().unwrap() >= wall[0]["seq"].as_i64().unwrap(),
            "{role}: the cover moved within the day: {got}"
        );
        let (est, aim) = (got["est"].as_f64().unwrap(), got["aim"].as_f64().unwrap());
        assert!(aim > 0.0 && est <= aim * 1.1, "{role}: {got}");
    }
}

/// A package with a hard line of `hard` tokens, and a soft one of `soft`.
fn package(soft: i64, hard: i64, tokens_prompt: i64) -> Value {
    json!({"input_soft": soft, "input_hard": hard, "cost_in": 10.0, "cost_cached_in": 0.0,
           "cache_expires_at": "2099-01-01T00:00:00Z", "tokens_prompt": tokens_prompt})
}

/// `n` earlier rounds of `chars` characters each, written straight into the
/// ledger just before now.
fn earlier_rounds(h: &mut Hive, n: i64, chars: usize) {
    rounds_from(
        h,
        chrono::Utc::now() - chrono::Duration::seconds(50),
        n,
        chars,
    );
}

/// `n` rounds of `chars` characters each, written straight into the ledger
/// from `base` on.
fn rounds_from(h: &mut Hive, base: chrono::DateTime<chrono::Utc>, n: i64, chars: usize) {
    for i in 0..n {
        let at = base + chrono::Duration::milliseconds(100 * i);
        let text = format!("{i} {}", "x".repeat(chars / 2));
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
}

#[test]
fn a_call_over_input_hard_leaves_cut_and_marked() {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent", json!(2)),
    ]);
    // The model's hard line: 4 000 tokens, the guard's margin 3 600.
    turn(&mut h, "s", "t0", "hello", "hi", package(2000, 4000, 100));
    // About 16 000 tokens of wall, and no plan: no rebuild has run.
    earlier_rounds(&mut h, 20, 2400);
    let call = h.curate("s", "t1", 0, json!([user("now")]), mode("Be brief."));
    let cuts: Vec<Value> = call.hop["cuts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c["what"] == "overflow_cut")
        .collect();
    assert_eq!(cuts.len(), 1, "{:?} {:?}", call.hop, h.stderr);
    assert!(cuts[0]["shown"].as_i64().unwrap() < 3600, "{cuts:?}");
    assert!(cuts[0]["total"].as_i64().unwrap() >= 3600, "{cuts:?}");
    // What leaves the cell is under the margin, by the cell's own estimate.
    let (est, _) = policy_scope(json!({}), "estimate_tokens(ARGS)", json!(call.messages()));
    assert!(est.as_i64().unwrap() < 3600, "{est}");
    assert!(
        texts(&call).iter().any(|t| t.ends_with("now")),
        "the newest round stays"
    );
    // Exactly one mark says it.
    assert_eq!(count(&h, "overflow_cut"), 1);
    let mark: Value = meclaw_core::serde_json::from_str(
        h.rows("SELECT value FROM marks WHERE kind = 'overflow_cut'")[0][0]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(
        mark["after"].as_i64().unwrap() < mark["limit"].as_i64().unwrap(),
        "{mark}"
    );
    assert_eq!(mark["limit"], json!(3600));
    // The answer comes back: the rebuild is ordered `hard`.
    h.tap(
        &call,
        "stop",
        package(2000, 4000, 3000),
        json!([said("ok")]),
    );
    assert_eq!(
        last_add(&h).body["emit_body"]["reason"],
        "hard",
        "{:?}",
        h.clock
    );
}

#[test]
fn a_failing_summary_does_not_reorder_every_turn() {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent", json!(1)),
        ("policy", "amortise_turns", json!(3)),
    ]);
    earlier_rounds(&mut h, 4, 1000);
    let ask = "y".repeat(1200);
    // Over soft: a rebuild is ordered, and its summary fails.
    turn(&mut h, "s", "t1", &ask, "a", package(300, 100_000, 5000));
    assert_eq!(last_add(&h).body["emit_body"]["reason"], "soft");
    h.fire(&last_add(&h));
    assert_eq!(h.summ.len(), 1, "{:?}", h.stderr);
    h.answer("", "error");
    // The next turns over the same line back off: no summarizer call.
    for (i, t) in ["t2", "t3", "t4"].into_iter().enumerate() {
        turn(&mut h, "s", t, &ask, "a", package(300, 100_000, 5000));
        assert_eq!(last_add(&h).body["emit_body"]["reason"], "soft");
        h.fire(&last_add(&h));
        assert!(
            h.summ.is_empty(),
            "turn {t}: no summarizer call: {:?}",
            h.summ
        );
        assert_eq!(count(&h, "rebuild_backoff"), i as i64 + 1);
    }
    // After `amortise_turns` orders the rebuild runs again.
    turn(&mut h, "s", "t5", &ask, "a", package(300, 100_000, 5000));
    h.fire(&last_add(&h));
    assert_eq!(h.summ.len(), 1, "{:?}", h.stderr);
    assert_eq!(count(&h, "rebuild_backoff"), 3);
}

/// A plan still over its aim with only the rounds it keeps raw says so: one
/// `over_aim` mark per rebuild, with the plan's estimate, its aim and the call
/// that ordered it (KD § 1 K-2, review I-1).
#[test]
fn a_plan_over_its_aim_leaves_one_over_aim_mark() {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent", json!(10)),
    ]);
    earlier_rounds(&mut h, 4, 2000);
    let call = turn(
        &mut h,
        "s",
        "t1",
        &"y".repeat(1200),
        "a",
        package(300, 100_000, 5000),
    );
    assert_eq!(last_add(&h).body["emit_body"]["reason"], "soft");
    h.fire(&last_add(&h));
    // `keep_recent` holds every round of the day: nothing to condense, the
    // plan stands over its aim.
    assert!(h.summ.is_empty(), "{:?}", h.summ);
    assert_ne!(h.plan_in(ROUND_E), json!({}), "{:?}", h.stderr);
    assert_eq!(count(&h, "over_aim"), 1, "{:?}", h.stderr);
    let mark: Value = meclaw_core::serde_json::from_str(
        h.rows("SELECT value FROM marks WHERE kind = 'over_aim'")[0][0]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let (est, aim) = (
        mark["estimate"].as_i64().unwrap(),
        mark["aim"].as_i64().unwrap(),
    );
    assert!(aim > 0 && est > aim, "{mark}");
    assert_eq!(mark["call"], call.hop["curator_call"], "{mark}");
}

/// The backoff of a failed rebuild ends when the cache goes cold: an order of
/// a line crossed backs off, the `cache` order of a turn under every line
/// runs the summarizer (KD § 1 K-2 "before the next cold cache or
/// `amortise_turns`", review M-1).
#[test]
fn a_cold_cache_ends_the_backoff() {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent", json!(1)),
        ("policy", "amortise_turns", json!(3)),
    ]);
    // Yesterday's rounds: the summary tier, whatever the aim.
    rounds_from(&mut h, days_ago(1, 12), 4, 1000);
    // The line is the package's (`tokens_prompt` over soft); today's round
    // is short, so the plan keeps its summary under the aim.
    turn(&mut h, "s", "t1", "q", "a", package(300, 100_000, 5000));
    h.fire(&last_add(&h));
    assert_eq!(h.summ.len(), 1, "{:?}", h.stderr);
    h.answer("", "error");
    // Over soft again: the order backs off.
    turn(&mut h, "s", "t2", "q", "a", package(300, 100_000, 5000));
    assert_eq!(last_add(&h).body["emit_body"]["reason"], "soft");
    h.fire(&last_add(&h));
    assert!(h.summ.is_empty(), "{:?}", h.summ);
    assert_eq!(count(&h, "rebuild_backoff"), 1);
    // Under every line the clock waits for the cache; its strike runs.
    turn(&mut h, "s", "t3", "q", "a", package(300, 100_000, 50));
    let order = last_add(&h);
    assert_eq!(order.body["emit_body"]["reason"], "cache");
    h.fire(&order);
    assert_eq!(h.summ.len(), 1, "the cold cache rebuilds: {:?}", h.stderr);
    assert_eq!(count(&h, "rebuild_backoff"), 1);
}
