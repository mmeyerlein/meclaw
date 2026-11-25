//! GH #1528 (d58-3 ballast) -- a judge that fails keeps the round.
//!
//! WHY this file exists: a mark of the ballast takes the person's words out
//! of the window. It may only follow from a clear `asked` of the judge. A
//! call that failed, ran out of its time, answered no JSON or a verdict that
//! is neither `keep` nor `asked`, answered `asked` without a line, did not
//! end with `stop` (whatever its text), answered JSON that is no object, or
//! never answered, says nothing about the
//! round -- and the round stays as it was (trust before saving: a round kept
//! by mistake costs some tokens, a round lost by mistake costs the person's
//! trust).
//!
//! Pinned on the wall `phone_asked`, the judge on, once per way to fail: the
//! judge was asked about every round of the wall, and after the rebuild every
//! row of the wall stands whole, no message is in the one-line form of the
//! ballast and no `gc` mark counts `ballast`.
//!
//! One test per way (GH #1640). The twelve ways ran in one test: 26 s alone, 73 s
//! in the middle of 125 gates, 198 s in the slowest green one, and the runner's kill
//! at 240 s in one of them. A way is one wall of python work that stretches
//! with the load of the lane and waits on no clock: with 64 tests at once one way
//! took 51 s against 2 s alone.

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

/// One way a judge call can go wrong.
type Way = fn(&Wall, &Msg) -> Judged;

fn fails(_: &Wall, _: &Msg) -> Judged {
    Judged::Failed("upstream_error")
}

fn times_out(_: &Wall, _: &Msg) -> Judged {
    Judged::TimedOut
}

fn says_no_json(_: &Wall, _: &Msg) -> Judged {
    Judged::Broken("asked, I think")
}

fn cuts_its_json(_: &Wall, _: &Msg) -> Judged {
    Judged::Broken("{\"verdict\": \"asked\", \"line\": \"asked the ti")
}

fn names_no_verdict_of_the_form(_: &Wall, _: &Msg) -> Judged {
    Judged::Broken("{\"verdict\": \"drop\", \"line\": \"asked the time\"}")
}

fn never_answers(_: &Wall, _: &Msg) -> Judged {
    Judged::Silent
}

// Review I-2: the ways the contract names that no variant above reached.
fn asks_without_a_line(_: &Wall, _: &Msg) -> Judged {
    Judged::Broken("{\"verdict\": \"asked\", \"line\": \"\"}")
}

fn asks_with_a_blank_line(_: &Wall, _: &Msg) -> Judged {
    Judged::Broken("{\"verdict\": \"asked\", \"line\": \"   \"}")
}

fn is_cut_at_its_length(_: &Wall, _: &Msg) -> Judged {
    Judged::Ends(
        "length",
        "{\"verdict\": \"asked\", \"line\": \"asked the time\"}",
    )
}

fn ends_in_an_error_with_text(_: &Wall, _: &Msg) -> Judged {
    Judged::Ends(
        "error",
        "{\"verdict\": \"asked\", \"line\": \"asked the time\"}",
    )
}

fn answers_an_array(_: &Wall, _: &Msg) -> Judged {
    Judged::Broken("[\"asked\", \"asked the time\"]")
}

fn answers_a_string(_: &Wall, _: &Msg) -> Judged {
    Judged::Broken("\"asked\"")
}

/// The wall, its judge failing this way, through its rebuild: every round was
/// put to the judge and every row stands whole.
fn the_round_stays(judge: Way) {
    let (wall, _) = phone_asked::wall();
    let wall = wall.judged_by(judge);
    let (h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    for (key, _) in &wall.asked {
        assert!(
            h.judged
                .iter()
                .any(|r| request_text(r).contains(key.as_str())),
            "{}: the judge is asked about {key}",
            o.who()
        );
    }
    for i in 0..wall.pieces.len() {
        assert_eq!(
            o.shown(&wall, i),
            Shown::Whole,
            "{}: the row {} stands whole\n{}",
            o.who(),
            wall.pieces[i].key,
            o.view()
        );
    }
    for t in o.window() {
        assert!(
            asked_ids(&t).is_empty(),
            "{}: no line of the ballast: {t}",
            o.who()
        );
    }
    for g in &o.gc {
        assert_eq!(g["classes"][BALLAST], json!(0), "{}: {g}", o.who());
    }
}

#[test]
fn a_failed_call_keeps_the_round() {
    the_round_stays(fails);
}

#[test]
fn a_call_out_of_its_time_keeps_the_round() {
    the_round_stays(times_out);
}

#[test]
fn an_answer_without_json_keeps_the_round() {
    the_round_stays(says_no_json);
}

#[test]
fn cut_json_keeps_the_round() {
    the_round_stays(cuts_its_json);
}

#[test]
fn an_unknown_verdict_keeps_the_round() {
    the_round_stays(names_no_verdict_of_the_form);
}

#[test]
fn no_answer_keeps_the_round() {
    the_round_stays(never_answers);
}

#[test]
fn asked_with_an_empty_line_keeps_the_round() {
    the_round_stays(asks_without_a_line);
}

#[test]
fn asked_with_a_blank_line_keeps_the_round() {
    the_round_stays(asks_with_a_blank_line);
}

#[test]
fn an_end_at_the_length_keeps_the_round() {
    the_round_stays(is_cut_at_its_length);
}

#[test]
fn an_error_end_with_a_text_keeps_the_round() {
    the_round_stays(ends_in_an_error_with_text);
}

#[test]
fn a_json_array_keeps_the_round() {
    the_round_stays(answers_an_array);
}

#[test]
fn a_json_string_keeps_the_round() {
    the_round_stays(answers_a_string);
}
