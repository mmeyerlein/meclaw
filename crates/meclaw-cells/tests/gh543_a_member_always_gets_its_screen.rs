//! GH #543 — a member grows a screen and an app, always, and the OS hands out
//! the port.
//!
//! WHAT THIS FILE IS
//! =================
//! A person in this substrate is the thing that has input and output devices.
//! Until now the fast lane grew the person and stopped: the screen and the
//! application that draws on it were a second, hand-written act, and every
//! colony that ever wanted one wrote the same two declarations again. Ruling
//! R-0904-4 closes that: **every member gets a screen and an app, always**, the
//! wish is not asked and cannot refuse, and the templates and the port base are
//! the builder's own configuration rather than anything the wish says.
//!
//! Three claims are measured here, and each one is measured positively:
//!
//! 1. **One manifest, in order.** One member wish leaves `recipes` as ONE
//!    `manifest` emission — the member first, its screen and app behind it. The
//!    first declaration is byte-unchanged
//!    (`examples/organism/grow-member.json`), the two behind it are
//!    `examples/organism/grow-screen.json`. That it is one emission and not two
//!    is GH #585: two submissions in the same turn have no order at the front,
//!    and the order is semantics.
//! 2. **The port is MEASURED, never claimed.** `screen_port_base + <index>`,
//!    where the index is how many members the organisation already carries, read
//!    off `/colony/graph` by the builder's own counting cell. A port nobody
//!    counted is the class of defect GH #517 made expensive.
//! 3. **The roll-forward holds.** The screen draws into `<member>/channels`, a
//!    scope only the declaration in front of it creates, and a manifest rolls
//!    forward with no rollback — so the order is not a preference: submitted on
//!    its own, before the person stands, the screen is refused, and that is
//!    asserted rather than assumed. Since GH #585 the order lives INSIDE one
//!    submission, where the door keeps it; the file that measures the wish
//!    against the front is
//!    `crates/meclaw-cells/tests/gh585_a_member_wish_is_one_submission.rs`.
//!
//! `the_os_hands_out_the_port` is the ADR anchor of
//! `plans/adr/0022-the-os-hands-out-what-is-system-near.md`: a colony carries
//! many organisations and ONE OS, and the OS is what allocates the system-near
//! things. The allocation in the builder is the first form of that
//! responsibility — the builder is part of the OS — and never an org's own
//! right.

use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ManifestOutcome, MutationDoorOutcome,
    MutationOutcome, RespawnFn, SpawnedCellKind, WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Headers, JsonValue, Message, Path, Uuid};
use meclaw_testing::{ColonyHandle, emit_all, shipped_script};
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

const RECIPES: &str = "templates/builder/recipes/config.json";
const TALLY: &str = "templates/builder/tally/config.json";
const CLASSIFY: &str = "templates/builder/classify/config.json";

/// The organisation the examples are written for, and the member they grow.
const ORG: &str = "/os/orgs/acme";
const MEMBER: &str = "alex";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Every level template this example instantiates, or nothing (GH #49).
fn shipped() -> bool {
    [
        RECIPES,
        TALLY,
        CLASSIFY,
        "examples/organism/grow-member.json",
        "examples/organism/grow-screen.json",
        "examples/organism/grow-os.json",
        "examples/organism/grow-org.json",
    ]
    .iter()
    .all(|f| repo(f).is_file())
}

fn library_is_complete() -> bool {
    ["meclaw-os", "org", "member", "display", "colony-view"]
        .iter()
        .all(|n| repo(&format!("templates/{n}/template.json")).is_file())
}

// ──────────────────────────────────────────────────────────────────────────────
// the renderer
// ──────────────────────────────────────────────────────────────────────────────

/// Every emission of `recipes` for one wish, with `hop.member_index` stamped
/// the way the counting cell stamps it.
fn run_recipes(payload: Value, member_index: &str) -> Vec<Value> {
    emit_all(
        &shipped_script(repo(RECIPES).to_str().expect("utf-8 path")),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe", "member_index": member_index},
                       "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": payload.to_string()}],
        }),
    )
}

