//! GH #1539 (d58-3 nachlese) -- with `summary_state_cond` "1" the state
//! ahead of the transcript is headed as what wins over the transcript, and one
//! sentence says that a point the state names settled or done gets no `open`
//! or `promised` line but a statement line keyed `<subject>/status`. With
//! "0" the request carries none of it: its transcript is the "1" transcript
//! without the state part, byte for byte.
//!
//! WHY this file exists: in the summary bank of d58-3 one answer in six per
//! role wrote a point the state named settled as an `open` line ("open: ...
//! now settled and no longer open"). The head said "Still true as of now --
//! keep it:", which asks to keep the point, and the prompt had no word on a
//! settled one. Whether a point is settled is the model's reading; the code
//! only says the rule.
//!
//! The state part stands AHEAD of the transcript (d58-3 oc9, P2R): behind it,
//! the summarizer of the bank (a small model) answered 6 of 6 brief requests
//! with stop reason `refusal`; the same request with the part ahead of the
//! transcript 1 of 12.
//!
//! Depends on Z1's `state_block`/`state_of_rows` (curator/policy). Stub
//! summarizer (`support/curator_hive.rs` `Hive::answer`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const FACT: &str = "The list for the market is settled now.";
const HEAD: &str = "The state as of now -- it wins over the transcript:";
const OLD_HEAD: &str = "Still true as of now -- keep it:";
const SETTLED: &str = "A point the state below names settled or done gets no `open` or \
                       `promised` line; write it as a statement line with the key \
                       `<subject>/status`.";

fn request_transcript(cond: &str) -> String {
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
        ("policy", "summary_state_cond", json!(cond)),
        ("policy", "state_lines", json!(8)),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    h.mark(
        backdated(5),
        "s1",
        "t1",
        "fact",
        &canonical(&json!({"subject": "market_list", "predicate": "status",
                           "claim": FACT, "source": "person", "stance": "asserted"})),
    );
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let req = h.answer("- open (2026-01-01): x", "stop");
    req.messages()[0]["text"]
        .as_str()
        .expect("a transcript")
        .to_string()
}

#[test]
fn the_state_condition_says_a_settled_point_gets_no_open_line() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let on = request_transcript("1");
    let off = request_transcript("0");
    let state = on
        .lines()
        .find(|l| l.starts_with("[state -- "))
        .unwrap_or_else(|| panic!("the state line rides with the transcript: {on}"))
        .to_string();
    assert!(
        state.contains(FACT),
        "the settled fact is in the state: {state}"
    );
    assert_eq!(
        on,
        format!("{HEAD}\n{SETTLED}\n{state}\n\n{off}"),
        "the state part is the head, the rule and the state line, ahead of the \
         transcript of \"0\""
    );
    assert!(!on.contains(OLD_HEAD), "{on}");
    assert!(
        !off.contains(HEAD) && !off.contains(SETTLED) && !off.contains("[state -- "),
        "with \"0\" none of it: {off}"
    );
}
