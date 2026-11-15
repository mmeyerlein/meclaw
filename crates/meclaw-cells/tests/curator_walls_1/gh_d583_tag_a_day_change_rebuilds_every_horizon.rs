//! d58-3 tag (GH #1527, R-58-20) -- with the floor a day change rebuilds at
//! every horizon, once.
//!
//! WHY this file exists: until R-58-20 only a role with the horizon `day`
//! rebuilt when its turn was on another UTC day than its last rebuild (mark
//! `day`, state `last_rebuild_day`, GH #1272); a role at `task` or `all`
//! never did, and the day before stood raw beside today until a harder line
//! fired. With `day_floor` "1" the day change orders the rebuild with the
//! reason `day` at every horizon (contract point 3 of the plan part
//! `core-tag`), and the cut of the day (`day_cut`) keeps the rounds of age
//! 0 -- the running day -- raw.
//!
//! Per horizon (`task`, `all`, `day`) the example set `consult` runs a wall
//! of yesterday over the day change (the bank's `Trigger::Day`: a turn on
//! the wall's day, the lab clock at the morning after, the first turn
//! there): the rebuild comes about with the reason `day`, every row of the
//! day before lies under the cover, and the turn after the rebuild, on the
//! same day, orders no second one (the day of the rebuild is kept). The raw
//! token budget is off, so the day before takes its tier form.
//!
//! The counter arm (re-check M-B): the same run with `day_floor` "0" at
//! `task` and `all` orders no rebuild of the day at the first turn of the
//! morning -- as shipped, only the horizon `day` does (GH #1272).
//!
//! That the running day stays raw is not stated here (review M-1): at the
//! day change it is the morning turn alone, which no horizon ever cuts. The
//! statement lives in `gh_d583_tag_task_keeps_today_raw_with_the_floor` and
//! `gh_d583_tag_all_keeps_today_raw_with_the_floor`, over a whole day of
//! rounds and with a counter arm.
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
use curator_hive::shipped;
use meclaw_core::serde_json::json;

/// Every horizon the policy knows.
const HORIZONS: [&str; 3] = ["task", "all", "day"];

/// The session of the wall and of the turns of the run.
const SESSION: &str = "s-tag-day";

/// Yesterday: four rounds of talk, the rebuild of the bank planned at 20:00.
fn yesterday() -> Wall {
    let mut w = WallBuilder::new("tag_day_change", "brindle");
    for i in 0..4usize {
        let turn = format!("y{i}");
        w.at(day(0, 9 + 2 * i as i64, 0));
        w.words(
            SESSION,
            &turn,
            "Did the parcel for the neighbour come, {k}?",
        );
        w.answer(SESSION, &turn, "Not yet; the van comes after lunch, {k}.");
    }
    w.build(day(0, 20, 0), SESSION).target(20_000)
}

#[test]
fn a_day_change_rebuilds_every_horizon() {
    if !shipped() {
        return;
    }
    let wall = yesterday();
    let mut found = Findings::default();
    for horizon in ["task", "all"] {
        let mut off = Run::of("consult")
            .with("horizon", json!(horizon))
            .with("keep_recent_tokens", json!(0))
            .with("day_floor", json!("0"));
        off.trigger = Trigger::Day;
        let who = format!("{} as {}", wall.name, off.label());
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rebuild(&wall, &off)));
        found.judge(&format!("{who}: no day rebuild without the floor"), || {
            let said = match &ran {
                Ok(o) => format!("a rebuild with the plan {}", o.plan),
                Err(e) => e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default(),
            };
            assert!(
                said.contains("orders no rebuild of the day"),
                "{who}: without the floor the first turn of the morning orders no rebuild of \
                 the day at the horizon {horizon}; it came to {said}"
            )
        });
    }
    for horizon in HORIZONS {
        let mut run = Run::of("consult")
            .with("horizon", json!(horizon))
            .with("keep_recent_tokens", json!(0))
            .with("day_floor", json!("1"));
        run.trigger = Trigger::Day;
        let who = format!("{} as {}", wall.name, run.label());
        let Some(o) = found.judge(&format!("{who}: the day change rebuilds"), || {
            rebuild(&wall, &run)
        }) else {
            continue;
        };
        found.judge(&format!("{who}: the day before"), || {
            let cover = o.cover();
            let raw: Vec<usize> = (0..o.seqs.len()).filter(|i| o.seqs[*i] > cover).collect();
            assert!(
                raw.is_empty(),
                "{who}: every row of the day before lies under the cover {cover}; above it \
                 stand the pieces {raw:?}\n{}",
                o.view()
            );
        });
        found.judge(&format!("{who}: one day, one rebuild"), || {
            assert_ne!(
                o.after["curate_mark"],
                json!("day"),
                "{who}: the turn after the rebuild is on the same day and orders no second \
                 rebuild of the day: {}",
                o.after
            );
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