/// The manifest emissions of one wish, in the order they left the cell. The
/// `bind` leg is not a manifest and is filtered out here rather than counted.
fn manifests(payload: Value, member_index: &str) -> Vec<Value> {
    run_recipes(payload, member_index)
        .into_iter()
        .filter(|m| m["header"]["operation"] == json!("recipe"))
        .collect()
}

fn member_wish(template: &str) -> Value {
    json!({"recipe": "grow_level", "request": "grow a member named alex",
           "params": {"scope": ORG, "level": "member", "name": MEMBER,
                      "template": template}})
}

/// The template `examples/organism/grow-member.json` currently names, so a
/// version bump of the member level does not make this file red.
fn member_template() -> String {
    read_json(&repo("examples/organism/grow-member.json"))["diff"]["add_nodes"][0]["template"]
        .as_str()
        .expect("the member example names a template")
        .to_string()
}

/// A declaration reduced to the three keys a declaration has, so a file that
/// omits an empty `ctx` and a renderer that writes one compare equal.
fn normalised(d: &Value) -> Value {
    json!({
        "scope": d["scope"],
        "ctx": if d["ctx"].is_object() { d["ctx"].clone() } else { json!({}) },
        "diff": d["diff"],
    })
}

#[test]
fn a_member_wish_renders_the_member_and_then_its_screen() {
    if !shipped() {
        return;
    }
    let out = manifests(member_wish(&member_template()), "0");
    assert_eq!(
        out.len(),
        1,
        "a member wish renders ONE manifest — the member, then the screen and \
         the app it always gets, in that order — and got {} instead (GH #585)",
        out.len()
    );
    assert_eq!(
        out[0]["header"]["declaration_count"],
        json!(3),
        "the person, the screen and the app: three containers, three storeys, \
         three declarations that cannot share a mutation (GH #503)"
    );
    assert_eq!(
        out[0]["manifest"][0]["scope"],
        json!("/os/orgs/acme/members"),
        "declaration 1 declares itself at the container the member grows into"
    );
    assert_eq!(
        out[0]["manifest"][1]["scope"],
        json!("/os/orgs/acme/members/alex/channels"),
        "the screen stands in the member's own channels, a scope only the \
         declaration in front of it creates"
    );
    assert_eq!(
        out[0]["manifest"][2]["scope"],
        json!("/os/orgs/acme/members/alex/apps"),
        "the app stands in the member's own apps"
    );
    assert!(
        out[0]["header"]["manifest_sha256"]
            .as_str()
            .is_some_and(|d| d.len() == 64),
        "one submission is one digest, and it is what the whole wish is refused \
         or committed under: {:?}",
        out[0]["header"]["manifest_sha256"]
    );
}

/// The ADR anchor. `plans/adr/0022-the-os-hands-out-what-is-system-near.md`.
///
/// A port is system-near: two screens on one port is one screen and one silent
/// degradation. The number is not in the wish and not in the template — the
/// builder, which is part of the OS, adds the member's index to its own base.
#[test]
fn the_os_hands_out_the_port() {
    if !shipped() {
        return;
    }
    let template = member_template();
    let port_of = |index: &str| -> Value {
        manifests(member_wish(&template), index)[0]["manifest"][1]["diff"]["add_nodes"][0]
            ["override_params"]["web"]["port"]
            .clone()
    };
    assert_eq!(
        port_of("0"),
        json!(7900),
        "the first member of an organisation gets the base port itself"
    );
    assert_eq!(
        port_of("1"),
        json!(7901),
        "the second member gets the next one: the index is what makes two \
         screens two sockets rather than one socket and one BindFailed"
    );
    assert_eq!(
        port_of("7"),
        json!(7907),
        "base + index, with nothing clever in between"
    );
}

