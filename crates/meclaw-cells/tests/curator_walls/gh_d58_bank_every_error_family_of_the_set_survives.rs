//! GH #1465 (D58, the bank) -- every error family of the set survives.
//!
//! WHY this file exists: a rebuild never shortens a failed tool result (GH
//! #1430): the model keeps seeing that a tool failed, in the words of the
//! failure. A shipped tool says that it failed in the header of its result
//! (`error_code`), which the collector hands on per call id in the slot
//! `tool_errors` of the curate (GH #1279); its text is an error object or
//! plain prose. The wall `errors` of the bank holds, for every tool the
//! shipped templates answer, one failed result as the tool writes it (the
//! inventory, with the source of text and code), and feeds each through the
//! lanes the way a live round arrives -- nothing is written into the ledger
//! by the test. After a rebuild, with every role the wall is made for, each
//! of them stands in the window byte for byte.
//!
//! The counter-probe in the same wall is the named limit: a result that
//! tells of a failure in prose alone, with no code in its header and no
//! error object as its text, is a result like any other -- an old one is
//! shortened exactly as the old result next to it is. No list of words
//! turns prose into a failure.
//!
//! Runs the shipped curator hive over the bank (`support/curator_bank.rs`).

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_bank::errors::{FAMILIES, PLAIN, UNMARKED, inventory};
use curator_bank::*;
use curator_hive::{policy_scope, shipped};
use meclaw_core::serde_json::{Value, json};

#[test]
fn every_error_family_of_the_set_survives() {
    survives_as(Run::clone);
}

/// The same statement over the runs of the wall `errors` as rebuilds for
/// an expired cache with the cold collection on (`Run::cold`, GH #1464): the
/// collection under the aim takes no failed result either.
#[test]
fn through_the_cache() {
    survives_as(Run::cold);
}

/// The statement over the runs of the wall `errors`, each made `as_run` of
/// it.
fn survives_as(as_run: fn(&Run) -> Run) {
    if !shipped() {
        return;
    }
    let bank = bank();
    let mut found = Findings::default();
    let inv = inventory();

    // The inventory names every family, each with the code of its header.
    for family in FAMILIES {
        if inv.iter().all(|f| f.family != family) {
            found.note(format!(
                "inventory: the tool family {family} has no failed result in the inventory \
                 (a gap of the bank, no statement about the policy)"
            ));
        }
    }
    for f in &inv {
        if f.code.is_empty() {
            found.note(format!(
                "inventory: the failed result of {} names no header code (a gap of the bank)",
                f.tool
            ));
        }
    }

    // What the text alone tells the policy (printed, no statement: a
    // failure in prose is known by its header code, not by its words).
    let texts: Vec<Value> = inv.iter().map(|f| json!(f.text)).collect();
    let (by_text, err) = policy_scope(
        json!({}),
        "[bool(is_error_result(t)) for t in ARGS]",
        Value::Array(texts),
    );
    assert!(!err.contains("Traceback"), "{err}");

    // The wall holds every failed result of the inventory, each on its way
    // through the lanes with its header code.
    let wall = bank.named("errors");
    let failed = wall.counted(|w| matches!(w, What::Error));
    assert_eq!(
        failed.len(),
        inv.len(),
        "the wall holds every failed result of the inventory"
    );
    for (n, i) in failed.iter().enumerate() {
        let p = &wall.pieces[*i];
        assert!(
            p.lane && p.code == inv[n].code && p.text() == inv[n].text,
            "the failed result of {} travels the lanes as the tool wrote it, with its code",
            inv[n].tool
        );
    }
    let result_of = |turn: &str| -> usize {
        let at = wall
            .pieces
            .iter()
            .position(|p| p.turn == turn && p.el["type"] == "tool_result");
        at.unwrap_or_else(|| panic!("the wall holds a tool result in the turn {turn}"))
    };
    let (probe, plain) = (result_of(UNMARKED), result_of(PLAIN));
    assert!(
        wall.pieces[probe].lane && wall.pieces[probe].code.is_empty(),
        "the counter-probe travels the lanes without a header code"
    );

    let runs = run_walls_as(&bank, |w| w.name == "errors", &mut found, as_run);
    assert!(!runs.is_empty(), "the wall errors runs");
    let mut probe_shortened = 0;
    for (wall, o) in &runs {
        // After the rebuild every failed result stands byte for byte.
        let mut raw = 0;
        for (n, i) in failed.iter().enumerate() {
            if !o.raw(*i) {
                continue;
            }
            raw += 1;
            let shown = o.shown(wall, *i);
            eprintln!(
                "recognition: {}: {} (header code {}, text alone reads as failed: {}): {}",
                o.who(),
                inv[n].tool,
                inv[n].code,
                by_text[n],
                if shown == Shown::Whole {
                    "stands whole"
                } else {
                    "NOT whole"
                }
            );
            if shown != Shown::Whole {
                found.note(format!(
                    "window: {}: the failed result of {} ({}, header code {}) does not stand \
                     byte for byte after the rebuild: {shown:?} -- source: {}",
                    o.who(),
                    inv[n].tool,
                    wall.pieces[*i].key,
                    inv[n].code,
                    inv[n].source
                ));
            }
        }
        if raw == 0 {
            found.note(format!(
                "window: {}: no failed result lies in the raw part of the plan",
                o.who()
            ));
        }

        // The counter-probe: prose without any sign is shown the way the
        // old result next to it is.
        if !(o.raw(probe) && o.raw(plain)) {
            found.note(format!(
                "counter-probe: {}: the prose result and the old result next to it do not \
                 both lie in the raw part of the plan",
                o.who()
            ));
            continue;
        }
        let (as_probe, as_plain) = (o.shown(wall, probe), o.shown(wall, plain));
        eprintln!(
            "counter-probe: {}: prose without a sign {as_probe:?}, old result {as_plain:?}",
            o.who()
        );
        let form = |s: &Shown| match s {
            Shown::Whole => "whole",
            Shown::Form(Form::Shortened) => "shortened",
            _ => "another form",
        };
        if form(&as_probe) != form(&as_plain) || form(&as_probe) == "another form" {
            found.note(format!(
                "counter-probe: {}: the prose result without a sign ({}) is not shown like the \
                 old result next to it: {as_probe:?} against {as_plain:?}",
                o.who(),
                wall.pieces[probe].key
            ));
        }
        if as_probe == Shown::Form(Form::Shortened) {
            probe_shortened += 1;
        }
    }
    if probe_shortened == 0 {
        found.note(
            "counter-probe: no run shortened the prose result without a sign (the probe \
             judged nothing)"
                .to_string(),
        );
    }
    found.finish();
}
