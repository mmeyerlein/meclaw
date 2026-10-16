//! GH #877, the same class one hive over -- a timed-out query must not leave
//! the access sweep without its watchdog.
//!
//! WHAT WAS BROKEN
//! ===============
//! `access/sweep` has no tick (R-AG-1, GH #1096): it runs after a grant is
//! written and when the one watchdog it armed for the next `expires_at` fires.
//! A refusal of the clock (`timer_op_error`) or of the store (an echo with
//! `error_code`) left on the error lane and armed NOTHING. Under I/O pressure a
//! cell.db write outlasts `query_timeout_ms` and the answer is `query_timeout`
//! (release gate 0.64.3, the `affinity/push` lock beside this file): the
//! watchdog for the next expiry was then never ordered, and a grant that ran
//! out stayed unbooked until some later grant write nudged the sweep again.
//!
//! WHAT IS PINNED HERE
//! ===================
//! * a `query_timeout` from the clock or from any store read or write of the
//!   sweep is still said on the error lane, and orders the watchdog again for
//!   the retry wait (`RETRY_S` of the script) -- ahead, never for now;
//! * a lasting refusal (`schedule_id_exists`, a store error that is not a
//!   timeout) orders nothing: the same order would meet the same refusal.
//!
//! Unit class (`docs/development-rules.md` § 11): one run of the shipped
//! script per case, no topology, no clock of the test's own. The moment is
//! judged against the stamp the SAME script wrote into a run before it.
//!
//! R2b / GH #49: a tree that does not carry the templates skips.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, shipped_script};

const SWEEP: &str = "templates/access/sweep/config.json";
/// The retry wait the sweep orders after a transient refusal (`RETRY_S`).
const RETRY_S: i64 = 60;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> Option<String> {
    let p = repo(SWEEP);
    p.exists()
        .then(|| shipped_script(p.to_str().expect("utf-8")))
}

/// One run of the shipped sweep over `hop`, in phase `phase`.
fn sweep_run(script: &str, hop: Value, phase: &str) -> Vec<Value> {
    emit_all(
        script,
        &json!({
            "target": "/access/sweep",
            "header": {"hop": hop,
                       "context": {"access_origin": "sweep", "ac_phase": phase,
                                   "ac_carry": ""}},
            "ttl": 64,
            "messages": [],
        }),
    )
}

/// The sweep watchdog orders among one run's messages.
fn watchdog_orders(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "clock" && m["schedule_name"] == "access-sweep")
        .collect()
}

/// Epoch milliseconds of an ISO stamp the sweep wrote.
fn ms(stamp: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(stamp)
        .unwrap_or_else(|e| panic!("an ISO stamp: {stamp} ({e})"))
        .timestamp_millis()
}

/// The stamp a plain run (phase `in`) reads its expired grants below: the
/// script's own `now()`, taken BEFORE the refusal run that follows it.
fn stamp_of_a_run_before(script: &str) -> i64 {
    let out = sweep_run(script, json!({"schedule_name": "access-sweep"}), "");
    let sel = out
        .iter()
        .find(|m| m["header"]["route"] == "astore")
        .unwrap_or_else(|| panic!("a plain run reads the grants: {out:?}"));
    let args: Value =
        meclaw_core::serde_json::from_str(sel["messages"][0]["text"].as_str().unwrap_or_default())
            .unwrap_or_else(|e| panic!("the read is a tool call: {sel} ({e})"));
    ms(args["where"]["expires_at"]["lt"]
        .as_str()
        .unwrap_or_default())
}

#[test]
fn a_timed_out_query_orders_the_sweep_again() {
    let Some(script) = shipped() else { return };
    let cases = [
        (
            json!({"msg_type": "timer_op_error", "error_code": "query_timeout"}),
            "",
        ),
        (
            json!({"route": "astore", "operation": "select", "error_code": "query_timeout"}),
            "next",
        ),
        (
            json!({"route": "astore", "operation": "insert", "error_code": "query_timeout"}),
            "written",
        ),
    ];
    for (hop, phase) in cases {
        let before = stamp_of_a_run_before(&script);
        let out = sweep_run(&script, hop.clone(), phase);
        assert!(
            out.iter().any(|m| m["header"]["route"] == "error"),
            "the refusal is still said on the error lane: {hop} -> {out:?}"
        );
        let orders = watchdog_orders(&out);
        assert_eq!(
            orders.len(),
            1,
            "a transient refusal orders the sweep watchdog again: {hop} -> {out:?}"
        );
        let order = orders[0];
        assert_eq!(order["op"], "add", "{order}");
        assert_eq!(order["rearm"], true, "{order}");
        assert_eq!(order["emit_to"], "../sweep", "{order}");
        let at = ms(order["at"].as_str().unwrap_or_default());
        assert!(
            at >= before + RETRY_S * 1000,
            "the order waits the retry wait -- an order for now would spin on an \
             overloaded cell: {order} (a run before stamped {before} ms)"
        );
    }
}

#[test]
fn a_lasting_refusal_orders_nothing() {
    let Some(script) = shipped() else { return };
    let cases = [
        (
            json!({"msg_type": "timer_op_error", "error_code": "schedule_id_exists"}),
            "",
        ),
        (
            json!({"route": "astore", "operation": "select", "error_code": "no_such_table"}),
            "next",
        ),
    ];
    for (hop, phase) in cases {
        let out = sweep_run(&script, hop.clone(), phase);
        assert!(
            out.iter().any(|m| m["header"]["route"] == "error"),
            "{hop} -> {out:?}"
        );
        assert!(
            watchdog_orders(&out).is_empty(),
            "a lasting refusal must not re-arm: {hop} -> {out:?}"
        );
    }
}