/// The three values live twice — as the instance's own `params`, which is what
/// ships and what an operator overrides, and as the floor the script falls back
/// to when a config carries none. Two copies of one number drift; this is the
/// only place they are compared.
#[test]
fn the_shipped_configuration_and_the_renderers_floor_agree() {
    if !shipped() {
        return;
    }
    let cfg = read_json(&repo(RECIPES));
    let script = std::fs::read_to_string(repo(RECIPES)).expect("the recipe config");
    for key in [
        "member_screen_template",
        "member_app_template",
        "screen_port_base",
    ] {
        let value = &cfg["params"][key];
        assert!(
            !value.is_null(),
            "`{key}` is not a param of the recipe cell, so an instance cannot \
             override it and the OS cannot be told what it hands out"
        );
        let spelled = match value {
            Value::String(s) => format!("\"{s}\""),
            other => other.to_string(),
        };
        assert!(
            script.contains(&spelled),
            "the shipped `params.{key}` ({spelled}) is not the floor the script \
             falls back to — two copies of one default that can disagree"
        );
        assert_eq!(
            cfg["contract"]["settings"][key]["default"], *value,
            "`contract.settings.{key}.default` publishes a different value than \
             the instance actually carries"
        );
    }
}

#[test]
fn the_screen_manifest_is_the_shipped_example() {
    if !shipped() {
        return;
    }
    // The port the example carries decides which member it is written for, so
    // the index is read back OUT of it rather than assumed here.
    let want = read_json(&repo("examples/organism/grow-screen.json"));
    let decls = want["manifest"].as_array().expect("a manifest of two");
    assert_eq!(
        decls.len(),
        2,
        "grow-screen.json carries the screen and the app and nothing else: the \
         third declaration — the way back into a generation — moved into the \
         assistant level, which is the one place that knows the generation's \
         name"
    );
    let port = decls[0]["diff"]["add_nodes"][0]["override_params"]["web"]["port"]
        .as_u64()
        .expect("the screen example names a port");
    let index = port - 7900;
    let got = manifests(member_wish(&member_template()), &index.to_string());
    // The devices are the TAIL of the one manifest a member wish renders
    // (GH #585); the example file stays the operator-applicable half of it.
    let rendered: Vec<Value> = got[0]["manifest"].as_array().expect("the one manifest")[1..]
        .iter()
        .map(normalised)
        .collect();
    let shipped: Vec<Value> = decls.iter().map(normalised).collect();
    assert_eq!(
        rendered, shipped,
        "the rendered screen manifest and examples/organism/grow-screen.json \
         are not the same bytes — one is generated from the other's table and \
         they cannot drift apart"
    );
}

#[test]
fn the_way_back_from_a_screen_is_part_of_the_assistant_level() {
    if !shipped() {
        return;
    }
    // The member splits `event`/`receipt` on `hop.owner` and hands anything
    // under `/assistants/` to the container. The second hop — into the
    // generation the owner names — is the LEVEL's, because the ordinary
    // `in_turn` door is guarded on `context.assistant` and a screen event
    // carries none. Measured on a live colony: without it a screen event
    // reaches the container and stops there.
    let out = manifests(
        json!({"recipe": "grow_level", "request": "…",
               "params": {"scope": "/os/orgs/acme/members/alex", "level": "assistant",
                          "name": "scribe", "template": "a-template@1.0.0"}}),
        "0",
    );
    assert_eq!(
        out.len(),
        1,
        "a wish is ONE manifest, and only a member wish carries the devices in it"
    );
    let edges = out[0]["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .expect("the assistant level's edges")
        .clone();
    assert!(
        edges.iter().any(|e| {
            e["from"] == json!(".")
                && e["to"] == json!("./scribe")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("hop.owner.contains('/assistants/scribe/')"))
        }),
        "the assistant level does not draw the way back from a screen — a view \
         event whose owner is this generation reaches the container and stops"
    );
    // and it is no longer in the screen manifest
    let screen = read_json(&repo("examples/organism/grow-screen.json"));
    let raw = screen.to_string();
    assert!(
        !raw.contains("/assistants/"),
        "grow-screen.json still carries the generation's own way back; it \
         belongs to the level that knows the generation's name"
    );
}

