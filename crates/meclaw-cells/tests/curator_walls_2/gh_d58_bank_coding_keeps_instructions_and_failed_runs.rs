//! GH #1465 (D58, the bank) -- a long task: the instructions stay, a failed
//! test run is a result, a tool that failed stays.
//!
//! WHY this file exists: an agent that works on a task for hours is told
//! what to do in a few sentences spread over the task. A test run that ended
//! non-zero is no failure of the tool: the tool worked and says what the
//! command came to, and were every such run kept for ever, a search for a
//! bug would fill the window with runs long overtaken. A tool that did not
//! work (not reachable, refused, timed out) is another matter: the model
//! keeps seeing that failure in its own words (OR-58-66). On the wall
//! `coding` of the bank, after the rebuild:
//! (a) the failed test run of the running part stands raw while it lies
//!     within `keep_rounds` -- the wall holds it in its last rounds; outside
//!     them it would be a stub with its id, in the running part too
//!     (OR-58-68);
//! (b) the failed test run of a closed part outside `keep_rounds` is what
//!     every old result is: a stub with its id, or gone with its part into
//!     the summary -- and the way back `history_read` stays;
//! (c) the result of the tool that failed itself (an `error_code` in its
//!     header: the shell tool's timeout) stays byte for byte where it
//!     stands;
//! (d) every instruction of the person and every answer in the raw part
//!     stands in the window byte for byte.
//! The role is a parameter of the run; the wall runs with its presets and
//! once with `gc` "1".
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

use curator_bank::coding::{FAILED, FAILED_CLOSED, INSTRUCTIONS, PARTS, TIMEOUT, turn};
use curator_bank::*;
use curator_hive::shipped;

#[test]
fn coding_keeps_instructions_and_failed_runs() {
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
    let running = PARTS[PARTS.len() - 1];
    let told = INSTRUCTIONS.iter().filter(|n| **n >= running.0).count();
    for (wall, o) in &runs {
        let who = o.who();

        // (d) Every instruction and every answer of the raw part, byte for byte.
        found.judge(&format!("{who}: instructions and answers"), || {
            talk_is_whole(wall, o)
        });
        let whole = wall
            .counted(|w| *w == What::Words)
            .into_iter()
            .filter(|i| o.raw(*i) && o.shown(wall, *i) == Shown::Whole)
            .count();
        if whole < told {
            found.note(format!(
                "{who}: {whole} instructions stand whole in the raw part, the running part \
                 holds {told}"
            ));
        }

        // The result of the tool round `n`.
        let result = |n: usize| {
            wall.pieces
                .iter()
                .position(|p| p.turn == turn(n) && p.el["type"] == "tool_result")
                .unwrap_or_else(|| panic!("the wall holds the result of round {n}"))
        };

        // (a) The failed test run of the running part, within `keep_rounds`,
        // stands raw.
        let i = result(FAILED);
        let shown = o.shown(wall, i);
        if !o.raw(i) || shown != Shown::Whole {
            found.note(format!(
                "{who}: (a) the failed test run of the running part (#{}) does not stand raw \
                 after the rebuild: in the raw part {}, {shown:?}",
                wall.pieces[i].id(),
                o.raw(i)
            ));
        }

        // (b) The failed test run of a closed part is an old result like any
        // other: a stub with its id, or gone into the summary with its part;
        // the way back stays.
        let i = result(FAILED_CLOSED);
        let id = wall.pieces[i].id();
        let shown = o.shown(wall, i);
        // A stub with its own way back: `Outcome::shown` gives
        // `Form::Shortened` only to a line that ends `history_read("#<id>")]`.
        let stub = shown == Shown::Form(Form::Shortened);
        let condensed = !o.raw(i)
            && shown == Shown::Absent
            && o.plan["summary"].as_str().is_some_and(|s| !s.is_empty())
            && (o.plan["sum_from"].as_i64().unwrap_or(0)..=o.plan["sum_to"].as_i64().unwrap_or(-1))
                .contains(&o.seqs[i]);
        if !stub && !condensed {
            found.note(format!(
                "{who}: (b) the failed test run of a closed part (#{id}) is neither a stub with \
                 its id nor gone into the summary: in the raw part {}, {shown:?}",
                o.raw(i)
            ));
        }
        // The way back is bound to this result (review bank minor 1), never
        // a word anywhere in the call: a stub is known as one only by its
        // end `history_read("#<id>")]` with its own id (`Outcome::shown`); a
        // result gone into the summary comes back through the summary that
        // holds its turn -- the plan's range covers its row (above), and the
        // call after the rebuild carries that very summary in its system
        // part (`history.summary`), the leaf the turn range is read from:
        // the head line of the policy (`SUMMARY_HEAD`, the ledger range of
        // the summary) and below it that summary, byte for byte. The plan
        // names its summary by id (`plan.summary`); its text is what the
        // summarizer answered, the stub's `wall.summary`.
        if condensed {
            let leaf = &o.next.body["system"]["history"]["summary"]["text"];
            let carried =
                leaf.as_str()
                    .and_then(|t| t.split_once('\n'))
                    .is_some_and(|(head, text)| {
                        head.starts_with("[earlier exchange, summarised") && text == wall.summary
                    });
            if !carried {
                found.note(format!(
                    "{who}: (b) the failed test run of a closed part (#{id}) went into the \
                     summary, and the call after the rebuild does not carry that summary: \
                     history {}, the summary {:?} (plan {})",
                    o.next.body["system"]["history"], wall.summary, o.plan["summary"]
                ));
            }
        }

        // (c) The result of the tool that failed itself stays where it stands.
        let i = result(TIMEOUT);
        assert_eq!(wall.pieces[i].what, What::Error, "the timeout is counted");
        let shown = o.shown(wall, i);
        if !o.raw(i) || shown != Shown::Whole {
            found.note(format!(
                "{who}: (c) the result of the tool that failed itself (#{}, error_code {}) does \
                 not stand byte for byte after the rebuild: in the raw part {}, {shown:?}",
                wall.pieces[i].id(),
                wall.pieces[i].code,
                o.raw(i)
            ));
        }
    }
    found.finish();
}
