//! GH #473 — a grown assistant can be given the door its identity comes
//! through, and the recipe draws exactly the half a manifest can carry.
//!
//! A colony grown entirely from wishes answers turns and reaches its brain with
//! an EMPTY `system` tree. Nothing in a grown topology writes a durable slot,
//! and the one lane that can — `in_pack`, GH #458 — needs an edge from the
//! member's own record into the new agent. `grow_level`'s assistant table drew
//! eleven edges and none of them was that one, so the affinity hive's push cron
//! fired into a lane with no consumer: the same agent that answers correctly on
//! `in_recall` says it has no stored information about its owner in a turn.
//!
//! Two decisions are pinned here, and both are the kind that look like an
//! omission from the outside:
//!
//!   * **The door is OPT-IN.** The submitter refuses an `in_pack` edge that does
//!     not end at the REQUESTER's own sealed hive (`subscribe_target_not_self`,
//!     GH #458) — the form check that keeps one agent from opening a channel
//!     into another agent's prompt. A level that always drew one could
//!     therefore be grown by nobody except the brain being grown, so the eleven
//!     edges of the level stay eleven and the door is a parameter.
//!   * **Only the graph half is renderable.** A `subscribers` row is a store
//!     write, not a mutation declaration; writing it stays a `subscribe` op
//!     through affinity's own gate (ruling R-Subscribe). What the recipe renders
//!     is the graph that row will need.
//!
//! The BOOTED half — a push that reaches a grown assistant's prompt over exactly
//! these edges — lives in `gh473_a_grown_assistant_hears_the_member_record.rs`.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, emit_one, shipped_script};
use std::path::PathBuf;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const BRIEF: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/brief/config.json"
);

/// The leg of the brief that reaches `./compose`. Since GH #477 the cell is a
/// multi-send: the store leg that parks the question and the instructions in
/// the round table travels first, the briefing itself second.
fn compose_leg(all: Vec<Value>) -> Value {
    all.into_iter()
        .find(|m| m["header"]["route"] == "compose")
        .expect("the brief's leg to the composer")
}

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

const MEMBER: &str = "/os/orgs/acme/members/alex";
const NAME: &str = "scribe";

/// One assistant level, with or without the door: the declaration's scope and
/// its edges. Both are needed since GH #503 — a plain level declares itself AT
/// `<member>/assistants` and spells its child `./<name>`, while a `subscribe`
/// wish keeps the wider declaration at the member, because `./affinity` is a
/// SIBLING of that container and no declaration standing inside it can name one.
/// The comparison below is therefore made on the ABSOLUTE edges.
fn assistant_declaration(subscribe: bool) -> (String, Vec<Value>) {
    let mut params = json!({"scope": MEMBER, "level": "assistant", "name": NAME,
                            "template": "a-template@1.0.0"});
    if subscribe {
        params["subscribe"] = json!(true);
    }
    let out = emit_one(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "grow_level", "request": "…",
                                         "params": params}).to_string()}],
        }),
    );
    let decls = out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("no manifest: {out}"));
    assert_eq!(
        decls.len(),
        1,
        "the door does not cost a second declaration: the node, its transit \
         edges and its identity door are one decision, and a manifest rolls \
         forward with no rollback"
    );
    (
        decls[0]["scope"]
            .as_str()
            .expect("a declaration has a scope")
            .to_string(),
        decls[0]["diff"]["add_edges"]
            .as_array()
            .expect("add_edges")
            .clone(),
    )
}

/// The edges one assistant level renders, as written.
fn assistant_edges(subscribe: bool) -> Vec<Value> {
    assistant_declaration(subscribe).1
}

/// The same edges with both endpoints resolved against the declaration they
/// stand in — what the door will actually draw, and the only form in which two
/// declarations at two scope roots are comparable.
fn absolute(subscribe: bool) -> Vec<Value> {
    let (scope, edges) = assistant_declaration(subscribe);
    let root = meclaw_core::Path::new(&scope);
    edges
        .into_iter()
        .map(|mut e| {
            for side in ["from", "to"] {
                let raw = e[side].as_str().expect("an endpoint").to_string();
                e[side] = json!(meclaw_core::Path::resolve(&root, &raw).as_str());
            }
            e
        })
        .collect()
}

/// The `set_hop` route an edge re-stamps onto, unquoted. A `set_hop` value is a
/// CEL expression, so a literal carries its quotes — the same unwrapping the
/// submitter's gate does when it recognises a subscribe.
fn restamp(edge: &Value) -> Option<String> {
    let raw = edge["modifier"]["set_hop"]["route"].as_str()?;
    Some(raw.trim_matches('\'').trim_matches('"').to_string())
}

