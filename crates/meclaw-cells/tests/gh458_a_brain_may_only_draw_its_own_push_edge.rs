//! GH #458 — a brain subscribes to its own identity, and the door it opens is
//! the one thing a policy row cannot describe.
//!
//! The `in_pack` lane is the only way anything outside a sealed agent composite
//! writes a durable `system.*` slot into its brain. A brain opens its own by
//! submitting a mutation that draws that edge — `add_edges` with a modifier
//! that re-stamps an arriving message onto `in_pack` — so drawing the edge, not
//! travelling it, is the act that needs a capability.
//!
//! # The split, and why it is where it is
//!
//! `policy`'s `mismatch()` makes four ONE-SIDED comparisons: `requester`,
//! `capability`, `subject`, and per-key `scope_match` against the resource
//! (plus the reserved `scope_prefix` path-prefix key). Not one of them compares
//! two fields of the same request against each other, and `*` only asserts
//! presence. "The edge's target is the subject itself" is therefore not
//! expressible in a rule at any shape.
//!
//! So the halves are checked on different sides:
//!
//! * The **gate** secures the FORM — the source is an affinity hive, the target
//!   is the requester's own hive — and refuses a malformed subscribe on the
//!   spot, without asking anybody. A malformed subscribe is not a permission
//!   question, and the manifest is readable nowhere else: `access` never sees
//!   it, because a broker answer replaces the body it travelled in.
//! * The **broker** answers the capability question and only that:
//!   `affinity.subscribe`, may this identity subscribe at all.
//!
//! Neither half is safe alone. A capability granted for "any `in_pack` edge"
//! with no form check would let one agent open a door into ANOTHER agent's
//! prompt, which is the failure this file exists to make impossible.
//!
//! The target comparison is a PATH-SEGMENT prefix and the `/os` vs `/oscar`
//! trap of `gh435_the_broker_compares_paths` is measured here in its own
//! spelling: `/…/members/ale` is a string prefix of `/…/members/alex/talky` and
//! is not a path prefix of it.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, run_shipped_script, shipped_script};

const GATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);
const SEED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/access/store/seed/policy.jsonl"
);

/// The brain, as the substrate stamps it onto the envelope it emitted.
const REQUESTER: &str = "/os/orgs/acme/members/alex/talky";
/// The affinity hive next to it. The HIVE path: `affinity`'s `params.ports` is
/// empty, so an edge naming `./push` is refused with `hive_port_boundary`.
const AFFINITY: &str = "/os/orgs/acme/members/alex/affinity";

fn digest_of(decls: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    String::from_utf8(run_shipped_script(program, &decls.to_string()).stdout).expect("hex")
}

fn op_of(msg: &Value) -> Value {
    meclaw_core::serde_json::from_str(msg["messages"][0]["text"].as_str().expect("a tool_call"))
        .expect("the args are json")
}

/// One subscribe declaration: an `in_pack` edge from `from` to `to`.
///
/// A `set_hop` value is a CEL expression, so the literal carries its own
/// quotes — the spelling every shipped edge uses.
fn subscribe_manifest(from: &str, to: &str) -> Value {
    json!([{
        "scope": "/os/orgs/acme", "ctx": {},
        "diff": { "add_edges": [{
            "from": from,
            "to": to,
            "condition": "has(hop.subscriber) && hop.subscriber == 'sub:alex-self'",
            "modifier": { "set_hop": { "route": "'in_pack'" } }
        }] }
    }])
}

/// The store's answer to an un-parking `select`, in one of the three phases.
fn unpark(phase: &str, decls: &Value) -> Vec<Value> {
    let sha = digest_of(decls);
    let rows = json!([{ "id": "p1", "manifest": decls, "requester": REQUESTER,
                        "tool_call_id": "op:c1", "manifest_sha256": sha }]);
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "header": {
                "hop": { "operation": "select", "rows_affected": 1 },
                "context": { "sub_origin": "gate", "sub_phase": phase,
                             "sub_carry": "{\"status\":\"allowed\"}" }
            },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "x",
                           "text": rows.to_string() }],
            "params": {}
        }),
    )
}

