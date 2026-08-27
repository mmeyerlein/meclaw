//! The fan-in. Four things it owes, and each one is a measured failure it
//! prevents:
//!   * a round is complete by ID MEMBERSHIP, never by row count -- arrival
//!     order cannot change the answer and a duplicate id cannot fake a
//!     complete set;
//!   * exactly ONE path crosses the re-entry edge (the `fired` claim);
//!   * a round whose result never comes must not park until the TTL, because a
//!     TTL death is silent -- direct to DLQ, no reply_to cascade;
//!   * the iteration budget ends the loop HERE, because `restore_ttl` on the
//!     re-entry edge deliberately takes TTL out of the game.
//!
//! Three notes on the wire this file speaks, each verified against the tree
//! rather than assumed:
//!
//! 1. The multi-send helper is `meclaw_testing::emit_all` (there is no
//!    `tests/support` module in this crate, and no `emit_many` anywhere); it
//!    returns the emissions as a `Vec`, which `run_weave` hands on as a JSON
//!    array so every case below reads them the way an edge would.
//! 2. `meclaw_testing::code_stdin` takes the message FLATLY spelled -- `header`
//!    moves into the envelope, `params` becomes the top-level params object and
//!    every other key is a body slot. A pre-built `{"envelope": …, "body": …}`
//!    would arrive as two body slots and the script would read an empty
//!    envelope. `params` is left EMPTY on purpose: that runs the script on the
//!    `_int()` defaults, which is exactly where the three knobs have to agree
//!    with `params.*` and `contract.settings.*.default` in the shipped file.
//! 3. The store answers a bundle with one `tool_result` turn per leg in
//!    `messages[]`, keyed by the leg's `tool_call_id`, and puts only per-op
//!    METADATA in the top-level `results[]` slot
//!    (`crates/meclaw-cells/src/store/output.rs`, `build_bundle_result`). The
//!    slate therefore arrives under `messages`, which is what `slate()` builds.
//!
//! Timestamps follow the convention `collector_window.rs` established for the
//! same idle arithmetic: year 2999 is newer than any cutoff minted from the
//! real clock, year 2000 is behind any idle window a test could configure. A
//! "today" literal would turn every case below into a time bomb two minutes
//! after the plan was written.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const WEAVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/weave/config.json"
);

const NORMALISE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/normalise/config.json"
);

const HIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/config.json"
);

fn run_weave(hop: Value, ctx: Value, body: Value) -> Value {
    let mut flat = json!({"header": {"hop": hop, "context": ctx}, "params": {}});
    if let Value::Object(slots) = body {
        for (slot, v) in slots {
            flat[slot] = v;
        }
    }
    Value::Array(emit_all(&shipped_script(WEAVE), &flat))
}

/// A slate row as the store hands it back.
fn row(iter: i64, role: &str, turn: &str, fired: i64, at: &str) -> Value {
    json!({"build_id": "b7", "iter": iter, "role": role, "turn": turn,
           "fired": fired, "recorded_at": at})
}

fn slate(rows: Vec<Value>) -> Value {
    json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "w-round-read",
                         "text": meclaw_core::serde_json::to_string(&rows).expect("rows")}]})
}

#[test]
fn an_incomplete_round_parks_and_emits_nothing() {
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": "0", "store_origin": "weave"}),
        slate(vec![
            row(
                0,
                "assistant",
                "[{\"id\":\"c-1\",\"type\":\"tool_call\"},{\"id\":\"c-2\",\"type\":\"tool_call\"}]",
                0,
                "2999-01-01T10:00:00.000000Z",
            ),
            row(
                0,
                "tool",
                "{\"id\":\"c-1\",\"type\":\"tool_result\"}",
                0,
                "2999-01-01T10:00:01.000000Z",
            ),
        ]),
    );
    assert_eq!(
        out.as_array().expect("multi-send").len(),
        0,
        "terminal by design: an incomplete fan-in emits NOTHING"
    );
}

