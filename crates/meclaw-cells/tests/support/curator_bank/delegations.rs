//! The wall `delegations` of the bank of constructed windows (GH #1465,
//! `support/curator_bank.rs`): one call with three errands handed on, a
//! bridge in front of each, and one sentence said ahead of its answer.
//!
//! What it counts: an errand opens a round of its own, the turn
//! `<turn>~<deadline_ms>~<hex8>`, whose `(delegation)` and `(advice)` rows
//! are nobody's words. Two errands are over when the rebuild plans (their
//! deadline has passed): their four rows are garbage of the class
//! `scaffold`. The third is still running (its deadline lies ten minutes
//! ahead): its row stays. The bridges, the sentence said ahead and every
//! answer are the companion's own words and stay as well.
//!
//! TWO ERRANDS END WITHOUT AN ANSWER (GH #1469). One nobody answers before
//! its deadline, and one whose answering call comes back with an
//! `error_code`. Both travel the lanes of the hive (`errand_round`), so the
//! curator parks them itself and hears the failure on its own tap. Each
//! leaves the `(delegation)` row alone in the ledger (bank find F6); past
//! its deadline that row is the scaffold of a closed errand like any other,
//! and it is counted so. What stays of such an errand is the row the curator
//! files with `errand_outcome` "1": it says `failed` or `no answer`. The run
//! that sets the knob is the last of the wall, and `outcomes_stand` reads it:
//! the outcome stands in the window after the rebuild.

use super::{Bank, Ending, Run, WallBuilder, What, day, declared, filler};
use chrono::{DateTime, Duration, Utc};
use meclaw_core::serde_json::json;

const SESSION: &str = "voice-2026-03-02T09:00";

/// The turn of the round an errand of `turn` opens, over at `deadline`.
fn errand(turn: &str, deadline: DateTime<Utc>, hex8: &str) -> String {
    format!("{turn}~{}~{hex8}", deadline.timestamp_millis())
}

/// The row a delegation opens its round with; the keyword stands last.
fn delegation(item: &str, heard: &str, key: &str) -> String {
    format!(
        "[delegation {item} from the voice channel: the caller expects backend work; last \
         heard: {heard} -- {} -- reference {key}]",
        filler(item.len() as u64, 3)
    )
}

/// The advice the reasoning core hands back; the keyword stands last.
fn advice(seed: u64, key: &str) -> String {
    format!(
        "[advice from your reasoning core -- say it in your own words]\n{}\nReference {key}.",
        filler(seed, 80)
    )
}

