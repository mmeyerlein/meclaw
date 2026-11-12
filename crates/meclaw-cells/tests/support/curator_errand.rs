//! Errands that end without an answer (GH #1469): the rounds the pins
//! `gh_d58_errand_*` hand on, and what they read of the outcome.
//!
//! WHY this file exists: an errand is a round of its own, the turn
//! `<turn>~<deadline_ms>~<hex8>` with a `delegation_id` beside it. When its
//! answering call comes back with an error, or nobody answers before the
//! deadline, the ledger held the `(delegation)` row alone, and a rebuild
//! shrank that row like the scaffold of an errand that was answered (bank
//! find F6). With `errand_outcome` "1" the curator files one row that names
//! the outcome. The pins hand errands on through the lanes of the shipped
//! hive, exactly as a live round arrives, and read three things:
//!
//! * the window of the next call: one message that names the errand by the
//!   text of its `(delegation)` row and says `failed` or `no answer`;
//! * the ledger: one wall row of kind `outcome` under the turn of the errand;
//! * the mark `errand_outcome`, one per `delegation_id`.
//!
//! Time: the hive runs behind the clock of the bank (`curator_bank::CLOCK`).
//! A deadline passes because the pin moves that clock; nothing waits.
#![allow(dead_code)]

use crate::curator_bank::{CLOCK, day};
use crate::curator_hive::*;
use chrono::{DateTime, Duration, Utc};
use meclaw_core::serde_json::{self as sj, Value, json};

/// The session every errand of a pin is handed on in.
pub const S: &str = "s-errand";

/// The knob.
pub const KNOB: &str = "errand_outcome";

/// The two outcomes, as the row says them.
pub const FAILED: &str = "failed";
pub const NO_ANSWER: &str = "no answer";

/// The instant a pin starts at.
pub fn start() -> DateTime<Utc> {
    day(0, 10, 0)
}

/// Whether the shipped policy declares the knob.
pub fn knob_declared() -> bool {
    cell_config("policy")["params"].get(KNOB).is_some()
}

/// The shipped hive behind the clock of the bank, at `start`, with
/// `errand_outcome` set to `knob` (`None`: as shipped) and `over` beside it.
///
/// A knob the policy does not declare cannot be set: the door refuses the
/// override (`gh_d58_errand_a_set_override_of_errand_outcome_passes_the_door`
/// holds the declaration). The run is then the shipped one, and a statement
/// about the outcome fails on the outcome, not on the fixture.
pub fn hive(knob: Option<&str>, over: &[(&str, &str, Value)]) -> Hive {
    let mut all: Vec<(&str, &str, Value)> = over.to_vec();
    if let Some(v) = knob
        && knob_declared()
    {
        all.push(("policy", KNOB, json!(v)));
    }
    let mut h = Hive::with_prelude(&all, CLOCK);
    h.set_clock(start().timestamp_micros());
    h
}

/// An errand handed on.
pub struct Errand {
    /// The turn of the person that handed it on.
    pub label: String,
    /// The turn of its own round, `<label>~<deadline_ms>~<hex8>`.
    pub inner: String,
    /// Its `delegation_id`.
    pub id: String,
    /// The text of its `(delegation)` row: one short line.
    pub text: String,
    pub deadline: DateTime<Utc>,
    /// The call of its round, out for the model.
    pub call: Msg,
}

/// At `at`: the person's turn `label`, the bridge that answers it, and the
/// errand it hands on, due `minutes` later. Nothing answers the errand yet.
pub fn hand_on(h: &mut Hive, at: DateTime<Utc>, label: &str, hex8: &str, minutes: i64) -> Errand {
    hand_on_in(h, at, label, hex8, minutes, None)
}

/// As `hand_on`, the round of the errand carrying the handle `live` of a
/// running session in its hop and context when one is given: it is parked
/// under that handle's scope (`park_key_in`, D58 live).
pub fn hand_on_in(
    h: &mut Hive,
    at: DateTime<Utc>,
    label: &str,
    hex8: &str,
    minutes: i64,
    live: Option<&str>,
) -> Errand {
    h.set_clock(at.timestamp_micros());
    turn(
        h,
        S,
        label,
        &format!("Please find out about {label} for me."),
        &format!("I am passing {label} on."),
        json!({}),
    );
    let deadline = at + Duration::minutes(minutes);
    let inner = format!("{label}~{}~{hex8}", deadline.timestamp_millis());
    let id = format!("dg-{hex8}");
    let text = format!("[delegation item_{hex8} from the caller: find out about {label}]");
    h.out.clear();
    let mut hop = json!({"session_id": S, "turn_id": inner, "iter": "0", "phase": "",
                         "delegation_id": id});
    let mut ctx = json!({"session_id": S, "turn_id": inner, "iter": "0", "channel": "test",
                         "audience_set": ROUND_E, "delegation_id": id});
    if let Some(l) = live {
        hop["live"] = json!(l);
        ctx["live"] = json!(l);
    }
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(&text)], "system": mode("Be brief.")}),
    );
    let calls = h.routed("brain");
    assert_eq!(
        calls.len(),
        1,
        "the round of the errand {label} leaves as one call: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        !h.state(&park_key_in(live.unwrap_or(S), label)).is_empty(),
        "the errand {label} is parked under its turn: {:?}",
        h.stderr
    );
    Errand {
        label: label.to_string(),
        inner,
        id,
        text,
        deadline,
        call: calls[0].clone(),
    }
}

