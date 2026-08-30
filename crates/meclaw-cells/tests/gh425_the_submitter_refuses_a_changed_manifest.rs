//! GH #425 — the manifest a human said yes to and the manifest that is applied
//! are the SAME bytes, or nothing is applied.
//!
//! The draft goes down into a chat as a `tool_result`, a human reads it, and the
//! model repeats it in the second tool call. A model that reformats, reorders or
//! quietly drops a declaration on the way produces a manifest that LOOKS like
//! the one that was approved. The digest travels with the draft and is checked
//! here, so a changed manifest is refused by name instead of applied by luck.
//!
//! The other half of this file is the identity: it comes off the ENVELOPE, where
//! the substrate stamped it (`.reply_to(em.sender_path.clone())`), and never out
//! of the body. A body that names itself is a claim, and a claim is not an
//! identity.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, run_shipped_script, shipped_script};

const SUBMIT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);

const REQUESTER: &str = "/os/orgs/acme/members/alex/assistants/scribe/tools/apply";

fn digest_of(decls: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    let out = run_shipped_script(program, &decls.to_string());
    String::from_utf8(out.stdout).expect("hex")
}

/// The DECISION of phase A: the last thing the gate emits.
///
/// Since GH #438 an accepted submission is TWO emissions — the row it
/// remembers, then what it does next — while every refusal stays one. The last
/// one is the decision in both cases, and it is the one every assertion in this
/// file is about; the row is measured in `gh438_the_submitter_has_a_memory`.
///
/// Since GH #435 that decision is a QUESTION rather than a submission: the
/// manifest is parked and the broker is asked. The `mutate` emission moved one
/// phase later, behind the verdict, and the tests below follow it there rather
/// than pretending it never moved.
fn run_submit(manifest: Value, claimed: &str, reply_to: &str) -> Value {
    let mut out = emit_all(
        &shipped_script(SUBMIT),
        &json!({
            "target": "/os/submit",
            "reply_to": reply_to,
            "header": {"hop": {"route": "in_apply", "manifest_sha256": claimed},
                       "context": {}},
            "ttl": 64,
            "manifest": manifest,
            "messages": [],
            "params": {},
        }),
    );
    out.pop().expect("the gate says something")
}

fn one_declaration() -> Value {
    json!([{"scope": "/os", "ctx": {},
            "diff": {"add_edges": [{"from": "./a", "to": "./b"}]}}])
}

/// The submission itself, one phase later: the broker said yes, the store hands
/// the parked row back, and the gate emits `[delete, insert, mutate]`.
fn run_unpark(decls: &Value, sha: &str, requester: &str) -> Value {
    let rows = json!([{ "id": "p1", "manifest": decls, "requester": requester,
                        "tool_call_id": "c2", "manifest_sha256": sha }]);
    let out = emit_all(
        &shipped_script(SUBMIT),
        &json!({
            "target": "/os/submit",
            "header": {
                "hop": {"operation": "select", "rows_affected": 1},
                "context": {"sub_origin": "gate", "sub_phase": "parked",
                            "sub_carry": "{\"status\":\"allowed\"}"}
            },
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                          "text": rows.to_string()}],
            "params": {},
        }),
    );
    out.into_iter()
        .find(|m| m["header"]["route"] == json!("mutate"))
        .expect("the un-parked manifest reaches the mutation lane")
}

#[test]
fn a_manifest_whose_bytes_changed_is_refused_by_name() {
    let decls = one_declaration();
    let honest = digest_of(&decls);
    let mut tampered = decls.clone();
    tampered[0]["diff"]["add_edges"][0]["to"] = json!("/os/argus");
    let out = run_submit(tampered, &honest, REQUESTER);
    assert_eq!(out["header"]["route"], json!("receipt"));
    assert_eq!(
        out["header"]["error_code"],
        json!("manifest_digest_mismatch")
    );
    assert!(
        out.get("manifest").is_none(),
        "a refused submission emits no mutation body"
    );
    // And nothing was asked either: a question about a manifest whose bytes are
    // already wrong is a question whose answer cannot help.
    assert_ne!(out["header"]["route"], json!("ask"));
}

