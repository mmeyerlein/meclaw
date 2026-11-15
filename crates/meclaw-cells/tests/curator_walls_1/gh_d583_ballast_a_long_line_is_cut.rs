//! GH #1528 (d58-3 ballast, review M-2) -- the judge's line is cut where a
//! one-line form cuts.
//!
//! WHY this file exists: an asked round stands in the window as ONE line,
//! the judge's. A judge that answers with a paragraph would put the
//! paragraph into the window of the model that speaks, in place of a round
//! that was only asked. The line is cut where every one-line form of the
//! policy cuts the head it keeps (`LINE_CHARS`, the bank's `FORM_HEAD`):
//! no new bound, the one the forms already share. A cut line ends in the
//! mark the policy's `clip` puts on every text it cuts (re-check M-neu-1):
//! the model that speaks never reads a cut line as the judge's whole one.
//!
//! Pinned on the wall `phone_asked`, the judge on and answering `asked`
//! with a line far over that bound: every asked round outside the newest
//! stands as one line whose text is the first `FORM_HEAD` characters of the
//! judge's line and the mark of the cut,
//! `...[cut: <shown> of <all> chars shown; the judge's line]`.

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

const LONG: &str = "asked the time in the morning while the kettle was on and the radio \
     played an old song about the sea, and then went on to talk about nothing in \
     particular for a while, as one does on a slow morning at home, with the window \
     open and the neighbour's dog barking twice at the postman who came late today";

fn says_a_paragraph(wall: &Wall, req: &Msg) -> Judged {
    match verdict_of_the_wall(wall, req) {
        Judged::Asked(_) => Judged::Asked(LONG.to_string()),
        other => other,
    }
}

#[test]
fn a_long_line_is_cut() {
    assert!(
        LONG.chars().count() > 2 * FORM_HEAD,
        "the line is far over the bound"
    );
    let (wall, keys) = phone_asked::wall();
    let wall = wall.judged_by(says_a_paragraph);
    let (_, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    let head: String = LONG.chars().take(FORM_HEAD).collect();
    let cut = format!(
        "{head}...[cut: {FORM_HEAD} of {} chars shown; the judge's line]",
        LONG.chars().count()
    );
    let window = o.window();
    for key in &keys.asked_out {
        let ids: Vec<String> = round_of(&wall, key)
            .iter()
            .map(|i| wall.pieces[*i].id())
            .collect();
        let naming: Vec<String> = window
            .iter()
            .filter(|t| ids.iter().any(|id| t.contains(&format!("#{id}"))))
            .cloned()
            .collect();
        assert_eq!(
            naming,
            vec![asked_form(&ids, &cut)],
            "{}: the round {key} stands as one line, the judge's cut at {FORM_HEAD} \
             with the mark of the cut\n{}",
            o.who(),
            o.view()
        );
    }
}
