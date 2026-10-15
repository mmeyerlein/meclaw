//! GH #479 — the identity door of GH #458 / #473 had no reachable caller, and
//! the reason is that the two halves of the repo never met in a test.
//!
//! `templates/builder/recipes`' `grow_level` renders `subscribe: true` as
//! `to: "./assistants/<name>"` — scope-relative, because the mutation door
//! refuses an absolute `add_edges` endpoint with `scope_out_of_bounds`
//! (`MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS` is the whole of the exception list).
//! `templates/submit/gate`'s form check compared that `to` against the requester
//! as a raw string and failed closed on anything that was not absolute. So the
//! one spelling the door accepts was the one spelling the gate could not read,
//! and every `subscribe: true` wish died with `subscribe_target_not_self`.
//!
//! The identity half is the deeper one. The requester at the gate is the
//! identity the substrate stamped on the SUBMISSION, and when a level is grown
//! that is the parent or the operator — never the brain whose door it is, which
//! does not exist yet. `under(requester, to)` can therefore never hold for a
//! grown door, at any spelling.
//!
//! Two things are pinned here, and neither is a new permission:
//!
//!   * **Endpoints are resolved against the `scope` of the declaration they
//!     stand in** — the same resolution the door makes, so the gate judges the
//!     edge the door will draw rather than the bytes the renderer typed.
//!   * **A second, narrower branch of the TARGET rule**: the edge may also end
//!     at a node THIS SAME DECLARATION creates with `add_nodes`. There is no
//!     "somebody else" whose door is being opened when the requester brought the
//!     addressee into the world in the same mutation.
//!
//! The SOURCE rule is untouched (`from` ends at an `affinity` hive), and the
//! broker still answers `affinity.subscribe`. A form branch is not a permission.
//!
//! This file drives the RENDERER's output into the GATE's input. The two truths
//! are measured against each other; a hand-written edge nobody emits would
//! measure neither.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, emit_one, run_shipped_script, shipped_script};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const GATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);

const MEMBER: &str = "/os/orgs/acme/members/alex";
const NAME: &str = "scribe";

/// The identity the substrate stamps on a grow submission. Since the operator
/// hive became the single front door, the only cell that emits onto `./submit`
/// is `operator/submit` — so this, and never the assistant being grown, is what
/// the gate reads off the parked row.
const OPERATOR: &str = "/os/operator/submit";

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

/// The manifest `grow_level` renders for an assistant, with or without the door.
/// The whole declaration list, not just its edges: the second branch of the form
/// rule reads `scope` and `add_nodes` out of the very same declaration.
fn grown_assistant(subscribe: bool) -> Value {
    let mut params = json!({"scope": MEMBER, "level": "assistant", "name": NAME,
                            "template": "a-template@1.0.0"});
    if subscribe {
        params["subscribe"] = json!(true);
    }
    let wish = json!({"recipe": "grow_level", "request": "grow an assistant",
                      "params": params})
    .to_string();
    let out = emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "", "text": wish}],
        }),
    );
    out["manifest"].clone()
}

/// The same manifest with BOTH opt-ins of the level asked for at once — the
/// identity door and the credential road (GH #560, GH #567).
fn grown_assistant_with_both_switches() -> Value {
    let params = json!({"scope": MEMBER, "level": "assistant", "name": NAME,
                        "template": "a-template@1.0.0", "subscribe": true,
                        "credential": {"cred_ref": "cred:example-provider:primary",
                                       "subject": "member:alex",
                                       "expires_at": "2099-01-01T00:00:00.000000Z"}});
    let wish = json!({"recipe": "grow_level", "request": "grow an assistant",
                      "params": params})
    .to_string();
    let out = emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "", "text": wish}],
        }),
    );
    out["manifest"].clone()
}

