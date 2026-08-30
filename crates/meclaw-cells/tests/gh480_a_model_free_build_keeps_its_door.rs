//! GH #480, second half — the fast lane loses its caller to a refusal.
//!
//! The first half (`builder@1.3.3`) repaired the design lane: the caller's six
//! coordinates are parked in the round table when the composer opens a round,
//! and read back whenever a round is decided. The fast lane has no composer, so
//! it opens no round, so it parks nothing — and a refusal that comes back for a
//! model-free build finds no owner for its digest:
//!
//! ```text
//! builder       -> builder/weave   route=in_receipt   bc=None
//! builder/weave -> builder         route=error        bc=None   error_code=build_unknown
//! builder       -> /os/orgs        route=in_build_result        <-- wrong door
//! ```
//!
//! The named refusal (`template_missing`, measured) went into the organisation
//! instead of back to the front door, and in a fresh colony `./orgs` is empty by
//! construction — a silent dead letter.
//!
//! **The ruling the issue asked for: a model-free build gets no repair round.**
//! A recipe is a pure function of the wish. There is no composer behind it, no
//! thread to hand back and no question to re-ask, so a repair round would call a
//! model that never ran with a thread that does not exist, to re-derive bytes
//! that were already determined. What a fast-lane refusal needs is the short
//! road: name the code, and give it back to the door the build came from.
//!
//! Two mechanisms, and both are the ones the design lane already uses:
//!
//! * `recipes` writes the same two rows `weave` and `normalise` write — the
//!   caller, and the digest binding that says which build owns those bytes. Its
//!   build id **is** the digest: a fast-lane build has no other identity, and
//!   the digest is the one handle that survives the submitter's foreign chain.
//! * the caller row carries `build_lane: "recipe"`, which is what tells `weave`
//!   on the read-back that there is no composer to repair with.
//!
//! No model. The recipe renderer is a string built out of a dict.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const WEAVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/weave/config.json"
);
const HIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/config.json"
);

fn config(path: &str) -> Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn edges() -> Vec<Value> {
    config(HIVE)["params"]["graph"]["edges"]
        .as_array()
        .expect("the graph declares edges")
        .clone()
}

/// The context an operator-driven fast-lane build carries: the door the shell
/// stamped at its rim, and no `build_call_id` — there is no agent tool call
/// behind a person at the front door.
fn operator_context() -> Value {
    json!({"build_caller": "operator", "build_auto_submit": "yes",
           "build_op": "draft", "build_scope": "/os"})
}

fn run_recipes(ctx: Value, wish: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(RECIPES),
        &json!({
            "header": {"hop": {"route": "recipe"}, "context": ctx},
            "params": {},
            "messages": [{"origin": "user", "type": "text", "id": "",
                          "text": meclaw_core::serde_json::to_string(&wish).expect("wish")}],
        }),
    )
}

/// A recipe the renderer can actually complete: one edge, rewired.
fn a_rewire() -> Value {
    json!({"recipe": "rewire_edge",
           "params": {"scope": "/os", "from": "./a", "to": "./c", "old_to": "./b"}})
}

fn leg<'a>(bundle: &'a Value, id: &str) -> &'a Value {
    bundle["messages"]
        .as_array()
        .expect("a bundle carries legs")
        .iter()
        .find(|m| m["id"] == id)
        .unwrap_or_else(|| panic!("no leg `{id}` in {bundle}"))
}

fn inserted_row(bundle: &Value, id: &str) -> Value {
    let op: Value =
        meclaw_core::serde_json::from_str(leg(bundle, id)["text"].as_str().unwrap_or(""))
            .expect("a store leg carries one json op");
    assert_eq!(op["operation"], "insert", "the leg has to WRITE the row");
    assert_eq!(op["table"], "thread");
    op["row"].clone()
}

fn bind_message(out: &[Value]) -> Value {
    out.iter()
        .find(|m| m["header"]["route"] == "bind")
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "the fast lane writes nothing down, so a refusal that comes back \
                 for it finds no owner for its digest and no door to answer -- \
                 {out:?}"
            )
        })
}

#[test]
fn a_finished_recipe_parks_its_caller_and_binds_its_digest() {
    let out = run_recipes(operator_context(), a_rewire());
    let manifest = out
        .iter()
        .find(|m| m["header"]["operation"] == "recipe")
        .cloned()
        .expect("the manifest still ships");
    let sha = manifest["header"]["manifest_sha256"]
        .as_str()
        .expect("a fast manifest carries its digest")
        .to_string();

    let bind = bind_message(&out);
    let caller = inserted_row(&bind, "w-fast-caller");
    let binding = inserted_row(&bind, "w-fast-bind");

    assert_eq!(
        binding["build_id"], sha,
        "a fast-lane build has no identity but its bytes, so the digest IS the \
         build id -- otherwise the two rows are filed under different builds and \
         the read-back finds half a slate"
    );
    assert_eq!(binding["role"], "manifest");
    assert_eq!(
        binding["turn"], sha,
        "the binding answers `who composed this digest`, which is the exact \
         question `weave`'s `w-build-lookup` asks"
    );
    assert_eq!(
        caller["build_id"], sha,
        "both rows are filed under one build"
    );
    assert_eq!(caller["role"], "caller");

    let said: Value = meclaw_core::serde_json::from_str(caller["turn"].as_str().unwrap_or("null"))
        .expect("the parked note is json");
    assert_eq!(
        said["build_caller"], "operator",
        "the door the shell stamped is what the row exists to remember: {said}"
    );
    assert_eq!(said["build_auto_submit"], "yes");
    assert_eq!(
        said["build_lane"], "recipe",
        "and the row says which lane it belongs to -- a refusal cannot be handed \
         to a composer that never ran, and the read-back has no other way to know"
    );
}

