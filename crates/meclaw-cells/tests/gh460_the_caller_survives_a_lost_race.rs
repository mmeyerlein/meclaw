//! GH #460 — the caller survives the leg that wins the round.
//!
//! The builder's fan-in is decided by a RACE: the slate of one round is read by
//! the leg that arrives LAST, and it is that leg's `context` that travels on. A
//! `lib` leg comes back on the build's own chain and carries everything; the
//! reply of a `/colony` eye starts a FRESH trace, so the edge `./eyes → ./weave`
//! restores exactly the three coordinates the LOOP needs — `build_id`, `iter`,
//! `repairs`. Everything the CALLER needs was on no list, and a build that lost
//! the race once handed the requester a draft under an empty `tool_call_id`
//! (measured, `CHANGELOG.md` § 0.27.0).
//!
//! These tests let the WRONG leg win: every read-back below is run on exactly
//! the context an eye restores and nothing else, and asserts that the caller is
//! on the emission anyway. The mechanism is the round table, the same one
//! `normalise` binds a digest in — `weave` parks a `caller` row on the one leg
//! of a round that is always on the build's own chain, and reads it back off the
//! slate when the round is decided.
//!
//! The wire conventions are `builder_weave_closes_the_round.rs`'s, verified
//! there against the tree: `code_stdin` takes the message FLATLY spelled, the
//! store answers a bundle with one `tool_result` turn per leg keyed by that
//! leg's id, and timestamps use the year-2999 convention so no case becomes a
//! time bomb.

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

const TRANSCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/transcript/config.json"
);

/// The four keys that ride the hop. `orig_request` is deliberately not among
/// them: it is prose, it stays in the row, and the README says why.
const CARRIED: [&str; 4] = ["build_call_id", "agent", "build_op", "build_scope"];

fn run_weave(hop: Value, ctx: Value, body: Value) -> Value {
    let mut flat = json!({"header": {"hop": hop, "context": ctx}, "params": {}});
    if let Value::Object(slots) = body {
        for (slot, v) in slots {
            flat[slot] = v;
        }
    }
    Value::Array(emit_all(&shipped_script(WEAVE), &flat))
}

fn hive() -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(HIVE).expect("hive config"))
        .expect("parses")
}

fn row(iter: i64, role: &str, turn: &str, fired: i64, at: &str) -> Value {
    json!({"build_id": "b7", "iter": iter, "role": role, "turn": turn,
           "fired": fired, "recorded_at": at})
}

fn slate(rows: Vec<Value>) -> Value {
    json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "w-round-read",
                         "text": meclaw_core::serde_json::to_string(&rows).expect("rows")}]})
}

/// The caller row exactly as the composer's own leg parks it.
fn caller_row(at: &str) -> Value {
    row(
        0,
        "caller",
        "{\"build_call_id\": \"s1\", \"agent\": \"scribe\", \"build_op\": \"draft\", \
         \"build_scope\": \"/os/orgs/acme\", \"orig_request\": \"build a research pipeline\"}",
        0,
        at,
    )
}

/// What the edge `./eyes → ./weave` leaves in the context, and nothing else.
/// This is the losing side of the race, spelled out.
fn context_an_eye_restores() -> Value {
    json!({"build_id": "b7", "iter": "0", "repairs": "0", "store_origin": "weave"})
}

/// A complete round: the assistant bundle and both results.
fn complete_round(with_caller: bool) -> Vec<Value> {
    let mut rows = vec![
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
    ];
    if with_caller {
        rows.insert(0, caller_row("2999-01-01T10:00:00.000000Z"));
    }
    rows
}

