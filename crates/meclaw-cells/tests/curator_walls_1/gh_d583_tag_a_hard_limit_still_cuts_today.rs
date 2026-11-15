//! d58-3 tag (GH #1527, R-58-20) -- the hard lines still cut the running day.
//!
//! WHY this file exists: the owner's ruling R-58-20 reads "the day/session
//! boundary holds only up to the other hard limits". The floor of
//! `day_floor` keeps the rows of the running day raw by their age; it is no
//! shield against a window that does not fit. The lines before it stay in
//! front, in the order of `curate_mark` (`hard`, `soft`, `day`, `target`,
//! `cost`), and a rebuild they order still moves the cover into today when
//! the day alone is over the aim (contract point 4 of the plan part
//! `core-tag`).
//!
//! Two lines, at every horizon (`task`, `all`, `day`), with the floor on,
//! over one day of long talk that is all today:
//!
//! - `target`: the wall stands over its target and its aim; the rebuild of
//!   the bank at the target (`rebuild`) moves the cover past the oldest row
//!   of today, and the window lands at its aim or says `over_aim`.
//! - `hard`: the turn reports a window over the model's `input_hard`; the
//!   turn's mark is `hard`, and the rebuild it orders moves the cover past
//!   the oldest row of today.
//!
//! The switch is a declared param of the policy.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`), behind the
//! bank's clock (`support/curator_bank.rs`); the lab clock, never the
//! machine's.

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
use curator_hive::{Hive, ROUND_E, mode, said, shipped, user};
use meclaw_core::serde_json::{self as sj, Value, json};

/// Every horizon the policy knows.
const HORIZONS: [&str; 3] = ["task", "all", "day"];

/// The session of the day.
const SESSION: &str = "s-tag-hard";

/// Rounds of the day.
const ROUNDS: usize = 8;

/// The text of round `i`: long, so the day alone is over every aim here.
fn long(i: usize, who: &str) -> String {
    format!("{who} {i}: {} {{k}}", filler(300 + i as u64, 20))
}

/// One day of long talk, all of it today; the bank's rebuild at 18:00 at a
/// small target.
fn long_day() -> Wall {
    let mut w = WallBuilder::new("tag_long_day", "marlinspike");
    for i in 0..ROUNDS {
        let turn = format!("d{i}");
        w.at(day(0, 9, 5 * i as i64));
        w.words(SESSION, &turn, &long(i, "Question"));
        w.answer(SESSION, &turn, &long(i, "Answer"));
    }
    w.build(day(0, 18, 0), SESSION).target(3_000)
}

/// The example set `consult` at `horizon`, one round kept raw beside the
/// tier, the raw token budget off, the floor on.
fn floor_run(horizon: &'static str) -> Run {
    Run::of("consult")
        .with("horizon", json!(horizon))
        .with("keep_recent", json!(1))
        .with("keep_recent_tokens", json!(0))
        .with("day_floor", json!("1"))
}

/// The line `target`: the bank's rebuild at the target of the wall.
fn target_cuts_today(wall: &Wall, horizon: &'static str) {
    let o = rebuild(wall, &floor_run(horizon));
    let who = o.who();
    assert!(
        !o.raw(0),
        "{who}: the rebuild at the target moves the cover into today (the oldest row of the day \
         left the raw part)\n{}",
        o.view()
    );
    lands_at_aim(&o);
}

/// The line `hard`: the wall written into the ledger, one turn whose window
/// lies over the model's `input_hard`, the rebuild it orders fired and
/// condensed by a stub.
fn hard_cuts_today(wall: &Wall, horizon: &'static str) {
    let run = floor_run(horizon);
    let who = format!("{} as {} over input_hard", wall.name, run.label());
    let over: Vec<(&str, &str, Value)> = run
        .knobs
        .iter()
        .filter(|(k, _)| *k == "day_floor")
        .map(|(k, v)| ("policy", *k, v.clone()))
        .collect();
    let mut h = Hive::with_prelude(&over, CLOCK);
    h.set_clock(day(0, 8, 0).timestamp_micros());
    let (hop, body) = curator_profiles::package_with(TARGET, profile_of(wall, &run));
    h.lane("in_catalog", json!({}), hop, body);
    let mut seqs = Vec::new();
    for p in &wall.pieces {
        let (seq, _) = h.row(p.at, &p.session, &p.turn, &p.kind, &p.el, p.final_);
        seqs.push(seq);
    }
    h.set_clock(wall.now.timestamp_micros());
    h.out.clear();
    let hop = json!({"session_id": SESSION, "turn_id": "hard-1", "iter": "0", "phase": "",
                     "target": TARGET});
    let ctx = json!({"session_id": SESSION, "turn_id": "hard-1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("And what now?")], "system": mode("Be brief.")}),
    );
    let calls = h.routed(TARGET);
    assert_eq!(calls.len(), 1, "{who}: one round, one call: {:?}", h.stderr);
    let call = calls[0].clone();
    h.out.clear();
    h.tap(
        &call,
        "stop",
        json!({"tokens_prompt": 5_000, "input_soft": 3_000, "input_hard": 4_000}),
        json!([said("Here is where we stand.")]),
    );
    let stamp: Value = sj::from_str(&h.state_in("curate", ROUND_E)).unwrap_or(Value::Null);
    assert_eq!(
        stamp["curate_mark"],
        json!("hard"),
        "{who}: the window over input_hard marks the turn hard: {stamp} {:?}",
        h.stderr
    );
    let order = h
        .clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("{who}: the hard line orders a rebuild: {:?}", h.stderr));
    h.fire(&order);
    while !h.summ.is_empty() {
        h.answer("Earlier today, condensed by the stub.", "stop");
    }
    let plan = h.plan_in(ROUND_E);
    let cover = plan["cover"].as_i64().unwrap_or(0);
    assert!(
        seqs[0] <= cover,
        "{who}: the rebuild of the hard line moves the cover into today (the oldest row of the \
         day, seq {}, under the cover {cover}): {plan} {:?}",
        seqs[0],
        h.stderr
    );
}

#[test]
fn a_hard_limit_still_cuts_today() {
    if !shipped() {
        return;
    }
    let wall = long_day();
    let mut found = Findings::default();
    for horizon in HORIZONS {
        found.judge(&format!("target at the horizon {horizon}"), || {
            target_cuts_today(&wall, horizon)
        });
        found.judge(&format!("hard at the horizon {horizon}"), || {
            hard_cuts_today(&wall, horizon)
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
