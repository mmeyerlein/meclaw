//! GH #1528 (d58-3 ballast) -- the newest `keep_recent` rounds stay raw,
//! whatever the judge said about them.
//!
//! WHY this file exists: the newest rounds are the ones the talk goes on
//! from. A question asked a minute ago and answered is still the live
//! context ("and tomorrow?"), so the floor of the raw part holds for the
//! ballast as for every other class: the judge may mark a recent round, its
//! line waits until the round has left the newest rounds.
//!
//! Pinned on the wall `phone_asked` (`keep_recent` 4 by the example set
//! `quick`), the judge on: the judge marked the time asked at 17:22, one of
//! the newest rounds at the rebuild, and that round stands whole; under a
//! `keep_recent` of 40 every round of the day is among the newest, every
//! asked round stands whole and the `gc` mark counts no `ballast`.

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
use curator_hive::Msg;
use meclaw_core::serde_json::json;

fn asked_and_whole(wall: &Wall, h: &curator_hive::Hive, o: &Outcome, key: &str) {
    let reqs: Vec<&Msg> = h
        .judged
        .iter()
        .filter(|r| request_text(r).contains(key))
        .collect();
    assert_eq!(reqs.len(), 1, "{}: the judge is asked about {key}", o.who());
    assert!(
        matches!(verdict_of_the_wall(wall, reqs[0]), Judged::Asked(_)),
        "{}: the stub marks {key}",
        o.who()
    );
    for i in round_of(wall, key) {
        assert_eq!(
            o.shown(wall, i),
            Shown::Whole,
            "{}: the recent row {} stands whole\n{}",
            o.who(),
            wall.pieces[i].key,
            o.view()
        );
    }
}

#[test]
fn recent_rounds_are_never_shrunk() {
    let (wall, keys) = phone_asked::wall();
    let (h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    assert_eq!(
        o.knobs.all["keep_recent"],
        json!(phone_asked::KEEP_RECENT),
        "{}: the run keeps the newest four rounds raw",
        o.who()
    );
    asked_and_whole(&wall, &h, &o, &keys.asked_recent);

    let (wall, keys) = phone_asked::sown();
    let wide = Run::cold(
        &Run::of("quick")
            .with(JUDGE_SWITCH, json!("1"))
            .with("keep_recent", json!(40)),
    );
    let wall = wall.run(wide);
    let (h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    for key in keys.asked_out.iter().chain([&keys.asked_recent]) {
        asked_and_whole(&wall, &h, &o, key);
    }
    for g in &o.gc {
        assert_eq!(
            g["classes"][BALLAST],
            json!(0),
            "{}: nothing is collected as ballast among the newest rounds: {g}",
            o.who()
        );
    }
}
