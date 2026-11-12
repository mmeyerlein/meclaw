//! The wall `over_aim` of the bank of constructed windows (GH #1465,
//! `support/curator_bank.rs`): 140 000 tokens of talk over four days, with
//! planted promises, open points and facts.
//!
//! What it counts: the wall is talk alone, the person's words and the
//! companion's answers, so no garbage collection brings it under its aim;
//! the rebuild has to hand rows to the summarizer (a stub in the gate). The
//! days are filled to a quota each (`DAYS`), the wall adds up to 140 000
//! tokens at the rate the policy counts with when the tap names none (three
//! characters a token).
//!
//! THE PLANTED POINTS (`planted`): every eighth turn carries one, in turn a
//! promise (in the companion's answer), an open point and a fact (in the
//! person's words). Each is one row of the wall and is known by the keyword
//! of that row, which no other row carries. `planted()` returns them as a
//! counted list with the day each was said on, so whoever reads the request
//! the summarizer was sent (`Outcome::requests`) can say which of them the
//! request holds.
//!
//! TWO KINDS MORE (D58 plant, GH #1490): condensing memories lose above all
//! the subject of a statement and keep an old state as valid (MedGraph,
//! arXiv 2611.08433, section 4 no. 1). The beats four turns behind the old
//! ones (`turn_no % 8 == 7`) plant, in a cycle of four, the old value of a
//! `superseded` point, a `third_party` fact (the person's fact about a third
//! person, `subject` names that person), the new value of the `superseded`
//! point two beats after its old one ("... is now {new}, no longer {old}"),
//! and nothing. All of them are the person's words. The old kinds keep their
//! beats, their order, their sentences and their keywords
//! (`gh_d58_plant_the_old_kinds_stay_as_they_were`): a row of a new kind
//! takes the length of the filler row it replaces as near as whole filler
//! sentences allow (`fitted`), the drift summed over the wall stays a few
//! characters, so every day ends on the turn it ended on before.

use super::{Bank, Run, Wall, WallBuilder, day, filler};
use crate::curator_hive::{Hive, canonical};
use chrono::Duration;
use meclaw_core::serde_json::json;

/// The characters a token counts at (`CHARS_PER_TOKEN` of the policy).
const RATE: usize = 3;

/// The tokens the wall adds up to, at least.
pub const TOKENS: usize = 140_000;

/// The days of the wall, oldest first: the day as an offset of the bank's
/// day, and the tokens said by the end of it.
const DAYS: [(i64, usize); 4] = [(-3, 10_000), (-2, 30_000), (-1, 90_000), (0, TOKENS)];

/// The beats of the new kinds: `turn_no % 8 == NEW_BEAT`, four turns behind
/// the beat of the old kinds.
const NEW_BEAT: usize = 7;

/// The third persons of the `third_party` facts.
const THIRD: [&str; 5] = [
    "daughter",
    "neighbour",
    "brother",
    "granddaughter",
    "nephew",
];

/// The matters of the `superseded` points, one per point: no matter changes
/// twice, so the newest value of each is the one valid.
const MATTERS: [&str; 10] = [
    "the meeting of the reading circle",
    "the check-up at the eye clinic",
    "the course of the choir",
    "the talk at the pension office",
    "the class of the chair gym",
    "the visit of the chimney sweep",
    "the evening of the chess club",
    "the hour of the library help",
    "the slot of the hearing test",
    "the session of the knitting group",
];

/// One planted point of the wall.
#[derive(Clone, Debug)]
pub struct Planted {
    /// `promise`, `open`, `fact`, `third_party` or `superseded`.
    pub kind: &'static str,
    /// The keyword only the row of this point carries.
    pub key: String,
    /// The sentence that was planted, the keyword in it.
    pub text: String,
    /// The day it was said on, as an offset of the bank's day.
    pub day: i64,
    /// The place of its row in the wall.
    pub piece: usize,
    /// `third_party`: the third person the fact is about (`daughter`), so
    /// whoever reads a summary can check whom it says the fact of.
    pub subject: Option<&'static str>,
    /// `superseded`: the row of the value the point replaces; `key`, `text`,
    /// `day` and `piece` above are those of the row with the new value.
    pub old: Option<OldRow>,
}

/// The row of the value a `superseded` point replaces.
#[derive(Clone, Debug)]
pub struct OldRow {
    /// The keyword only this row carries.
    pub key: String,
    /// The sentence that was planted, the keyword in it.
    pub text: String,
    /// The day it was said on, as an offset of the bank's day.
    pub day: i64,
    /// The place of its row in the wall.
    pub piece: usize,
}

const KINDS: [&str; 3] = ["promise", "open", "fact"];

const THINGS: [&str; 6] = [
    "the parcel for the harbour office",
    "the letter to the town archive",
    "the booking of the late ferry",
    "the key of the boat shed",
    "the list for the upper market",
    "the form of the tide office",
];

/// The session of a day of the wall.
fn session(d: i64) -> String {
    format!("voice-{}", day(d, 7, 0).format("%Y-%m-%dT%H:%M"))
}

