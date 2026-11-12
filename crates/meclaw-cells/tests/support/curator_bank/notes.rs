//! The wall `notes` of the bank of constructed windows (GH #1465,
//! `support/curator_bank.rs`): one written conversation with notes another
//! hive handed the curator and candidates it pushed, two per item.
//!
//! What it counts beside the rows (the notes, handed in on `in_note` the way
//! their sender does): two items with two notes each -- the older is spent,
//! the younger is open and stands; one note of scope `turn`, which the turn
//! in front of the rebuild shows and uses up; one note of scope `session`
//! without an item, which stands. The spent notes are the class
//! `spent_notes` (GH #1451): with `notes_latest_per_item` "1" only the
//! youngest note of one `(source, item)` stands in the notes block. The
//! shipped value of the knob is "0"; the wall sets "1" on every run.
//!
//! What it counts among the rows (the candidates): a candidate is shown
//! once, as a pair in the wall (`call_candidates_<hex>`). Two items with two
//! pairs each: the older pair lies outside `keep_rounds` when the rebuild
//! plans and is an old result like any other (`shrunk_tools`); the younger
//! pair of the youngest tool round stays as it came.

use super::{Bank, Collects, Hive, Knobs, Run, WallBuilder, What, day, filler, tool_result};
use meclaw_core::serde_json::json;

const SESSION: &str = "chat-2026-03-02T15:00";

/// The share of the window a run gives its notes block. A role whose preset
/// is 0 (every role but one, `notes_share` of the role table) shows no note
/// at all -- the budget of its block is nothing --, so a wall that counts
/// which notes stand names the share on those runs.
const SHARE: f64 = 0.02;

/// `(id, item, scope, counted as, text)` of every note; the keyword is the
/// last word of the text.
const NOTES: [(&str, &str, &str, bool, &str); 6] = [
    (
        "on-000000000001",
        "it-a",
        "session",
        true,
        "due: question on the roof repair, quillon-note-a1",
    ),
    (
        "on-000000000002",
        "it-a",
        "session",
        false,
        "due: the roof repair was moved to Friday, quillon-note-a2",
    ),
    (
        "on-000000000003",
        "it-b",
        "session",
        true,
        "due: question on the parcel for the neighbour, quillon-note-b1",
    ),
    (
        "on-000000000004",
        "it-b",
        "session",
        false,
        "due: the parcel was handed over at noon, quillon-note-b2",
    ),
    (
        "on-000000000005",
        "",
        "session",
        false,
        "the caller hears badly on the left, quillon-note-s1",
    ),
    (
        "on-000000000006",
        "",
        "turn",
        true,
        "open with the name of the caller, quillon-note-t1",
    ),
];

fn key_of(text: &str) -> &str {
    text.rsplit(' ').next().unwrap_or("")
}

/// The notes, handed in one by one on `in_note`.
fn hand_in(h: &mut Hive) {
    for (id, item, scope, _, text) in NOTES {
        let mut n = json!({"id": id, "source": "orga", "text": text, "scope": scope});
        if !item.is_empty() {
            n["item"] = json!(item);
        }
        h.lane(
            "in_note",
            json!({"session_id": SESSION}),
            json!({}),
            json!({"notes": [n]}),
        );
    }
}

/// The spent notes go with the knob the wall sets on every run: statement
/// 1 judges them on each (the note is gone from the window).
fn spent_notes(_: &Knobs) -> Collects {
    Collects::Always
}

/// A candidates pair in the wall: the call, and the push line with the
/// keyword behind the frame line.
fn candidates(w: &mut WallBuilder, what: What, turn: &str, n: u64, item: &str, bulk: usize) {
    let id = format!("call_candidates_{n:016x}");
    w.push(What::Other, SESSION, turn, "recall", 0, |k| {
        json!({"origin": "assistant", "type": "tool_call", "id": id,
               "text": json!({"name": "memory_recall",
                              "arguments": json!({"ref": k}).to_string()}).to_string()})
    });
    w.push(what, SESSION, turn, "recall", 0, |k| {
        tool_result(
            &id,
            &format!(
                "[addendum -- notes handed to this conversation for this round; not said by \
                 the person]\n[About {item}: {} Reference {k}.]",
                filler(n, bulk)
            ),
        )
    });
}

/// Registers the walls of this seat.
pub fn register(bank: &mut Bank) {
    // Spent notes are cleared by every call, in the notes block it builds
    // at the end of the window -- never by the rebuild, so the `gc` mark has
    // no figure for them (plan part `core-bank`, addendum F4).
    bank.class_of_the_call("spent_notes", spent_notes);
    let mut w = WallBuilder::new("notes", "quillon");
    let s = SESSION;

    w.at(day(0, 15, 0));
    w.words(s, "n01", "Hello, {k}. Let us go through the week.");
    w.answer(s, "n01", "Hello. I am listening, {k}.");

    // The older pair of each item.
    for (i, item) in ["the roof repair", "the parcel"].into_iter().enumerate() {
        let turn = format!("n{:02}", i + 2);
        w.at(day(0, 15, 5) + chrono::Duration::minutes(5 * i as i64));
        w.words(s, &turn, &format!("What about {item}, {{k}}?"));
        candidates(
            &mut w,
            What::Junk("shrunk_tools"),
            &turn,
            i as u64 + 1,
            item,
            90,
        );
        w.answer(
            s,
            &turn,
            &format!("I have a note on {item}, {{k}}. It is being looked after."),
        );
    }

    for i in 4..=9 {
        let turn = format!("n{i:02}");
        w.at(day(0, 15, 20) + chrono::Duration::minutes(4 * (i - 4)));
        w.words(
            s,
            &turn,
            &format!("And day {i} of the week, {{k}}: is anything planned?"),
        );
        w.answer(
            s,
            &turn,
            &format!("Day {i} is free so far, {{k}}. {}", filler(i as u64, 8)),
        );
    }

    // The younger pair of each item, in the youngest tool round.
    w.at(day(0, 15, 50));
    w.words(s, "n10", "Once more the roof and the parcel, {k}.");
    candidates(&mut w, What::Other, "n10", 11, "the roof repair", 6);
    candidates(&mut w, What::Other, "n10", 12, "the parcel", 6);
    w.answer(
        s,
        "n10",
        "The roof is done on Friday and the parcel is handed over, {k}.",
    );

    let mut wall = w
        .build(day(0, 16, 0), SESSION)
        .target(3000)
        .knob("policy", "notes_latest_per_item", json!("1"))
        .prepare(hand_in)
        .run(Run::of("talky"))
        .run(Run::of("quick").with("notes_share", json!(SHARE)))
        .run(
            Run::of("quick")
                .with("notes_share", json!(SHARE))
                .with("gc", json!("1")),
        );
    for (_, _, _, spent, text) in NOTES {
        let what = if spent {
            What::Junk("spent_notes")
        } else {
            What::Open("spent_notes")
        };
        wall = wall.beside(what, key_of(text));
    }
    bank.wall(wall);
}
