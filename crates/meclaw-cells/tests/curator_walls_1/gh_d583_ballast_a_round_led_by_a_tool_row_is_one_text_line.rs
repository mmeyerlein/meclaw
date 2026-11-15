//! GH #1528 (d58-3 ballast, re-check M-neu-2) -- the one line of a round
//! that begins with a tool row is a text.
//!
//! WHY this file exists: the one line of an asked round stands where the
//! round's first row stood. When the curator pushed a recall in front of
//! the person's words, that first row is a tool call. Were the line built
//! from it, it would leave as a tool call whose text is the line -- a call
//! a provider parses and cannot. The line is a text of the person's side
//! whatever row it stands in for.
//!
//! Pinned on the wall `phone_asked_beside`, the judge on: the round `b1-3`
//! (the recall's call and result, the words, the answer) stands as exactly
//! one message, `[#<call> #<result> #<words> #<answer> asked: <line>]`, and
//! that message is a text (`origin` user, `type` text) without the call's
//! id.

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

#[test]
fn a_round_led_by_a_tool_row_is_one_text_line() {
    let (wall, keys) = phone_asked::wall_beside();
    let (_, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    // The round from its first row: the recall stands in front of the
    // words `round_of` starts at.
    let words = &wall.pieces[round_of(&wall, &keys.led)[0]];
    let round: Vec<usize> = (0..wall.pieces.len())
        .filter(|i| wall.pieces[*i].session == words.session && wall.pieces[*i].turn == words.turn)
        .collect();
    let kinds: Vec<&str> = round
        .iter()
        .map(|i| wall.pieces[*i].el["type"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        kinds,
        ["tool_call", "tool_result", "text", "text"],
        "the round: the recall's call and result, the words, the answer"
    );

    let ids: Vec<String> = round.iter().map(|i| wall.pieces[*i].id()).collect();
    let want = asked_form(&ids, phone_asked::BESIDE_LINES[2]);
    let naming: Vec<_> = o
        .next
        .messages()
        .into_iter()
        .filter(|m| {
            let t = m["text"].as_str().unwrap_or("");
            ids.iter().any(|id| t.contains(&format!("#{id}")))
        })
        .collect();
    assert_eq!(
        naming.len(),
        1,
        "{}: the round stands as exactly one message: {naming:?}\n{}",
        o.who(),
        o.view()
    );
    let line = &naming[0];
    assert_eq!(line["text"], want.as_str(), "{}: the one line", o.who());
    assert_eq!(
        (line["origin"].as_str(), line["type"].as_str()),
        (Some("user"), Some("text")),
        "{}: the one line is a text, never the tool call it stands in for: {line}",
        o.who()
    );
    assert!(
        line.get("id").is_none(),
        "{}: the one line carries no call id: {line}",
        o.who()
    );
}
