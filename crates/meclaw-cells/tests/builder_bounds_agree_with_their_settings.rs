//! A CEL condition cannot read `params`, so the loop's two bounds are written
//! twice: as a literal on the edge that enforces them, and as a settings
//! default in the cell that documents them. Two places, one number — or the
//! knob and the guard drift apart silently, which is the failure mode this file
//! exists to make loud.

use meclaw_core::serde_json::Value;
use std::path::PathBuf;

fn json_at(rel: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel);
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).expect(rel)).expect("parses")
}

fn weave() -> Value {
    json_at("templates/builder/weave/config.json")
}

fn edges() -> Vec<Value> {
    json_at("templates/builder/config.json")["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

/// The integer a condition compares `int(<src>.<key>)` against.
///
/// RECALIBRATED with the repair lane: the compartment is a parameter now,
/// because the two counters do not travel the same way. The round counter is on
/// the loop's own chain and lives in `context`; the repair counter arrives on the
/// submitter's foreign chain, which carries none of it, so `weave` counts its own
/// receipt rows and hands the total back as `hop.repairs`.
///
/// The trailing space in the needle is load-bearing: it is what tells `< ` from
/// `<= `.
fn bound_in_condition(src: &str, key: &str, op: &str) -> Option<u64> {
    let needle = format!("int({src}.{key}) {op} ");
    for e in edges() {
        let c = e["condition"].as_str().unwrap_or("").to_string();
        if let Some(i) = c.find(&needle) {
            let rest = &c[i + needle.len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty() {
                return digits.parse().ok();
            }
        }
    }
    None
}

#[test]
fn the_iteration_bound_on_the_edge_is_the_max_iter_default() {
    let setting = weave()["contract"]["settings"]["max_iter"]["default"]
        .as_u64()
        .expect("max_iter declared");
    let param = weave()["params"]["max_iter"]
        .as_u64()
        .expect("max_iter in params");
    assert_eq!(setting, param, "the shipped default is one number, not two");
    assert_eq!(
        bound_in_condition("context", "iter", "<"),
        Some(setting),
        "the re-entry edge enforces exactly what the setting promises"
    );
    assert_eq!(
        bound_in_condition("context", "iter", ">="),
        Some(setting),
        "the capped lane is the exact negation, or a round parks with its fire \
         guard already spent and nothing left to emit"
    );
}

#[test]
fn the_repair_bound_on_the_edge_is_the_max_repairs_default() {
    let setting = weave()["contract"]["settings"]["max_repairs"]["default"]
        .as_u64()
        .expect("max_repairs declared");
    assert_eq!(weave()["params"]["max_repairs"].as_u64(), Some(setting));
    // RECALIBRATED: `<=`, not `<`, and on the hop rather than on the context.
    // `weave` goes to `give_up` at `done > max_repairs`, so the edge has to let
    // the LAST permitted repair through -- a `<` here would swallow it silently,
    // which is the exact failure this pair of numbers exists to prevent.
    assert_eq!(bound_in_condition("hop", "repairs", "<="), Some(setting));
}

#[test]
fn the_idle_window_is_declared_even_though_no_edge_reads_it() {
    let setting = weave()["contract"]["settings"]["round_idle_ms"]["default"]
        .as_u64()
        .expect("round_idle_ms declared");
    assert_eq!(weave()["params"]["round_idle_ms"].as_u64(), Some(setting));
    assert!(
        setting > 0,
        "an idle window of zero closes every round on arrival"
    );
}