/// The state row an errand of the turn `label` is parked under.
pub fn park_key(label: &str) -> String {
    park_key_in(S, label)
}

/// The state row an errand of the turn `label` is parked under in the scope
/// `scope`: the session, or the handle of a running session (D58 live).
pub fn park_key_in(scope: &str, label: &str) -> String {
    format!("pending:delegation:{scope}#{label}")
}

/// The call of the errand comes back as an error, with `code` in its header.
pub fn fails(h: &mut Hive, at: DateTime<Utc>, e: &Errand, code: &str) {
    h.set_clock(at.timestamp_micros());
    h.tap(&e.call, "error", json!({"error_code": code}), json!([]));
}

/// The call of the errand comes back with its answer.
pub fn answers(h: &mut Hive, at: DateTime<Utc>, e: &Errand, text: &str) {
    h.set_clock(at.timestamp_micros());
    h.tap(&e.call, "stop", json!({}), json!([said(text)]));
}

/// At `at`: the next turn of the session. Returns its call, unanswered.
pub fn ask(h: &mut Hive, at: DateTime<Utc>, turn_id: &str) -> Msg {
    h.set_clock(at.timestamp_micros());
    h.curate(
        S,
        turn_id,
        0,
        json!([user(&format!("And what about {turn_id}?"))]),
        mode("Be brief."),
    )
}

/// The answer that ends the turn of `call`.
pub fn close(h: &mut Hive, call: &Msg) {
    h.tap(call, "stop", json!({}), json!([said("Noted.")]));
}

/// The messages of `call` that say the outcome of `e`: the text of its
/// `(delegation)` row and one of the two outcomes, in no one-line form.
pub fn outcome_lines(call: &Msg, e: &Errand) -> Vec<String> {
    texts(call)
        .into_iter()
        .filter(|t| {
            t.contains(&e.text)
                && (t.contains(FAILED) || t.contains(NO_ANSWER))
                && !t.contains(" [shortened, ")
        })
        .collect()
}

/// The place of the first message of `call` that holds `needle`.
pub fn place(call: &Msg, needle: &str) -> Option<usize> {
    texts(call).iter().position(|t| t.contains(needle))
}

/// The texts of the wall rows of kind `outcome` under the turn of `e`,
/// oldest first.
pub fn outcome_rows(h: &Hive, e: &Errand) -> Vec<String> {
    h.rows(&format!(
        "SELECT b.body FROM wall w JOIN blocks b ON b.hash = w.hash \
         WHERE w.kind = 'outcome' AND w.turn_id = '{}' ORDER BY w.seq",
        e.inner
    ))
    .into_iter()
    .map(|r| {
        let el: Value = sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null);
        el["text"].as_str().unwrap_or("").to_string()
    })
    .collect()
}

/// Every wall row of kind `outcome` the ledger holds.
pub fn all_outcome_rows(h: &Hive) -> usize {
    h.rows("SELECT seq FROM wall WHERE kind = 'outcome'").len()
}

/// The values of the marks `errand_outcome` that name the errand `e`.
pub fn outcome_marks(h: &Hive, e: &Errand) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'errand_outcome' ORDER BY seq")
        .into_iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null))
        .filter(|v: &Value| v["delegation_id"] == json!(e.id))
        .collect()
}

/// The keys the marks `pending_swept` name, over all sweeps.
pub fn swept_keys(h: &Hive) -> Vec<String> {
    h.rows("SELECT value FROM marks WHERE kind = 'pending_swept' ORDER BY seq")
        .into_iter()
        .flat_map(|r| {
            let v: Value = sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null);
            v["keys"].as_array().cloned().unwrap_or_default()
        })
        .map(|k| k.as_str().unwrap_or("").to_string())
        .collect()
}

/// At `at`: a turn of another session whose answer leaves the cache warm,
/// the strike of the order it armed, and every request of the summarizer
/// answered. The probe of that strike sweeps the parked rows past their age.
pub fn strike(h: &mut Hive, at: DateTime<Utc>, turn_id: &str) {
    h.set_clock(at.timestamp_micros());
    turn(
        h,
        "s-beside",
        turn_id,
        &format!("A word beside, {turn_id}."),
        &format!("Heard, {turn_id}."),
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    let order = h
        .clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("the turn {turn_id} arms an order: {:?}", h.stderr));
    h.fire(&order);
    while !h.summ.is_empty() {
        h.answer("What came before, condensed.", "stop");
    }
}
