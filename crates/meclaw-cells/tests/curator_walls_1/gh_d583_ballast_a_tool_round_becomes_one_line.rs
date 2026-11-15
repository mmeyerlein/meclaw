//! GH #1528 (d58-3 ballast, review I-1) -- a round asked through a tool
//! becomes one line, and the judge saw the tool's result.
//!
//! WHY this file exists: the weather is asked on the phone, and the
//! companion answers it through the tool `weather` -- the round holds the
//! person's words, the tool call, its result and the answer. The judge and
//! the one line must work on ONE set of rows: the judge is shown every row
//! the line will stand for, and the line stands only for rows the judge was
//! shown. A judge that saw only the words and the answer would judge a round
//! whose tool result it never read; a plan that took only text rounds would
//! leave every weather round of a real day whole.
//!
//! Pinned on the wall `phone_asked_beside` (`support/curator_bank/
//! phone_asked.rs`), the judge on: the one request about the weather round
//! holds the person's words, the tool call, the tool's result and the
//! answer; after the rebuild the four rows stand as exactly one message,
//! `[#<words> #<call> #<result> #<answer> asked: <line>]`, in the place of
//! the round; no keyword of the round is left; the `gc` mark counts the four
//! blocks in the class `ballast` and none of them as `shrunk_tools`.
//!
//! Re-check I-neu-1: the weather's result is one the collection shrinks --
//! a younger tool round (the weather of tomorrow) is the one `keep_rounds` 1
//! keeps whole, so the same wall with the judge off counts the result (and
//! the recall's of a later round) as `shrunk_tools`. With the judge on the
//! shrunk result is ballast all the same: it is in the one line, not in the
//! plan's `shrunk`, and not counted twice.

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
use meclaw_core::serde_json::{Value, json};

#[test]
fn a_tool_round_becomes_one_line() {
    let (wall, keys) = phone_asked::wall_beside();

    // The collection shrinks the weather's result: the same wall, the same
    // cold rebuild, the judge off.
    let (_, off) = rebuild_with_hive(&wall, &Run::cold(&Run::of("quick")));
    let weather_round = round_of(&wall, &keys.weather);
    assert_eq!(
        off.shown(&wall, weather_round[2]),
        Shown::Form(Form::Shortened),
        "{}: without the judge the collection shrinks the weather's result\n{}",
        off.who(),
        off.view()
    );
    assert_eq!(
        off.gc_mark()["classes"]["shrunk_tools"],
        json!(2),
        "{}: without the judge the results of the weather round and of the recall are \
         shrunk tools: {}",
        off.who(),
        off.gc_mark()
    );

    let (h, o) = rebuild_with_hive(&wall, &wall.runs[0]);
    let round = round_of(&wall, &keys.weather);
    let kinds: Vec<&str> = round
        .iter()
        .map(|i| wall.pieces[*i].el["type"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        kinds,
        ["text", "tool_call", "tool_result", "text"],
        "the weather round: words, call, result, answer"
    );

    // The judge was shown every row of the round, the tool's result too.
    let reqs: Vec<&Msg> = h
        .judged
        .iter()
        .filter(|r| request_text(r).contains(&keys.weather))
        .collect();
    assert_eq!(
        reqs.len(),
        1,
        "{}: one request about the weather round: {} in all",
        o.who(),
        h.judged.len()
    );
    let text = request_text(reqs[0]);
    for i in &round {
        let p = &wall.pieces[*i];
        assert!(
            text.contains(&p.key),
            "{}: the request about the weather round shows {} ({}): {text}",
            o.who(),
            p.key,
            kinds[round.iter().position(|j| j == i).unwrap_or(0)]
        );
    }
    let result = &wall.pieces[round[2]];
    assert!(
        text.contains(result.text()),
        "{}: the request holds the tool's result `{}`: {text}",
        o.who(),
        result.text()
    );

    // After the rebuild: one line for the four rows, in the place of the
    // round.
    let ids: Vec<String> = round.iter().map(|i| wall.pieces[*i].id()).collect();
    let want = asked_form(&ids, phone_asked::BESIDE_LINES[0]);
    let window = o.window();
    let naming: Vec<String> = window
        .iter()
        .filter(|t| ids.iter().any(|id| t.contains(&format!("#{id}"))))
        .cloned()
        .collect();
    assert_eq!(
        naming,
        vec![want],
        "{}: the weather round stands as exactly one line\n{}",
        o.who(),
        o.view()
    );
    for i in &round {
        let p = &wall.pieces[*i];
        assert_eq!(
            o.shown(&wall, *i),
            Shown::Form(Form::Asked),
            "{}: {}",
            o.who(),
            p.key
        );
        assert!(
            window.iter().all(|t| !t.contains(&p.key)),
            "{}: the keyword {} of the weather round is gone\n{}",
            o.who(),
            p.key,
            o.view()
        );
    }

    // The count: the four blocks are ballast, none of them a shrunk tool.
    let mark = o.gc_mark();
    assert_eq!(
        mark["classes"][BALLAST],
        json!(4 + 2 + 4),
        "{}: the gc mark counts the four rows of the weather round, the two judged rows \
         of the time round and the four rows of the round led by a recall: {mark}",
        o.who()
    );
    let result_id = wall.pieces[round[2]].id();
    let shrunk: Vec<&str> = o.plan["shrunk"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert!(
        !shrunk.contains(&result_id.as_str()),
        "{}: the weather's result is in the one line, not in the plan's shrunk: {}",
        o.who(),
        o.plan
    );
    assert_eq!(
        mark["classes"]["shrunk_tools"],
        json!(0),
        "{}: the tool result of an asked round is no shrunk tool: {mark}",
        o.who()
    );
}
