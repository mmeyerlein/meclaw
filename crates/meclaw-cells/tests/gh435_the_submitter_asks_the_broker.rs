//! GH #435 — the submitter does not decide any more. It asks.
//!
//! The decision moves out of `params.policy` and into the capability broker,
//! in the **check-only** form: one question over the manifest's scope ROOT,
//! answered with a verdict and no grant. The manifest itself cannot ride that
//! round trip — `access` answers with a `tool_result` that REPLACES the body —
//! so it waits parked in the store beside the gate, under its own digest.
//!
//! Since GH #556 the round trip crosses TWO rims rather than one: the `submit`
//! hive is an occupant of `operator`, so the question leaves the front door on
//! `ask` before the shell hands it to `./access`, and the verdict is let back in
//! the same way. The pair the shell draws is therefore `./operator -> ./access`
//! and `./access -> ./operator`, and the requester it promotes is the path of
//! the occupant the rule is about — `/os/operator/submit`.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, run_shipped_script, shipped_script};

const HIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/config.json"
);
const GATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);
const OS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/meclaw-os/config.json"
);
const OPERATOR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/operator/config.json"
);
const REQUESTER: &str = "/os/orgs/acme/members/alex/assistants/scribe/tools/build-apply";

fn read(path: &str) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(path).expect(path)).expect(path)
}

/// A shipped template file, addressed relative to the repository root.
fn shipped(rel: &str) -> Value {
    read(&format!("{}/../../{rel}", env!("CARGO_MANIFEST_DIR")))
}

/// The canonical digest of a declaration list, drawn by the same two lines the
/// shipped helper draws it with.
fn digest_of(decls: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    String::from_utf8(run_shipped_script(program, &decls.to_string()).stdout).expect("hex")
}

/// The one operation a store message carries, parsed out of its `tool_call`.
fn op_of(msg: &Value) -> Value {
    meclaw_core::serde_json::from_str(msg["messages"][0]["text"].as_str().expect("a tool_call"))
        .expect("the args are json")
}

fn two_declarations() -> Value {
    json!([
        { "scope": "/os/orgs/acme", "ctx": {}, "diff": { "add_edges": [] } },
        { "scope": "/os/orgs/acme/members", "ctx": {}, "diff": { "add_edges": [] } }
    ])
}

/// Phase A: a submission arriving on `in_apply`.
fn submit(decls: &Value, claimed: &str, reply_to: &str) -> Vec<Value> {
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/operator/submit",
            "reply_to": reply_to,
            "header": { "hop": { "route": "in_apply", "manifest_sha256": claimed,
                                 "tool_call_id": "c2" }, "context": {} },
            "ttl": 64,
            "manifest": decls,
            "messages": [{ "origin": "assistant", "type": "tool_call",
                           "id": "c2", "text": "{}" }],
            "params": {}
        }),
    )
}

/// The broker's answer, arriving on the lane the shell re-stamps it onto.
fn verdict(status: &str, sha: &str, readable: bool, store_error: bool) -> Vec<Value> {
    let text = if readable {
        json!({ "status": status, "capability": "colony.mutate", "reason_code": "" }).to_string()
    } else {
        "not json at all".to_string()
    };
    let mut hop = json!({ "route": "in_verdict" });
    if store_error {
        hop["error_code"] = json!("sql_error");
    }
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/operator/submit",
            "header": { "hop": hop,
                        "context": { "sub_ask": "1", "sub_sha": sha } },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "q1",
                           "text": text }],
            "params": {}
        }),
    )
}

/// The store's answer to the un-parking `select`.
fn unpark(rows: &Value) -> Vec<Value> {
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/operator/submit",
            "header": {
                "hop": { "operation": "select", "rows_affected": 1 },
                "context": { "sub_origin": "gate", "sub_phase": "parked",
                             "sub_carry": "{\"status\":\"allowed\"}" }
            },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "x",
                           "text": rows.to_string() }],
            "params": {}
        }),
    )
}

// ── T14 — the submitter parks and asks ───────────────────────────────────────