/// The rendered `add_edges` of one declaration, in an `EdgeTable` the real
/// router can be asked. `.` is the container the declaration stands in.
fn table_of(decl: &Value, container: &str) -> EdgeTable {
    let abs = |ep: &str| -> String {
        match ep {
            "." | "./" => container.to_string(),
            other => format!("{container}/{}", other.trim_start_matches("./")),
        }
    };
    let mut t = EdgeTable::new();
    for e in decl["diff"]["add_edges"].as_array().expect("add_edges") {
        let condition = e["condition"].as_str().map(|src| {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|err| panic!("condition {src:?}: {err}"))
        });
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(e["from"].as_str().expect("from"))),
            to: Path::new(&abs(e["to"].as_str().expect("to"))),
            condition,
            modifier: None,
            is_default: e["default"].as_bool().unwrap_or(false),
            lane: None,
        });
    }
    t
}

fn headers(hop: Value, context: Value) -> Headers {
    let obj = |v: Value| match v {
        Value::Object(m) => m,
        _ => meclaw_core::serde_json::Map::new(),
    };
    Headers::from_parts(obj(context), obj(hop))
}

/// GH #543 — the way back and the ordinary door must be EXACT complements.
///
/// `apply_edges` fans out to every matching regular edge; there is no
/// exactly-one semantics in this substrate. `context` is persistent along a
/// trace, and nothing between an agent's answer and the receipt the display
/// sends back deletes `context.assistant` — so a receipt carries the context
/// AND the owner, and two edges that both accept it deliver the same turn to the
/// generation twice. Counted here, positively, on the real router.
#[test]
fn a_display_receipt_reaches_the_generation_exactly_once() {
    if !shipped() {
        return;
    }
    let assistants = "/os/orgs/acme/members/alex/assistants";
    let generation = format!("{assistants}/scribe");
    let out = manifests(
        json!({"recipe": "grow_level", "request": "…",
               "params": {"scope": "/os/orgs/acme/members/alex", "level": "assistant",
                          "name": "scribe", "template": "a-template@1.0.0"}}),
        "0",
    );
    let table = table_of(&out[0]["manifest"][0], assistants);
    let here = Path::new(assistants);
    let owner = format!("{generation}/talky");

    // The receipt: the owner the display stamped, AND the context the answer
    // that put the view up was carried on.
    let both = apply_edges(
        &table,
        &here,
        &headers(
            json!({"route": "in_turn", "owner": owner}),
            json!({"assistant": "scribe"}),
        ),
    );
    let hits: Vec<&str> = both
        .iter()
        .filter(|d| d.target.as_str() == generation)
        .map(|d| d.target.as_str())
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "a display receipt carrying both `context.assistant` and `hop.owner` is \
         delivered {} times to {generation} — `apply_edges` fans out to every \
         matching regular edge, so the door and the way back have to be exact \
         complements",
        hits.len()
    );

    // And the case the way back exists for: a screen EVENT, raised on a fresh
    // trace, with an owner and no context at all. It arrives, and it arrives
    // once.
    let event = apply_edges(
        &table,
        &here,
        &headers(json!({"route": "in_turn", "owner": owner}), json!({})),
    );
    let reached: Vec<&str> = event
        .iter()
        .filter(|d| d.target.as_str() == generation)
        .map(|d| d.target.as_str())
        .collect();
    assert_eq!(
        reached.len(),
        1,
        "a screen event whose owner names this generation does not reach it — \
         that is the hop a built colony was measured losing at the container"
    );
}