#[test]
fn a_complete_round_fires_once_and_claims_the_row() {
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": "0", "store_origin": "weave"}),
        slate(vec![
            row(
                0,
                "user",
                "{\"origin\":\"user\",\"type\":\"text\",\"text\":\"a pipeline\"}",
                0,
                "2999-01-01T10:00:00.000000Z",
            ),
            row(
                0,
                "assistant",
                "[{\"id\":\"c-1\",\"type\":\"tool_call\"},{\"id\":\"c-2\",\"type\":\"tool_call\"}]",
                0,
                "2999-01-01T10:00:00.500000Z",
            ),
            row(
                0,
                "tool",
                "{\"id\":\"c-1\",\"type\":\"tool_result\"}",
                0,
                "2999-01-01T10:00:01.000000Z",
            ),
            row(
                0,
                "tool",
                "{\"id\":\"c-2\",\"type\":\"tool_result\"}",
                0,
                "2999-01-01T10:00:02.000000Z",
            ),
        ]),
    );
    let msgs = out.as_array().expect("multi-send");
    let fire = msgs
        .iter()
        .find(|m| m["header"]["route"] == "fire")
        .expect("fire lane");
    assert_eq!(
        fire["messages"].as_array().expect("rebuild").len(),
        5,
        "the thread is rebuilt cumulatively: user, BOTH tool_call turns of the \
         assistant bundle, both results -- a bundle is N turns in UBF, not one \
         (`collector/assemble`'s `thread_of` flattens it the same way), so the \
         count is 1+2+2"
    );
    assert!(
        msgs.iter().any(|m| m["header"]["route"] == "cstore"
            && m["messages"][0]["text"]
                .as_str()
                .unwrap_or("")
                .contains("\"fired\": 1")),
        "the claim travels WITH the fire, not one hop in front of it"
    );
}

#[test]
fn a_round_already_claimed_never_fires_a_second_time() {
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": "0", "store_origin": "weave"}),
        slate(vec![
            row(
                0,
                "assistant",
                "[{\"id\":\"c-1\",\"type\":\"tool_call\"}]",
                1,
                "2999-01-01T10:00:00.000000Z",
            ),
            row(
                0,
                "tool",
                "{\"id\":\"c-1\",\"type\":\"tool_result\"}",
                0,
                "2999-01-01T10:00:09.000000Z",
            ),
        ]),
    );
    assert_eq!(
        out.as_array().expect("multi-send").len(),
        0,
        "a late result reads a complete set too -- the mark makes the election \
         permanent"
    );
}

#[test]
fn an_idle_round_is_closed_with_synthetic_results_rather_than_parked() {
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": "0", "store_origin": "weave"}),
        slate(vec![
            row(
                0,
                "assistant",
                "[{\"id\":\"c-1\",\"type\":\"tool_call\"},{\"id\":\"c-2\",\"type\":\"tool_call\"}]",
                0,
                "2000-01-01T00:00:00.000000Z",
            ),
            row(
                0,
                "tool",
                "{\"id\":\"c-1\",\"type\":\"tool_result\"}",
                0,
                "2000-01-01T00:00:01.000000Z",
            ),
        ]),
    );
    let msgs = out.as_array().expect("multi-send");
    assert!(
        msgs.iter().any(|m| meclaw_core::serde_json::to_string(m)
            .unwrap_or_default()
            .contains("c-2")),
        "the missing call gets a stand-in under its own id, so the round \
         becomes fan-in-complete through the REGULAR machine"
    );
}

#[test]
fn the_iteration_budget_ends_the_loop_here_and_says_so() {
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": "6", "store_origin": "weave"}),
        slate(vec![
            row(
                6,
                "assistant",
                "[{\"id\":\"c-9\",\"type\":\"tool_call\"}]",
                0,
                "2999-01-01T10:00:00.000000Z",
            ),
            row(
                6,
                "tool",
                "{\"id\":\"c-9\",\"type\":\"tool_result\"}",
                0,
                "2999-01-01T10:00:01.000000Z",
            ),
        ]),
    );
    let msgs = out.as_array().expect("multi-send");
    let out_msg = msgs
        .iter()
        .find(|m| m["header"]["route"] == "draft")
        .expect("at the cap the round leaves for normalise, not for compose");
    assert_eq!(out_msg["header"]["round_capped"], "1");
}

#[test]
fn a_refusal_is_repaired_by_name_and_the_second_one_stops_the_build() {
    let first = run_weave(
        json!({"route": "in_receipt", "error_code": "requirement_missing"}),
        json!({"build_id": "b7", "iter": "1", "repairs": "0"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                             "text": "ctx key \"model\" is required"}]}),
    );
    let msgs = first.as_array().expect("multi-send");
    assert!(
        msgs.iter().any(|m| meclaw_core::serde_json::to_string(m)
            .unwrap_or_default()
            .contains("requirement_missing")),
        "NAME the code -- a refusal the model cannot name is one it cannot repair"
    );

    let second = run_weave(
        json!({"route": "in_receipt", "error_code": "requirement_missing"}),
        json!({"build_id": "b7", "iter": "1", "repairs": "2"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                             "text": "ctx key \"model\" is required"}]}),
    );
    let msgs = second.as_array().expect("multi-send");
    let err = msgs
        .iter()
        .find(|m| m["header"]["route"] == "give_up")
        .expect("the repair budget ends the loop, named");
    assert_eq!(err["header"]["error_code"], "requirement_missing");
}

