//! Which door a build's answer comes back through is `context.build_caller`,
//! and the key did not survive a broken message chain.
//!
//! `meclaw-os` stamps it once, at the rim: an operator-initiated build carries
//! `build_caller: 'operator'` (plus `build_auto_submit: 'yes'` when the caller
//! asked for one act), and the four edges out of `./builder` read both back to
//! decide between the front door and `./orgs`. Measured on a throwaway colony
//! (GH #480), `bc` = `context.build_caller`:
//!
//! ```text
//! builder/dispatcher -> builder/eyes  route=tool      bc='operator'
//! builder/eyes     -> builder/weave   route=eye_out   bc=None      <-- lost
//! builder/weave    -> builder/compose route=fire      bc=None
//! builder          -> /os/orgs        route=in_build_result        <-- wrong door
//! ```
//!
//! A `/colony` read starts a fresh trace, so `./eyes -> ./weave` restores off
//! the echoed tag exactly what the tag carries — `build_id`, `iter`, `repairs`.
//! The same happens to a refusal: `in_receipt` arrives on a foreign chain by
//! construction, and `context` is empty there whatever the loop counted.
//!
//! GH #460 built the machine for precisely this — the caller is written into
//! the round table on route `calls`, the one leg always on the build's own
//! chain, and read back whenever a round is decided. Two things kept it from
//! helping:
//!
//! * `build_caller` was not one of `CALLER_KEYS`, and neither was
//!   `build_auto_submit` — the same rim modifier sets both and the same guards
//!   read both, so half a repair here is the other half measured later; and
//! * the row was parked only `if caller["build_call_id"]`, and an operator-driven
//!   build has none — there is no agent tool call behind it. The one build that
//!   most needed the row was the one build that never wrote it.
//!
//! Severity is the draft lane, not the error lane: an operator build that
//! consults the graph and then SUCCEEDS delivered its manifest into an
//! organisation, and in a fresh colony `./orgs` is empty by construction — a
//! silent dead letter, which is the failure GH #469 was opened to remove.
//!
//! Nothing here runs a model. Two producers are covered because two were
//! measured: the eye that wins a round, and the refusal that comes back.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const WEAVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/weave/config.json"
);
const HIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/config.json"
);
const SHELL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/meclaw-os/config.json"
);

const BUILD: &str = "b480";

/// The two keys the shell stamps at the rim and reads back at the door. They
/// are named here once and checked against both halves below.
const DOOR_KEYS: [&str; 2] = ["build_caller", "build_auto_submit"];

fn run_weave(hop: Value, ctx: Value, body: Value) -> Vec<Value> {
    let mut flat = json!({"header": {"hop": hop, "context": ctx}, "params": {}});
    if let Value::Object(slots) = body {
        for (slot, v) in slots {
            flat[slot] = v;
        }
    }
    emit_all(&shipped_script(WEAVE), &flat)
}

fn config(path: &str) -> Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn edges(path: &str) -> Vec<Value> {
    config(path)["params"]["graph"]["edges"]
        .as_array()
        .expect("the graph declares edges")
        .clone()
}

/// The context an operator-driven build carries while its chain is intact: the
/// door, the disposition, and NO `build_call_id` — there is no agent tool call
/// behind an operator.
fn operator_context() -> Value {
    json!({"build_id": BUILD, "iter": "0", "repairs": "0",
           "build_op": "draft", "build_scope": "/os",
           "build_caller": "operator", "build_auto_submit": "yes"})
}

/// The caller row as `weave` ACTUALLY parks it, run rather than spelled: the
/// cell that writes the row is the authority on its shape, and a hand-written
/// fixture would only ever prove itself.
fn parked_caller_row(at: &str) -> Value {
    let out = run_weave(
        json!({"route": "calls"}),
        operator_context(),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "c-1",
                             "text": "{}"}]}),
    );
    let bundle = out
        .iter()
        .find(|m| m["header"]["route"] == "cstore")
        .cloned()
        .expect("the composer's round is parked");
    let leg = bundle["messages"]
        .as_array()
        .expect("bundle legs")
        .iter()
        .find(|o| o["id"] == "w-caller-row")
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "an operator-driven build parks no caller row at all -- the row \
                 is written only when a `build_call_id` is named, and an \
                 operator has none: {bundle}"
            )
        });
    let insert: Value =
        meclaw_core::serde_json::from_str(leg["text"].as_str().unwrap_or("{}")).expect("op json");
    let mut row = insert["row"].clone();
    row["recorded_at"] = json!(at);
    row
}

fn row(iter: i64, role: &str, turn: &str, fired: i64, at: &str) -> Value {
    json!({"build_id": BUILD, "iter": iter, "role": role, "turn": turn,
           "fired": fired, "recorded_at": at})
}

fn slate(rows: &[Value]) -> Value {
    json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "w-round-read",
                         "text": meclaw_core::serde_json::to_string(rows).expect("rows")}]})
}

fn closed_round(iter: i64, call: &str) -> Vec<Value> {
    vec![
        row(
            iter,
            "assistant",
            &format!("[{{\"origin\":\"assistant\",\"type\":\"tool_call\",\"id\":\"{call}\"}}]"),
            0,
            "2999-01-01T10:00:00.000000Z",
        ),
        row(
            iter,
            "tool",
            &format!("{{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"{call}\"}}"),
            0,
            "2999-01-01T10:00:01.000000Z",
        ),
    ]
}

/// The context a leg leaves behind when its chain was broken: an eye's reply
/// and a submitter's receipt both arrive with the loop's own coordinates at
/// best, and never with the door.
fn broken_chain_context() -> Value {
    json!({"build_id": BUILD, "iter": "0", "repairs": "0", "store_origin": "weave"})
}