/// A verdict on one named capability, on the lane the shell re-stamps it onto.
fn verdict(capability: &str, status: &str, sha: &str) -> Vec<Value> {
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "header": { "hop": { "route": "in_verdict" },
                        "context": { "sub_ask": "1", "sub_sha": sha } },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "q1",
                "text": json!({ "status": status, "capability": capability,
                                "reason_code": "" }).to_string() }],
            "params": {}
        }),
    )
}

/// Nothing on the `ask` lane: the broker was never consulted.
fn asked_nothing(out: &[Value]) -> bool {
    out.iter().all(|m| m["header"]["route"] != json!("ask"))
}

// ── the rule is on: the question is asked and the manifest reaches the door ──

#[test]
fn a_brain_that_draws_its_own_push_edge_is_asked_about_and_let_through() {
    let decls = subscribe_manifest(AFFINITY, REQUESTER);
    let sha = digest_of(&decls);

    // 1. The third question, and nothing else. The row stays parked: one row
    //    and one digest correlate every answer over this manifest.
    let asked = unpark("parked", &decls);
    assert_eq!(asked.len(), 1, "one more question, and nothing else");
    assert_eq!(asked[0]["header"]["route"], "ask");
    let args = op_of(&asked[0]);
    assert_eq!(args["capability"], "affinity.subscribe");
    assert_eq!(args["check_only"], true);
    // R-AC-1: the identity the substrate stamped travels as `subject`.
    assert_eq!(args["subject"], REQUESTER);
    assert!(args.get("requester").is_none());
    assert_eq!(args["resource"]["scope"], "/os/orgs/acme");
    assert_eq!(args["resource"]["actions"], json!(["apply"]));
    assert_eq!(asked[0]["header"]["manifest_sha256"], sha.as_str());

    // 2. The verdict comes back allowed, and un-parks into a phase of its own.
    let allowed = verdict("affinity.subscribe", "allowed", &sha);
    assert_eq!(allowed.len(), 1);
    assert_eq!(op_of(&allowed[0])["operation"], "select");
    assert_eq!(allowed[0]["header"]["phase"], "subscribing");

    // 3. That phase submits unconditionally — every question has been answered,
    //    and asking again would be a loop.
    let done = unpark("subscribing", &decls);
    assert_eq!(
        done.len(),
        3,
        "forget the park, remember the flight, submit"
    );
    assert_eq!(done[2]["header"]["route"], "mutate");
    assert_eq!(done[2]["manifest"][0]["ctx"]["requester"], REQUESTER);
    assert_eq!(done[2]["header"]["manifest_sha256"], sha.as_str());
}

#[test]
fn the_edge_may_end_at_the_hive_the_brain_lives_in() {
    // The requester is a cell inside its own sealed composite; the edge ends at
    // the composite. That is a path-segment prefix and it is the shape the
    // shipped subscribe mutation draws.
    let decls = subscribe_manifest(AFFINITY, "/os/orgs/acme/members/alex");
    let out = unpark("parked", &decls);
    assert_eq!(out.len(), 1);
    assert_eq!(op_of(&out[0])["capability"], "affinity.subscribe");
}

// ── an operator who switches the rule off: a denial with a name of its own ───