/// The composer's leg is the one that is always on the build's own chain, so it
/// is where the caller is written down — in the SAME bundle as the assistant
/// row, because the store runs a bundle's ops in order and the trailing select
/// has to see both.
#[test]
fn the_round_the_composer_opens_writes_the_caller_down() {
    let out = run_weave(
        json!({"route": "calls"}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0",
               "build_call_id": "s1", "agent": "scribe", "build_op": "draft",
               "build_scope": "/os/orgs/acme",
               "orig_request": "build a research pipeline"}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call",
                             "id": "c-1", "text": "{}"}]}),
    );
    let msgs = out.as_array().expect("multi-send");
    assert_eq!(msgs.len(), 1, "one bundle, not two messages");
    let legs = msgs[0]["messages"].as_array().expect("the bundle's ops");
    let ids: Vec<&str> = legs
        .iter()
        .map(|m| m["id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        ids,
        vec!["w-caller-row", "w-round-row", "w-round-read"],
        "the caller rides in FRONT of the round row and the select is last, or \
         the read-back cannot see what was just written"
    );
    let op = legs[0]["text"].as_str().unwrap_or("");
    assert!(
        op.contains("\"role\": \"caller\"") && op.contains("\"build_id\": \"b7\""),
        "the row is filed under the build it belongs to, in its own role: {op}"
    );
    for key in CARRIED {
        assert!(op.contains(key), "the row records {key}: {op}");
    }
    assert!(
        op.contains("build a research pipeline"),
        "and the prose too — orig_request is adopted through the store because \
         a header is not where prose belongs: {op}"
    );
}

/// A build driven without the tool surface has no caller to lose, and an empty
/// row would shadow a good one on the read-back.
#[test]
fn a_build_with_no_caller_parks_no_row_for_one() {
    let out = run_weave(
        json!({"route": "calls"}),
        json!({"build_id": "b7", "iter": "0", "repairs": "0"}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call",
                             "id": "c-1", "text": "{}"}]}),
    );
    let legs = out[0]["messages"].as_array().expect("the bundle's ops");
    let ids: Vec<&str> = legs
        .iter()
        .map(|m| m["id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        ids,
        vec!["w-round-row", "w-round-read"],
        "no caller, no row"
    );
}

/// THE case. The read-back runs on the context an eye restores — three
/// coordinates and nothing else — and the caller is on the emission anyway,
/// because the slate carries the row.
#[test]
fn a_round_an_eye_wins_still_names_the_caller() {
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        context_an_eye_restores(),
        slate(complete_round(true)),
    );
    let msgs = out.as_array().expect("multi-send");
    let fire = msgs
        .iter()
        .find(|m| m["header"]["route"] == "fire")
        .expect("the round fires");
    assert_eq!(
        fire["header"]["build_call_id"], "s1",
        "the requester's tool_call_id is what the fan-in at the tool surface \
         waits for: an empty one is a round that never ends"
    );
    assert_eq!(fire["header"]["agent"], "scribe");
    assert_eq!(fire["header"]["build_op"], "draft");
    assert_eq!(fire["header"]["build_scope"], "/os/orgs/acme");
}

/// Without the row the same slate loses the caller — this is the measured
/// failure, kept as a test so the fix cannot be mistaken for a coincidence.
#[test]
fn the_row_is_what_carries_it_and_not_the_context() {
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        context_an_eye_restores(),
        slate(complete_round(false)),
    );
    let fire = out
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "fire")
        .cloned()
        .expect("the round still fires");
    assert_eq!(
        fire["header"]["build_call_id"], "",
        "present and EMPTY rather than absent — a missing hop key makes a CEL \
         modifier fail, and a failed modifier skips its edge"
    );
}

/// A leg that DID carry the caller must not be overwritten by the slate, and a
/// slate row must not be preferred over a live context either.
#[test]
fn a_leg_that_carried_the_caller_keeps_its_own() {
    let mut ctx = context_an_eye_restores();
    ctx["build_call_id"] = json!("s9");
    ctx["agent"] = json!("other");
    ctx["build_op"] = json!("draft");
    ctx["build_scope"] = json!("/os");
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        ctx,
        slate(complete_round(true)),
    );
    let fire = out
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "fire")
        .cloned()
        .expect("the round fires");
    assert_eq!(
        fire["header"]["build_call_id"], "s9",
        "the context is the live fact; the row is the fallback"
    );
}