// ═════════════════════════════════════════════════════════ the opt-in half

/// The level alone is untouched. This is not tidiness: an `in_pack` edge in
/// every grown assistant would make the level unsubmittable by the operator
/// that grows it.
#[test]
fn the_level_alone_draws_no_identity_door() {
    let plain = assistant_edges(false);
    assert!(
        plain
            .iter()
            .all(|e| restamp(e).as_deref() != Some("in_pack")),
        "the level itself must not open a durable write channel into a brain's \
         prompt — the submitter refuses that edge unless the requester IS the \
         hive it ends at, so a level that always drew one could not be grown"
    );
    assert!(
        plain.iter().all(|e| !e["condition"]
            .as_str()
            .unwrap_or_default()
            .contains("pack_ack")),
        "and without the push edge the receipt drain has nothing to drain"
    );
}

/// Asked for, it is exactly two edges — and they are the two the assistant's own
/// contract pairs. The count is derived from the difference rather than written
/// down, so it cannot drift from the table.
#[test]
fn subscribe_draws_the_push_edge_and_its_receipt_drain_and_nothing_else() {
    // Resolved, because the two declarations do not stand at the same scope
    // root since GH #503 — and resolved is the form the door reads anyway.
    let plain = absolute(false);
    let with_door = absolute(true);

    assert_eq!(
        with_door[..plain.len()],
        plain[..],
        "the door is APPENDED: the level's own edges must not move, or every \
         byte pin against examples/organism becomes a diff about ordering"
    );
    let extra: Vec<Value> = assistant_edges(true)[plain.len()..].to_vec();
    assert_eq!(
        extra.len(),
        4,
        "the identity door is two V-LANES and their two receipt drains — \
         nothing more, and never a half of it. Since GH #561 the pack ends at \
         the two BRAIN RIMS of the generation rather than at its rim, because \
         the level that used to fan it out inside no longer carries the lane: \
         {extra:?}"
    );

    let target = format!("./assistants/{NAME}");
    for (i, rim) in ["talky", "cogny"].iter().enumerate() {
        let push = &extra[i];
        assert_eq!(
            push["from"],
            json!("./affinity"),
            "the push leaves the affinity HIVE. `affinity`'s `params.ports` is \
             empty, so an edge naming a cell inside it is refused at the door \
             with `hive_port_boundary` — the hive path is the only endpoint \
             there is"
        );
        assert_eq!(
            push["to"],
            json!(format!("{target}/{rim}")),
            "and it ends at the generation's `{rim}` rim, which is what the \
             assistant's own contract names as the connect point for this lane"
        );
        assert_eq!(
            push["lane"],
            json!("in_pack"),
            "an edge that skips a level has to NAME the lane it carries — no \
             validation reads a lane out of a CEL guard, and without the field \
             this is an ordinary deep edge nobody vouched for: {push}"
        );
        assert_eq!(
            restamp(push).as_deref(),
            Some("in_pack"),
            "the push leaves affinity on `answer` like every other emission of \
             that hive; what makes it an identity write is the re-stamp onto \
             the subscriber's own lane: {push}"
        );
        let guard = push["condition"].as_str().unwrap_or_default();
        assert!(
            guard.contains(&format!("hop.subscriber == '{target}'")),
            "the guard must name the SUBSCRIBER, which is the generation and \
             not one of its rims: a subscription is one row about one agent, \
             and the fan-out is the two edges. `./brief` sets `subscriber` on \
             EVERY answer it speaks — empty string for the tool lane — so an \
             edge without that comparison also collects every brief meant for \
             somebody else: {guard}"
        );

        let drain = &extra[2 + i];
        assert_eq!(
            drain["from"],
            json!(format!("{target}/{rim}")),
            "the receipt leaves the rim that answered it"
        );
        assert_eq!(
            drain["to"],
            json!("./assistants"),
            "and it stops at the container, where the member's own boundary \
             edge for `pack_ack` takes it the rest of the way out"
        );
        assert_eq!(
            drain["lane"],
            json!("pack_ack"),
            "the way back skips the same level and is declared the same way: \
             a v-lane is judged at BOTH of its endpoints: {drain}"
        );
        assert!(
            drain["condition"]
                .as_str()
                .unwrap_or_default()
                .contains("hop.route == 'pack_ack'"),
            "the drain is a plain route match — the substrate's own pairing \
             check reads it and nothing else: {drain}"
        );
    }
}