/// A number that arrived unreadable is NAMED, never rounded down to zero.
///
/// The silent version of this is the whole failure class the allocation exists
/// to prevent: a screen quietly given the base port is a second holder of a
/// socket somebody else already has, and it surfaces as a page that never loads.
#[test]
fn an_unreadable_number_refuses_instead_of_taking_the_base_port() {
    if !shipped() {
        return;
    }
    let template = member_template();

    // (a) the index the counting cell stamped is not a number
    let out = run_recipes(member_wish(&template), "abc");
    assert_eq!(
        out.len(),
        1,
        "a refusal answers ONCE and drafts nothing: {out:?}"
    );
    assert_eq!(out[0]["header"]["error_code"], json!("count_unavailable"));
    assert!(
        out[0]["manifest"].is_null(),
        "no manifest slot on a refusal — an empty manifest is a failure wearing \
         the face of an honest answer"
    );

    // (b) the base the builder is configured with is not a number. It used to
    // raise an uncaught ValueError and take the cell down with it.
    let broken = emit_all(
        &shipped_script(repo(RECIPES).to_str().expect("utf-8 path")),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe", "member_index": "1"},
                       "context": {}},
            "ttl": 64,
            "params": {"screen_port_base": "not-a-port"},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": member_wish(&template).to_string()}],
        }),
    );
    assert_eq!(
        broken[0]["header"]["error_code"],
        json!("count_unavailable"),
        "a builder configured with a port base that is not a number must refuse \
         by name, not crash the cell"
    );

    // and an ABSENT index is not that case: nobody counted, so this is the
    // first member and the base port is the answer.
    let absent = emit_all(
        &shipped_script(repo(RECIPES).to_str().expect("utf-8 path")),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": member_wish(&template).to_string()}],
        }),
    )
    .into_iter()
    .filter(|m| m["header"]["operation"] == json!("recipe"))
    .collect::<Vec<_>>();
    assert_eq!(
        absent[0]["manifest"][1]["diff"]["add_nodes"][0]["override_params"]["web"]["port"],
        json!(7900),
        "an absent index means nobody counted, which is a statement and not a \
         guess: this is the first member"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// the counting cell
// ──────────────────────────────────────────────────────────────────────────────

/// One leg of `tally`, driven the way the hive drives it.
fn run_tally(hop: Value, context: Value, body: Value) -> Vec<Value> {
    let mut input = json!({
        "target": "/os/builder/tally",
        "header": {"hop": hop, "context": context},
        "ttl": 64,
    });
    for (k, v) in body.as_object().expect("a body object") {
        input[k] = v.clone();
    }
    if input["messages"].is_null() {
        input["messages"] = json!([]);
    }
    emit_all(
        &shipped_script(repo(TALLY).to_str().expect("utf-8 path")),
        &input,
    )
}

/// The three legs of the count, with a real `/colony/graph` answer in the
/// middle. What is measured is that the index the cell stamps is the number of
/// members the graph showed — not a number the wish carried.
fn index_from(nodes: Vec<&str>) -> String {
    let wish = json!({"recipe": "grow_level", "request": "…",
                      "params": {"scope": ORG, "level": "member", "name": MEMBER,
                                 "template": "member@1.6.0"}});
    // leg 1: the wish arrives, the round is parked and the graph is asked
    let first = run_tally(
        json!({"route": "count", "recipe": "grow_level"}),
        json!({}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                             "text": wish.to_string()}]}),
    );
    let ask = first
        .iter()
        .find(|m| m["header"]["route"] == json!("graph"))
        .expect("the counting cell asks /colony/graph");
    assert_eq!(
        ask["query"]["scope"],
        json!("/os/orgs/acme/members"),
        "the count is taken over the organisation's own members container"
    );
    let tag = ask["query"]["tag"]
        .as_str()
        .expect("the round travels in the tag: a colony answer carries nothing else")
        .to_string();

    // leg 2: the answer comes back on a fresh trace, with an EMPTY context
    let second = run_tally(
        json!({}),
        json!({}),
        json!({"graph": {"scope": "/os/orgs/acme/members", "tag": tag,
                         "nodes": nodes.iter().map(|p| json!({"path": p, "cell_type": "hive"}))
                                       .collect::<Vec<_>>(),
                         "edges": []}}),
    );
    let read = second
        .iter()
        .find(|m| m["header"]["route"] == json!("cstore"))
        .expect("the counting cell reads its parked round back");
    read["header"]["member_index"]
        .as_str()
        .expect("the index rides in the hop so the store hop cannot lose it")
        .to_string()
}

