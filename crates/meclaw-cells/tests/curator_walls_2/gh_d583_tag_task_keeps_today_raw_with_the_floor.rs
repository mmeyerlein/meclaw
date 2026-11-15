//! d58-3 tag (GH #1527, R-58-20) -- at the horizon `task` the running day
//! stays raw with the floor.
//!
//! WHY this file exists: R-58-20 makes the day the starting point of every
//! curator. At the horizon `task` a segment opens at every new topic, and a
//! closed topic of this morning aged past the raw tier by the evening: a
//! rebuild at the target folded it although it was said today. With
//! `day_floor` "1" every row of the running day (the UTC calendar day of
//! `day_of`) has the age 0 and stays raw; the rows of the day before keep
//! the logic of the horizon (contract point 2 of the plan part `core-tag`).
//!
//! The wall: three rounds the day before, four today, each round today but
//! the first opening a topic of its own; the rebuild on the running day
//! (`window_today`): the order of an expired cache, no limit, aimed at the
//! aim the package names (`rebuild_to` of the soft line, far over the
//! wall). No overflow cuts anything (that a hard limit still cuts today is
//! the statement of `gh_d583_tag_a_hard_limit_still_cuts_today`), so the
//! day before leaves the raw part by its tier alone, and a floor that held
//! every row at the age 0 would leave it raw (review M-2).
//! One round is kept raw beside the tier (`keep_recent` 1) and the raw
//! token budget is off, so what stays raw stays raw by its age. The same
//! wall with `day_floor` "0" folds rows of the running day: the counter
//! arm that makes the pin cut.
//!
//! Runs the shipped curator hive over a wall of the bank's builder
//! (`support/curator_bank.rs`); the lab clock, never the machine's.

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_bank::*;
use curator_hive::{Hive, canonical, shipped};
use meclaw_core::serde_json::json;

/// The session of the wall and of the two turns of the run.
const SESSION: &str = "s-tag";

/// The rounds of the day before and of the running day, by turn id.
const YESTERDAY: [&str; 3] = ["y1", "y2", "y3"];
const TODAY: [&str; 4] = ["t1", "t2", "t3", "t4"];

/// The instant of round `i` of a day `days` from the bank's day.
fn at_round(days: i64, i: usize) -> chrono::DateTime<chrono::Utc> {
    day(days, 9 + i as i64, 0)
}

/// Two days of talk: three rounds the day before, four rounds today; the
/// rebuild plans at 18:00 of today.
fn two_days(name: &'static str) -> Wall {
    let mut w = WallBuilder::new(name, "quillon");
    for (days, turns) in [(-1, &YESTERDAY[..]), (0, &TODAY[..])] {
        for (i, turn) in turns.iter().enumerate() {
            w.at(at_round(days, i));
            w.words(SESSION, turn, "What about the garden today, {k}?");
            w.answer(SESSION, turn, "The roses want water before noon, {k}.");
        }
    }
    w.build(day(0, 18, 0), SESSION)
}

/// The pieces of the day before (two per round, the first of the wall).
fn yesterday_pieces() -> std::ops::Range<usize> {
    0..2 * YESTERDAY.len()
}

/// The pieces of the running day (two per round, after the day before).
fn today_pieces() -> std::ops::Range<usize> {
    2 * YESTERDAY.len()..2 * (YESTERDAY.len() + TODAY.len())
}

/// The run of `wall` as the example set `consult`: one round kept raw
/// beside the tier, the raw token budget off, the floor `floor`, `extra`
/// on top.
fn run_of(floor: &'static str, extra: &[(&'static str, &'static str)]) -> Run {
    let mut run = Run::of("consult")
        .with("keep_recent", json!(1))
        .with("keep_recent_tokens", json!(0))
        .with("day_floor", json!(floor));
    for (k, v) in extra {
        run = run.with(k, json!(v));
    }
    run
}

/// The rebuild at a glance, for the message of a failed statement.
fn view(w: &DayWindow) -> String {
    let mut plan = w.plan.clone();
    if let Some(p) = plan.as_object_mut() {
        p.remove("blocks");
        p.remove("marks_on");
    }
    format!(
        "curate_mark {} reason {} requests {} seqs {:?}\nplan: {plan}",
        w.mark, w.reason, w.requests, w.seqs
    )
}

/// The rebuild of `wall` on the running day with the floor on; then the
/// statement: the rebuild ran on the order of the cache, every row of the
/// day before left the raw part by its tier (no overflow at the aim of the
/// cache), every row of the running day stays raw, the same rebuild with
/// `day_floor` "0" folds rows of the running day (the counter arm: without
/// it the pin would not cut), and the switch is declared.
fn keeps_today_raw(wall: &Wall, extra: &[(&'static str, &'static str)]) {
    let who = format!("{} as consult {extra:?}", wall.name);
    let mut found = Findings::default();
    if let Some(w) = found.judge("rebuild", || window_today(wall, &run_of("1", extra))) {
        found.judge("the order", || {
            assert!(
                w.mark == "none" && w.reason == "cache",
                "{who}: the rebuild runs on the order of the cache, no line crossed\n{}",
                view(&w)
            )
        });
        for i in yesterday_pieces() {
            found.judge("the day before", || {
                assert!(
                    !w.raw(i),
                    "{who}: the row {} of the day before left the raw part by its tier\n{}",
                    wall.pieces[i].key,
                    view(&w)
                )
            });
        }
        for i in today_pieces() {
            found.judge("the running day", || {
                assert!(
                    w.raw(i),
                    "{who}: the row {} of the running day stays raw (age 0 with day_floor \"1\")\n{}",
                    wall.pieces[i].key,
                    view(&w)
                )
            });
        }
    }
    if let Some(w) = found.judge("rebuild without the floor", || {
        window_today(wall, &run_of("0", extra))
    }) {
        found.judge("the counter arm", || {
            assert!(
                today_pieces().any(|i| !w.raw(i)),
                "{who}: with day_floor \"0\" the same rebuild folds rows of the running day \
                 (else the pin does not cut)\n{}",
                view(&w)
            )
        });
    }
    found.judge("the switch", || {
        assert!(
            declared("policy", "day_floor"),
            "templates/curator/policy/config.json declares the switch day_floor"
        )
    });
    found.finish();
}

/// A topic of its own on every round of the running day but the first
/// (`start`): at the horizon `task` the day falls into four segments, and
/// without the floor the three older ones age past the raw tier.
fn topics_of_today(h: &mut Hive) {
    for (i, turn) in TODAY.iter().enumerate().skip(1) {
        let start = canonical(&json!({"movement": "start", "name": format!("errand {turn}")}));
        h.mark(at_round(0, i), SESSION, turn, "topic", &start);
    }
}

#[test]
fn task_keeps_today_raw_with_the_floor() {
    if !shipped() {
        return;
    }
    let wall = two_days("tag_task").prepare(topics_of_today);
    keeps_today_raw(&wall, &[]);
}