/// § 2d, the second opinion. The two routes the recipe draws are not this file's
/// opinion about what an assistant accepts: they are read out of the shipped
/// `assistant` contract, which pairs them itself. Drawing the push without the
/// drain is refused by the door with `required_drain_missing`, so a renderer
/// that drew one of the two would produce a manifest that cannot be applied.
#[test]
fn the_two_routes_are_the_pairing_the_assistant_template_declares() {
    let raw = std::fs::read_to_string(repo("templates/assistant/config.json"))
        .expect("the assistant template travels with the library");
    let tpl: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    let pairing = tpl["params"]["required_drains"]
        .as_array()
        .expect("required_drains")
        .iter()
        .find(|d| d["accepts"] == json!("in_pack"))
        .unwrap_or_else(|| {
            panic!("the assistant no longer pairs `in_pack` with a drain — the door would accept a push edge alone, and every receipt would dead-letter")
        })
        .clone();
    assert_eq!(
        pairing["emits"],
        json!("pack_ack"),
        "the pairing moved and the recipe still draws the old drain"
    );

    let extra: Vec<Value> = {
        let plain = assistant_edges(false);
        let all = assistant_edges(true);
        all[plain.len()..].to_vec()
    };
    assert_eq!(
        restamp(&extra[0]).as_deref(),
        pairing["accepts"].as_str(),
        "the push edge re-stamps onto the lane the template accepts"
    );
    assert!(
        extra[2]["condition"]
            .as_str()
            .unwrap_or_default()
            .contains(&format!(
                "hop.route == '{}'",
                pairing["emits"].as_str().unwrap_or_default()
            )),
        "and the drain takes the lane the template emits"
    );

    // GH #561 — and the SECOND thing the template now declares about the pair:
    // where a v-lane on it may dock. The renderer must not invent a connect
    // point, so the two ends it draws are read out of the `at` list, not typed.
    let raw = std::fs::read_to_string(repo("templates/assistant/config.json"))
        .expect("the assistant template travels with the library");
    let tpl: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    for (side, route) in [
        ("accepts", pairing["accepts"].as_str().unwrap_or_default()),
        ("emits", pairing["emits"].as_str().unwrap_or_default()),
    ] {
        let at = tpl["params"]["contract"][side]
            .as_array()
            .expect("a contract side")
            .iter()
            .find(|l| l["route"] == json!(route))
            .unwrap_or_else(|| panic!("the assistant declares `{route}`"))["at"]
            .clone();
        assert_eq!(
            at,
            json!(["./talky", "./cogny"]),
            "the connect points of `{route}` are what the door ends at; a \
             renderer drawing anywhere else is refused `v_lane_no_connect_point`"
        );
    }
    let drawn: Vec<String> = extra
        .iter()
        .take(2)
        .map(|e| {
            e["to"]
                .as_str()
                .unwrap_or_default()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        drawn,
        vec!["talky".to_string(), "cogny".to_string()],
        "and the two edges end at exactly those two rims, in that order: {extra:?}"
    );
}

/// § 2d, the prose half. Both surfaces a reader and a model meet say the door is
/// asked for rather than given, and say which half of a subscription a manifest
/// can carry at all — and the mechanism above is what makes both sentences true.
#[test]
fn both_surfaces_say_the_door_is_asked_for_and_the_row_is_not_a_mutation() {
    let readme = std::fs::read_to_string(repo("templates/builder/README.md"))
        .expect("the builder README travels with the template");
    let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("The identity door is opt-in"),
        "the README no longer publishes that the door is a parameter"
    );
    assert!(
        flat.contains("it does not write the `subscribers` row"),
        "the README must say which half of a subscription a manifest cannot \
         carry, or a reader will read the missing row as a defect"
    );

    let brief = compose_leg(emit_all(
        &shipped_script(BRIEF),
        &json!({
            "target": "/os/builder/brief",
            "header": {"hop": {"route": "brief"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "user", "type": "text", "id": "",
                          "text": "grow something"}],
        }),
    ));
    let text = brief["system"]["instructions"]["text"]
        .as_str()
        .expect("the briefing carries instructions");
    assert!(
        text.contains("IDENTITY DOOR"),
        "the design lane is never taught the two edges, so a wish with anything \
         extra in it reaches a model that has to invent them"
    );
    assert!(
        text.contains("Draw all four or none"),
        "the briefing must name the pairing: a push edge without its drain is \
         refused with `required_drain_missing` and nothing is applied"
    );

    // The mechanism: both sentences are true of the table.
    assert_eq!(
        assistant_edges(true).len() - assistant_edges(false).len(),
        4,
        "the surfaces describe a door of four v-lanes that is drawn on request; \
         the renderer must agree with them"
    );
}