#[test]
fn the_index_is_the_number_of_members_the_graph_showed() {
    if !shipped() {
        return;
    }
    assert_eq!(
        index_from(vec![]),
        "0",
        "an organisation with no members yet gives the first one the base port"
    );
    assert_eq!(
        index_from(vec!["/os/orgs/acme/members/blake"]),
        "1",
        "one member standing means the next index is 1"
    );
    assert_eq!(
        index_from(vec![
            "/os/orgs/acme/members/blake",
            "/os/orgs/acme/members/blake/talky",
            "/os/orgs/acme/members/blake/channels",
            "/os/orgs/acme/members/casey",
        ]),
        "2",
        "a member is a DIRECT child of the container: counting every node under \
         the prefix would count a person's own furniture as people"
    );
}

#[test]
fn the_wish_survives_the_colony_round_trip_with_the_index_stamped() {
    if !shipped() {
        return;
    }
    let wish = json!({"recipe": "grow_level", "request": "grow a member named alex",
                      "params": {"scope": ORG, "level": "member", "name": MEMBER,
                                 "template": "member@1.6.0"}});
    let parked = run_tally(
        json!({"route": "count", "recipe": "grow_level"}),
        json!({"build_caller": "/os/operator/intake"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "",
                             "text": wish.to_string()}]}),
    );
    let park = parked
        .iter()
        .find(|m| m["header"]["route"] == json!("cstore"))
        .expect("the wish is parked before the colony is asked");
    let op: Value = meclaw_core::serde_json::from_str(
        park["messages"][0]["text"].as_str().expect("the store op"),
    )
    .expect("json");
    assert_eq!(op["operation"], json!("insert"));
    let row_turn: Value =
        meclaw_core::serde_json::from_str(op["row"]["turn"].as_str().expect("the parked turn"))
            .expect("json");
    assert_eq!(
        row_turn["caller"]["build_caller"],
        json!("/os/operator/intake"),
        "the door the answer goes back to is parked with the wish: a colony \
         answer starts a fresh trace, so a caller that is not written down is a \
         caller that is gone"
    );

    // the last leg: the round table answers, and the wish leaves for `recipes`
    let handed = run_tally(
        json!({"operation": "select"}),
        json!({"store_origin": "tally", "tally_tag": "t-1", "member_index": "3"}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "t-round",
                             "text": json!([{"build_id": "t-1", "iter": 0, "role": "wish",
                                             "turn": json!({"payload": wish,
                                                            "caller": {"build_caller": "/os/operator/intake"}})
                                                     .to_string(),
                                             "fired": 0, "recorded_at": "z"}]).to_string()}]}),
    );
    let on = handed
        .iter()
        .find(|m| m["header"]["route"] == json!("recipe"))
        .expect("the wish goes on to the renderer");
    assert_eq!(
        on["header"]["member_index"],
        json!("3"),
        "the index the graph was read for is what the renderer is told"
    );
    assert_eq!(
        on["header"]["build_caller"],
        json!("/os/operator/intake"),
        "and the caller is put back on the hop, so the edge into `recipes` can \
         restore it: a modifier that reads a missing key fails and SKIPS"
    );
    let text = on["messages"][0]["text"].as_str().expect("the wish");
    let back: Value = meclaw_core::serde_json::from_str(text).expect("json");
    assert_eq!(
        back, wish,
        "the wish that reaches the renderer is the wish that arrived"
    );
}

