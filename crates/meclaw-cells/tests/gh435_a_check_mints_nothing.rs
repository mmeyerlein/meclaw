//! GH #435 — a verdict without an instrument.
//!
//! `submit` wants to know WHETHER a manifest may be carried to the mutation
//! door, not to be handed something with which to carry it. The broker's only
//! answer today is a grant: a row in `grants`, a `granted` row in
//! `grant_events`, a handle with an expiry. A grant nobody ever spends is a
//! bearer row with an expiry date that only `./sweep` touches again — issued,
//! journalled and never redeemed.
//!
//! So `access.request` gains `check_only`. What is pinned here:
//!
//! 1. **A check answers and mints nothing.** Exactly two emissions — one audit
//!    row with `outcome: "checked"`, and the answer. Neither names `grants` nor
//!    `grant_events`, and the answer carries an empty `grant_id`, because there
//!    is no grant to name.
//! 2. **A refused check is the ordinary refusal.** Same reason_code, same audit
//!    `outcome: "denied"` — a refused check and a refused request are the same
//!    fact, and the deny branch never minted anything anyway.
//! 3. **A grant request is untouched.** The same rule without `check_only`
//!    still mints, in four emissions, exactly as before.
//! 4. **The question travels in the carry.** `check_only` is read once, in
//!    phase `in`, and rides the store round trip in `context.ac_carry` — the
//!    only memory a `code` cell has (there is no `cell.db` for one).
//!
//! Precedent in the very same script: the `require_approval` branch answers and
//! mints nothing. This is that shape, for a verdict that is a yes.
//!
//! **R2b guard (GH #49 form).** `access` is PRIVATE — it does not travel with
//! the export, so in the public clone these tests skip.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const POLICY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/access/policy/config.json"
);

const REQUESTER: &str = "/os/submit";

fn shipped_policy() -> Option<String> {
    if std::path::Path::new(POLICY).exists() {
        Some(shipped_script(POLICY))
    } else {
        None
    }
}

/// One enabled rule as it comes off the `policy` table.
fn rule(verdict: &str) -> Value {
    json!({
        "rule_id": "colony.mutate.default",
        "requester": REQUESTER,
        "capability": "colony.mutate",
        "subject": "*",
        "scope_match": {"scope_prefix": "/os/orgs", "actions": ["apply"]},
        "verdict": verdict,
        "max_ttl_ms": 60000,
        "constraints": {},
        "priority": 100,
        "cred_ref": "",
    })
}

/// The `rules` phase, with the request rebuilt out of `context.ac_carry`.
fn decide(script: &str, verdict: &str, check_only: bool) -> Vec<Value> {
    let mut carry = json!({
        "call_id": "call-1",
        "requester": REQUESTER,
        "capability": "colony.mutate",
        "subject": "member:alex",
        "resource": {"scope": "/os/orgs/acme"},
        "purpose": "",
        "ttl_ms": 0,
    });
    if check_only {
        carry["check_only"] = json!(true);
    }
    emit_all(
        script,
        &json!({
            "target": "/os/access",
            "header": {
                "hop": {"operation": "select", "route": "astore"},
                // The shipped edge `./policy -> ./store` promotes THREE keys,
                // and `access_origin` is the one the script recognises its own
                // echo by (a `hop.operation` is written by whoever emitted the
                // message, so a caller may carry one). A fixture that omits it
                // is not modelling the edge.
                "context": {"access_origin": "policy", "ac_phase": "rules",
                            "ac_carry": carry.to_string()},
            },
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "s1",
                          "text": json!([rule(verdict)]).to_string()}],
        }),
    )
}

/// The JSON a `code` emission carries in its single turn.
fn turn(m: &Value) -> Value {
    meclaw_core::serde_json::from_str(m["messages"][0]["text"].as_str().expect("text"))
        .expect("the turn carries json")
}

