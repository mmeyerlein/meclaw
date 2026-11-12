//! GH #1469 (D58 errand) -- `errand_outcome` off is byte identical.
//!
//! WHY this file exists: the knob ships "0" and a set turns it on. A colony
//! on an older set must see the curator of before: the same calls, the same
//! operations on the ledger in the same order, no row of kind `outcome` and
//! no mark `errand_outcome` -- for an errand that failed, one nobody
//! answered and a rebuild behind both. The run is driven by the clock of
//! the bank, so two runs are equal to the byte or the knob changed one.
//!
//! Two figures are not the clock's: the snapshot row of a call carries
//! `cut_ms`, the station time of its cut, and the `lead` mark of a call
//! carries `handle_ms`, the station time of the lead -- both read off a
//! monotonic stopwatch (`time.perf_counter`), which the clock of the bank
//! does not drive (red run 4a36db7e: 561 operations each, 7 differ, every
//! one in `cut_ms` alone; under full load once more, `handle_ms` 1 against
//! 0, D58 plant OR-58-121). They say how long the machine took and nothing
//! about what the curator did, so the comparison reads the operations
//! without them -- every other byte of a row, the value of the `lead` mark
//! beside `handle_ms` included, stays compared.

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_errand.rs"]
#[allow(unused_imports)]
use crate::curator_errand; // harness_fold: mod curator_errand;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use chrono::Duration;
use curator_errand::*;
use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// One failed errand, one nobody answers, a turn past both deadlines and a
/// rebuild: the calls that left for the model, and the hive.
fn run(knob: Option<&str>) -> (Hive, Vec<Msg>) {
    let mut h = hive(knob, &[("policy", "keep_recent", json!(1))]);
    let mut calls = Vec::new();
    let failed = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    calls.push(failed.call.clone());
    fails(
        &mut h,
        start() + Duration::seconds(20),
        &failed,
        "upstream_error",
    );
    let silent = hand_on(&mut h, start() + Duration::minutes(1), "p02", "bbbb2222", 5);
    calls.push(silent.call.clone());
    let call = ask(&mut h, start() + Duration::minutes(10), "t3");
    close(&mut h, &call);
    calls.push(call);
    strike(&mut h, start() + Duration::minutes(20), "b1");
    let call = ask(&mut h, start() + Duration::minutes(21), "t4");
    close(&mut h, &call);
    calls.push(call);
    (h, calls)
}

/// The station times a row of the ledger carries, read off a stopwatch:
/// the table, the column whose JSON holds the figure, and the figure.
const STATION_TIMES: [(&str, &str, &str); 2] = [
    ("snapshots", "cut", "cut_ms"),
    ("marks", "value", "handle_ms"),
];

/// The operations on the ledger as the store was asked to run them, in
/// their order, each row without its station times (`STATION_TIMES`). The
/// run must have measured each of them: a run with no such figure fails
/// here.
fn ops_without_station_time(h: &Hive) -> Vec<(String, Value)> {
    let mut measured = [0usize; STATION_TIMES.len()];
    let ops: Vec<(String, Value)> = h
        .ledger_ops
        .iter()
        .cloned()
        .map(|(sender, mut op)| {
            for (i, (table, column, figure)) in STATION_TIMES.iter().enumerate() {
                if op["table"] != json!(table) {
                    continue;
                }
                let Some(text) = op["row"][column].as_str() else {
                    continue;
                };
                let Ok(mut inner) = sj::from_str::<Value>(text) else {
                    continue;
                };
                if let Some(o) = inner.as_object_mut()
                    && o.remove(*figure).is_some()
                {
                    measured[i] += 1;
                    op["row"][column] = inner;
                }
            }
            (sender, op)
        })
        .collect();
    for ((_, _, figure), n) in STATION_TIMES.iter().zip(measured) {
        assert!(n > 0, "the run wrote rows with the station time {figure}");
    }
    ops
}

#[test]
fn off_is_byte_identical() {
    if !shipped() {
        return;
    }
    let (plain, p_calls) = run(None);
    let (off, o_calls) = run(Some("0"));
    assert_eq!(p_calls.len(), o_calls.len());
    for (i, (p, o)) in p_calls.iter().zip(&o_calls).enumerate() {
        assert_eq!(o.body, p.body, "call {i} with the knob off");
        assert_eq!(o.hop, p.hop, "the header of call {i} with the knob off");
    }
    assert_eq!(
        off.ledger_ops.len(),
        plain.ledger_ops.len(),
        "off runs no ledger operation more"
    );
    let (o_ops, p_ops) = (
        ops_without_station_time(&off),
        ops_without_station_time(&plain),
    );
    for (i, (o, p)) in o_ops.iter().zip(&p_ops).enumerate() {
        assert_eq!(o, p, "operation {i} on the ledger with the knob off");
    }
    assert!(!plain.ledger_ops.is_empty(), "the run wrote to the ledger");
    for (name, h) in [("unset", &plain), ("off", &off)] {
        assert_eq!(all_outcome_rows(h), 0, "{name}: no outcome row");
        assert!(
            h.rows("SELECT seq FROM marks WHERE kind = 'errand_outcome'")
                .is_empty(),
            "{name}: no mark of an outcome"
        );
        assert!(
            swept_keys(h).contains(&park_key("p01")),
            "{name}: the sweep takes the parked row as before: {:?}",
            swept_keys(h)
        );
    }
}