/// A refusal row as `in_receipt` parks it: its own role, so the fan-in of the
/// round it belongs to cannot mistake it for a tool answer. `fired` is the
/// hand-back claim -- 0 while the composer has not seen it, 1 once it has.
///
/// The turn is a plain `user` text turn and not a `tool_result`: it answers no
/// tool_call, and an id-less tool_result is a 400 on the wire the rebuilt
/// thread travels (`builder_weave_rebuilds_a_wire_legal_thread.rs`).
fn receipt_row(iter: i64, code: &str, fired: i64, at: &str) -> Value {
    let turn = json!({"origin": "user", "type": "text",
                      "text": format!("the submission was refused: {code} -- ctx key \"model\" is required")});
    row(
        iter,
        "receipt",
        &meclaw_core::serde_json::to_string(&turn).expect("turn"),
        fired,
        at,
    )
}

/// The gap the first six tests left open: they pin that a refusal is NAMED and
/// that the budget stops the build, but not that a refusal below the budget
/// ever reaches the composer again. Without this the repair edge
/// (`hop.route == 'repair' && int(context.repairs) < 2`) is dead: `repairs` is
/// never incremented, so `give_up` is unreachable too and the build dies in
/// silence at the `fired=1` park -- the one property the loop may not lose.
#[test]
fn a_refusal_below_the_budget_reaches_the_composer_again() {
    // The ear parks the refusal under its own role, so `round_sets` -- which
    // reads `assistant` and `tool` only -- cannot count it as an answer.
    let parked = run_weave(
        json!({"route": "in_receipt", "error_code": "requirement_missing"}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                             "text": "ctx key \"model\" is required"}]}),
    );
    let msgs = parked.as_array().expect("multi-send");
    assert!(
        msgs.iter().any(|m| m["header"]["route"] == "cstore"
            && m["messages"][0]["text"]
                .as_str()
                .unwrap_or("")
                .contains("\"role\": \"receipt\"")),
        "a refusal is a turn of its own kind, not a tool answer"
    );

    // The round it refuses is long since fired -- that is what a receipt IS.
    let refused = vec![
        row(
            0,
            "assistant",
            "[{\"id\":\"c-1\",\"type\":\"tool_call\"}]",
            1,
            "2999-01-01T10:00:00.000000Z",
        ),
        row(
            0,
            "tool",
            "{\"id\":\"c-1\",\"type\":\"tool_result\"}",
            0,
            "2999-01-01T10:00:01.000000Z",
        ),
        receipt_row(0, "requirement_missing", 0, "2999-01-01T10:00:09.000000Z"),
    ];

    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0", "store_origin": "weave"}),
        slate(refused.clone()),
    );
    let msgs = out.as_array().expect("multi-send");
    let repair = msgs
        .iter()
        .find(|m| m["header"]["route"] == "repair")
        .expect("below the budget the refusal goes BACK to the composer");
    assert!(
        meclaw_core::serde_json::to_string(&repair["messages"])
            .unwrap_or_default()
            .contains("requirement_missing"),
        "and it carries the named code in the thread the composer reads"
    );
    assert!(
        msgs.iter().any(|m| m["header"]["route"] == "cstore"
            && m["messages"][0]["text"]
                .as_str()
                .unwrap_or("")
                .contains("\"receipt\"")),
        "the hand-back is claimed on the receipt row, in the same multi-send"
    );

    // Claimed once, the same slate must not hand it back a second time -- and
    // the claim is on the ROW, not in the context: `repairs` still says 0 here.
    let claimed = vec![
        refused[0].clone(),
        refused[1].clone(),
        receipt_row(0, "requirement_missing", 1, "2999-01-01T10:00:09.000000Z"),
    ];
    let again = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0", "store_origin": "weave"}),
        slate(claimed),
    );
    assert_eq!(
        again.as_array().expect("multi-send").len(),
        0,
        "one refusal, one repair -- and the mark that says so is written down"
    );
}

