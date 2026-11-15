//! d58-3 tag (GH #1527) -- the floor ships off, and off is the curator of
//! before, byte for byte.
//!
//! WHY this file exists: the switch `day_floor` changes how old every row
//! of the running day is, at every horizon, and when the day change orders
//! a rebuild. Shipped, it must change nothing: the policy declares it with
//! the preset "0" (params and the contract's settings), and a run with the
//! switch set to "0" is the run without the knob -- the same plan, the same
//! window, the same requests to the summarizer, byte for byte. The corpus of
//! the curator (`gh_d38_c0_policy_corpus_is_byte_identical`, fixtures
//! `d38_c0`) and the gh277 goldens hold the shipped hive itself unchanged;
//! this test holds the off arm to the absent knob on the bank's walls.
//!
//! Per wall of the bank (`phone_day` as `talky`, `coding` as `coding`) and
//! per way into the rebuild (at the target, and over a day change at the
//! horizon `day`, the one that rebuilt at the day change before): the run
//! without the knob and the run with `day_floor` "0" leave the same plan,
//! the same window and the same requests.
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
use curator_hive::{canonical, cell_config, shipped};
use meclaw_core::serde_json::{Value, json};

/// The walls of the statement and the example value set each runs as.
const WALLS: [(&str, &str); 2] = [("phone_day", "talky"), ("coding", "coding")];

/// What a run leaves, as one canonical text: the plan, the window, the
/// requests to the summarizer.
fn left(o: &Outcome) -> String {
    let requests: Vec<Value> = o
        .requests
        .iter()
        .map(|m| json!({"route": m.route(), "body": m.body}))
        .collect();
    canonical(&json!({"plan": o.plan, "window": o.window(), "requests": requests}))
}

#[test]
fn off_is_byte_identical() {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    for (name, role) in WALLS {
        let wall = bank.named(name).clone();
        let mut day = Run::of(role).with("horizon", json!("day"));
        day.trigger = Trigger::Day;
        for base in [Run::of(role), day] {
            let off = base.clone().with("day_floor", json!("0"));
            let who = format!("{name} as {}", off.label());
            let (Some(a), Some(b)) = (
                found.judge(&format!("{who}: without the knob"), || {
                    rebuild(&wall, &base)
                }),
                found.judge(&format!("{who}: with the knob"), || rebuild(&wall, &off)),
            ) else {
                continue;
            };
            let (a, b) = (left(&a), left(&b));
            if a != b {
                found.note(format!(
                    "{who}: day_floor \"0\" leaves another plan, window or request than no \
                     knob ({} against {} bytes)",
                    b.len(),
                    a.len()
                ));
            }
        }
    }
    found.judge("shipped off", || {
        let policy = cell_config("policy");
        assert_eq!(
            policy["params"]["day_floor"],
            json!("0"),
            "templates/curator/policy/config.json declares day_floor, shipped \"0\""
        );
        assert_eq!(
            policy["contract"]["settings"]["day_floor"]["default"],
            json!("0"),
            "the setting's default is the param"
        );
    });
    found.finish();
}
