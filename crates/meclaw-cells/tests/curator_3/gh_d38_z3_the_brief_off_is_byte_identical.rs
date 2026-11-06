//! GH #1440 (D38 Z3) -- `summary_form` "prose" and `summary_state_cond` "0"
//! are the rebuild of before, byte for byte.
//!
//! WHY this file exists: both knobs ship off (the set switches them on,
//! measured by S10 first), so a hive that sets nothing, or sets the shipped
//! values, must hand the summarizer exactly the request of before -- the
//! prose prompt, the transcript without ids, a chained summary as the
//! "Summary of the exchange before this part" -- and store the answer as it
//! came, the old summary among the sources (GH #925). A ledger with
//! state (a fact of today) changes nothing while the
//! condition is off. A mutant that builds the brief or the state list
//! without its knob turns this red.
//!
//! Stub summarizer (`support/curator_hive.rs` `Hive::answer`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const FIRST_PROMPT_WORDS: &str = "You condense the earlier part of an exchange with a model so \
                                  that a short text can stand in for it. Write down what was \
                                  said, in the order it was said";

/// Two rebuilds; returns both summarizer requests (system + messages) and the
/// stored rows (text, sources).
fn run(over: &[(&str, &str, Value)], sow: bool) -> (Vec<Value>, Vec<Vec<Value>>) {
    let mut o = vec![("policy", "keep_recent", json!(1))];
    o.extend(over.iter().cloned());
    let mut h = Hive::with(&o);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    if sow {
        h.mark(
            backdated(5),
            "s1",
            "t1",
            "fact",
            &canonical(&json!({"subject": "Hannah", "predicate": "lives_in",
                               "claim": "Hannah lives in Porto.", "source": "person",
                               "stance": "asserted"})),
        );
    }
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
    let r1 = h.answer("S1: q1 a1 q2 a2.", "stop");
    turn(
        &mut h,
        "s1",
        "t4",
        "q4",
        "a4",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
    );
    h.fire(&last_add(&h));
    let r2 = h.answer("S2.", "stop");
    let reqs = [r1, r2]
        .iter()
        .map(|r| json!({"system": r.body["system"], "messages": r.messages()}))
        .collect();
    let rows = h.rows(
        "SELECT b.body, s.sources, s.hash FROM summaries s JOIN blocks b ON b.hash = s.hash \
         ORDER BY s.rowid",
    );
    (reqs, rows)
}

#[test]
fn the_brief_off_is_byte_identical() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let policy = cell_config("policy");
    for (knob, off) in [("summary_form", "prose"), ("summary_state_cond", "0")] {
        assert_eq!(policy["params"][knob], json!(off), "{knob} ships {off:?}");
        assert_eq!(
            policy["contract"]["settings"][knob]["default"],
            json!(off),
            "the setting of {knob} says the same"
        );
    }
    let (base, base_rows) = run(&[], false);
    let prompt = base[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");
    assert!(prompt.starts_with(FIRST_PROMPT_WORDS), "{prompt}");
    assert_eq!(
        base[0]["messages"][0]["text"],
        json!("user: q1\nassistant: a1\nuser: q2\nassistant: a2"),
        "the transcript without ids"
    );
    assert_eq!(
        base[1]["messages"][0]["text"],
        json!(
            "Summary of the exchange before this part:\nS1: q1 a1 q2 a2.\n\nuser: q3\nassistant: a3"
        )
    );
    let chained: Value =
        meclaw_core::serde_json::from_str(base_rows[1][1].as_str().expect("sources"))
            .expect("json");
    let first_hash = base_rows[0][2].as_str().expect("a hash").to_string();
    assert!(
        chained
            .as_array()
            .expect("a list")
            .iter()
            .any(|s| s.as_str() == Some(first_hash.as_str())),
        "the prose chain counts the old summary as a source, as before"
    );
    for over in [
        vec![("policy", "summary_form", json!("prose"))],
        vec![("policy", "summary_state_cond", json!("0"))],
        vec![
            ("policy", "summary_form", json!("prose")),
            ("policy", "summary_state_cond", json!("0")),
        ],
    ] {
        for sow in [false, true] {
            let (reqs, rows) = run(&over, sow);
            assert_eq!(reqs, base, "{over:?} sow={sow}: the requests of before");
            let texts = |r: &Vec<Vec<Value>>| r.iter().map(|x| x[0].clone()).collect::<Vec<_>>();
            assert_eq!(texts(&rows), texts(&base_rows), "{over:?} sow={sow}");
        }
    }
    // The condition alone, on the prose form, still adds the list: it is a
    // knob of its own (OR-d38-2.z3.3) -- so a prose run with it on differs.
    // Its cap is Z1's `state_lines` (OR-d38-2.z3.4); the default role's
    // preset is 0, which lists nothing, so the run sets one.
    let (cond, _) = run(
        &[
            ("policy", "summary_state_cond", json!("1")),
            ("policy", "state_lines", json!(8)),
        ],
        true,
    );
    assert_ne!(
        cond[0]["messages"], base[0]["messages"],
        "the condition is no part of the form"
    );
}