/// The sentence of the `n`-th planted point; `{k}` is the keyword.
fn sentence(kind: &str, n: usize) -> String {
    let thing = THINGS[n % THINGS.len()];
    match kind {
        "promise" => format!(
            "I promise to bring up {thing} again on the morning after, under the note {{k}}."
        ),
        "open" => {
            format!("Still open, and please keep it so: {thing} is not settled, see {{k}}.")
        }
        _ => format!(
            "For the record: the number that belongs to {thing} is {}, filed as {{k}}.",
            4100 + 37 * n
        ),
    }
}

/// The third person and the sentence of the `n`-th `third_party` point;
/// `{k}` is the keyword.
fn third_party(n: usize) -> (&'static str, String) {
    let who = THIRD[n % THIRD.len()];
    let thing = THINGS[(n + 1) % THINGS.len()];
    (
        who,
        format!("My {who} now looks after {thing}, not me, noted as {{k}}."),
    )
}

/// The sentences of the `n`-th `superseded` point, old and new; `{k}` is
/// the keyword.
fn superseded(n: usize) -> (String, String) {
    let matter = MATTERS[n];
    let (old, new) = (7100 + 53 * n, 8300 + 61 * n);
    (
        format!("For the record: {matter} is in room {old}, noted as {{k}}."),
        format!(
            "That has changed: {matter} is now in room {new}, no longer in room {old}, \
             noted as {{k}}."
        ),
    )
}

/// The row of a new kind in place of a filler row of `len` characters:
/// `sentence` and as many filler sentences (none to six) as bring the
/// drift of the wall -- the characters the new rows added over the filler
/// rows they replace, `drift` -- nearest to nought. Returns the row and
/// moves the drift on.
fn fitted(sentence: &str, seed: u64, len: usize, drift: &mut i64) -> String {
    let row = (0..=6)
        .map(|n| match n {
            0 => sentence.to_string(),
            n => format!("{sentence} {}", filler(seed, n)),
        })
        .min_by_key(|r| (*drift + r.len() as i64 - len as i64).abs())
        .expect("a row");
    *drift += row.len() as i64 - len as i64;
    row
}

/// The wall and its planted points.
fn build() -> (Wall, Vec<Planted>) {
    let mut w = WallBuilder::new("over_aim", "quillon");
    let mut planted: Vec<Planted> = Vec::new();
    let (mut chars, mut turn_no, mut pieces) = (0usize, 0usize, 0usize);
    // The old kinds count their own points: their sentences stay those of
    // the wall without the new kinds.
    let mut old_kinds = 0usize;
    let (mut thirds, mut sups, mut drift) = (0usize, 0usize, 0i64);
    // The old row of the `superseded` point whose new row is still to come.
    let mut open_old: Option<(usize, OldRow)> = None;
    for (d, quota) in DAYS {
        let s = session(d);
        let mut at = day(d, 7, 0);
        while chars < quota * RATE {
            turn_no += 1;
            let turn = format!("o{turn_no:03}");
            let seed = turn_no as u64;
            let plant = (turn_no % 8 == 3).then(|| KINDS[(turn_no / 8) % KINDS.len()]);
            w.at(at);
            let mut said = format!("Listen, {{k}}. {}", filler(seed, 4));
            if let Some(kind) = plant.filter(|k| *k != "promise") {
                said = format!("{} {}", sentence(kind, old_kinds), filler(seed, 4));
            }
            // A beat of the new kinds: its place in the cycle of four.
            let new = (turn_no % 8 == NEW_BEAT).then_some((turn_no / 8) % 4);
            let new_sentence = match new {
                Some(0) => Some(superseded(sups).0),
                Some(1) => Some(third_party(thirds).1),
                Some(2) => {
                    let (n, _) = open_old
                        .as_ref()
                        .expect("over_aim: an old value before its new one");
                    Some(superseded(*n).1)
                }
                _ => None,
            };
            if let Some(sentence) = &new_sentence {
                said = fitted(sentence, seed, said.len(), &mut drift);
            }
            let key = w.words(&s, &turn, &said);
            chars += said.len() - 3 + key.len();
            pieces += 1;
            let new_text = new_sentence.map(|t| t.replace("{k}", &key));
            if let Some(kind) = plant.filter(|k| *k != "promise") {
                planted.push(Planted {
                    kind,
                    text: sentence(kind, old_kinds).replace("{k}", &key),
                    key: key.clone(),
                    day: d,
                    piece: pieces - 1,
                    subject: None,
                    old: None,
                });
                old_kinds += 1;
            }
            match (new, new_text) {
                (Some(0), Some(text)) => {
                    let row = OldRow {
                        key,
                        text,
                        day: d,
                        piece: pieces - 1,
                    };
                    open_old = Some((sups, row));
                    sups += 1;
                }
                (Some(1), Some(text)) => {
                    planted.push(Planted {
                        kind: "third_party",
                        text,
                        key,
                        day: d,
                        piece: pieces - 1,
                        subject: Some(third_party(thirds).0),
                        old: None,
                    });
                    thirds += 1;
                }
                (Some(2), Some(text)) => {
                    let (_, old) = open_old.take().expect("over_aim: the old row of the value");
                    planted.push(Planted {
                        kind: "superseded",
                        text,
                        key,
                        day: d,
                        piece: pieces - 1,
                        subject: None,
                        old: Some(old),
                    });
                }
                _ => {}
            }
            let mut back = format!("{} So much for {{k}}.", filler(seed + 5000, 22));
            if plant == Some("promise") {
                back = format!(
                    "{} {}",
                    sentence("promise", old_kinds),
                    filler(seed + 5000, 22)
                );
            }
            let key = w.answer(&s, &turn, &back);
            chars += back.len() - 3 + key.len();
            pieces += 1;
            if plant == Some("promise") {
                planted.push(Planted {
                    kind: "promise",
                    text: sentence("promise", old_kinds).replace("{k}", &key),
                    key,
                    day: d,
                    piece: pieces - 1,
                    subject: None,
                    old: None,
                });
                old_kinds += 1;
            }
            at += Duration::minutes(3);
        }
        assert!(
            at < day(d, 23, 0),
            "over_aim: day {d} holds its turns before midnight"
        );
    }
    let tokens = chars / RATE;
    assert!(
        (TOKENS..TOKENS + 2_000).contains(&tokens),
        "over_aim: the wall adds up to {TOKENS} tokens, counted {tokens}"
    );
    for kind in KINDS {
        let n = planted.iter().filter(|p| p.kind == kind).count();
        assert!(
            n >= 10,
            "over_aim: at least ten planted {kind}, counted {n}"
        );
    }
    assert!(
        open_old.is_none(),
        "over_aim: every old value of a superseded point has its new one"
    );
    for (d, _) in DAYS {
        assert!(
            planted.iter().any(|p| p.day == d),
            "over_aim: day {d} holds a planted point"
        );
    }
    // The wall stands over the target each role presets, so no run names one.
    let wall = w
        .build(day(0, 23, 30), &session(0))
        .run(Run::of("talky"))
        .run(Run::of("quick"));
    for p in &planted {
        assert!(
            wall.pieces[p.piece].key == p.key && wall.pieces[p.piece].text().contains(&p.text),
            "over_aim: the planted point {} is the row it names",
            p.key
        );
        if let Some(old) = &p.old {
            assert!(
                wall.pieces[old.piece].key == old.key
                    && wall.pieces[old.piece].text().contains(&old.text),
                "over_aim: the old row {} of {} is the row it names",
                old.key,
                p.key
            );
        }
    }
    (wall, planted)
}