/// The rule SHIPS ON since ruling R-Policy-Default (2026-08-28) — see
/// [`the_broker_grants_the_capability_on_a_fresh_tree`] — so this is the case
/// where an operator deliberately took it away again. The denial keeps its own
/// name either way: a brain refused its identity door is not the same fact as a
/// brain refused the door altogether.
#[test]
fn a_switched_off_rule_refuses_the_same_manifest_under_its_own_code() {
    // A disabled rule is a MISSING rule to the broker: it answers `denied` with
    // `capability_unknown`, because a missing rule is a denial rather than a
    // silence. The gate reads WHICH question that answers off the answer.
    let out = verdict("affinity.subscribe", "denied", "abc123");
    assert_eq!(out.len(), 2, "forget the park, then refuse");
    assert_eq!(out[1]["header"]["route"], "receipt");
    assert_eq!(out[1]["header"]["error_code"], "subscribe_not_permitted");
    assert!(
        out.iter().all(|m| m["header"]["route"] != json!("mutate")),
        "nothing reaches the door"
    );
    let del = op_of(&out[0]);
    assert_eq!(del["operation"], "delete");
    assert_eq!(
        del["where"],
        json!({ "kind": "parked", "manifest_sha256": "abc123" })
    );

    // The other two refusals keep their own names: three noes, three codes.
    assert_eq!(
        verdict("code.author", "denied", "abc123")[1]["header"]["error_code"],
        "code_author_denied"
    );
    assert_eq!(
        verdict("colony.mutate", "denied", "abc123")[1]["header"]["error_code"],
        "requester_not_permitted"
    );
}

/// The seeded rule, and that it is one of the two a colony cannot start without.
///
/// Ruling R-Policy-Default (2026-08-28): an agent that cannot subscribe has no
/// identity, so a default that refused would leave every shipped brain silent
/// until somebody remembered an `UPDATE policy`. What keeps that honest is the
/// shape of the row rather than the switch — requester `/os/operator/submit`, action
/// `apply`, scoped to `/os/orgs`, verdict `allow`, no `cred_ref` — and the fact
/// that the gate, not this row, holds the edge to the requester's own hive.
#[test]
fn the_broker_grants_the_capability_on_a_fresh_tree() {
    let raw = std::fs::read_to_string(SEED).expect("policy seed");
    let rows: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str(l).expect("a jsonl row"))
        .collect();
    let row = rows
        .iter()
        .find(|r| r["capability"] == "affinity.subscribe")
        .expect("a seeded affinity.subscribe rule");
    assert_eq!(
        row["enabled"], 1,
        "a fresh colony must be able to register its brains for their own identity \
         (R-Policy-Default); the bound is the row's SHAPE and the gate's form check, \
         not the switch"
    );
    assert_eq!(
        row["requester"], "/os/operator/submit",
        "edge-borne, R-AC-1"
    );
    assert_eq!(row["subject"], "*");
    assert_eq!(row["scope_match"]["scope_prefix"], "/os/orgs");
    assert_eq!(row["scope_match"]["actions"], json!(["apply"]));
    assert_eq!(row["verdict"], "allow");
    assert_eq!(row["cred_ref"], "", "a verdict, never a credential");
}

// ── the form, refused here and never asked about ─────────────────────────────

#[test]
fn an_edge_into_somebody_elses_hive_is_refused_without_asking_anybody() {
    for foreign in [
        // A different member entirely.
        "/os/orgs/acme/members/dana/talky",
        // The string-prefix trap, in the spelling of `/os` vs `/oscar`:
        // `/…/members/ale` IS a string prefix of `/…/members/alex/talky` and is
        // NOT a path prefix of it. A naive `startswith` hands this brain the
        // identity door of a hive nobody named.
        "/os/orgs/acme/members/ale",
        // And the same trap one segment out.
        "/os/orgs/acme/members/alexander/talky",
        // A sibling that merely shares a parent.
        "/os/orgs/acme/members/alex/affinity",
    ] {
        let decls = subscribe_manifest(AFFINITY, foreign);
        let out = unpark("parked", &decls);
        assert_eq!(out.len(), 2, "forget the park, then refuse: {foreign}");
        assert_eq!(
            out[1]["header"]["error_code"], "subscribe_target_not_self",
            "`{foreign}` is not the requester's own hive"
        );
        assert!(
            asked_nothing(&out),
            "a malformed subscribe is not a permission question — \
             the broker must never be asked about `{foreign}`"
        );
        assert!(
            out.iter().all(|m| m["header"]["route"] != json!("mutate")),
            "nothing reaches the door for `{foreign}`"
        );
        assert_eq!(op_of(&out[0])["operation"], "delete");
    }
}