/// Every table the emissions write to.
fn tables(out: &[Value]) -> Vec<String> {
    out.iter()
        .filter(|m| m["header"]["route"] == json!("astore"))
        .filter_map(|m| turn(m)["table"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn a_check_only_request_answers_and_mints_nothing() {
    let Some(script) = shipped_policy() else {
        return;
    };
    let out = decide(&script, "allow", true);
    assert_eq!(
        out.len(),
        2,
        "a check is one audit row and one answer, nothing else: {out:?}"
    );

    let audit = turn(&out[0]);
    assert_eq!(audit["table"], json!("audit"));
    assert_eq!(audit["row"]["outcome"], json!("checked"));
    assert_eq!(audit["row"]["capability"], json!("colony.mutate"));
    assert_eq!(
        audit["row"]["detail"]["rule_id"],
        json!("colony.mutate.default")
    );
    assert_eq!(audit["row"]["detail"]["subject"], json!("member:alex"));

    assert_eq!(
        tables(&out),
        vec!["audit".to_string()],
        "a check writes the audit row and NOTHING else — no grant, no grant event"
    );

    let answer = &out[1];
    assert_eq!(answer["header"]["route"], json!("grant"));
    assert_eq!(answer["header"]["verdict"], json!("allowed"));
    assert_eq!(
        answer["header"]["grant_id"],
        json!(""),
        "there is no grant, so there is no grant_id to name"
    );
    let payload = turn(answer);
    assert_eq!(payload["status"], json!("allowed"));
    assert_eq!(payload["capability"], json!("colony.mutate"));
    assert_eq!(payload["reason_code"], json!(""));
    assert!(
        payload.get("grant_id").is_none() && payload.get("expires_at").is_none(),
        "an allowed CHECK hands out neither a handle nor an expiry: {payload}"
    );
    assert_eq!(answer["messages"][0]["id"], json!("call-1"));
}

#[test]
fn a_check_only_denial_is_the_ordinary_denial() {
    let Some(script) = shipped_policy() else {
        return;
    };
    let out = decide(&script, "deny", true);
    assert_eq!(out.len(), 2, "a refusal has always been two emissions");
    let audit = turn(&out[0]);
    assert_eq!(audit["table"], json!("audit"));
    assert_eq!(
        audit["row"]["outcome"],
        json!("denied"),
        "a refused check and a refused request are the same fact"
    );
    assert_eq!(tables(&out), vec!["audit".to_string()]);
    let payload = turn(&out[1]);
    assert_eq!(payload["status"], json!("denied"));
    assert_eq!(payload["reason_code"], json!("denied_by_rule"));
    assert_eq!(out[1]["header"]["verdict"], json!("denied"));
}

#[test]
fn a_grant_request_is_untouched() {
    let Some(script) = shipped_policy() else {
        return;
    };
    let out = decide(&script, "allow", false);
    assert_eq!(out.len(), 4, "the grant lane is four emissions, as before");
    assert_eq!(
        tables(&out),
        vec![
            "grants".to_string(),
            "grant_events".to_string(),
            "audit".to_string()
        ]
    );
    let audit = turn(&out[2]);
    assert_eq!(audit["row"]["outcome"], json!("granted"));
    let payload = turn(&out[3]);
    assert_eq!(payload["status"], json!("granted"));
    assert_eq!(out[3]["header"]["verdict"], json!("granted"));
    assert!(
        out[3]["header"]["grant_id"]
            .as_str()
            .is_some_and(|g| g.starts_with("grant:")),
        "a grant request still gets a handle"
    );
}

#[test]
fn the_question_travels_in_the_carry() {
    let Some(script) = shipped_policy() else {
        return;
    };
    // Phase `in`: a code cell has no cell.db, so what the request asked for has
    // to ride the store round trip or it is gone by the time the rules arrive.
    let out = emit_all(
        &script,
        &json!({
            "target": "/os/access",
            "header": {"hop": {"route": "in_request"},
                       "context": {"requester": REQUESTER}},
            "ttl": 64,
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-1",
                          "text": json!({
                              "capability": "colony.mutate",
                              "subject": "member:alex",
                              "resource": {"scope": "/os/orgs/acme"},
                              "check_only": true,
                          }).to_string()}],
        }),
    );
    assert_eq!(out.len(), 1, "phase `in` reads the rules and nothing else");
    let carry: Value = meclaw_core::serde_json::from_str(
        out[0]["header"]["carry"].as_str().expect("a carry string"),
    )
    .expect("the carry is json");
    assert_eq!(
        carry["check_only"],
        json!(true),
        "the question has to survive the round trip: {carry}"
    );
}

#[test]
fn a_request_that_carries_a_hop_operation_is_answered_and_not_swallowed() {
    let Some(script) = shipped_policy() else {
        return;
    };
    // Regression lock. The broker used to recognise its own store echo by
    // `"operation" in hop` — a key written by whoever EMITTED the message, not
    // by the colony. A caller that carries one for reasons of its own (the
    // submitter stamps `operation` on its own lanes) was then read as "this is
    // my store answering", and the broker fell into its echo branch and
    // answered NOTHING. A broker that silently does not answer is worse than
    // one that denies: the caller waits forever.
    //
    // What it recognises now is the marker its OWN edge promotes to context.
    let out = emit_all(
        &script,
        &json!({
            "target": "/os/access",
            "header": {"hop": {"route": "in_request", "operation": "submit"},
                       "context": {"requester": REQUESTER}},
            "ttl": 64,
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-9",
                          "text": json!({
                              "capability": "colony.mutate",
                              "subject": "member:alex",
                              "resource": {"scope": "/os/orgs/acme"},
                              "check_only": true,
                          }).to_string()}],
        }),
    );
    assert_eq!(out.len(), 1, "phase `in` reads the rules and nothing else");
    assert_eq!(out[0]["header"]["route"], json!("astore"));
    assert_eq!(out[0]["header"]["phase"], json!("rules"));
}