/// The caller is a note the loop keeps about itself. Handing it to a provider
/// would be a message nobody wrote — and one that carries a path and an id into
/// a prompt.
#[test]
fn the_caller_row_is_not_a_turn_in_the_rebuilt_thread() {
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        context_an_eye_restores(),
        slate(complete_round(true)),
    );
    let fire = out
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "fire")
        .cloned()
        .expect("the round fires");
    let thread = fire["messages"].as_array().expect("the rebuilt thread");
    assert_eq!(
        thread.len(),
        4,
        "two tool_call turns of the assistant bundle plus two results — the \
         caller row is skipped, not counted: {thread:?}"
    );
    let rendered = meclaw_core::serde_json::to_string(&fire["messages"]).expect("thread");
    assert!(
        !rendered.contains("build_call_id"),
        "and none of it reaches the provider: {rendered}"
    );
}

/// The capped round leaves for `normalise` rather than for the composer, and
/// that is the lane the DRAFT goes out on — losing the caller there is losing it
/// on the way to the requester.
#[test]
fn the_capped_round_carries_the_caller_to_normalise() {
    let mut ctx = context_an_eye_restores();
    ctx["iter"] = json!("6");
    let mut rows = complete_round(true);
    for r in rows.iter_mut() {
        if r["role"] != "caller" {
            r["iter"] = json!(6);
        }
    }
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        ctx,
        slate(rows),
    );
    let draft = out
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "draft")
        .cloned()
        .expect("the iteration budget ends the loop here");
    assert_eq!(draft["header"]["build_call_id"], "s1");
}

/// A build that STOPS has to say so to the caller that asked, so `give_up`
/// carries the identity too — the error lane is the same lane as far as the
/// tool round is concerned.
#[test]
fn the_named_stop_carries_the_caller_as_well() {
    let mut rows = complete_round(true);
    rows.push(row(
        0,
        "receipt",
        "{\"origin\":\"user\",\"type\":\"text\",\"text\":\"the submission was refused: schema\"}",
        0,
        "2999-01-01T10:00:03.000000Z",
    ));
    rows.push(row(
        0,
        "receipt",
        "{\"origin\":\"user\",\"type\":\"text\",\"text\":\"the submission was refused: schema\"}",
        0,
        "2999-01-01T10:00:04.000000Z",
    ));
    rows.push(row(
        0,
        "receipt",
        "{\"origin\":\"user\",\"type\":\"text\",\"text\":\"the submission was refused: schema\"}",
        0,
        "2999-01-01T10:00:05.000000Z",
    ));
    let out = run_weave(
        json!({"operation": "bundle", "route": "cstore"}),
        context_an_eye_restores(),
        slate(rows),
    );
    let stop = out
        .as_array()
        .expect("multi-send")
        .iter()
        .find(|m| m["header"]["route"] == "give_up")
        .cloned()
        .expect("the repair budget is spent and the build says so");
    assert_eq!(stop["header"]["build_call_id"], "s1");
    assert_eq!(stop["header"]["error_code"], "schema");
}

/// The hop half is only half the fix: a key on the hop that no edge lifts into
/// the context is lost at the next cell. Every INTERIOR lane out of `./weave`
/// that leads anywhere but into its own store restores all four.
///
/// RECALIBRATED by GH #499. `./weave -> .` is a fan-in lane and an EXIT edge at
/// once, and the two rules meet on it: what a hive remembers about its own
/// interior ends at the rim, so an exit edge lifts only what somebody OUTSIDE
/// this hive reads off the context. That is three keys and not six —
/// `build_caller` and `build_auto_submit`, which the shell's four
/// `./builder -> X` edges decide the door on, and `build_call_id`, which
/// `tools/build` and `tools/apply` read back when the answer returns through
/// their `in_build_result` door. `agent`, `build_op` and `build_scope` still
/// leave on the HOP, where `head()` stamps them; nothing outside reads them off
/// the context, and a `set_context` beside the `delete_context` of the same key
/// on the same edge would be an edge contradicting itself (set runs first, then
/// delete).
const CARRIED_PAST_THE_RIM: [&str; 3] = ["build_call_id", "build_caller", "build_auto_submit"];

