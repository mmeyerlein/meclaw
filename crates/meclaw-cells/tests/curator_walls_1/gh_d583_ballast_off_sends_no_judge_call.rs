//! GH #1528 (d58-3 ballast) -- off, nothing changes: no judge, no call.
//!
//! WHY this file exists: the judge costs one model call per round. It is a
//! switch of the set (`ballast_judge`), shipped "0", and off the curator
//! behaves byte for byte as before -- which the policy corpus
//! (`gh_d38_c0_policy_corpus_is_byte_identical`) and the goldens of GH #277
//! hold for every other path; this test holds it for the path of the judge.
//!
//! Pinned: `./policy` declares `ballast_judge`, shipped "0", with its setting
//! in the contract; on the wall `phone_asked`, the shipped switch and the
//! switch set "0" each send the judge no request, show no line of the
//! ballast, leave every row of the wall whole, and send the same window.

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
use curator_hive::cell_config;
use meclaw_core::serde_json::json;

#[test]
fn off_sends_no_judge_call() {
    let policy = cell_config("policy");
    assert_eq!(
        policy["params"][JUDGE_SWITCH],
        json!("0"),
        "templates/curator/policy/config.json declares ballast_judge, shipped \"0\""
    );
    let setting = &policy["contract"]["settings"][JUDGE_SWITCH];
    assert_eq!(
        setting["default"],
        json!("0"),
        "the setting's default is the param"
    );
    assert_eq!(setting["type"], json!("string"), "{setting}");
    assert!(
        setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the setting says what it does: {setting}"
    );

    let mut windows = Vec::new();
    for run in [
        Run::cold(&Run::of("quick")),
        Run::cold(&Run::of("quick").with(JUDGE_SWITCH, json!("0"))),
    ] {
        let (wall, _) = phone_asked::sown();
        let wall = wall.run(run);
        let (h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
        assert!(
            h.judged.is_empty(),
            "{}: off, the judge is sent nothing: {:?}",
            o.who(),
            h.judged
        );
        for i in 0..wall.pieces.len() {
            assert_eq!(
                o.shown(&wall, i),
                Shown::Whole,
                "{}: the row {} stands whole",
                o.who(),
                wall.pieces[i].key
            );
        }
        for t in o.window() {
            assert!(
                asked_ids(&t).is_empty(),
                "{}: no line of the ballast: {t}",
                o.who()
            );
        }
        windows.push(o.window());
    }
    assert_eq!(
        windows[0], windows[1],
        "the switch set \"0\" sends the window of the shipped switch"
    );
}
