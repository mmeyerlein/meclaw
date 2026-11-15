//! GH #1465 (D58, the bank) -- a long task: the running part stays raw, the
//! closed parts are folded.
//!
//! WHY this file exists: the curator keeps the window of every model, not of
//! a conversation alone. The wall `coding` of the bank is one long task in
//! three parts with a large tool result in every step; `topic` marks close
//! two of the parts. After a rebuild the closed parts are gone from the
//! window and stand in the summary (the request went to the summarizer, a
//! stub here), every row of the running part is still raw, every result of
//! the running part outside `keep_rounds` is a one-line form that names its
//! id, and the window lies at the aim of the role. The role is a parameter
//! of the run; the wall runs with its presets and once with `gc` "1".
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

use curator_bank::coding::{PARTS, part_of};
use curator_bank::*;
use curator_hive::shipped;
use meclaw_core::serde_json::Value;

/// The part of the wall the piece `i` belongs to.
fn part(wall: &Wall, i: usize) -> usize {
    part_of(
        wall.pieces[i].turn[1..]
            .parse()
            .expect("a round of the wall"),
    )
}

#[test]
fn coding_keeps_the_running_task_raw_and_folds_the_closed_ones() {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    let runs = run_walls(&bank, |w| w.name == "coding", &mut found);
    assert_eq!(
        runs.len(),
        2,
        "the wall coding runs twice: {:?}",
        found.lines
    );
    let running = PARTS.len() - 1;
    for (wall, o) in &runs {
        let who = o.who();
        let window = o.window();
        let all: Vec<usize> = (0..wall.pieces.len()).collect();

        // The closed parts: under the cover, out of the window, in the request.
        let asked: String = o
            .requests
            .iter()
            .map(|m| Value::Object(m.body.clone()).to_string())
            .collect();
        if o.requests.is_empty() {
            found.note(format!("{who}: the rebuild sent the summarizer no request"));
        }
        for i in all.iter().filter(|i| part(wall, **i) < running) {
            let p = &wall.pieces[*i];
            if o.raw(*i) || o.shown(wall, *i) != Shown::Absent {
                found.note(format!(
                    "{who}: the row {} of a closed part is still in the window: raw {}, {:?}",
                    p.key,
                    o.raw(*i),
                    o.shown(wall, *i)
                ));
            }
            if window.iter().any(|t| t.contains(&p.key)) {
                found.note(format!(
                    "{who}: the keyword {} of a closed part is still in the window",
                    p.key
                ));
            }
            if p.what == What::Words && !asked.contains(&p.key) {
                found.note(format!(
                    "{who}: the instruction {} of a closed part is not in the request to the \
                     summarizer",
                    p.key
                ));
            }
        }
        let sent = Value::Object(o.next.body.clone()).to_string();
        if !sent.contains(&wall.summary) {
            found.note(format!(
                "{who}: the call after the rebuild does not carry the summary of the closed parts"
            ));
        }

        // The running part: raw, row by row, and not in the request.
        for i in all.iter().filter(|i| part(wall, **i) == running) {
            let p = &wall.pieces[*i];
            if !o.raw(*i) {
                found.note(format!(
                    "{who}: the row {} of the running part left the raw part (cover {})",
                    p.key,
                    o.cover()
                ));
            }
            if asked.contains(&p.key) {
                found.note(format!(
                    "{who}: the row {} of the running part went to the summarizer",
                    p.key
                ));
            }
        }
        found.judge(&format!("{who}: the talk of the running part"), || {
            talk_is_whole(wall, o)
        });

        // Old results outside `keep_rounds`: one-line forms that name the id.
        let old = wall.counted(|w| matches!(w, What::Junk(_)));
        assert!(!old.is_empty(), "the wall counts old results");
        for i in old {
            let shown = o.shown(wall, i);
            if shown != Shown::Form(Form::Shortened) {
                found.note(format!(
                    "{who}: the old result {} (#{}) is no one-line form with its id: {shown:?}",
                    wall.pieces[i].key,
                    wall.pieces[i].id()
                ));
            }
        }

        // The window lies at the aim of the role.
        found.judge(&format!("{who}: the aim"), || lands_at_aim(o));
    }
    found.finish();
}