#[test]
fn phase_a_parks_the_manifest_and_asks_once() {
    let decls = two_declarations();
    let sha = digest_of(&decls);
    let out = submit(&decls, &sha, REQUESTER);
    assert_eq!(out.len(), 2, "park, then ask");

    let park = op_of(&out[0]);
    assert_eq!(park["operation"], "insert");
    assert_eq!(park["table"], "submissions");
    assert_eq!(park["row"]["kind"], "parked");
    assert_eq!(park["row"]["manifest_sha256"], sha.as_str());
    assert_eq!(
        park["row"]["manifest"], decls,
        "the json column keeps the list"
    );
    assert_eq!(park["row"]["requester"], REQUESTER);
    assert_eq!(park["row"]["tool_call_id"], "c2");

    let h = &out[1]["header"];
    assert_eq!(h["route"], "ask");
    assert_eq!(h["manifest_sha256"], sha.as_str());
    let args = op_of(&out[1]);
    assert_eq!(args["capability"], "colony.mutate");
    assert_eq!(args["check_only"], true);
    // R-AC-1: the identity the SUBSTRATE stamped travels as `subject`. It is
    // never claimed as `requester` -- that word belongs to the edge.
    assert_eq!(args["subject"], REQUESTER);
    assert!(
        args.get("requester").is_none(),
        "the requester is the edge's word, never the body's"
    );
    // ONE question, over the manifest's scope ROOT. If the root is under P,
    // every declaration is under P, because every declaration is under the
    // root. A manifest that straddles two branches asks for their join --
    // stricter, never more permissive.
    assert_eq!(args["resource"]["scope"], "/os/orgs/acme");
    assert_eq!(args["resource"]["actions"], json!(["apply"]));
}

#[test]
fn the_scope_root_of_two_branches_is_their_join() {
    for (scopes, root) in [
        (json!(["/os/orgs/acme", "/os/orgs/beta"]), "/os/orgs"),
        (json!(["/a", "/b"]), "/"),
        (json!(["/os/orgs/acme", "/os/orgs/acme"]), "/os/orgs/acme"),
        // A sibling whose NAME merely starts with the other's: the join is the
        // parent, never the longer string. `/oscar` is not under `/os`.
        (json!(["/os", "/oscar"]), "/"),
    ] {
        let decls: Value = scopes
            .as_array()
            .expect("scopes")
            .iter()
            .map(|s| json!({ "scope": s, "ctx": {}, "diff": { "add_edges": [] } }))
            .collect::<Vec<_>>()
            .into();
        let sha = digest_of(&decls);
        let out = submit(&decls, &sha, REQUESTER);
        assert_eq!(op_of(&out[1])["resource"]["scope"], root, "{scopes:?}");
    }
}

#[test]
fn a_declaration_without_a_scope_makes_the_root_the_colony() {
    // Fail closed: an empty scope cannot narrow anything, so it widens the
    // question to the root and lets the broker refuse it.
    let decls = json!([
        { "scope": "/os/orgs/acme", "ctx": {}, "diff": { "add_edges": [] } },
        { "ctx": {}, "diff": { "add_edges": [] } }
    ]);
    let sha = digest_of(&decls);
    let out = submit(&decls, &sha, REQUESTER);
    assert_eq!(op_of(&out[1])["resource"]["scope"], "/");
}

#[test]
fn a_refusal_still_parks_nothing() {
    // The three checks that survive the move all refuse before the park. A
    // parked row nobody un-parks is a manifest waiting forever under a digest.
    let decls = two_declarations();
    let sha = digest_of(&decls);
    for (claimed, reply_to, code) in [
        ("deadbeef", REQUESTER, "manifest_digest_mismatch"),
        (sha.as_str(), "", "requester_unknown"),
    ] {
        let out = submit(&decls, claimed, reply_to);
        assert_eq!(out.len(), 1, "{code}: a refusal is one message");
        assert_eq!(out[0]["header"]["route"], "receipt");
        assert_eq!(out[0]["header"]["error_code"], code);
    }
}

// ── T15 — the verdict decides, the refusal keeps its name ────────────────────

#[test]
fn an_allowed_verdict_unparks_by_digest() {
    let out = verdict("allowed", "abc123", true, false);
    assert_eq!(out.len(), 1);
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "submissions");
    assert_eq!(
        op["where"],
        json!({ "kind": "parked", "manifest_sha256": "abc123" })
    );
    assert_eq!(out[0]["header"]["phase"], "parked");
}