#[test]
fn an_honest_manifest_asks_once_and_then_reaches_the_mutation_lane_once() {
    let decls = one_declaration();
    let d = digest_of(&decls);
    let asked = run_submit(decls.clone(), &d, REQUESTER);
    assert_eq!(asked["header"]["route"], json!("ask"));
    assert_eq!(asked["header"]["manifest_sha256"], json!(d));

    let out = run_unpark(&decls, &d, REQUESTER);
    assert_eq!(out["header"]["route"], json!("mutate"));
    assert_eq!(out["header"]["declaration_count"], json!(1));
    assert!(
        out["manifest"].is_array(),
        "the manifest form is a flat array of single-form bodies (Lane 4)"
    );
    assert!(
        out.get("scope").is_none() && out.get("diff").is_none(),
        "a body is EITHER a single mutation OR a manifest — both is ManifestError::BothForms"
    );
}

#[test]
fn the_requester_is_taken_from_the_envelope_and_never_from_the_body() {
    // The body claims to be the argus. The envelope says otherwise, and the
    // envelope is what the substrate wrote.
    let mut claiming = one_declaration();
    claiming[0]["ctx"] = json!({"requester": "/os/argus"});
    let claimed_digest = digest_of(&claiming);

    // It is already true of the QUESTION: the identity travels as `subject`,
    // and the claim in the body is never read.
    let asked = run_submit(claiming.clone(), &claimed_digest, REQUESTER);
    let args: Value = meclaw_core::serde_json::from_str(
        asked["messages"][0]["text"].as_str().expect("a tool_call"),
    )
    .expect("the args are json");
    assert_eq!(args["subject"], json!(REQUESTER));

    let out = run_unpark(&claiming, &claimed_digest, REQUESTER);
    assert_eq!(
        out["manifest"][0]["ctx"]["requester"],
        json!(REQUESTER),
        "the substrate stamps reply_to on every cell emission; a body that names \
         itself is a claim, and a claim is not an identity"
    );
    assert_eq!(
        out["manifest"][0]["ctx"]["manifest_sha256"],
        json!(claimed_digest),
        "the stamp names the digest that was CHECKED, so the mutation_log row and \
         the draft a human approved can be put side by side"
    );
}

#[test]
fn the_audit_stamp_lands_in_every_entry_because_a_manifest_has_no_shared_ctx() {
    let decls = json!([
        {"scope": "/os", "ctx": {}, "diff": {"add_edges": [{"from": "./a", "to": "./b"}]}},
        {"scope": "/os", "ctx": {}, "diff": {"add_edges": [{"from": "./b", "to": "./c"}]}}
    ]);
    let d = digest_of(&decls);
    let out = run_unpark(&decls, &d, REQUESTER);
    for i in 0..2 {
        assert_eq!(
            out["manifest"][i]["ctx"]["requester"],
            json!(REQUESTER),
            "entry {i} carries no attribution — the manifest form has no \
             manifest-wide ctx, so a stamp at the top level reaches no \
             mutation_log row at all"
        );
    }
}

#[test]
fn an_envelope_with_nobody_on_it_is_refused_rather_than_attributed_to_the_void() {
    let decls = one_declaration();
    let d = digest_of(&decls);
    let out = run_submit(decls, &d, "");
    assert_eq!(out["header"]["error_code"], json!("requester_unknown"));
}

#[test]
fn the_submitter_carries_no_policy_of_its_own_and_no_way_out() {
    // GH #435 moved the decision. What is measured here is the shape that is
    // left: a code cell with no politics in its params and no network to fetch
    // any. The permission ROWS are the broker's now, and that they ship
    // disabled is measured where they live —
    // `gh435_the_broker_ships_its_first_row`; that `/os` does not permit
    // `/oscar` is measured in `gh435_the_broker_compares_paths`, in the one
    // comparator that decides it.
    let raw = std::fs::read_to_string(SUBMIT).expect("submit config");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    assert_eq!(cfg["cell"]["type"], json!("code"));
    assert_eq!(
        cfg["params"]["sandbox"]["network"],
        json!("deny"),
        "the mutation door is not a network connection"
    );
    assert!(
        cfg["params"].get("policy").is_none(),
        "the decision moved to the broker; a second source of it in the ONE cell \
         with the mutation edge is an audit trail that cannot say who decided"
    );
}