#[test]
fn an_edge_from_anything_but_an_affinity_hive_is_refused_without_asking_anybody() {
    for source in [
        "/os/orgs/acme/members/alex/collector",
        // The port, not the hive. `affinity`'s `params.ports` is empty, so this
        // edge is refused at the door with `hive_port_boundary` anyway —
        // demanding it here would demand a spelling that can never be applied.
        "/os/orgs/acme/members/alex/affinity/push",
        // A name that merely ends in the right letters.
        "/os/orgs/acme/members/alex/my-affinity",
        "/os/orgs/beta/members/carol/affinity/../affinity-b",
    ] {
        let decls = subscribe_manifest(source, REQUESTER);
        let out = unpark("parked", &decls);
        assert_eq!(out.len(), 2, "forget the park, then refuse: {source}");
        assert_eq!(
            out[1]["header"]["error_code"], "subscribe_source_not_affinity",
            "`{source}` is not an affinity hive"
        );
        assert!(
            asked_nothing(&out),
            "the broker is not asked about {source}"
        );
    }
}

// ── the derivation, and the order of the three questions ─────────────────────

#[test]
fn an_ordinary_manifest_asks_no_third_question() {
    let decls = json!([{
        "scope": "/os/orgs/acme", "ctx": {},
        "diff": { "add_edges": [{
            "from": "/os/orgs/acme/members/alex/collector",
            "to": REQUESTER,
            "modifier": { "set_hop": { "route": "'in_turn'" } }
        }] }
    }]);
    let out = unpark("parked", &decls);
    assert_eq!(
        out.len(),
        3,
        "no question: nothing here opens an identity door"
    );
    assert_eq!(out[2]["header"]["route"], "mutate");
}

#[test]
fn the_bare_spelling_of_the_lane_counts_too() {
    // A derivation that saw only the quoted CEL literal would be a check an
    // author can step around by typing.
    let decls = json!([{
        "scope": "/os/orgs/acme", "ctx": {},
        "diff": { "add_edges": [{
            "from": AFFINITY, "to": REQUESTER,
            "modifier": { "set_hop": { "route": "in_pack" } }
        }] }
    }]);
    let out = unpark("parked", &decls);
    assert_eq!(out.len(), 1);
    assert_eq!(op_of(&out[0])["capability"], "affinity.subscribe");
}

#[test]
fn a_manifest_that_both_authors_code_and_subscribes_answers_three_questions() {
    // ONE parked row, ONE digest, three sequential rounds. The order is fixed:
    // `code.author` first, `affinity.subscribe` last, and the store phase name
    // is the whole of the correlation.
    let decls = json!([{
        "scope": "/os/orgs/acme", "ctx": {},
        "diff": {
            "add_nodes": [{ "name": "helper", "template": "terminal",
                            "override_params": { "script_inline": "print(1)" } }],
            "add_edges": [{ "from": AFFINITY, "to": REQUESTER,
                            "modifier": { "set_hop": { "route": "'in_pack'" } } }]
        }
    }]);

    let first = unpark("parked", &decls);
    assert_eq!(first.len(), 1);
    assert_eq!(op_of(&first[0])["capability"], "code.author");

    let second = unpark("authored", &decls);
    assert_eq!(second.len(), 1);
    assert_eq!(op_of(&second[0])["capability"], "affinity.subscribe");

    let third = unpark("subscribing", &decls);
    assert_eq!(third.len(), 3);
    assert_eq!(third[2]["header"]["route"], "mutate");
}

#[test]
fn the_gate_still_carries_no_policy_of_its_own() {
    let gate: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(GATE).expect("gate"))
            .expect("json");
    assert!(
        gate["params"].get("policy").is_none(),
        "it derives a QUESTION from the diff and checks a FORM; it grants nothing"
    );
    let script = gate["params"]["script_inline"].as_str().expect("script");
    for word in [
        "affinity.subscribe",
        "in_pack",
        "subscribe_target_not_self",
        "subscribe_source_not_affinity",
        "subscribe_not_permitted",
    ] {
        assert!(script.contains(word), "the gate must name {word}");
    }
}