#[test]
fn the_unparked_manifest_is_submitted_with_its_attribution() {
    let decls = two_declarations();
    let sha = digest_of(&decls);
    let rows = json!([{ "id": "p1", "manifest": decls, "requester": REQUESTER,
                        "tool_call_id": "c2", "manifest_sha256": sha }]);
    let out = unpark(&rows);
    assert_eq!(out.len(), 3, "forget the park, remember the flight, submit");

    let del = op_of(&out[0]);
    assert_eq!(del["operation"], "delete");
    assert_eq!(del["where"], json!({ "id": "p1" }));

    let flight = op_of(&out[1]);
    assert_eq!(flight["operation"], "insert");
    assert_eq!(flight["row"]["kind"], "flight");
    assert_eq!(flight["row"]["tool_call_id"], "c2");
    assert_eq!(flight["row"]["manifest_sha256"], sha.as_str());

    assert_eq!(out[2]["header"]["route"], "mutate");
    assert_eq!(out[2]["header"]["manifest_sha256"], sha.as_str());
    assert_eq!(out[2]["manifest"][0]["ctx"]["requester"], REQUESTER);
    assert_eq!(
        out[2]["manifest"][0]["ctx"]["manifest_sha256"],
        sha.as_str()
    );
    assert_eq!(out[2]["manifest"][1]["ctx"]["requester"], REQUESTER);
}

#[test]
fn a_denied_verdict_refuses_in_the_form_it_always_had() {
    // `requester_not_permitted` is the string the template has always used. A
    // new one here would break every caller that greps for the old one.
    let out = verdict("denied", "abc123", true, false);
    assert_eq!(out.len(), 2, "forget the park, then refuse");
    let del = op_of(&out[0]);
    assert_eq!(del["operation"], "delete");
    assert_eq!(
        del["where"],
        json!({ "kind": "parked", "manifest_sha256": "abc123" })
    );
    assert_eq!(out[1]["header"]["route"], "receipt");
    assert_eq!(out[1]["header"]["error_code"], "requester_not_permitted");
}

#[test]
fn a_broker_that_does_not_answer_a_verdict_refuses_rather_than_submits() {
    // An answer without a readable status, or a store error on the way back:
    // `submission_check_failed`, nothing on `mutate`. Fail closed -- the ONE
    // cell with an edge onto the mutation door does not guess.
    for (readable, store_error) in [(false, false), (true, true)] {
        let out = verdict("allowed", "abc123", readable, store_error);
        assert!(
            out.iter().all(|m| m["header"]["route"] != "mutate"),
            "nothing reaches the door"
        );
        let last = out.last().expect("an answer");
        assert_eq!(last["header"]["route"], "receipt");
        assert_eq!(last["header"]["error_code"], "submission_check_failed");
    }
}

#[test]
fn an_unparked_nothing_says_so_rather_than_falling_silent() {
    let out = unpark(&json!([]));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "receipt");
    assert_eq!(out[0]["header"]["error_code"], "submission_check_failed");
}

// ── T16 — the contract catches up with the behaviour ─────────────────────────

#[test]
fn the_hive_declares_the_broker_lanes_and_requires_the_drain() {
    let hive = read(HIVE);
    let accepts = hive["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts");
    assert!(accepts.iter().any(|a| a["route"] == "in_verdict"));
    let emits = hive["params"]["contract"]["emits"]
        .as_array()
        .expect("emits");
    assert!(emits.iter().any(|e| e["route"] == "ask"));
    let drains = hive["params"]["required_drains"]
        .as_array()
        .expect("required_drains");
    // A composition that wires the question and not the answer has arranged for
    // every submission to hang. The pair is one decision.
    assert!(
        drains
            .iter()
            .any(|d| d["accepts"] == "in_verdict" && d["emits"] == "ask")
    );
}

#[test]
fn the_gate_no_longer_carries_a_policy_of_its_own() {
    let gate = read(GATE);
    assert!(
        gate["params"].get("policy").is_none(),
        "the decision moved; two sources in the one cell with the mutation edge \
         is an audit trail that cannot say who decided"
    );
    let values = gate["contract"]["emits"]["hop"]["route"]["values"]
        .as_array()
        .expect("route values");
    assert!(values.iter().any(|v| v == "ask"));
}

// ── T17 — the shell wires submitter and broker ───────────────────────────────

