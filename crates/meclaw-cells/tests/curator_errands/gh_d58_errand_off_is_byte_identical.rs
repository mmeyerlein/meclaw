//! GH #1469 (D58 errand) -- `errand_outcome` off is byte identical.
//!
//! WHY this file exists: the knob ships "0" and a set turns it on. A set that
//! names no knob (an older set) and a set that says "0" must run the same
//! curator: the same calls, the same operations on the ledger in the same
//! order, no row of kind `outcome` and no mark `errand_outcome` -- for an
//! errand that failed, one nobody answered and a rebuild behind both. Both
//! runs are of the code at hand (review errand M3): the pin holds "0" equal
//! to the default and free of every write of the knob. That the default is
//! the curator of before the errand strand is not this file's claim; it
//! rests on the strand's full run of the curator binaries on the tree of
//! before (run B, 1205 green) and on the reading of the code: every new leg
//! and every write hangs on `ERRAND_OUTCOME`. That holds for the guard of a
//! second errand in one turn too (wave d58-2 close, inventory 39, first gap:
//! the conditional park, the step `dlg-park`, the mark "not followed"); the
//! second test runs two errands of one turn and holds "0" to the
//! unconditional park of before, with no such mark. The run is driven by the
//! clock of the bank, so two runs are equal to the byte or the knob changed
//! one.
//!
//! Two figures are not the clock's: the snapshot row of a call carries
//! `cut_ms`, the station time of its cut, and the mark of kind `lead`
//! carries `handle_ms`, the station time of the lead -- both read off a
//! monotonic stopwatch (`time.perf_counter`), which the clock of the bank
//! does not drive (red run 4a36db7e: 561 operations each, 7 differ, every
//! one in `cut_ms` alone; under full load once more, `handle_ms` 1 against
//! 0, D58 plant OR-58-121). They say how long the machine took and nothing
//! about what the curator did: a wall-clock reading is never the verdict
//! (development rules § 11). The comparison reads the operations with each
//! of them replaced by a mark that keeps only that the figure is there and
//! is a number of milliseconds, not below zero (`cut_ms` is rounded to the
//! microsecond, `handle_ms` whole) -- only these two fields, only in rows
//! of that table and that kind; every other byte of a row stays compared.
//! The red runs of `handle_ms` 3 against 0 at the close of D58 (judge,
//! operations 65 and 133) came from branches without this normalisation.

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
/// the table, the kind of the row (`None`: every row of the table), the
/// column whose JSON holds the figure, and the figure. `handle_ms` is the
/// lead's alone (review plant m4): a mark of another kind that carried it
/// would be compared with it.
const STATION_TIMES: [(&str, Option<&str>, &str, &str); 2] = [
    ("snapshots", None, "cut", "cut_ms"),
    ("marks", Some("lead"), "value", "handle_ms"),
];

/// The operations on the ledger as the store was asked to run them, in
/// their order, each row with its station times (`STATION_TIMES`) marked. The
/// run must have measured each of them: a run with no such figure fails
/// here.
fn ops_without_station_time(h: &Hive) -> Vec<(String, Value)> {
    let mut measured = [0usize; STATION_TIMES.len()];
    let ops: Vec<(String, Value)> = h
        .ledger_ops
        .iter()
        .cloned()
        .map(|(sender, mut op)| {
            for (i, (table, kind, column, figure)) in STATION_TIMES.iter().enumerate() {
                if op["table"] != json!(table)
                    || kind.is_some_and(|k| op["row"]["kind"] != json!(k))
                {
                    continue;
                }
                let Some(text) = op["row"][column].as_str() else {
                    continue;
                };
                let Ok(mut inner) = sj::from_str::<Value>(text) else {
                    continue;
                };
                // The figure is there and a number of milliseconds, not
                // below zero: its value is the machine's, never compared;
                // anything else stays as it is and is compared.
                if let Some(o) = inner.as_object_mut()
                    && o.get(*figure)
                        .and_then(Value::as_f64)
                        .is_some_and(|ms| ms >= 0.0)
                {
                    o.insert((*figure).to_string(), json!("<station time>"));
                    measured[i] += 1;
                    op["row"][column] = inner;
                }
            }
            (sender, op)
        })
        .collect();
    for ((_, _, _, figure), n) in STATION_TIMES.iter().zip(measured) {
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

/// The round of a second errand of the turn `label` that is already there:
/// the lane of `hand_on` without the person's turn before it.
fn second_round_of(h: &mut Hive, label: &str, hex8: &str, minutes: i64) -> String {
    let deadline = start() + Duration::minutes(minutes);
    let inner = format!("{label}~{}~{hex8}", deadline.timestamp_millis());
    let id = format!("dg-{hex8}");
    let text = format!("[delegation item_{hex8} from the caller: find out about {label}]");
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": S, "turn_id": inner, "iter": "0", "channel": "test",
               "audience_set": ROUND_E, "delegation_id": id}),
        json!({"session_id": S, "turn_id": inner, "iter": "0", "phase": "",
               "delegation_id": id}),
        json!({"messages": [user(&text)], "system": mode("Be brief.")}),
    );
    id
}