/// The planted points of the wall, in wall order: a counted list.
pub fn planted() -> Vec<Planted> {
    build().1
}

/// Sows the facts planted on the last day as `fact` marks of that day, the
/// way `./intake` files what a sidecar states: the state of the round then
/// holds them as set today.
fn sow_state(h: &mut Hive) {
    let (wall, planted) = build();
    for p in planted.iter().filter(|p| p.day == 0 && p.kind == "fact") {
        let row = &wall.pieces[p.piece];
        let value = canonical(&json!({
            "subject": p.key, "predicate": "is_filed", "claim": p.text,
            "source": "person", "stance": "asserted"}));
        h.mark(row.at, &row.session, &row.turn, "fact", &value);
    }
}

/// Sows one `topic` per day of the wall: `start` on the first turn of a
/// day, `end` behind the last turn of every day but the last, which runs.
fn sow_topics(h: &mut Hive) {
    let (wall, _) = build();
    for (i, (d, _)) in DAYS.iter().enumerate() {
        let s = session(*d);
        let rows: Vec<_> = wall.pieces.iter().filter(|p| p.session == s).collect();
        let (first, last) = (rows[0], rows[rows.len() - 1]);
        let name = format!("the errands of day {}", i + 1);
        let start = canonical(&json!({"movement": "start", "name": name}));
        h.mark(first.at, &s, &first.turn, "topic", &start);
        if i + 1 < DAYS.len() {
            let end = canonical(&json!({"movement": "end", "name": name}));
            h.mark(
                last.at + Duration::seconds(30),
                &s,
                &last.turn,
                "topic",
                &end,
            );
        }
    }
}

/// The two variants of the wall, the rows and the planted points of
/// `over_aim` in both: `over_aim_state` holds the facts of the last day as
/// state of the round as well, `over_aim_topics` names every day a topic of
/// its own, the last one running. They are no walls of the bank (no seat
/// registers them): whoever reads what a rebuild hands the summarizer runs
/// them beside the wall.
pub fn variants() -> Vec<Wall> {
    let mut state = build().0.prepare(sow_state);
    state.name = "over_aim_state";
    let mut topics = build().0.prepare(sow_topics);
    topics.name = "over_aim_topics";
    vec![state, topics]
}

/// Registers the walls of this seat.
pub fn register(bank: &mut Bank) {
    bank.wall(build().0);
}