#[test]
fn every_edge_out_of_the_fan_in_lifts_the_caller_back_into_the_context() {
    let cfg = hive();
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    let mut interior = 0;
    let mut exits = 0;
    for e in edges.iter().filter(|e| e["from"] == "./weave") {
        let cond = e["condition"].as_str().unwrap_or("");
        if cond.contains("'cstore'") {
            // The bundle on its way INTO the round table is what reads the
            // caller back; there is nothing to restore in front of it.
            continue;
        }
        if e["to"] == "." {
            exits += 1;
            for key in CARRIED_PAST_THE_RIM {
                assert_eq!(
                    e["modifier"]["set_context"][key],
                    Value::String(format!("hop.{key}")),
                    "the exit lane `{cond}` drops {key} on the floor, and it is \
                     read outside this hive"
                );
            }
            let cleared: Vec<String> = e["modifier"]["delete_context"]
                .as_array()
                .map(|l| {
                    l.iter()
                        .filter_map(|k| k.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            for key in ["agent", "build_op", "build_scope"] {
                assert!(
                    cleared.iter().any(|k| k == key),
                    "the exit lane `{cond}` lets {key} out of the hive: it is \
                     interior memory, and context is persistent for the life of \
                     a chain (GH #494 / #499)"
                );
            }
            continue;
        }
        interior += 1;
        for key in CARRIED {
            assert_eq!(
                e["modifier"]["set_context"][key],
                Value::String(format!("hop.{key}")),
                "the lane `{cond}` drops {key} on the floor"
            );
        }
    }
    assert_eq!(
        interior, 3,
        "fire, repair and draft — a fourth interior lane out of the fan-in would \
         need the same modifier and this count is what says so"
    );
    assert_eq!(exits, 1, "give_up is the fan-in's one way out of the hive");
}

/// The cell declares what it emits. An undeclared hop key is a contract the
/// tree does not carry, and `strict_validation` is where that stops being
/// cosmetic.
#[test]
fn the_carried_keys_are_declared_on_both_sides() {
    let cfg: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(WEAVE).expect("weave config"))
            .expect("parses");
    for key in CARRIED {
        assert_eq!(
            cfg["contract"]["emits"]["hop"][key]["type"], "string",
            "{key} travels on the hop and is not declared as emitted"
        );
        assert_eq!(
            cfg["contract"]["consumes"]["context"][key]["type"], "string",
            "{key} is read off the context and is not declared as consumed"
        );
    }
    assert_eq!(
        cfg["contract"]["consumes"]["context"]["orig_request"]["type"], "string",
        "the prose is consumed too — it is what gets written into the row"
    );
    assert!(
        cfg["contract"]["emits"]["hop"]
            .as_object()
            .expect("emitted hop keys")
            .get("orig_request")
            .is_none(),
        "and it is NOT emitted on the hop: a header is not where prose belongs"
    );
}

/// § 2d drift lock. The round table's own description names the roles it holds;
/// the mechanism half asserts that `caller` is a role the shipped fan-in really
/// writes and really reads.
#[test]
fn the_round_table_names_the_role_it_now_holds() {
    let cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(TRANSCRIPT).expect("transcript config"),
    )
    .expect("parses");
    let purpose = cfg["description"]["purpose"]
        .as_str()
        .expect("the round table says what a row is");
    assert!(
        purpose.contains("`caller`"),
        "a role the table holds and its own description does not name is prose \
         that outlived its mechanism: {purpose}"
    );
    let script = std::fs::read_to_string(WEAVE).expect("weave config");
    assert!(
        script.contains("\\\"role\\\": \\\"caller\\\""),
        "the fan-in writes a caller row, or the sentence is a wish"
    );
    assert!(
        script.contains("r.get(\\\"role\\\") == \\\"caller\\\""),
        "and reads it back, or the row is written into the void"
    );
}