/// Two errands handed on in one turn of the person, then a turn: the calls
/// that left for the model, the hive and the id of the second errand.
fn run_two(knob: Option<&str>) -> (Hive, Vec<Msg>, String) {
    let mut h = hive(knob, &[("policy", "keep_recent", json!(1))]);
    let mut calls = Vec::new();
    let first = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    calls.push(first.call.clone());
    let second = second_round_of(&mut h, "p01", "bbbb2222", 7);
    calls.extend(h.routed("brain"));
    let call = ask(&mut h, start() + Duration::minutes(10), "t3");
    close(&mut h, &call);
    calls.push(call);
    (h, calls, second)
}

/// Review close I1 (wave d58-2 close): the guard of inventory 39 (first gap)
/// -- a park written only while none stands, read back in phase `dlg-park`,
/// and the mark "not followed" for a second errand of the turn -- hangs on
/// the knob as a whole. Unset and "0" run the same curator, write no mark
/// and no conditional park, take no step `dlg-park`, and the second park of
/// the turn is the unconditional set of before: it names the second errand.
///
/// Red on the tree before this fix (fe707cdd6): the mark and the guarded
/// park were written with the knob off.
#[test]
fn two_errands_in_one_turn_off_is_byte_identical_and_parks_as_before() {
    if !shipped() {
        return;
    }
    let (plain, p_calls, p_second) = run_two(None);
    let (off, o_calls, o_second) = run_two(Some("0"));
    assert_eq!(
        p_second, o_second,
        "check of the setup: the same second errand"
    );
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
    for (name, h) in [("unset", &plain), ("off", &off)] {
        assert!(
            h.rows("SELECT seq FROM marks WHERE kind = 'errand_outcome'")
                .is_empty(),
            "{name}: no mark of an outcome: {:?}",
            h.rows("SELECT value FROM marks WHERE kind = 'errand_outcome'")
        );
        let steps: Vec<&String> = h
            .ledger_steps
            .iter()
            .map(|(_, phase)| phase)
            .filter(|phase| *phase == "dlg-park")
            .collect();
        assert!(steps.is_empty(), "{name}: no step `dlg-park`: {steps:?}");
        let guarded: Vec<&Value> = h
            .ledger_ops
            .iter()
            .map(|(_, op)| op)
            .filter(|op| {
                op["table"] == json!("state")
                    && op["row"]["key"] == json!(park_key("p01"))
                    && op.get("if_absent").is_some()
            })
            .collect();
        assert!(
            guarded.is_empty(),
            "{name}: the park is no conditional insert: {guarded:?}"
        );
        assert!(
            h.ledger_ops
                .iter()
                .any(|(_, op)| op["table"] == json!("state")
                    && op["row"]["key"] == json!(park_key("p01"))),
            "check of the setup: {name} parks the turn"
        );
        let row: Value = sj::from_str(&h.state(&park_key("p01"))).unwrap_or(Value::Null);
        assert_eq!(
            row["delegation_id"],
            json!(p_second),
            "{name}: the second park of the turn is set over the first, as before: {row}"
        );
    }
}