#[test]
fn the_switch_sends_a_member_wish_to_be_counted_first() {
    if !shipped() {
        return;
    }
    let out = emit_all(
        &shipped_script(repo(CLASSIFY).to_str().expect("utf-8 path")),
        &json!({
            "target": "/os/builder/classify",
            "header": {"hop": {"route": "in_build"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_call", "id": "c1",
                "text": json!({"request": "…", "recipe": "grow_level",
                    "params": {"scope": ORG, "level": "member", "name": MEMBER,
                               "template": "member@1.6.0"}}).to_string()}],
        }),
    );
    assert_eq!(
        out[0]["header"]["route"],
        json!("count"),
        "a member wish takes the counting hop first — the screen it always gets \
         needs an index, and the renderer reads nothing"
    );
    let other = emit_all(
        &shipped_script(repo(CLASSIFY).to_str().expect("utf-8 path")),
        &json!({
            "target": "/os/builder/classify",
            "header": {"hop": {"route": "in_build"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_call", "id": "c1",
                "text": json!({"request": "…", "recipe": "grow_level",
                    "params": {"scope": "/os", "level": "org", "name": "acme",
                               "template": "org@1.3.0"}}).to_string()}],
        }),
    );
    assert_eq!(
        other[0]["header"]["route"],
        json!("recipe"),
        "every other level goes straight to the renderer: nothing else needs a \
         number the tree has to be read for"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// the roll-forward, against a real colony
// ──────────────────────────────────────────────────────────────────────────────

struct InertCellFactory;

impl CellFactory for InertCellFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn is_lazy(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        _path: Path,
        _params: JsonValue,
        _outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<std::time::Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<std::time::Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let capacity = mailbox_capacity.max(1);
        let (sender, receiver) = mpsc::channel::<Message>(capacity);
        let wake: WakeFn = Box::new(|mut rx: mpsc::Receiver<Message>| {
            tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let (stop_tx, _stop_rx) = oneshot::channel::<()>();
            let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
            (stop_tx, death_ack_rx)
        });
        let respawn: RespawnFn = Box::new(move || {
            let (tx, mut rx) = mpsc::channel::<Message>(capacity);
            let (peace_tx, peace_rx) = oneshot::channel::<()>();
            let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
            let join = tokio::spawn(async move {
                let _peace_keep = peace_tx;
                while rx.recv().await.is_some() {}
            });
            (tx, join, peace_rx, backstop_rx)
        });
        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        })
    }
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn cell_types_in(root: &std::path::Path) -> BTreeSet<String> {
    fn walk(dir: &std::path::Path, out: &mut BTreeSet<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().and_then(|n| n.to_str()) == Some("config.json")
                && let Ok(raw) = std::fs::read_to_string(&p)
                && let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw)
                && let Some(t) = v["cell"]["type"].as_str()
            {
                out.insert(t.to_string());
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, &mut out);
    out.remove("hive");
    out.remove("ref");
    out
}

fn build_root(root: &std::path::Path) {
    copy_tree(&repo("examples/organism/seed"), root);
    copy_tree(&repo("templates"), &root.join("templates"));
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\n\
         MODEL_BRAIN=gpt-4o-mock\n\
         MODEL_CORE=gpt-4o-mock\n\
         MODEL_CORE_FAST=gpt-4o-mock-fast\n\
         MODEL_SURFACE=gpt-4o-mock-surface\n\
         MODEL_CLOSER=gpt-4o-mock\n\
         MODEL_DIALECTIC=gpt-4o-mock\n\
         MODEL_DREAMER=gpt-4o-mock\n\
         TELEGRAM_BOT_TOKEN=test-token\n\
         TELEGRAM_BOT_TOKEN_2=test-token-2\n\
         TELEGRAM_ALLOWED_USER_ID=0\n\
         EXAMPLE_CHAT_TOKEN=test-chat-token\n",
    )
    .unwrap();
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let fs = factories(td.path());
    let h = ColonyHandle::new_with_factories_at(td, fs.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the empty seed of examples/organism must boot");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("GH #440: the rescan must not have aborted");
    h
}

fn factories(root: &std::path::Path) -> Vec<(String, Arc<dyn CellFactory>)> {
    cell_types_in(&root.join("templates"))
        .into_iter()
        .map(|t| (t, Arc::new(InertCellFactory) as Arc<dyn CellFactory>))
        .collect()
}

async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

fn committed(o: &MutationOutcome) -> bool {
    matches!(o, MutationOutcome::Committed { .. })
}

