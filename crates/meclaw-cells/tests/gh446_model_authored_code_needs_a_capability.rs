//! GH #446 — model-authored code enters through a door nobody was watching, and
//! `code.author` is the name that door never had.
//!
//! The prohibition lived in one line of the drafting prompt
//! (`templates/builder/brief`, "no bare cell type"). Nothing enforced it: the
//! normaliser does not inspect the inner `add_nodes` keys, the fast lane passes
//! `override_params` through verbatim, the submitter's gate checked only the
//! digest and the scope root, and the mutation door checks that a param key
//! EXISTS, never what it contains. Five shipped `code` templates take
//! `script_inline` as a param, and `add_templates` registers a whole template
//! class with an arbitrary one.
//!
//! So the need is derived from the DIFF and asked as a second, check-only
//! question. Denied by default, because a missing rule is a denial rather than
//! a silence — and a manifest with no script asks nothing extra at all.

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
const REQUESTER: &str = "/os/operator/submit";

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

/// A manifest that hands a `code` cell a body nobody reviewed.
fn manifest_with_a_script() -> Value {
    json!([{
        "scope": "/os/orgs/acme", "ctx": {},
        "diff": { "add_nodes": [{
            "name": "helper", "template": "terminal",
            "override_params": { "script_inline": "import sys\nsys.stdout.write('[]')\n" }
        }] }
    }])
}

/// The same act one level up: a whole template class with an arbitrary script.
fn manifest_with_a_template() -> Value {
    json!([{
        "scope": "/os/orgs/acme", "ctx": {},
        "diff": { "add_templates": [{ "name": "mine", "version": "1.0.0" }] }
    }])
}

/// `override_params` is addressed PER CELL, so the key sits one level down.
fn manifest_with_a_nested_script() -> Value {
    json!([{
        "scope": "/os/orgs/acme", "ctx": {},
        "diff": { "add_nodes": [{
            "name": "unit", "template": "retry",
            "override_params": { "gate": { "script_inline": "print(1)" } }
        }] }
    }])
}

fn plain_manifest() -> Value {
    json!([{ "scope": "/os/orgs/acme", "ctx": {}, "diff": { "add_edges": [] } }])
}

/// Phase A: the submission arrives, is parked, and the first question goes out.
fn submit(decls: &Value) -> Vec<Value> {
    let sha = digest_of(decls);
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "reply_to": REQUESTER,
            "header": { "hop": { "route": "in_apply", "manifest_sha256": sha,
                                 "tool_call_id": "op:c1" }, "context": {} },
            "ttl": 64,
            "manifest": decls,
            "messages": [],
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

/// The store's answer to an un-parking `select`, in one of the two phases.
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

// ── the need is derived, not declared ────────────────────────────────────────

#[test]
fn a_manifest_without_a_script_asks_no_second_question() {
    let decls = plain_manifest();
    // Phase A is unchanged: park, then ONE question.
    let a = submit(&decls);
    assert_eq!(a.len(), 2, "park, then ask — no third message");
    assert_eq!(op_of(&a[1])["capability"], "colony.mutate");

    // And the un-parking goes straight through to the door.
    let out = unpark("parked", &decls);
    assert_eq!(out.len(), 3, "forget the park, remember the flight, submit");
    assert_eq!(out[2]["header"]["route"], "mutate");
}

#[test]
fn a_manifest_that_authors_code_is_asked_about_before_anything_is_unparked() {
    for decls in [
        manifest_with_a_script(),
        manifest_with_a_template(),
        manifest_with_a_nested_script(),
    ] {
        let out = unpark("parked", &decls);
        assert_eq!(out.len(), 1, "one more question, and nothing else");
        assert_eq!(out[0]["header"]["route"], "ask");
        assert!(
            out.iter().all(|m| m["header"]["route"] != json!("mutate")),
            "nothing reaches the door while a question about it is open"
        );
        let args = op_of(&out[0]);
        assert_eq!(args["capability"], "code.author");
        assert_eq!(args["check_only"], true);
        // Same subject, same scope root: it is the same manifest, asked about
        // twice. R-AC-1 — the substrate's identity travels as `subject`.
        assert_eq!(args["subject"], REQUESTER);
        assert_eq!(args["resource"]["scope"], "/os/orgs/acme");
        assert_eq!(args["resource"]["actions"], json!(["apply"]));
        assert!(args.get("requester").is_none());
        // The row is left where it is: nothing is un-parked and nothing is
        // deleted, so one row and one digest correlate both answers.
        assert_eq!(
            out[0]["header"]["manifest_sha256"],
            digest_of(&decls).as_str()
        );
    }
}

#[test]
fn the_derivation_does_not_fire_on_a_params_key_that_merely_looks_like_one() {
    let decls = json!([{
        "scope": "/os/orgs/acme", "ctx": {},
        "diff": { "add_nodes": [{
            "name": "unit", "template": "terminal",
            "override_params": { "description": "a script of sorts", "port": 7810 }
        }] }
    }]);
    let out = unpark("parked", &decls);
    assert_eq!(out.len(), 3, "no second question: nothing here is a script");
    assert_eq!(out[2]["header"]["route"], "mutate");
}

// ── the verdict, and a refusal class of its own ──────────────────────────────

#[test]
fn a_missing_rule_denies_and_says_which_question_it_answered() {
    // The broker answers `denied` with `capability_unknown` while the seeded
    // row is disabled. The gate reads WHICH capability off the answer, so the
    // refusal cannot be confused with "you may not submit at all".
    let out = verdict("code.author", "denied", "abc123");
    assert_eq!(out.len(), 2, "forget the park, then refuse");
    assert_eq!(out[1]["header"]["route"], "receipt");
    assert_eq!(out[1]["header"]["error_code"], "code_author_denied");
    let del = op_of(&out[0]);
    assert_eq!(del["operation"], "delete");
    assert_eq!(
        del["where"],
        json!({ "kind": "parked", "manifest_sha256": "abc123" })
    );
}

#[test]
fn the_submission_refusal_keeps_the_name_it_always_had() {
    let out = verdict("colony.mutate", "denied", "abc123");
    assert_eq!(out[1]["header"]["error_code"], "requester_not_permitted");
    // A verdict with no capability on it at all is the question this cell has
    // always asked — the older broker answers stay readable.
    let bare = emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "header": { "hop": { "route": "in_verdict" },
                        "context": { "sub_ask": "1", "sub_sha": "abc123" } },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "q1",
                           "text": "{\"status\": \"denied\"}" }],
            "params": {}
        }),
    );
    assert_eq!(bare[1]["header"]["error_code"], "requester_not_permitted");
}