fn assert_carries_the_door(emission: &Value, what: &str) {
    for key in DOOR_KEYS {
        let got = emission["header"][key].as_str().unwrap_or("");
        assert!(
            !got.is_empty(),
            "{what}: hop.{key} is absent or empty, so the edge modifier that \
             lifts it back into context has nothing to lift -- the answer goes \
             to whichever door the missing key defaults to: {}",
            emission["header"]
        );
    }
    assert_eq!(
        emission["header"]["build_caller"], "operator",
        "{what}: the door is not the one the rim stamped"
    );
    assert_eq!(
        emission["header"]["build_auto_submit"], "yes",
        "{what}: the disposition is not the one the caller asked for -- an \
         auto-submit build reaching the operator on `in_draft` is a build \
         waiting for an approval nobody asked for"
    );
}

#[test]
fn the_round_table_writes_down_a_caller_that_has_no_tool_call_id() {
    // The seam. An operator-driven build is exactly the build with no
    // `build_call_id`, and the park guard tested for that one key.
    let parked = parked_caller_row("2999-01-01T09:59:59.000000Z");
    let said: Value = meclaw_core::serde_json::from_str(parked["turn"].as_str().unwrap_or("null"))
        .expect("the parked note is json");
    assert_eq!(said["build_caller"], "operator");
    assert_eq!(said["build_auto_submit"], "yes");
    assert_eq!(
        parked["build_id"], BUILD,
        "and it is filed under the build it belongs to"
    );
}

#[test]
fn an_eye_that_won_the_round_hands_the_door_back() {
    let mut rows = vec![parked_caller_row("2999-01-01T09:59:59.000000Z")];
    rows.extend(closed_round(0, "c-1"));

    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        broken_chain_context(),
        slate(&rows),
    );
    let fired = out
        .iter()
        .find(|m| m["header"]["route"] == "fire")
        .unwrap_or_else(|| panic!("the closed round re-enters the composer: {out:?}"));

    assert_carries_the_door(fired, "the round an eye won");
}

#[test]
fn a_refusal_on_a_foreign_chain_finds_the_door_again() {
    // The second measured producer, and the one that needs no model at all: a
    // refused submission comes back on `in_receipt`, whose chain is foreign by
    // construction.
    let receipt = "{\"origin\":\"user\",\"type\":\"text\",\"text\":\"the submission \
                   was refused: template_missing\"}";

    let mut rows = vec![parked_caller_row("2999-01-01T09:59:59.000000Z")];
    rows.extend(closed_round(0, "c-1"));
    rows.push(row(0, "receipt", receipt, 0, "2999-01-01T10:00:09.000000Z"));

    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        broken_chain_context(),
        slate(&rows),
    );
    let repair = out
        .iter()
        .find(|m| m["header"]["route"] == "repair")
        .unwrap_or_else(|| panic!("the refusal goes back to the composer: {out:?}"));
    assert_carries_the_door(repair, "the repair a refusal opened");

    // And the exit the issue measured: the budget is spent, the build stops on
    // the error lane, and THAT is the message the operator has to receive.
    let mut spent = vec![parked_caller_row("2999-01-01T09:59:59.000000Z")];
    spent.extend(closed_round(0, "c-1"));
    for (n, at) in [
        "2999-01-01T10:00:09.000000Z",
        "2999-01-01T10:00:10.000000Z",
        "2999-01-01T10:00:11.000000Z",
    ]
    .iter()
    .enumerate()
    {
        spent.push(row(n as i64, "receipt", receipt, 0, at));
    }
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        broken_chain_context(),
        slate(&spent),
    );
    let give_up = out
        .iter()
        .find(|m| m["header"]["route"] == "give_up")
        .unwrap_or_else(|| panic!("above the budget the build stops, named: {out:?}"));
    assert_carries_the_door(give_up, "the named stop");
}

#[test]
fn every_edge_out_of_the_fan_in_lifts_the_door_back_into_context() {
    // A hop key nothing lifts is a hop key nobody reads: the guards at the
    // shell are written against `context`, so the row and the header only carry
    // half the way.
    let mut checked = 0;
    for e in edges(HIVE) {
        if e["from"] != "./weave" {
            continue;
        }
        let set = &e["modifier"]["set_context"];
        if set["build_call_id"].is_null() {
            continue; // not one of the edges that restores the caller at all
        }
        checked += 1;
        for key in DOOR_KEYS {
            assert_eq!(
                set[key],
                json!(format!("hop.{key}")),
                "the edge ./weave -> {} restores the caller but not {key}, so \
                 the value the round table recovered stops at the hop: {e}",
                e["to"]
            );
        }
    }
    assert!(
        checked >= 4,
        "expected the four edges out of ./weave that restore the caller \
         (fire, draft, repair, give_up), found {checked}"
    );
}

#[test]
fn the_shell_reads_the_names_the_builder_restores() {
    // Two halves of one decision, one file apart. A key renamed on either side
    // is a build answering at the wrong door, and nothing is red.
    let guards = meclaw_core::serde_json::to_string(&edges(SHELL)).expect("shell edges");
    for key in DOOR_KEYS {
        assert!(
            guards.contains(&format!("context.{key}")),
            "the shell no longer guards on context.{key}, but the builder still \
             carries it -- one of the two moved and the other did not"
        );
    }

    let hive = config(HIVE);
    let declared = meclaw_core::serde_json::to_string(&hive).expect("hive config");
    for key in DOOR_KEYS {
        assert!(
            declared.contains(key),
            "the builder hive names {key} nowhere, so nothing inside it can \
             carry the shell's decision across a broken chain"
        );
    }
}