/// The ruling of 2026-08-27: the counter is DERIVED, never carried.
///
/// A receipt does not come from the loop. `templates/submit/gate` renders it off
/// its own remembered row -- "the colony's answer begins a fresh trace and
/// carries neither" -- and the `./submit -> ./builder` edge in
/// `templates/meclaw-os/config.json` re-stamps `hop.route` and nothing else. So
/// no counter this loop set can survive the journey, and `context.repairs`
/// arrives as 0 or not at all. What survives is what was written down: the
/// `receipt` rows of this build. This case says the budget is read from THEM,
/// with the context deliberately lying at 0.
#[test]
fn the_repair_budget_is_counted_from_the_slate_not_from_the_context() {
    let round = vec![
        row(
            0,
            "assistant",
            "[{\"id\":\"c-1\",\"type\":\"tool_call\"}]",
            1,
            "2999-01-01T10:00:00.000000Z",
        ),
        row(
            0,
            "tool",
            "{\"id\":\"c-1\",\"type\":\"tool_result\"}",
            0,
            "2999-01-01T10:00:01.000000Z",
        ),
    ];

    // Second refusal, first one already handed back. Two collected, the budget
    // is two -- so this one is still repaired, and the counter the re-entry
    // edge reads must say 2 even though the context says 0.
    let mut second = round.clone();
    second.push(receipt_row(
        0,
        "requirement_missing",
        1,
        "2999-01-01T10:00:09.000000Z",
    ));
    second.push(receipt_row(
        0,
        "scope_not_permitted",
        0,
        "2999-01-01T10:00:19.000000Z",
    ));
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0", "store_origin": "weave"}),
        slate(second),
    );
    let repair = out
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "repair")
        .cloned()
        .expect("two collected against a budget of two is still a repair");
    assert_eq!(
        repair["header"]["repairs"], "2",
        "the derived count is what the re-entry edge reads, not the context's 0"
    );

    // Third refusal: three collected against a budget of two. The build stops,
    // and it stops NAMED -- with the code of the refusal that is still open.
    let mut third = round;
    third.push(receipt_row(
        0,
        "requirement_missing",
        1,
        "2999-01-01T10:00:09.000000Z",
    ));
    third.push(receipt_row(
        0,
        "requirement_missing",
        1,
        "2999-01-01T10:00:19.000000Z",
    ));
    third.push(receipt_row(
        0,
        "scope_not_permitted",
        0,
        "2999-01-01T10:00:29.000000Z",
    ));
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0", "store_origin": "weave"}),
        slate(third),
    );
    let err = out
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "give_up")
        .cloned()
        .expect("over the budget the build stops -- counted from the slate");
    assert_eq!(
        err["header"]["error_code"], "scope_not_permitted",
        "the code of the refusal that is still open, recovered from the row it \
         was written into -- a build that stops says WHY"
    );
}

// ===================================================== THE LOOP OWNS ITS OWN
// CORRELATION (orchestrator ruling, 2026-08-27)
//
// A receipt arrives with none of the loop's context: `templates/submit/gate`
// renders it off its own remembered row -- "the colony's answer begins a fresh
// trace and carries neither" -- and the `./submit -> ./builder` edge re-stamps
// `hop.route` and nothing else. What DOES survive is `hop.manifest_sha256`,
// because the submitter parks and pops a submission under its digest.
//
// So the digest is the handle, and the loop writes down what it means:
// `normalise` -- the only cell in the hive that sees the digest (it computes it)
// AND `context.build_id` -- parks a binding row, and `weave` reads it back. That
// also covers the commonest path, where the model answers without a tool round
// and `compose -> normalise` bypasses `weave` altogether.

/// A binding row as `normalise` parks it: the digest of the manifest it just
/// composed, filed under the build that composed it.
fn binding_row(build: &str, sha: &str, at: &str) -> Value {
    json!({"build_id": build, "iter": 0, "role": "manifest", "turn": sha,
           "fired": 0, "recorded_at": at})
}

/// The store's answer to the binding lookup leg.
fn lookup(rows: Vec<Value>) -> Value {
    json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "w-build-lookup",
                         "text": meclaw_core::serde_json::to_string(&rows).expect("rows")}]})
}

fn ops_of(bundle: &Value) -> Vec<Value> {
    bundle["messages"].as_array().cloned().unwrap_or_default()
}

