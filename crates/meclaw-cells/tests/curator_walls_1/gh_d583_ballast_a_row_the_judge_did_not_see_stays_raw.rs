//! GH #1528 (d58-3 ballast, review I-1) -- a row of an asked round the
//! judge never saw stays raw in its place.
//!
//! WHY this file exists: the one line of an asked round takes the person's
//! words out of the window, and that is allowed only for words a model
//! judged (R-583-2). The judge is shown the person's words, the answer and
//! the tool rows of a round -- never a row that is nobody's words: a timer
//! that rang, an app, a peer, the advice of the reasoning core. Such a row
//! can stand in the same round (a timer frame in the same message as the
//! words); the one line must not take it, or a row would leave the window
//! that no model ever judged.
//!
//! Pinned on the wall `phone_asked_beside`, the judge on: the request about
//! the time round does not hold the timer frame; after the rebuild the
//! words and the answer of the round stand as one line naming exactly their
//! two ids, and the timer frame stands whole.
//!
//! Re-check I-neu-1: a row of the kinds the judge is shown can still be one
//! it never saw -- a sentence written under the round's turn after the
//! judge's verdict. The mark names the rows the judge saw (`seen`), and a
//! round whose rows are not exactly those is no candidate: the judge said
//! `asked` about the round, and the whole round stands raw, no line names
//! any of its rows.

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

#[test]
fn a_row_the_judge_did_not_see_stays_raw() {
    let (wall, keys) = phone_asked::wall_beside();
    let (h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    let round = round_of(&wall, &keys.time);
    let timer = wall
        .pieces
        .iter()
        .position(|p| p.key == keys.timer)
        .expect("the timer frame is a row of the wall");
    assert!(
        round.contains(&timer),
        "the timer frame is a row of the time round"
    );
    let judged: Vec<usize> = round.iter().copied().filter(|i| *i != timer).collect();

    let reqs: Vec<&Msg> = h
        .judged
        .iter()
        .filter(|r| request_text(r).contains(&keys.time))
        .collect();
    assert_eq!(
        reqs.len(),
        1,
        "{}: one request about the time round",
        o.who()
    );
    let text = request_text(reqs[0]);
    assert!(
        !text.contains(&keys.timer),
        "{}: the judge is not shown the timer frame: {text}",
        o.who()
    );

    let ids: Vec<String> = judged.iter().map(|i| wall.pieces[*i].id()).collect();
    let want = asked_form(&ids, phone_asked::BESIDE_LINES[1]);
    let window = o.window();
    let naming: Vec<String> = window
        .iter()
        .filter(|t| !asked_ids(t).is_empty() && ids.iter().any(|id| t.contains(&format!("#{id}"))))
        .cloned()
        .collect();
    assert_eq!(
        naming,
        vec![want],
        "{}: the words and the answer of the time round stand as one line\n{}",
        o.who(),
        o.view()
    );
    for i in &judged {
        assert_eq!(
            o.shown(&wall, *i),
            Shown::Form(Form::Asked),
            "{}: {}",
            o.who(),
            wall.pieces[*i].key
        );
    }
    assert_eq!(
        o.shown(&wall, timer),
        Shown::Whole,
        "{}: the timer frame {} stands whole\n{}",
        o.who(),
        keys.timer,
        o.view()
    );
    assert!(
        window
            .iter()
            .any(|t| t.contains(&keys.timer) && asked_ids(t).is_empty()),
        "{}: the timer frame stands in the window, outside every line of the ballast\n{}",
        o.who(),
        o.view()
    );
}

#[test]
fn a_round_with_a_row_after_the_verdict_stays_raw() {
    let (wall, keys) = phone_asked::wall_beside();
    let (h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    let round = round_of(&wall, &keys.late_words);
    let late = wall
        .pieces
        .iter()
        .position(|p| p.key == keys.late)
        .expect("the late row is a row of the wall");
    assert!(round.contains(&late), "the late row is a row of the round");

    // The judge was asked about the round before the late row came, and
    // said `asked`.
    let reqs: Vec<&Msg> = h
        .judged
        .iter()
        .filter(|r| request_text(r).contains(&keys.late_words))
        .collect();
    assert_eq!(reqs.len(), 1, "{}: one request about the round", o.who());
    assert!(
        !request_text(reqs[0]).contains(&keys.late),
        "{}: the judge never saw the late row",
        o.who()
    );
    assert_eq!(
        verdict_of_the_wall(&wall, reqs[0]),
        Judged::Asked(phone_asked::BESIDE_LINES[3].to_string()),
        "{}: the judge said the round was only asked",
        o.who()
    );

    // The rows of the round are not the rows the judge saw: no line, every
    // row whole.
    let window = o.window();
    for i in &round {
        let id = wall.pieces[*i].id();
        assert!(
            window
                .iter()
                .all(|t| !asked_ids(t).contains(&format!("#{id}"))),
            "{}: no line of the ballast names the row {}\n{}",
            o.who(),
            wall.pieces[*i].key,
            o.view()
        );
        assert_eq!(
            o.shown(&wall, *i),
            Shown::Whole,
            "{}: the row {} of the round stands whole\n{}",
            o.who(),
            wall.pieces[*i].key,
            o.view()
        );
    }
}
