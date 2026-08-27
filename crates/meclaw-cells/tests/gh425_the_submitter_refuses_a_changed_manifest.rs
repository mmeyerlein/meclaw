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
use meclaw_testing::{emit_one, run_shipped_script, shipped_script};

const SUBMIT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);

const REQUESTER: &str = "/os/orgs/acme/members/alex/assistants/scribe/tools/apply";

/// The policy that lets `REQUESTER` touch the shell and everything under it.
fn open_policy() -> Value {
    json!([{"requester_prefix": "/os/orgs/", "verdict": "allow", "scopes": ["/os"]}])
}

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

fn run_submit(manifest: Value, claimed: &str, reply_to: &str, policy: Value) -> Value {
    emit_one(
        &shipped_script(SUBMIT),
        &json!({
            "target": "/os/submit",
            "reply_to": reply_to,
            "header": {"hop": {"route": "in_apply", "manifest_sha256": claimed},
                       "context": {}},
            "ttl": 64,
            "manifest": manifest,
            "messages": [],
            "params": {"policy": policy},
        }),
    )
}

fn one_declaration() -> Value {
    json!([{"scope": "/os", "ctx": {},
            "diff": {"add_edges": [{"from": "./a", "to": "./b"}]}}])
}

#[test]
fn a_manifest_whose_bytes_changed_is_refused_by_name() {
    let decls = one_declaration();
    let honest = digest_of(&decls);
    let mut tampered = decls.clone();
    tampered[0]["diff"]["add_edges"][0]["to"] = json!("/os/steward");
    let out = run_submit(tampered, &honest, REQUESTER, open_policy());
    assert_eq!(out["header"]["route"], json!("receipt"));
    assert_eq!(
        out["header"]["error_code"],
        json!("manifest_digest_mismatch")
    );
    assert!(
        out.get("manifest").is_none(),
        "a refused submission emits no mutation body"
    );
}

#[test]
fn an_honest_manifest_reaches_the_mutation_lane_once() {
    let decls = one_declaration();
    let d = digest_of(&decls);
    let out = run_submit(decls, &d, REQUESTER, open_policy());
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
    // The body claims to be the steward. The envelope says otherwise.
    let mut claiming = one_declaration();
    claiming[0]["ctx"] = json!({"requester": "/os/steward"});
    let claimed_digest = digest_of(&claiming);
    let out = run_submit(claiming, &claimed_digest, REQUESTER, open_policy());
    assert_eq!(out["header"]["route"], json!("mutate"));
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
    let out = run_submit(decls, &d, REQUESTER, open_policy());
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
fn a_fresh_instance_submits_nothing() {
    // The shipped default is an empty policy, the same discipline as `access`'s
    // seed with `enabled: 0`. A colony nobody has authorised applies nothing.
    let decls = one_declaration();
    let d = digest_of(&decls);
    let out = run_submit(decls, &d, REQUESTER, json!([]));
    assert_eq!(
        out["header"]["error_code"],
        json!("requester_not_permitted")
    );
    assert!(out.get("manifest").is_none());
}

#[test]
fn a_declaration_outside_the_permitted_scope_takes_the_whole_submission_down() {
    let decls = json!([
        {"scope": "/os/orgs/acme", "ctx": {}, "diff": {"add_edges": [{"from": "./a", "to": "./b"}]}},
        {"scope": "/", "ctx": {}, "diff": {"add_edges": [{"from": "./x", "to": "./y"}]}}
    ]);
    let d = digest_of(&decls);
    let policy = json!([{"requester_prefix": "/os/orgs/", "verdict": "allow",
                         "scopes": ["/os/orgs/acme"]}]);
    let out = run_submit(decls, &d, REQUESTER, policy);
    assert_eq!(out["header"]["error_code"], json!("scope_not_permitted"));
    assert!(
        out.get("manifest").is_none(),
        "the check is pre-destructive: a manifest rolls forward with no rollback, \
         so half a submission is worse than none"
    );
}

#[test]
fn an_envelope_with_nobody_on_it_is_refused_rather_than_attributed_to_the_void() {
    let decls = one_declaration();
    let d = digest_of(&decls);
    let out = run_submit(decls, &d, "", open_policy());
    assert_eq!(out["header"]["error_code"], json!("requester_unknown"));
}

#[test]
fn the_submitter_carries_the_policy_as_rows_and_ships_it_empty() {
    let raw = std::fs::read_to_string(SUBMIT).expect("submit config");
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    assert_eq!(cfg["cell"]["type"], json!("code"));
    assert_eq!(
        cfg["params"]["sandbox"]["network"],
        json!("deny"),
        "the mutation door is not a network connection"
    );
    let policy = &cfg["params"]["policy"];
    let empty = match policy {
        Value::Array(a) => a.is_empty(),
        Value::String(s) => s.contains(":-[]") || s == "[]",
        _ => false,
    };
    assert!(empty, "the shipped policy is not empty: {policy}");
}

#[test]
fn a_permitted_scope_is_a_path_prefix_and_not_a_string_prefix() {
    // `/os` permits `/os` and `/os/orgs`. It must not permit `/oscar` — a plain
    // startswith would hand out a scope nobody named, which is the one mistake
    // a permission check may not make.
    let decls = json!([{"scope": "/oscar", "ctx": {},
                        "diff": {"add_edges": [{"from": "./a", "to": "./b"}]}}]);
    let d = digest_of(&decls);
    let out = run_submit(decls, &d, REQUESTER, open_policy());
    assert_eq!(out["header"]["error_code"], json!("scope_not_permitted"));
}