#[test]
fn an_enabled_rule_lets_the_same_manifest_through() {
    // The second `allowed` verdict un-parks into the OTHER phase, and that
    // phase submits unconditionally — every question over this manifest has
    // been answered, so asking again would be a loop.
    let out = verdict("code.author", "allowed", "abc123");
    assert_eq!(out.len(), 1);
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(out[0]["header"]["phase"], "authored");

    let decls = manifest_with_a_script();
    let done = unpark("authored", &decls);
    assert_eq!(
        done.len(),
        3,
        "forget the park, remember the flight, submit"
    );
    assert_eq!(done[2]["header"]["route"], "mutate");
    assert_eq!(done[2]["manifest"][0]["ctx"]["requester"], REQUESTER);
}

#[test]
fn the_first_verdict_still_lands_where_it_always_did() {
    let out = verdict("colony.mutate", "allowed", "abc123");
    assert_eq!(out[0]["header"]["phase"], "parked");
}

// ── the row ships, and it ships disabled ─────────────────────────────────────

#[test]
fn the_broker_seeds_the_capability_and_grants_it_to_nobody() {
    let raw = std::fs::read_to_string(SEED).expect("policy seed");
    let rows: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str(l).expect("a jsonl row"))
        .collect();
    let row = rows
        .iter()
        .find(|r| r["capability"] == "code.author")
        .expect("a seeded code.author rule");
    assert_eq!(row["enabled"], 0, "a fresh colony authors no code");
    assert_eq!(row["requester"], "/os/submit", "edge-borne, R-AC-1");
    assert_eq!(row["subject"], "*");
    assert_eq!(row["scope_match"]["scope_prefix"], "/os/orgs");
    assert_eq!(row["scope_match"]["actions"], json!(["apply"]));
    assert_eq!(row["verdict"], "allow");
    assert_eq!(row["cred_ref"], "", "a verdict, never a credential");
    // Ruling R-Policy-Default (2026-08-28) retired "every seeded row ships
    // disabled" for exactly two rows — `colony.mutate.default`, so a fresh OS
    // can build at all, and `affinity.subscribe.default`, so its brains can
    // register for their own identity. `code.author` is deliberately NOT among
    // them, and that is the whole point of this issue: a fresh colony may carry
    // a manifest to the door, and it still may not bring executable behaviour
    // through it. The named on-set is pinned in
    // `gh435_the_broker_ships_its_first_row.rs`; what is asserted here is the
    // line this capability sits on.
    let on: Vec<&str> = rows
        .iter()
        .filter(|r| r.get("capability").is_some() && r["enabled"] == 1)
        .map(|r| r["rule_id"].as_str().unwrap_or_default())
        .collect();
    assert!(
        !on.contains(&"code.author.default"),
        "code.author ships enabled — the default set stops at building and subscribing: {on:?}"
    );
}

#[test]
fn the_gate_still_carries_no_policy_of_its_own() {
    let gate: Value =
        meclaw_core::serde_json::from_str(&std::fs::read_to_string(GATE).expect("gate"))
            .expect("json");
    assert!(
        gate["params"].get("policy").is_none(),
        "it derives a QUESTION from the diff; it decides nothing"
    );
    let script = gate["params"]["script_inline"].as_str().expect("script");
    assert!(
        script.contains("code.author"),
        "the capability is named in the one cell that asks about it"
    );
    for word in [
        "script_inline",
        "script_path",
        "add_templates",
        "swap_nodes",
    ] {
        assert!(script.contains(word), "the derivation must see {word}");
    }
}
