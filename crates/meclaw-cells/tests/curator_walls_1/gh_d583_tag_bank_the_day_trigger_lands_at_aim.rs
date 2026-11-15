//! d58-3 tag (GH #1527, R-58-20) -- the bank's day trigger lands at its aim,
//! the day before rebuilt, at every horizon.
//!
//! WHY this file exists: the owner's ruling R-58-20 makes the day the
//! starting point of every curator ("not only talky ... all curators work
//! that way"). Before it, only a role with the horizon `day` rebuilt when the
//! day changed (GH #1272); a role at the horizon `task` or `all` carried
//! yesterday raw into today until a harder line fired. With `day_floor` "1"
//! the first turn of a new day orders a rebuild with the reason `day` at
//! every horizon (contract point 3 of the plan part `core-tag`).
//!
//! The bank's `Trigger::Day` runs a wall as the day before: one turn on the
//! wall's day under every line, the lab clock moved to the morning after
//! (`morning_after`, relative to the wall, never the machine's clock), the
//! first turn there orders the rebuild. Per wall (`phone_day`, a day of
//! talk; `coding`, a task) and per horizon (`task`, `all`, `day`), with the
//! raw token budget off so the day before takes its tier form: the rebuild
//! comes about with the reason `day`, the window after it lies at its aim
//! (`lands_at_aim`), and every row of the wall -- the day before -- lies
//! under the cover. The switch is a declared param of the policy.
//!
//! What this pin does not say (review M-1): that the running day stays raw.
//! At the day change the running day is the morning turn alone, and the
//! newest round is never cut, whatever the horizon -- such a statement could
//! not turn red. It is carried by `gh_d583_tag_task_keeps_today_raw_with_the_floor`
//! and `gh_d583_tag_all_keeps_today_raw_with_the_floor` (a whole day of
//! rounds, a counter arm without the floor) and by the capture
//! `gh_d583_tag_bank_the_day_window_is_captured`.
//!
//! Runs the shipped curator hive over the bank (`support/curator_bank.rs`).

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

/// The walls of the statement and the example value set each runs as.
const WALLS: [(&str, &str); 2] = [("phone_day", "talky"), ("coding", "coding")];

/// The run of `role` at `horizon` over the day change, the floor on, the
/// raw token budget off.
fn day_run(role: &'static str, horizon: &str) -> Run {
    let mut run = Run::of(role)
        .with("horizon", json!(horizon))
        .with("keep_recent_tokens", json!(0))
        .with("day_floor", json!("1"));
    run.trigger = Trigger::Day;
    run
}

#[test]
fn the_day_trigger_lands_at_aim() {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    for (name, role) in WALLS {
        let wall = bank.named(name).clone();
        for horizon in HORIZONS {
            let run = day_run(role, horizon);
            let who = format!("{name} as {}", run.label());
            let Some(o) = found.judge(&format!("{who}: the day change rebuilds"), || {
                rebuild(&wall, &run)
            }) else {
                continue;
            };
            found.judge(&format!("{who}: aim"), || lands_at_aim(&o));
            found.judge(&format!("{who}: the day before is rebuilt"), || {
                let cover = o.cover();
                let raw: Vec<usize> = (0..o.seqs.len()).filter(|i| o.seqs[*i] > cover).collect();
                assert!(
                    raw.is_empty(),
                    "{who}: every row of the day before lies under the cover {cover}; above it \
                     stand the pieces {raw:?}\n{}",
                    o.view()
                );
            });
        }
    }
    found.judge("the switch", || {
        assert!(
            declared("policy", "day_floor"),
            "templates/curator/policy/config.json declares the switch day_floor"
        )
    });
    found.finish();
}
