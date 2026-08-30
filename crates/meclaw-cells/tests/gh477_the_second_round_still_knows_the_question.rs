//! Round 0 of a build is briefed, and every round after it used to be briefed
//! with nothing.
//!
//! Measured on a throwaway colony against a real model (GH #477): the wish went
//! in on `in_build`, `./brief -> ./compose` carried `system.instructions` and
//! the request as a `user` turn, and the model opened with two
//! `catalogue_lookup` calls -- exactly the first move the briefing asks for.
//! The fan-in closed the round, `./weave -> ./compose` fired, and the body it
//! fired with was
//!
//! ```text
//! json.loads(body_payload).keys() == ["messages"]
//! ```
//!
//! two `tool_call` turns and two `tool_result` turns. No `user` turn, no
//! `system` tree. The model answered `{"question": "... I don't see an actual
//! request ..."}`, `normalise` named it `wish_incomplete`, and the build was
//! over without anything having gone wrong.
//!
//! Two holes, one round apart:
//!
//! * `rebuild()` reads the thread out of the `thread` table alone and sorts it
//!   with `{"user": 0, "assistant": 1}` -- the role is provided for in the sort
//!   key and was never written, because no edge carried the brief's prompt into
//!   the round table.
//! * The `system` tree travelled one hop and then existed nowhere. From round 1
//!   on the composer had no GRAMMAR, no ENDPOINTS rule and no scope line: the
//!   four blocks the briefing exists for.
//!
//! Why no case was red: every scenario case of the design lane drives a STUB
//! model (`workshop/evals/builder-scenarios/answers/*.json`) that answers by
//! position and never reads its prompt. A stub cannot see a missing user turn.
//! So this file asserts the PROMPT and never an answer -- it runs the two
//! shipped scripts off the tree and reads what `./weave` would hand `./compose`.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const BRIEF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/brief/config.json"
);
const WEAVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/weave/config.json"
);

const BUILD: &str = "b477";
const REQUEST: &str = "a feed cell under the researcher that fetches three RSS \
                       feeds every ten minutes and emits one headline document \
                       per new item";
const SCOPE: &str = "/os/orgs/newsroom/members/researcher";

/// Everything the brief emits for one wish, over the shipped script.
fn brief_emissions() -> Vec<Value> {
    emit_all(
        &shipped_script(BRIEF),
        &json!({
            "target": "/os/builder/brief",
            "header": {"hop": {"route": "brief", "stage": "briefed", "hits": 0},
                       "context": {"build_id": BUILD, "build_scope": SCOPE}},
            "ttl": 64,
            "messages": [{"origin": "user", "type": "text", "id": "",
                          "text": REQUEST}],
        }),
    )
}

fn emission_on(route: &str) -> Value {
    brief_emissions()
        .into_iter()
        .find(|m| m["header"]["route"] == route)
        .unwrap_or_else(|| {
            panic!(
                "the brief emits nothing on route {route:?} -- it emitted {:?}",
                brief_emissions()
                    .iter()
                    .map(|m| m["header"]["route"].clone())
                    .collect::<Vec<_>>()
            )
        })
}

/// The rows the brief ACTUALLY parks, read back out of its own store bundle and
/// dressed as the slate the transcript answers with. Run, never spelled: the
/// cell that writes the row is the authority on its shape, and a hand-written
/// fixture here would only ever prove itself.
fn parked_rows() -> Vec<Value> {
    let bundle = emission_on("cstore");
    bundle["messages"]
        .as_array()
        .expect("the bundle legs")
        .iter()
        .map(|leg| {
            let op: Value = meclaw_core::serde_json::from_str(leg["text"].as_str().unwrap_or("{}"))
                .expect("op json");
            assert_eq!(
                op["operation"], "insert",
                "the brief parks rows and reads nothing back: {op}"
            );
            assert_eq!(op["table"], "thread", "into the round table: {op}");
            op["row"].clone()
        })
        .collect()
}

fn row(iter: i64, role: &str, turn: &str, fired: i64, at: &str) -> Value {
    json!({"build_id": BUILD, "iter": iter, "role": role, "turn": turn,
           "fired": fired, "recorded_at": at})
}

fn slate(rows: &[Value]) -> Value {
    json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "w-round-read",
                         "text": meclaw_core::serde_json::to_string(rows).expect("rows")}]})
}

fn run_weave(hop: Value, ctx: Value, body: Value) -> Vec<Value> {
    let mut flat = json!({"header": {"hop": hop, "context": ctx}, "params": {}});
    if let Value::Object(slots) = body {
        for (slot, v) in slots {
            flat[slot] = v;
        }
    }
    emit_all(&shipped_script(WEAVE), &flat)
}

/// One closed tool round: the assistant turn that opened it and the result that
/// answered it.
fn closed_round(iter: i64, call: &str) -> Vec<Value> {
    vec![
        row(
            iter,
            "assistant",
            &format!(
                "[{{\"origin\":\"assistant\",\"type\":\"tool_call\",\"id\":\"{call}\",\
                 \"text\":\"{{}}\"}}]"
            ),
            0,
            "2999-01-01T10:00:00.000000Z",
        ),
        row(
            iter,
            "tool",
            &format!(
                "{{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"{call}\",\
                 \"text\":\"a catalogue row\"}}"
            ),
            0,
            "2999-01-01T10:00:01.000000Z",
        ),
    ]
}

