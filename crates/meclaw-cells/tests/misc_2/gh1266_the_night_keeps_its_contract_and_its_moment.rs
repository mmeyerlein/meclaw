//! GH #1266 -- the memory hive's night keeps its own contract and its moment.
//!
//! Two defects, one pin each, both on the shipped scripts (no colony boots):
//!
//! - `./dream-glue`, once the store has booked the run (phase `closed`), sends
//!   the night's entity consolidation (`entity`, GH #1057) and the recurrence
//!   pass (`recur`, GH #1249) in ONE emission. The `code` cell checks an
//!   emission against the cell's own `contract.emits` all or nothing, and the
//!   contract asks every message for the run's `dream_run` and `dream_to`. An
//!   `entity` header without them turned both messages into one
//!   `contract_violation` reply to the store, which no edge carries: every
//!   booked night ended in a dead letter, and neither pass ever ran.
//! - `./writer` orders the night asked for now (`clock_op`) under an id of its
//!   own. Under the id every written episode re-arms with `rearm: true`, an
//!   episode written before that order fired moved the night to the next
//!   `night_at_utc`, and the night asked for never ran.
//!
//! The colony half of the second pin -- a trigger and an episode in one
//! breath, and the night still runs -- stands beside the other clock tests in
//! `gh1019_a_schedule_fires_now_by_name.rs`.

use meclaw_core::serde_json::{Value, json};

const GLUE: &str = "../../templates/memory-hive/dream-glue/config.json";
const WRITER: &str = "../../templates/memory-hive/writer/config.json";
const HIVE: &str = "../../templates/memory-hive/config.json";
const NIGHT: &str = "2026-10-18T03:00:00Z";

/// Run a shipped code cell over one flat stdin document (`header`, the body
/// slots beside it) -> the messages it emits.
fn run(config: &str, doc: Value) -> Vec<Value> {
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(config),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "{config}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).expect("a message array")
}

/// Every emission judged by the cell's OWN declared `emits`, compiled and
/// applied the way the `code` cell applies it before anything leaves.
fn assert_within_contract(config: &str, out: &[Value]) {
    let cfg: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(config).expect("config"))
            .expect("config json");
    let emits: meclaw_core::EmitsBlock =
        meclaw_core::serde_json::from_value(cfg["contract"]["emits"].clone()).expect("emits");
    let compiled = meclaw_core::CompiledEmits::compile(&emits).expect("compile emits");
    assert!(!out.is_empty(), "{config}: nothing was emitted at all");
    for m in out {
        meclaw_core::validate_emits(m, &compiled).unwrap_or_else(|e| {
            panic!("{config}: the cell's own contract refuses what it emits: {e} -- {m}")
        });
    }
}

/// The one order the writer hands its clock (`./writer -> ./clock`).
fn clock_order(out: &[Value]) -> Value {
    let orders: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "clock")
        .collect();
    assert_eq!(orders.len(), 1, "one clock order: {out:?}");
    orders[0].clone()
}

fn trigger() -> Value {
    json!({"header": {"context": {"mem_phase": "clock_op"}, "hop": {"route": "clock_op"}},
           "messages": [], "op": "trigger", "schedule_name": "nightly-dream"})
}

fn episode() -> Value {
    json!({"header": {"context": {"mem_phase": "episode", "session_id": "s-1266",
                                  "turn_id": "t-1", "channel": "c-1266",
                                  "audience_set": r#"["member:e"]"#, "speaker": "member:e"},
                      "hop": {"route": "in_episode"}},
           "messages": [{"origin": "user", "type": "text", "text": "The bakery opens at six."}]})
}

#[test]
fn the_booked_night_sends_entity_and_recur_within_its_own_contract() {
    let booked = run(
        GLUE,
        json!({"header": {"context": {"store_origin": "dream", "mem_phase": "closed",
                                      "dream_run": "r-1266", "dream_to": NIGHT},
                          "hop": {"operation": "update", "rows_affected": 1}},
               "messages": [{"origin": "tool", "type": "tool_result", "id": "d-closed",
                             "text": "[]"}]}),
    );
    let routes: Vec<&str> = booked
        .iter()
        .filter_map(|m| m["header"]["route"].as_str())
        .collect();
    assert_eq!(routes, ["entity", "recur"], "{booked:?}");
    assert_within_contract(GLUE, &booked);
    for m in &booked {
        assert_eq!(
            m["header"]["dream_run"], "r-1266",
            "the run's id rides: {m}"
        );
        assert_eq!(
            m["header"]["dream_to"], NIGHT,
            "the run's window end rides: {m}"
        );
    }
}

#[test]
fn the_night_asked_for_now_stands_under_an_id_no_episode_rearms() {
    let now = run(WRITER, trigger());
    let written = run(WRITER, episode());
    assert_within_contract(WRITER, &now);
    let (now, armed) = (clock_order(&now), clock_order(&written));
    assert_ne!(
        now["schedule_id"], armed["schedule_id"],
        "an episode's re-arm replaces the row of its own id: the night asked for \
         now must not share it ({now} / {armed})"
    );
    for order in [&now, &armed] {
        assert_eq!(order["op"], "add", "{order}");
        assert_eq!(order["rearm"], true, "{order}");
        assert_eq!(order["schedule_name"], "nightly-dream", "{order}");
        assert_eq!(order["emit_to"], "../dream-glue", "{order}");
    }
    // One standing order for now: a second trigger replaces the first.
    assert_eq!(
        clock_order(&run(WRITER, trigger()))["schedule_id"],
        now["schedule_id"]
    );

    // The clock's strike reaches the night by its NAME, so the new id needs no edge.
    let hive: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(HIVE).expect("the hive config"))
            .expect("hive json");
    let edges = hive["params"]["graph"]["edges"].as_array().expect("edges");
    let strike = edges
        .iter()
        .find(|e| e["from"] == "./clock" && e["to"] == "./dream-glue")
        .expect("an edge from the clock to the night");
    let cond = strike["condition"].as_str().expect("a condition");
    assert!(
        cond.contains("hop.schedule_name == 'nightly-dream'") && !cond.contains("schedule_id"),
        "{cond}"
    );
}