/// The store's answer to an un-parking `select`, with the requester the
/// substrate stamped on the submission.
fn unpark(phase: &str, decls: &Value, requester: &str) -> Vec<Value> {
    let sha = digest_of(decls);
    let rows = json!([{ "id": "p1", "manifest": decls, "requester": requester,
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

/// The refusal code of a gate answer, or `None` when nothing was refused.
fn refused(out: &[Value]) -> Option<String> {
    out.iter()
        .find(|m| m["header"]["route"] == json!("receipt"))
        .and_then(|m| m["header"]["error_code"].as_str())
        .map(str::to_string)
}

/// One declaration in the shape the renderer emits: a scope, the node it
/// creates, and one `in_pack` edge. `creates` is what separates the second
/// branch from the first.
fn declaration(scope: &str, creates: Option<&str>, from: &str, to: &str) -> Value {
    let mut diff = json!({ "add_edges": [{
        "from": from, "to": to,
        "condition": "has(hop.route) && hop.route == 'answer'",
        "modifier": { "set_hop": { "route": "'in_pack'" } }
    }] });
    if let Some(name) = creates {
        diff["add_nodes"] = json!([{ "name": name, "template": "a-template@1.0.0" }]);
    }
    json!({ "scope": scope, "ctx": {}, "diff": diff })
}

// ═══════════════════════════════ the two truths, measured against each other

/// The renderer's own output reaches the broker instead of dying at the gate.
///
/// This is the whole of #479: the wish `subscribe: true` was documented as the
/// way a parent draws the door of the child it grows, and there was no caller
/// for whom the old rule could hold.
#[test]
fn the_manifest_grow_level_renders_survives_the_form_check() {
    let decls = grown_assistant(true);
    let out = unpark("parked", &decls, OPERATOR);
    assert_eq!(
        refused(&out),
        None,
        "the one manifest the shipped renderer emits for `subscribe: true` was \
         refused by the shipped gate — the feature had no reachable caller: {out:?}"
    );
    assert_eq!(out.len(), 1, "one more question, and nothing else: {out:?}");
    assert_eq!(out[0]["header"]["route"], "ask");
    let args = op_of(&out[0]);
    assert_eq!(
        args["capability"], "affinity.subscribe",
        "the form branch is not a permission: the broker still answers"
    );
    assert_eq!(args["check_only"], true);
    assert_eq!(
        args["subject"], OPERATOR,
        "R-AC-1: the identity the substrate stamped, unchanged"
    );
}

/// Both opt-ins at once are still ONE declaration, and the gate still reads it.
///
/// Since `builder@1.6.1` the credential road is drawn beside the level instead
/// of one declaration later (GH #567), and both switches move the declaration to
/// the MEMBER — the same wide form, for the same reason: an edge lives in the
/// graph of the lowest common ancestor of its endpoints. What matters here is
/// that the created-node branch of the form check still finds its addressee:
/// the `in_pack` edge ends inside a node named `assistants/<name>` in the very
/// same diff, and four v-lanes now stand between it and the end of the list.
#[test]
fn both_switches_at_once_are_one_declaration_the_gate_still_reads() {
    let decls = grown_assistant_with_both_switches();
    let list = decls.as_array().expect("a manifest");
    assert_eq!(
        list.len(),
        1,
        "the generation, its door and its credential road are one act: {decls:?}"
    );
    assert_eq!(
        list[0]["diff"]["add_nodes"][0]["name"],
        json!("assistants/scribe"),
        "the wide form names its child through the container"
    );
    assert!(
        list[0]["diff"]["seed_rows"].is_array(),
        "the grants ride in the same diff: {decls:?}"
    );
    let out = unpark("parked", &decls, OPERATOR);
    assert_eq!(
        refused(&out),
        None,
        "the manifest the shipped renderer emits for both switches was refused \
         by the shipped gate: {out:?}"
    );
}

/// Why the gate has to resolve at all, said in the door's own arithmetic: the
/// edge the renderer draws and the node it creates are ONE path once the
/// declaration's scope is applied — and the spelling is relative because the
/// door refuses an absolute `add_edges` endpoint outright.
#[test]
fn the_rendered_target_is_the_node_the_same_declaration_creates() {
    let decls = grown_assistant(true);
    let decl = &decls[0];
    let scope = decl["scope"].as_str().expect("a scope");
    let created = decl["diff"]["add_nodes"][0]["name"]
        .as_str()
        .expect("the level creates its node");
    let door = decl["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
        .iter()
        .find(|e| {
            e["modifier"]["set_hop"]["route"]
                .as_str()
                .unwrap_or_default()
                .trim_matches('\'')
                == "in_pack"
        })
        .expect("the identity door")
        .clone();
    let to = door["to"].as_str().expect("a target");
    assert!(
        !to.starts_with('/'),
        "an absolute `add_edges` endpoint is refused by the door with \
         `scope_out_of_bounds`, so the renderer cannot spell it that way: {to}"
    );
    // GH #561 — the door ends at a brain RIM of the generation now, one segment
    // below the node the declaration creates: the identity pack rides a v-lane
    // and the level it used to stop at declares the two rims as its connect
    // points instead of carrying the lane. The rule the gate applies is
    // unchanged in what it protects — the addressee came into the world in THIS
    // mutation — and the comparison is segment-wise for the same reason it
    // always was: `/os/…/scribe` must not cover `/os/…/scribe-of-somebody`.
    let to_abs = meclaw_colony::mutation::resolve_scoped_path(scope, to);
    let created_abs = meclaw_colony::mutation::resolve_scoped_path(scope, created);
    assert!(
        to_abs.as_str() == created_abs.as_str()
            || to_abs
                .as_str()
                .starts_with(&format!("{}/", created_abs.as_str())),
        "resolved as the DOOR resolves them, the door's target must be the node \
         this declaration brings into the world, or something inside it: \
         {to_abs:?} vs {created_abs:?}"
    );
}

/// The first branch, in the spelling the door accepts. A brain drawing its own
/// door writes `./talky`, not `/os/…/talky`, and that used to fail closed.
#[test]
fn a_brain_may_draw_its_own_door_in_the_relative_spelling_too() {
    let brain = format!("{MEMBER}/talky");
    let decls = json!([declaration(MEMBER, None, "./affinity", "./talky")]);
    let out = unpark("parked", &decls, &brain);
    assert_eq!(
        refused(&out),
        None,
        "the requester's own hive, resolved against the declaration's scope: {out:?}"
    );
    assert_eq!(op_of(&out[0])["capability"], "affinity.subscribe");
}

// ═══════════════════════════════════════════ the three refusals that remain

/// The SOURCE rule is untouched by the second branch. Creating the addressee
/// does not license an arbitrary sender writing a durable slot into it.
#[test]
fn a_created_node_does_not_excuse_a_source_that_is_not_affinity() {
    for from in ["./collector", "./my-affinity", "./affinity/push"] {
        let decls = json!([declaration(
            MEMBER,
            Some("assistants/scribe"),
            from,
            "./assistants/scribe"
        )]);
        let out = unpark("parked", &decls, OPERATOR);
        assert_eq!(
            refused(&out).as_deref(),
            Some("subscribe_source_not_affinity"),
            "`{from}` is not an affinity hive, whoever created the target"
        );
        assert!(
            out.iter().all(|m| m["header"]["route"] != json!("ask")),
            "and the broker is never asked about `{from}`"
        );
    }
}

/// The second branch is a branch about CREATION, not about spelling. A manifest
/// that merely names a relative path it does not create is refused exactly as
/// before — this is the case that would turn the fix into a hole.
#[test]
fn a_target_the_declaration_does_not_create_is_still_refused() {
    // The same edge as the renderer's, minus the `add_nodes` that brings the
    // addressee into the world: an existing sibling, opened by a stranger.
    let decls = json!([declaration(
        MEMBER,
        None,
        "./affinity",
        "./assistants/scribe"
    )]);
    let out = unpark("parked", &decls, OPERATOR);
    assert_eq!(
        refused(&out).as_deref(),
        Some("subscribe_target_not_self"),
        "nothing here creates `./assistants/scribe`, and the operator is not it"
    );
    assert!(out.iter().all(|m| m["header"]["route"] != json!("ask")));

    // And the creation has to be in the SAME declaration. A manifest rolls
    // forward with no rollback, so "some other declaration will create it" is a
    // promise the gate cannot check and the door never made.
    let split = json!([
        json!({ "scope": MEMBER, "ctx": {},
                "diff": { "add_nodes": [{ "name": "assistants/scribe",
                                          "template": "a-template@1.0.0" }] } }),
        declaration(MEMBER, None, "./affinity", "./assistants/scribe"),
    ]);
    let out = unpark("parked", &split, OPERATOR);
    assert_eq!(
        refused(&out).as_deref(),
        Some("subscribe_target_not_self"),
        "the creating declaration is a different one: {out:?}"
    );
}

/// GH #566: the created-node branch trusts a name only when the SAME
/// declaration can actually bring it into the world — under its own scope,
/// with a template. A name that escapes the scope or carries no template is
/// not "a node this declaration creates", so the edge is refused by form.
#[test]
fn an_anchor_that_escapes_the_scope_is_not_a_created_node() {
    let decls = json!([{ "scope": MEMBER, "ctx": {},
        "diff": { "add_nodes": [{ "name": "../dana/assistants/scribe",
                                  "template": "a-template@1.0.0" }],
                  "add_edges": [{ "from": "./affinity",
                                  "to": "../dana/assistants/scribe/talky",
                                  "modifier": { "set_hop": { "route": "'in_pack'" } } }] } }]);
    let out = unpark("parked", &decls, OPERATOR);
    assert_eq!(
        refused(&out).as_deref(),
        Some("subscribe_target_not_self"),
        "a node outside the declaration's own scope anchors nothing: {out:?}"
    );
    assert!(
        out.iter().all(|m| m["header"]["route"] != json!("ask")),
        "a form refusal never reaches the broker"
    );
}

/// The other half of GH #566: an `add_nodes` entry that instantiates nothing —
/// neither a `template` nor an `adopt` block — brings no addressee into the
/// world, so the name it carries cannot anchor a door.
#[test]
fn an_anchor_without_a_template_is_not_a_created_node() {
    let decls = json!([{ "scope": MEMBER, "ctx": {},
        "diff": { "add_nodes": [{ "name": "assistants/scribe" }],
                  "add_edges": [{ "from": "./affinity", "to": "./assistants/scribe/talky",
                                  "modifier": { "set_hop": { "route": "'in_pack'" } } }] } }]);
    let out = unpark("parked", &decls, OPERATOR);
    assert_eq!(
        refused(&out).as_deref(),
        Some("subscribe_target_not_self"),
        "an entry that instantiates nothing anchors nothing: {out:?}"
    );
    assert!(
        out.iter().all(|m| m["header"]["route"] != json!("ask")),
        "a form refusal never reaches the broker"
    );
}

/// The other direction of GH #566: `adopt` is the SECOND way a declaration
/// brings a node into the world — an instantiation from an existing on-disk
/// cell, with a fresh cell id — and the door's own grammar makes `adopt` and
/// `template` mutually exclusive. So an adopted node anchors the created-node
/// branch exactly like an instantiated one, and a check that demanded a
/// `template` would refuse a door the declaration really does create.
#[test]
fn an_adopted_node_is_a_created_node() {
    let decls = json!([{ "scope": MEMBER, "ctx": {},
        "diff": { "add_nodes": [{ "name": "assistants/scribe",
                                  "adopt": { "type": "echo", "version": "0.1.0" } }],
                  "add_edges": [{ "from": "./affinity", "to": "./assistants/scribe/talky",
                                  "modifier": { "set_hop": { "route": "'in_pack'" } } }] } }]);
    let out = unpark("parked", &decls, OPERATOR);
    assert_eq!(
        refused(&out),
        None,
        "an adopt entry instantiates, so the addressee is this declaration's own: {out:?}"
    );
    assert!(
        out.iter().any(|m| m["header"]["route"] == json!("ask")),
        "and the form having held, the broker is asked for the permission"
    );
}

/// A foreign, existing path stays foreign — in both spellings, and even when the
/// same declaration creates something else entirely.
#[test]
fn a_foreign_existing_target_is_refused_in_either_spelling() {
    for to in [
        // Absolute, another member's brain — the case of GH #458.
        "/os/orgs/acme/members/dana/talky",
        // Relative, and out of the scope it is resolved against.
        "../dana/talky",
        // The string-prefix trap, one segment short of the created node.
        "./assistants/scrib",
        // A sibling of the created node that nothing in this diff creates.
        "./assistants/other",
    ] {
        let decls = json!([declaration(
            MEMBER,
            Some("assistants/scribe"),
            "./affinity",
            to
        )]);
        let out = unpark("parked", &decls, OPERATOR);
        assert_eq!(
            refused(&out).as_deref(),
            Some("subscribe_target_not_self"),
            "`{to}` is neither the requester nor a node this declaration creates"
        );
        assert!(
            out.iter().all(|m| m["header"]["route"] != json!("ask")),
            "a malformed subscribe is not a permission question: `{to}`"
        );
    }
}

// ═══════════════════════════════════════════════════════ § 2d — the surfaces

/// § 2d. The README publishes the target rule, and after #479 it publishes two
/// branches. The sentence and the mechanism are asserted together, because a
/// grep alone pins a string and an assertion alone lets the prose drift away
/// from it.
#[test]
fn the_readme_publishes_both_branches_of_the_target_rule() {
    let readme = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../templates/submit/README.md"
    ))
    .expect("the submit README travels with the template");
    let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("resolved against the `scope` of the declaration it stands in"),
        "the README must say that an endpoint is resolved before it is judged — \
         a reader who believes `to` is compared as written cannot explain why a \
         relative edge is accepted"
    );
    assert!(
        flat.contains("a node the same declaration creates with `add_nodes`"),
        "the README must publish the second branch of the target rule"
    );
    // GH #566: the branch is only as narrow as its anchor, so the two
    // properties the anchor must have are published beside the branch itself.
    assert!(
        flat.contains("under the declaration's own scope"),
        "the README must say that a created name only anchors a door when it \
         lies under the declaration's own scope"
    );
    assert!(
        flat.contains("a `template`, or an `adopt` block"),
        "the README must publish BOTH instantiating shapes as anchors — a reader \
         told only about `template` would believe an adopted node cannot have a door"
    );

    // The other public surface says the same thing, or a model reading the
    // catalogue learns a rule the gate no longer has.
    let tpl: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../templates/submit/template.json"
        ))
        .expect("the submit template.json travels with the template"),
    )
    .expect("json");
    let purpose = tpl["description"]["purpose"].as_str().expect("a purpose");
    assert!(
        purpose.contains("or at a node the same declaration creates"),
        "the template's own description still promises the one-branch rule"
    );
    assert!(
        purpose.contains("RESOLVED against that declaration's scope"),
        "and it must say that the endpoints are resolved before they are compared"
    );
    // The refusal's own words are the surface a caller repairs from, and #566
    // widened them rather than minting a second code — so the detail is pinned
    // where the code is decided, in the shipped script itself. The anchor is a
    // fragment of ONE source literal: the script's own `"a" "b"` concatenation
    // is a runtime join, and a phrase spanning two literals is not in the file.
    assert!(
        shipped_script(GATE).contains("entry that instantiates nothing -- no template"),
        "the `subscribe_target_not_self` detail must name the anchor case too, \
         or a caller told only the class repairs blind"
    );
    assert!(
        purpose.contains("verifies its own anchor"),
        "and the catalogue must publish the anchor check of GH #566 too — a model \
         reading only the description would otherwise learn the wider rule"
    );

    // The mechanism, both halves: the resolution and the creation branch.
    assert_eq!(
        refused(&unpark("parked", &grown_assistant(true), OPERATOR)),
        None,
        "the parent's door, drawn for the child it creates in the same declaration"
    );
    assert_eq!(
        refused(&unpark(
            "parked",
            &json!([declaration(
                MEMBER,
                None,
                "./affinity",
                "./assistants/scribe"
            )]),
            OPERATOR
        ))
        .as_deref(),
        Some("subscribe_target_not_self"),
        "and nothing wider than that"
    );
}