#[test]
fn a_refused_recipe_writes_nothing_down() {
    // A build that produced no bytes has no digest to bind and nothing to be
    // refused at a door later. A row here would be a slate no receipt can reach.
    let out = run_recipes(operator_context(), json!({"recipe": "no-such-recipe"}));
    assert_eq!(
        out.len(),
        1,
        "a refusal on the fast lane is one message: {out:?}"
    );
    assert_eq!(out[0]["header"]["error_code"], "recipe_unknown");
}

#[test]
fn the_bind_leg_has_its_own_edge_and_leaves_by_no_other() {
    let bind = bind_message(&run_recipes(operator_context(), a_rewire()));
    let route = bind["header"]["route"].as_str().unwrap_or("");
    let operation = bind["header"]["operation"].as_str().unwrap_or("");

    let to_store = edges().into_iter().find(|e| {
        e["from"] == "./recipes"
            && e["to"] == "./transcript"
            && e["condition"]
                .as_str()
                .unwrap_or("")
                .contains(&format!("hop.route == '{route}'"))
    });
    let to_store = to_store.expect(
        "the binding has no edge to the transcript, so it dead-letters as \
         `no_route` and the fast lane is exactly as unrecoverable as before",
    );
    assert_eq!(
        to_store["modifier"]["set_context"]["store_origin"], "'weave'",
        "the store's answer finds its way back on the same marker every other \
         transcript leg uses"
    );

    // And it must NOT leave the hive as a manifest: both `./recipes -> .` edges
    // are conditioned on `hop.operation == 'recipe'`, so a binding wearing that
    // operation would be delivered to the caller as a second answer.
    assert_ne!(
        operation, "recipe",
        "the binding wears the operation the exit edges match on: {}",
        bind["header"]
    );
}

// ---- The read-back: what a refusal to a model-free build does.

const SHA: &str = "3f5a7c1d9e2b4086a1c3d5e7f9012345678901234567890abcdef0123456789a";

fn row(role: &str, turn: &str, at: &str) -> Value {
    json!({"build_id": SHA, "iter": 0, "role": role, "turn": turn, "fired": 0,
           "recorded_at": at})
}

fn caller_row(lane: Option<&str>) -> Value {
    let mut said = json!({"build_call_id": "", "agent": "", "build_op": "draft",
                          "build_scope": "/os", "build_caller": "operator",
                          "build_auto_submit": "yes"});
    if let Some(lane) = lane {
        said["build_lane"] = json!(lane);
    }
    row(
        "caller",
        &meclaw_core::serde_json::to_string(&said).expect("note"),
        "2999-01-01T09:00:00.000000Z",
    )
}

fn receipt_row(code: &str) -> Value {
    let turn = json!({"origin": "user", "type": "text",
                      "text": format!("the submission was refused: {code}")});
    row(
        "receipt",
        &meclaw_core::serde_json::to_string(&turn).expect("turn"),
        "2999-01-01T09:00:02.000000Z",
    )
}

/// The slate as it stands when the store answers the read-back: the context is
/// EMPTY, because a receipt arrives on the submitter's chain.
fn read_back(rows: Vec<Value>) -> Vec<Value> {
    emit_all(
        &shipped_script(WEAVE),
        &json!({
            "header": {"hop": {"route": "cstore"},
                       "context": {"store_origin": "weave"}},
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

#[test]
fn a_refused_model_free_build_takes_the_short_road_to_its_door() {
    let out = read_back(vec![
        caller_row(Some("recipe")),
        row("manifest", SHA, "2999-01-01T09:00:01.000000Z"),
        receipt_row("template_missing"),
    ]);
    let answer = decided(&out);
    assert_eq!(
        answer["header"]["route"], "give_up",
        "a model-free build has no composer, no thread and no question -- a \
         repair round would call a model that never ran: {}",
        answer["header"]
    );
    assert_eq!(
        answer["header"]["error_code"], "template_missing",
        "and the refusal is named, not renamed: {}",
        answer["header"]
    );
    assert_eq!(
        answer["header"]["build_caller"], "operator",
        "the door comes back off the parked row, or the answer goes into an \
         organisation that never asked for it: {}",
        answer["header"]
    );
    assert_eq!(answer["header"]["build_auto_submit"], "yes");
    assert!(
        out.iter().any(|m| m["header"]["route"] == "cstore"),
        "and the receipt is claimed, or the next read of the same slate answers \
         it a second time: {out:?}"
    );
}

#[test]
fn a_composed_build_still_gets_its_repair_round() {
    // The non-regression that makes the branch a decision rather than a
    // shortcut: the same slate without the lane marker is the design lane, and
    // it keeps the repair machine GH #460 built.
    let out = read_back(vec![
        caller_row(None),
        row("manifest", SHA, "2999-01-01T09:00:01.000000Z"),
        receipt_row("template_missing"),
    ]);
    let answer = decided(&out);
    assert_eq!(
        answer["header"]["route"], "repair",
        "a composed build is repaired by the composer that wrote it: {}",
        answer["header"]
    );
}

#[test]
fn a_build_that_names_no_caller_parks_nothing() {
    // The same guard `weave` parks its own caller row under: a build nobody
    // stamped a door on has no door to recover, and a slate written for a build
    // nobody is waiting on is a table that only grows.
    let out = run_recipes(json!({}), a_rewire());
    assert_eq!(
        out.len(),
        1,
        "a caller-less recipe wrote rows anyway: {out:?}"
    );
}