fn op_text(ops: &[Value], id: &str) -> String {
    ops.iter()
        .find(|o| o["id"] == id)
        .map(|o| o["text"].as_str().unwrap_or("").to_string())
        .unwrap_or_default()
}

#[test]
fn a_receipt_without_a_build_id_finds_its_build_through_the_manifest_digest() {
    // The real wire: no build_id in the context, no build_id on the hop. Only
    // the digest the submitter parked the submission under.
    let asked = run_weave(
        json!({"route": "in_receipt", "error_code": "requirement_missing",
               "manifest_sha256": "d19e57"}),
        json!({}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                             "text": "ctx key \"model\" is required"}]}),
    );
    let msgs = asked.as_array().expect("multi-send");
    let bundle = msgs
        .iter()
        .find(|m| m["header"]["route"] == "cstore")
        .expect("the ear asks the transcript who owns this digest");
    let ops = ops_of(bundle);
    assert!(
        op_text(&ops, "w-build-lookup").contains("d19e57"),
        "the lookup is BY the digest -- the only handle that survived the trip"
    );
    assert!(
        op_text(&ops, "w-receipt-row").contains("\"build_id\": \"d19e57\""),
        "and the refusal is staged UNDER the digest, so two builds refused at \
         the same time cannot pool into one"
    );

    // The binding answers: this digest belongs to build b7.
    let adopted = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"store_origin": "weave"}),
        lookup(vec![binding_row(
            "b7",
            "d19e57",
            "2999-01-01T09:00:00.000000Z",
        )]),
    );
    let msgs = adopted.as_array().expect("multi-send");
    let bundle = msgs
        .iter()
        .find(|m| m["header"]["route"] == "cstore")
        .expect("the staged refusal is adopted into the build it refuses");
    let ops = ops_of(bundle);
    assert!(
        op_text(&ops, "w-adopt").contains("\"build_id\": \"b7\""),
        "adopted by name"
    );
    assert!(
        op_text(&ops, "w-round-read").contains("b7"),
        "and the slate that comes back is the build's own, so everything after \
         this is the regular machine"
    );

    // No binding row at all -- a manifest nobody in this hive composed. It must
    // not park: a build that stops says so.
    let orphan = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        json!({"store_origin": "weave"}),
        lookup(vec![]),
    );
    let err = orphan
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "give_up")
        .cloned()
        .expect("a refusal that cannot find its build ends the build, named");
    assert_eq!(err["header"]["error_code"], "build_unknown");
}

#[test]
fn normalise_parks_the_digest_under_the_build_that_composed_it() {
    let answer = "here you go: {\"declarations\":[{\"scope\":\"/org/acme\",\
                  \"diff\":{\"add_nodes\":[]}}]}";
    let out = Value::Array(emit_all(
        &shipped_script(NORMALISE),
        &json!({"header": {"hop": {}, "context": {"build_id": "b7", "iter": "2"}},
                "messages": [{"origin": "assistant", "type": "text", "id": "", "text": answer}],
                "params": {}}),
    ));
    let msgs = out.as_array().expect("multi-send");
    let manifest = msgs
        .iter()
        .find(|m| m["header"]["operation"] == "normalise")
        .expect("the manifest still leaves");
    let sha = manifest["header"]["manifest_sha256"]
        .as_str()
        .expect("the digest it composed")
        .to_string();
    let bind = msgs
        .iter()
        .find(|m| m["header"]["route"] == "bind")
        .expect("and the binding row is parked beside it");
    let op = bind["messages"][0]["text"].as_str().unwrap_or("");
    assert!(
        op.contains(&sha),
        "the row records THE digest of THIS manifest: {op}"
    );
    assert!(
        op.contains("\"build_id\": \"b7\"") && op.contains("\"role\": \"manifest\""),
        "under the build that composed it, in its own role: {op}"
    );
}

#[test]
fn the_binding_row_has_an_edge_to_travel_on() {
    let cfg: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(HIVE).expect("hive config"))
            .expect("parses");
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    let bind = edges
        .iter()
        .find(|e| e["from"] == "./normalise" && e["to"] == "./transcript")
        .expect("normalise reaches the round table, or the binding row is written nowhere");
    assert!(
        bind["condition"].as_str().unwrap_or("").contains("'bind'"),
        "and only the binding row travels it -- the manifest keeps its own way out"
    );
    assert_eq!(
        bind["modifier"]["set_context"]["store_origin"], "'weave'",
        "the store's answer needs a way home; weave parks it unread"
    );
}