/// One BODY at the door, form unknown to the caller — the door `--apply`,
/// `POST /colony/mutations` and the submitter all knock on. A manifest is a
/// body, not a declaration, and it is judged as one.
async fn knock(h: &ColonyHandle, payload: Value) -> MutationDoorOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::MutationDoor {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send body");
    ack_rx.await.expect("door ack")
}

/// How many declarations of a manifest the door applied. A manifest rolls
/// forward and stops at the first refusal, so this number IS the verdict: a
/// refused entry leaves everything in front of it standing and everything
/// behind it untouched.
fn applied(o: &MutationDoorOutcome) -> usize {
    match o {
        MutationDoorOutcome::Manifest(ManifestOutcome::Committed { ids }) => ids.len(),
        MutationDoorOutcome::Manifest(ManifestOutcome::Rejected { ids, .. }) => ids.len(),
        _ => 0,
    }
}

/// The paths `/colony/graph` reports under a scope — the same read the counting
/// cell makes, made here to feed the counting cell a real answer.
async fn graph_nodes(h: &ColonyHandle, scope: &str) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new(scope),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .nodes
        .iter()
        .map(|n| n.path.to_string())
        .collect()
}

/// The whole claim, against a real colony: the index is read off the tree, the
/// three declarations apply in the order they were rendered, and only in that
/// order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_screen_lands_after_the_member_and_the_index_is_read_off_the_tree() {
    if !shipped() || !library_is_complete() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    build_root(td.path());
    let h = boot(&td).await;
    assert!(
        committed(&mutate(&h, read_json(&repo("examples/organism/grow-os.json"))).await),
        "the shell must be committed before anything is grown into it"
    );
    assert!(
        committed(&mutate(&h, read_json(&repo("examples/organism/grow-org.json"))).await),
        "the organisation must be committed before a member is grown into it"
    );

    // The count, taken the way the counting cell takes it.
    let before = graph_nodes(&h, "/os/orgs/acme/members").await;
    assert!(
        before.is_empty(),
        "the organisation has no members yet, so the first index is 0: {before:?}"
    );

    let out = manifests(member_wish(&member_template()), "0");
    let decls = out[0]["manifest"].as_array().expect("the one manifest");
    let whole = json!({"manifest": decls});
    let devices = json!({"manifest": decls[1..]});

    // THE ROLL-FORWARD, measured. The screen draws into a scope only the
    // declaration in front of it creates, so on its own it has nowhere to land.
    let early = knock(&h, devices).await;
    assert_eq!(
        applied(&early),
        0,
        "the devices applied something BEFORE the member existed — then the \
         order this recipe renders in would be decoration rather than \
         semantics: {early:?}"
    );
    let whole_outcome = knock(&h, whole).await;
    assert_eq!(
        applied(&whole_outcome),
        3,
        "the wish did not commit as ONE submission of three declarations — the \
         person, the screen, the app (GH #585): {whole_outcome:?}"
    );

    let grown = graph_nodes(&h, "/os/orgs/acme/members/alex").await;
    // A hive leaves no registry row, so both devices are named by an occupant
    // of theirs: the display's own socket, and the app's own layout.
    for want in [
        "/os/orgs/acme/members/alex/channels/display/web",
        "/os/orgs/acme/members/alex/apps/colony-view/layout",
    ] {
        assert!(
            grown.iter().any(|p| p == want),
            "{want} is not in the grown tree: {grown:?}"
        );
    }

    // and the index of the NEXT member is 1, read off the tree rather than
    // counted by this file
    let members = graph_nodes(&h, "/os/orgs/acme/members").await;
    let direct: BTreeSet<&str> = members
        .iter()
        .filter_map(|p| p.strip_prefix("/os/orgs/acme/members/"))
        .map(|rest| rest.split('/').next().unwrap_or(rest))
        .collect();
    assert_eq!(
        direct.len(),
        1,
        "one member stands, so the next screen takes 7901: {direct:?}"
    );
    h.shutdown().await;
}