/// The half of the assertion that is the whole point: what the composer is
/// asked with in round N is what it was asked with in round 0, plus the round.
fn assert_still_briefed(fired: &Value, what: &str) {
    let thread = fired["messages"].as_array().expect("a rebuilt thread");

    let asked = thread
        .iter()
        .find(|t| t["origin"] == "user" && t["type"] == "text")
        .unwrap_or_else(|| {
            panic!(
                "{what}: the thread carries no user turn -- the request the \
                 build was asked for exists nowhere in it: {thread:?}"
            )
        });
    assert_eq!(
        asked["text"].as_str().unwrap_or(""),
        REQUEST,
        "{what}: the user turn is not the question that was asked"
    );

    let first = &thread[0];
    assert_eq!(
        first["origin"], "user",
        "{what}: the question is not the FIRST turn of the thread -- \
         `rebuild` sorts `user` ahead of `assistant` for a reason: {thread:?}"
    );

    let instructions = fired["system"]["instructions"]["text"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "{what}: the emission carries no system.instructions -- from \
                 this round on the composer has no GRAMMAR, no ENDPOINTS rule \
                 and no scope line: {:?}",
                fired["system"]
            )
        });
    for block in [
        "GRAMMAR --",
        "ENDPOINTS --",
        "CONNECTIVITY --",
        "REQUIREMENTS --",
    ] {
        assert!(
            instructions.contains(block),
            "{what}: the instructions lost the {block:?} block"
        );
    }
    assert!(
        instructions.contains(SCOPE),
        "{what}: the instructions lost the scope line naming {SCOPE}"
    );
    assert!(
        fired["system"]["tools"]["catalogue_lookup"]["text"].is_string(),
        "{what}: the tool schemas do not travel either, so the model is asked \
         to call tools it can no longer see: {:?}",
        fired["system"]["tools"]
    );
}

#[test]
fn the_brief_parks_the_question_and_its_instructions() {
    // The seam. Whatever the brief writes into the round table is verbatim what
    // `weave` hands the composer a round later, so the row has to be right
    // where it is minted.
    let rows = parked_rows();

    let asked = rows
        .iter()
        .find(|r| r["role"] == "user")
        .unwrap_or_else(|| panic!("the brief parks no `user` row: {rows:?}"));
    assert_eq!(
        asked["build_id"], BUILD,
        "the row names the build it belongs to"
    );
    let turn: Value = meclaw_core::serde_json::from_str(asked["turn"].as_str().unwrap_or("null"))
        .expect("the parked turn is json");
    assert_eq!(turn["origin"], "user");
    assert_eq!(turn["type"], "text");
    assert_eq!(turn["text"].as_str().unwrap_or(""), REQUEST);

    let briefed = rows
        .iter()
        .find(|r| r["role"] == "system")
        .unwrap_or_else(|| panic!("the brief parks no `system` row: {rows:?}"));
    assert_eq!(briefed["build_id"], BUILD);
    let tree: Value = meclaw_core::serde_json::from_str(briefed["turn"].as_str().unwrap_or("null"))
        .expect("the parked system tree is json");
    assert_eq!(
        tree,
        emission_on("compose")["system"],
        "the parked tree is the SAME tree round 0 was briefed with -- two \
         spellings of one prompt would drift the first time one of them moved"
    );
}

#[test]
fn the_second_round_carries_the_question_and_the_grammar() {
    let mut rows = parked_rows();
    rows.extend(closed_round(0, "c-1"));

    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": BUILD, "iter": "0", "repairs": "0",
               "store_origin": "weave"}),
        slate(&rows),
    );
    let fired = out
        .iter()
        .find(|m| m["header"]["route"] == "fire")
        .unwrap_or_else(|| panic!("the closed round re-enters the composer: {out:?}"));

    assert_still_briefed(fired, "round 1 of the design lane");
}

#[test]
fn the_repair_round_carries_them_too() {
    // The refine ear takes the same road in the other direction, and it was
    // handed the same empty prompt: a model asked to repair a refusal it can
    // read, in a language it can no longer see.
    let mut rows = parked_rows();
    rows.extend(closed_round(0, "c-1"));
    rows.push(row(
        0,
        "receipt",
        "{\"origin\":\"user\",\"type\":\"text\",\"text\":\"the submission was \
         refused: edge_schema\"}",
        0,
        "2999-01-01T10:00:09.000000Z",
    ));

    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": BUILD, "iter": "0", "repairs": "0",
               "store_origin": "weave"}),
        slate(&rows),
    );
    let repair = out
        .iter()
        .find(|m| m["header"]["route"] == "repair")
        .unwrap_or_else(|| panic!("the refusal goes back to the composer: {out:?}"));

    assert_still_briefed(repair, "the repair round");
    assert!(
        repair["messages"]
            .as_array()
            .and_then(|t| t.last())
            .and_then(|t| t["text"].as_str())
            .unwrap_or("")
            .contains("the submission was refused"),
        "and the refusal is still the newest turn of it"
    );
}
