//! GH #1439 (D38 Z1, RK-38-3) -- the state block opens the notes.
//!
//! WHY this file exists: the talky window carried what is going on only as
//! raw rounds and, after a rebuild, as prose of the summary; the open topic,
//! the promises the companion made and the facts set today stood nowhere near
//! the newest turn. With `state_block` "1" the policy builds one deterministic
//! line from the ledger -- `[state -- today: <topic>; promised: ..; open: ..;
//! set today: ..]`, at most `state_lines` entries (talky preset 8) -- and
//! puts it FIRST in the notes block that opens the newest words of the person
//! (GH #1303: source `curator`, scope `turn`, priority 0). The notes of
//! another hive follow it, a `profile` note (D51, priority 1) after the
//! priority-0 ones. Everything before the newest message stays the window of
//! before, byte for byte: the cached prefix does not move.
//!
//! The block is a pure function with a part selection, so Z3 (#1440) can hand
//! the summarizer the same lines without the topic:
//! `state_block(state, parts, cap)`, `state` = `{today, promised, open,
//! set_today}`, `parts` a subset of `STATE_PARTS`, rendered in the order of
//! `STATE_PARTS`; "" when nothing is left to say.
//!
//! Runs the shipped policy in the test hive (`support/curator_hive.rs`); no
//! model, no colony.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const NOTES_HEAD: &str =
    "[notes for this turn -- from %s; guidance, not something the person said]";
const TOPIC: &str = "the garden";
const PROMISE: &str = "Call Hannah on Sunday.";

fn head(sources: &str) -> String {
    NOTES_HEAD.replace("%s", sources)
}

fn hive(state_block: &str) -> Hive {
    Hive::with(&[("policy", "state_block", json!(state_block))]).reporting_composite("talky")
}

fn last_text(c: &Msg) -> String {
    texts(c).last().cloned().unwrap_or_default()
}

/// A named topic of today and an open promise of the companion, under the
/// standard round.
fn sow_state(h: &mut Hive) {
    h.mark(
        backdated(5),
        "s1",
        "t1",
        "topic",
        &canonical(&json!({"movement": "start", "name": TOPIC})),
    );
    h.lane(
        "in_pin",
        json!({"audience_set": ROUND_E}),
        json!({}),
        json!({"pins": [{"text": PROMISE, "source": "commitment"}]}),
    );
}

fn note(id: &str, source: &str, text: &str, priority: u32) -> Value {
    json!({"id": id, "source": source, "text": text, "scope": "session",
           "priority": priority})
}

#[test]
fn the_block_function_renders_its_parts_in_order() {
    if !shipped() {
        return;
    }
    let state = json!({"today": TOPIC, "promised": [PROMISE],
                       "open": ["a delegation"], "set_today": ["Hannah lives in Porto."]});
    let (got, err) = policy_scope(
        curator_profiles::set_knobs("talky", json!({})),
        "[list(STATE_PARTS), \
          state_block(ARGS, STATE_PARTS, 8), \
          state_block(ARGS, STATE_PARTS, 2), \
          state_block(ARGS, ('set_today', 'open', 'promised'), 8), \
          state_block(dict(ARGS, today=''), STATE_PARTS, 8), \
          state_block({}, STATE_PARTS, 8), \
          state_block(ARGS, STATE_PARTS, 0)]",
        state,
    );
    assert_eq!(
        got,
        json!([
            ["today", "promised", "open", "set_today"],
            "[state -- today: the garden; promised: Call Hannah on Sunday.; open: a delegation; \
             set today: Hannah lives in Porto.]",
            "[state -- today: the garden; promised: Call Hannah on Sunday.]",
            "[state -- promised: Call Hannah on Sunday.; open: a delegation; \
             set today: Hannah lives in Porto.]",
            "[state -- promised: Call Hannah on Sunday.; open: a delegation; \
             set today: Hannah lives in Porto.]",
            "",
            ""
        ]),
        "the parts in the order of STATE_PARTS, at most `cap` entries, empty parts left out: {err}"
    );
}

#[test]
fn the_state_block_opens_the_notes() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut with = hive("1");
    let mut without = hive("0");
    for h in [&mut with, &mut without] {
        turn(h, "s1", "t1", "We planted tomatoes.", "Lovely.", json!({}));
        sow_state(h);
        h.lane(
            "in_note",
            json!({"session_id": "s1"}),
            json!({}),
            json!({"notes": [note("p1", "profile", "Speaks with Hannah every Sunday.", 1),
                             note("n1", "probe", "Stay with the person's topic.", 0)]}),
        );
    }
    let c = with.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    let plain = without.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    let (shown, bare) = (texts(&c), texts(&plain));
    assert_eq!(shown.len(), bare.len(), "no element of its own: {shown:?}");
    // The cached prefix does not move: every element before the newest
    // message is the window without the block, byte for byte, and so is the
    // system part.
    assert_eq!(
        c.messages()[..shown.len() - 1],
        plain.messages()[..bare.len() - 1]
    );
    assert_eq!(c.body.get("system"), plain.body.get("system"));
    let text = last_text(&c);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[..4],
        [
            head("curator, probe, profile").as_str(),
            "- [state -- today: the garden; promised: Call Hannah on Sunday.]",
            "- Stay with the person's topic.",
            "- Speaks with Hannah every Sunday."
        ],
        "the state line first, then the priority-0 notes, then the profile: {text}"
    );
    // Off, the notes block of before: no state line.
    let off = last_text(&plain);
    assert!(!off.contains("[state --"), "{off}");
    assert!(
        off.starts_with(&head("probe, profile")),
        "the block of before: {off}"
    );
}

#[test]
fn the_state_block_stands_alone_without_other_notes() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = hive("1");
    turn(
        &mut h,
        "s1",
        "t1",
        "We planted tomatoes.",
        "Lovely.",
        json!({}),
    );
    sow_state(&mut h);
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    let id = format!("[#{}] ", short_id(&user("c")));
    assert_eq!(
        last_text(&c),
        format!(
            "{}\n- [state -- today: the garden; promised: Call Hannah on Sunday.]\n\n{id}\
             [the person, now -- their own words to you]\nc",
            head("curator")
        ),
        "{:?}",
        h.stderr
    );
}

#[test]
fn state_lines_caps_the_entries() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    let mut h = Hive::with(&[
        ("policy", "state_block", json!("1")),
        ("policy", "state_lines", json!(1)),
    ])
    .reporting_composite("talky");
    turn(
        &mut h,
        "s1",
        "t1",
        "We planted tomatoes.",
        "Lovely.",
        json!({}),
    );
    sow_state(&mut h);
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("Be brief."));
    let text = last_text(&c);
    assert!(
        text.contains("- [state -- today: the garden]\n"),
        "one entry, the first of the order: {text}"
    );
    assert!(!text.contains(PROMISE), "{text}");
}