fn os_edges() -> Vec<Value> {
    read(OS)["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

#[test]
fn the_shell_wires_the_submitter_to_the_broker_and_back() {
    let edges = os_edges();
    let ask = edges
        .iter()
        .find(|e| e["from"] == "./operator" && e["to"] == "./access")
        .expect("operator -> access");
    assert_eq!(ask["condition"], "has(hop.route) && hop.route == 'ask'");
    // The broker's own door lane, set by the COLONY on the edge -- and the
    // requester with it. R-AC-1 lives here.
    assert_eq!(ask["modifier"]["set_hop"]["route"], "'in_request'");
    // The path promoted is the OCCUPANT the rule is about, not the hive that
    // relays for it: since GH #556 the submitter stands inside the front door,
    // and a rule written for `/os/operator` would be a rule about every lane
    // that hive has rather than about the one node with the mutation edge.
    assert_eq!(
        ask["modifier"]["set_context"]["requester"], "'/os/operator/submit'",
        "the broker reads the requester from context and nowhere else"
    );
    // A marker in the submitter's OWN key space: `access_origin` and `ac_*` are
    // overwritten on the broker's internal edges.
    assert_eq!(ask["modifier"]["set_context"]["sub_ask"], "'1'");
    // `hop.*` lives for exactly one hop, so the digest the answer has to be
    // matched against is promoted to context here or it is gone.
    assert_eq!(
        ask["modifier"]["set_context"]["sub_sha"],
        "hop.manifest_sha256"
    );

    let back = edges
        .iter()
        .find(|e| e["from"] == "./access" && e["to"] == "./operator")
        .expect("access -> operator");
    assert_eq!(
        back["condition"],
        "context.sub_ask == '1' && has(hop.route) && hop.route == 'grant'"
    );
    assert_eq!(back["modifier"]["set_hop"]["route"], "'in_verdict'");

    // …and the half the shell cannot draw, because the submitter is not its
    // occupant any more: the question has to LEAVE the front door and the
    // answer has to be let back in. Without this pair the two edges above are
    // wired to a hive that neither raises `ask` nor forwards `in_verdict`.
    let inside = read(OPERATOR)["params"]["graph"]["edges"].clone();
    let inside = inside.as_array().expect("the operator's edges").clone();
    assert!(
        inside.iter().any(|e| e["from"] == "./submit"
            && e["to"] == "."
            && e["condition"] == "has(hop.route) && hop.route == 'ask'"),
        "the submitter's question crosses the front door's rim"
    );
    assert!(
        inside.iter().any(|e| e["from"] == "."
            && e["to"] == "./submit"
            && e["condition"] == "has(hop.route) && hop.route == 'in_verdict'"),
        "and the verdict is handed back to the occupant that asked"
    );
}

#[test]
fn a_verdict_for_the_submitter_does_not_also_leave_the_shell() {
    // Edges FAN OUT: every matching out-edge fires. Without a guard on the rim
    // edge, one check-only question would answer the submitter AND hand a grant
    // to whoever wired the shell's `grant` lane -- an answer to a question the
    // outside never asked.
    let edges = os_edges();
    let rim = edges
        .iter()
        .find(|e| {
            e["from"] == "./access"
                && e["to"] == "."
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'grant'"))
        })
        .expect("access -> . on grant");
    let cond = rim["condition"].as_str().expect("a condition");
    assert!(
        cond.contains("!has(context.sub_ask)"),
        "the rim edge must exclude the submitter's own round: {cond}"
    );
}

#[test]
fn every_ref_on_the_road_to_the_broker_is_pinned() {
    // The versions are DERIVED, not typed (`docs/development-rules.md` § 2d): a
    // version written here as a literal goes stale on the next bump of either
    // template and turns a correct tree red — which it did, on `submit@2.3.0`.
    //
    // Three refs since GH #556, not two: the pair the shell draws is
    // `operator`/`access`, and the submitter the pair exists for is a ref one
    // storey further in, inside the front door. A bare name anywhere on that
    // road resolves to whatever is newest on disk.
    for (holder, name) in [
        ("templates/meclaw-os", "access"),
        ("templates/meclaw-os", "operator"),
        ("templates/operator", "submit"),
    ] {
        let want = format!(
            "{name}@{}",
            shipped(&format!("templates/{name}/template.json"))["version"]
                .as_str()
                .expect("the referenced template declares a version")
        );
        assert_eq!(
            shipped(&format!("{holder}/{name}/config.json"))["cell"]["template"],
            want,
            "the `ref` to {name} under {holder} lags the template it names"
        );
    }
}