/// Registers the walls of this seat.
pub fn register(bank: &mut Bank) {
    let mut w = WallBuilder::new("delegations", "orrery");
    let s = SESSION;
    let scaffold = What::Junk("scaffold");
    let now = day(0, 18, 0);

    w.at(day(0, 9, 0));
    w.words(
        s,
        "d01",
        "Hello, {k}. I have a few things to sort out today.",
    );
    w.answer(s, "d01", "Hello. Let us take them one by one, {k}.");

    // The first errand: handed on, answered, over.
    w.at(day(0, 9, 2));
    w.words(
        s,
        "d02",
        "What is in my calendar for the week after next, {k}?",
    );
    w.answer(s, "d02", "One moment, {k}, I am having that looked up.");
    let inner = errand("d02", day(0, 9, 4), "0a1b2c3d");
    w.push(scaffold.clone(), s, &inner, "delegation", 0, |k| {
        super::user(&delegation(
            "item_Qm3x",
            "What is in my calendar for the week after next",
            k,
        ))
    });
    w.push(scaffold.clone(), s, &inner, "advice", 0, |k| {
        super::user(&advice(7, k))
    });
    w.answer(
        s,
        &inner,
        "The week after next holds the dentist on Tuesday and the choir on Thursday, {k}.",
    );

    // A sentence said ahead of the answer.
    w.at(day(0, 9, 10));
    w.words(
        s,
        "d03",
        "And do I have to bring anything to the choir, {k}?",
    );
    w.interim(s, "d03", "Let me think about the choir for a second, {k}.");
    w.answer(
        s,
        "d03",
        "Only the blue folder, {k}. The sheets are handed out there.",
    );

    // The second errand, over as well.
    w.at(day(0, 9, 15));
    w.words(
        s,
        "d04",
        "Please find out whether the dentist can move me to the morning, {k}.",
    );
    w.answer(
        s,
        "d04",
        "I will have that checked, {k}. It takes a moment.",
    );
    let inner = errand("d04", day(0, 9, 17), "4e5f6a7b");
    w.push(scaffold.clone(), s, &inner, "delegation", 0, |k| {
        super::user(&delegation(
            "item_Rt8k",
            "whether the dentist can move me to the morning",
            k,
        ))
    });
    w.push(scaffold, s, &inner, "advice", 0, |k| {
        super::user(&advice(19, k))
    });
    w.answer(
        s,
        &inner,
        "The dentist has a slot at nine on Tuesday, {k}. Shall I ask for it?",
    );

    // Eight plain turns.
    for i in 5..=12 {
        let turn = format!("d{i:02}");
        w.at(day(0, 9, 20) + Duration::minutes(5 * (i - 5)));
        w.words(
            s,
            &turn,
            &format!(
                "While we are at it, {{k}}: remind me what stood under number {i} of my notes."
            ),
        );
        w.answer(
            s,
            &turn,
            &format!("Number {i} was about the cellar shelves, {{k}}. Nothing in it is urgent."),
        );
    }

    // An errand nobody answers: handed on at 17:30, due at 17:58. No turn
    // of the session comes between its deadline and the turn that orders
    // the rebuild, so that turn is the first to find it over.
    w.at(day(0, 17, 30));
    w.words(
        s,
        "d14",
        "Before I forget, {k}: find out when the cellar is read for the meter.",
    );
    w.answer(s, "d14", "I am having that asked, {k}.");
    w.errand_round(
        s,
        &errand("d14", day(0, 17, 58), "2a3b4c5d"),
        Ending::Silent,
        |k| {
            super::user(&delegation(
                "item_Lh5n",
                "when the cellar is read for the meter",
                k,
            ))
        },
    );

    // An errand that fails: handed on at 17:40, due at 17:45; the call of
    // its round comes back as an error of the provider.
    w.at(day(0, 17, 40));
    w.words(
        s,
        "d15",
        "And ask the landlord about the broken letterbox, {k}.",
    );
    w.answer(s, "d15", "That goes out as well, {k}.");
    w.errand_round(
        s,
        &errand("d15", day(0, 17, 45), "6e7f8a9b"),
        Ending::Failed("upstream_error"),
        |k| {
            super::user(&delegation(
                "item_Zc4d",
                "the landlord about the broken letterbox",
                k,
            ))
        },
    );

    // The third errand: still running when the rebuild plans.
    w.at(day(0, 17, 55));
    w.words(
        s,
        "d13",
        "One last thing, {k}: ask whether the choir still needs a second bass.",
    );
    w.answer(s, "d13", "I am passing that on right now, {k}.");
    let inner = errand("d13", now + Duration::minutes(10), "8c9d0e1f");
    w.push(What::Open("scaffold"), s, &inner, "delegation", 0, |k| {
        super::user(&delegation(
            "item_Vw2p",
            "whether the choir still needs a second bass",
            k,
        ))
    });

    // The run that files the outcome of an errand without an answer. A knob
    // the policy does not declare cannot be set (the door refuses it); the
    // run is then the one before it, and `outcomes_stand` says what is
    // missing.
    let mut outcome = Run::of("consult").with("gc", json!("1"));
    if declared("policy", "errand_outcome") {
        outcome = outcome.with("errand_outcome", json!("1"));
    }
    // The target. The call of the errand that fails comes back on the tap
    // at 17:40, and a tap measures the window: one that finds it at its
    // target orders a rebuild there and holds the next order until the
    // window has grown (`target_hold`). Nothing strikes inside a wall, so
    // the turn of the bank would then order none (red run 4a36db7e: target
    // 3400, the window 4206 at that tap and 4409 at the turn of the bank,
    // `curate_mark` "none"). The target grows with the two errands the wall
    // gained: over the window of that tap, under the one of the turn of the
    // bank, which is so the first to reach it. Read anew after the rebase on
    // the turn pin (GH #1471): the wall passes for every target from 4207 to
    // 4409 (4206 and 4410 fail); the target stands in the middle of that band.
    bank.wall(
        w.build(now, SESSION)
            .target(4308)
            .run(Run::of("talky"))
            .run(Run::of("consult"))
            .run(Run::of("consult").with("gc", json!("1")))
            .run(outcome),
    );
}
