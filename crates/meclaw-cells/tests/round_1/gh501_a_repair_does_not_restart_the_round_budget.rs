//! GH #501 — a repair re-entered the round loop at zero.
//!
//! The design loop is bounded by `max_iter` (6, shipped): `./weave -> ./compose`
//! on route `fire` fires only while `int(context.iter) < 6`, and the counter
//! lives in `context` because it travels on the loop's own chain.
//!
//! A refusal does not travel on that chain. `templates/submit/gate` renders a
//! receipt off its own remembered row, so `in_receipt` arrives with an empty
//! context — which `weave` already knows about the *repair* budget and answers
//! by counting its own receipt rows. The *round* counter got no such treatment:
//! it is read once, at the top, as `int(ctx.get("iter", 0) or 0)`, and on the
//! receipt road that is `0`. Every emission is stamped with it, the repair edge
//! restores `context.iter` from `hop.iter`, and the composer re-enters at round
//! zero with a full budget.
//!
//! A build that spent all six rounds, drafted, and was refused at the door could
//! therefore spend six more — twice — on a wish whose budget the briefing had
//! already named as six (`brief`, BUDGET; held to one number by
//! `builder_bounds_agree_with_their_settings`). A budget a build can re-enter is
//! not a budget.
//!
//! The repair is the one the repair counter already got: the round table carries
//! an `iter` column on every row of the build, and the read-back has the whole
//! slate in hand. The highest `iter` on it is the round the build reached. It is
//! read there and only where the context arrived without one — a round decided
//! on the loop's own chain keeps reading `context.iter`, which is the counter
//! that is actually correct there.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const WEAVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/weave/config.json"
);

const SHA: &str = "9c1f0b7e5a3d2846c0e2f4a6b8d0123456789abcdef0123456789abcdef01234";
const BUILD: &str = "b501";

fn row(build: &str, iter: i64, role: &str, turn: &str, at: &str) -> Value {
    json!({"build_id": build, "iter": iter, "role": role, "turn": turn,
           "fired": 0, "recorded_at": at})
}

/// One closed round of the design loop, as the loop parks it.
fn closed_round(iter: i64) -> Vec<Value> {
    let call = format!("c{iter}");
    vec![
        row(
            BUILD,
            iter,
            "assistant",
            &format!("[{{\"origin\":\"assistant\",\"type\":\"tool_call\",\"id\":\"{call}\"}}]"),
            &format!("2999-01-01T09:0{iter}:00.000000Z"),
        ),
        row(
            BUILD,
            iter,
            "tool",
            &format!("{{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"{call}\"}}"),
            &format!("2999-01-01T09:0{iter}:01.000000Z"),
        ),
    ]
}

fn caller_row() -> Value {
    let said = json!({"build_call_id": "tc-1", "agent": "/os/orgs/a/m", "build_op": "draft",
                      "build_scope": "/os", "build_caller": "", "build_auto_submit": ""});
    row(
        BUILD,
        0,
        "caller",
        &meclaw_core::serde_json::to_string(&said).expect("note"),
        "2999-01-01T08:59:00.000000Z",
    )
}

/// The receipt row as the `in_receipt` lane writes it: `iter` is whatever the
/// empty context said, which is zero.
fn receipt_row() -> Value {
    let turn = json!({"origin": "user", "type": "text",
                      "text": "the submission was refused: template_missing"});
    row(
        BUILD,
        0,
        "receipt",
        &meclaw_core::serde_json::to_string(&turn).expect("turn"),
        "2999-01-01T09:30:00.000000Z",
    )
}

fn read_back(rows: Vec<Value>, ctx: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(WEAVE),
        &json!({
            "header": {"hop": {"route": "cstore"}, "context": ctx},
            "params": {},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "w-round-read",
                          "text": meclaw_core::serde_json::to_string(&rows).expect("rows")}],
        }),
    )
}

fn decided(out: &[Value]) -> Value {
    out.iter()
        .find(|m| m["header"]["route"] != "cstore")
        .cloned()
        .unwrap_or_else(|| panic!("the read-back decided nothing: {out:?}"))
}

/// A build that spent rounds 0..=4 and was refused at the door.
fn a_spent_build() -> Vec<Value> {
    let mut rows = vec![caller_row()];
    for iter in 0..=4 {
        rows.extend(closed_round(iter));
    }
    rows.push(row(
        BUILD,
        5,
        "manifest",
        SHA,
        "2999-01-01T09:20:00.000000Z",
    ));
    rows.push(receipt_row());
    rows
}

#[test]
fn a_repair_re_enters_at_the_round_the_build_reached() {
    let out = read_back(a_spent_build(), json!({"store_origin": "weave"}));
    let answer = decided(&out);
    assert_eq!(answer["header"]["route"], "repair");
    assert_eq!(
        answer["header"]["iter"], "5",
        "the repair re-enters at round zero, so the edge that caps the loop at \
         six lets six more rounds through -- on a wish whose briefing named six: \
         {}",
        answer["header"]
    );
}

#[test]
fn the_round_counter_is_still_read_off_a_chain_that_carried_one() {
    // The non-regression: on the loop's own chain the context IS the counter,
    // and it is the one that is right -- the slate can hold rows of a round that
    // has not been decided yet.
    let mut rows = vec![caller_row()];
    rows.extend(closed_round(0));
    let out = read_back(
        rows,
        json!({"build_id": BUILD, "iter": "0", "repairs": "0"}),
    );
    let answer = decided(&out);
    assert_eq!(answer["header"]["route"], "fire");
    assert_eq!(
        answer["header"]["iter"], "0",
        "the round that was just decided is round zero, and `iter` is the \
         COORDINATE of that round: {}",
        answer["header"]
    );
    assert_eq!(answer["header"]["rounds_done"], "1");
}
