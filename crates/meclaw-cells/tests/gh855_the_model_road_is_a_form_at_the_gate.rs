//! GH #855 — the model registry's road is a FORM the submit gate checks, the
//! way it checks the identity door of GH #458.
//!
//! In `meclaw-os` the registry's pushes arrive at `/os/orgs` restamped
//! `in_model`, and the builder draws one edge per brain from there onto the
//! brain's composite door. The shipped broker permits every submission scoped
//! at `/os/orgs` (`colony.mutate.default`), an agent's included. Without a form
//! check two edges were drawable there (review of the L2a strand, I-1):
//!
//!   * a REDIRECT — `{"from": ".", "to": "./<somebody else>/talky",
//!     "condition": "hop.route == 'in_model'"}` — copying every push of every
//!     subscriber into a brain nobody addressed, where the package keys act
//!     because the body is params-only;
//!   * a BRIDGE — an edge from a cell that emits tool calls up to the
//!     container, restamped `model_subscribe`, so a model-written tool call
//!     reaches the registry's hand on the lane the tree announces its brains on.
//!
//! What the gate lets through is exactly what the builder renders, and it
//! refuses everything else WITHOUT asking the broker, because a malformed road
//! is not a permission question:
//!
//!   * a PUSH edge (`in_model`) starts at the declaration's own container,
//!     ends at one composite under it, and carries only the pushes addressed
//!     to one llm cell standing directly in that composite (`hop.subscriber == '<to>/<cell>'`,
//!     one segment: a talky's brain, or since GH #858 a memory hive's four cells);
//!   * an ANNOUNCEMENT edge (`model_subscribe`) starts at a generation the SAME
//!     manifest brings into the world, fires on its mutation receipt only, and
//!     announces brains inside that generation, stamping the generation into
//!     `context.model_generation` and the brains into `context.model_announced`;
//!   * any other edge that names the road's lanes or its context keys is
//!     refused by name.
//!
//! A spelling check alone is not enough (review of fix round 1): the pushes
//! arrive at the container ALREADY stamped, a hive transit carries `hop`
//! unchanged, and `set_hop` is CEL. So the gate also refuses, on every edge
//! whatever it names, a write of the hop keys the road is addressed by
//! (`subscriber`, `subscribe` via `set_hop`; those two and `route` via
//! `delete_hop` -> `model_road_key`) and a computed `route` whose values it
//! cannot list (`model_route_computed`). What stays open is an edge that
//! leaves the road's keys and its route as they are -- no modifier, or one that
//! writes other keys, or `set_hop route "hop.route"` -- from the container or an
//! addressed composite into a foreign composite: it copies pushes the way any
//! forward copies messages, and under the shipped broker default `swap_nodes`
//! on a foreign brain is the same power. That residue is pinned below as a
//! broker question, and documented in the submit README.
//!
//! The renderer's output is driven into the gate, as in `gh479`: a hand-written
//! "legal" edge nobody emits would measure nothing.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{code_stdin, emit_all, run_shipped_script, shipped_script};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const GATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);
const GROW_ASSISTANT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/organism/grow-assistant.json"
);

const MEMBER: &str = "/os/orgs/acme/members/alex";
const SCOPE: &str = "/os/orgs";
const GEN: &str = "/os/orgs/acme/members/alex/assistants/scribe";
/// Somebody else's generation, already in the tree.
const OTHER: &str = "./beta/members/bo/assistants/helper";
const OPERATOR: &str = "/os/operator/submit";
/// An agent inside `/os/orgs` -- the identity a model-driven submission carries.
const AGENT: &str = "/os/orgs/acme/members/alex/assistants/scribe/talky/brain";

fn shipped() -> bool {
    [RECIPES, GATE, GROW_ASSISTANT]
        .iter()
        .all(|p| std::path::Path::new(p).is_file())
}

fn digest_of(decls: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    String::from_utf8(run_shipped_script(program, &decls.to_string()).stdout).expect("hex")
}

/// Phase A of the gate: a fresh submission from `requester`.
fn submit(decls: &Value, requester: &str) -> Vec<Value> {
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/operator/submit",
            "reply_to": requester,
            "header": {"hop": {"manifest_sha256": digest_of(decls)}, "context": {}},
            "ttl": 64,
            "manifest": decls,
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "op:c1",
                          "text": "{}"}],
            "params": {}
        }),
    )
}

/// The refusal code of a Phase-A answer, or "" when the manifest was parked
/// and its first question asked.
fn refused(out: &[Value]) -> String {
    out.iter()
        .find(|m| m["header"]["route"] == "receipt")
        .and_then(|m| m["header"]["error_code"].as_str())
        .unwrap_or_default()
        .to_string()
}

fn parked_and_asked(out: &[Value]) -> bool {
    refused(out).is_empty()
        && out.iter().any(|m| m["header"]["route"] == "sstore")
        && out.iter().any(|m| m["header"]["route"] == "ask")
}

/// The start values the shipped example grows its assistant with.
fn example_ctx() -> Value {
    json!({"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}",
           "model_surface": "${MODEL_SURFACE}"})
}

/// What `grow_level assistant` answers for `name` and `ctx` in a tree with a
/// registry -- the recipe's answer and what it wrote to stderr.
fn render_assistant(name: &str, ctx: &Value) -> (Value, String) {
    let raw = std::fs::read_to_string(GROW_ASSISTANT).expect("grow-assistant");
    let example: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    let template = example["diff"]["add_nodes"][0]["template"]
        .as_str()
        .expect("the example names its assistant")
        .to_string();
    let wish = json!({"recipe": "grow_level", "request": "grow an assistant",
                      "params": {"scope": MEMBER, "level": "assistant", "name": name,
                                 "template": template, "ctx": ctx}})
    .to_string();
    let doc = code_stdin(&json!({
        "target": "/os/builder/recipes",
        "header": {"hop": {"route": "recipe"}, "context": {}},
        "ttl": 64,
        "params": {"model_registry_scope": SCOPE},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "", "text": wish}],
    }));
    let out = run_shipped_script(&shipped_script(RECIPES), &doc.to_string());
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the recipe exited non-zero: {stderr}");
    let all: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("json out");
    let m = all
        .as_array()
        .and_then(|a| a.iter().find(|m| m["header"]["operation"] == "recipe"))
        .cloned()
        .expect("the recipe answered");
    assert!(m["header"]["error_code"].is_null(), "{m}");
    (m, stderr)
}

/// The manifest `grow_level assistant` renders in a tree with a registry.
fn grown() -> Value {
    let (m, _) = render_assistant("scribe", &example_ctx());
    let manifest = m["manifest"].clone();
    assert_eq!(
        manifest.as_array().map(Vec::len),
        Some(2),
        "the level and the road: {manifest}"
    );
    manifest
}

/// One declaration at the container, drawing `edges` and nothing else.
fn at_scope(edges: Value) -> Value {
    json!([{"scope": SCOPE, "diff": {"add_edges": edges}}])
}

const PUSH_COND: &str = "has(hop.route) && hop.route == 'in_model' && \
                         has(hop.subscriber) && hop.subscriber == '";

#[test]
fn the_road_the_builder_renders_passes_the_gate_and_is_asked_about() {
    if !shipped() {
        return;
    }
    let manifest = grown();
    let road = &manifest[1]["diff"]["add_edges"];
    let announce = road
        .as_array()
        .and_then(|e| e.iter().find(|e| e["to"] == "."))
        .expect("an announcement edge");
    assert_eq!(
        announce["modifier"]["set_context"]["model_generation"],
        format!("'{GEN}'"),
        "the announcement stamps its generation as EDGE truth: {announce}"
    );
    assert!(
        announce["modifier"]["set_context"]["model_announced"]
            .as_str()
            .is_some_and(|l| l.contains(&format!("{GEN}/talky/brain"))),
        "and the brains it announces, in the same context: {announce}"
    );
    assert_eq!(
        announce["modifier"]["set_hop"],
        json!({"route": "'model_subscribe'"}),
        "no hop key carries the brains any more: {announce}"
    );
    for requester in [OPERATOR, AGENT] {
        let out = submit(&manifest, requester);
        assert!(
            parked_and_asked(&out),
            "{requester}: the rendered road is the legal form: {out:?}"
        );
    }
}

#[test]
fn a_redirect_of_the_pushes_into_somebody_elses_brain_is_refused_unasked() {
    if !shipped() {
        return;
    }
    for edge in [
        // the review's edge: every push, into a foreign composite
        json!({"from": ".", "to": format!("{OTHER}/talky"),
               "condition": "has(hop.route) && hop.route == 'in_model'"}),
        // the rendered form, but addressed at somebody else's brain
        json!({"from": ".", "to": format!("{OTHER}/talky"),
               "condition": format!("{PUSH_COND}{GEN}/talky/brain'"),
               "modifier": {"set_hop": {"route": "'in_model'"}}}),
        // the rendered form with an escape hatch in the condition
        json!({"from": ".", "to": format!("{OTHER}/talky"),
               "condition": format!("{PUSH_COND}/os/orgs/beta/members/bo/assistants/helper/talky/brain' || true"),
               "modifier": {"set_hop": {"route": "'in_model'"}}}),
        // any other cell restamping its output onto the door
        json!({"from": format!("{OTHER}/talky"), "to": format!("{OTHER}/cogny"),
               "condition": "has(hop.route) && hop.route == 'tool'",
               "modifier": {"set_hop": {"route": "'in_model'"}}}),
        // the bare spelling of the lane counts too
        json!({"from": format!("{OTHER}/talky"), "to": format!("{OTHER}/cogny"),
               "modifier": {"set_hop": {"route": "in_model"}}}),
    ] {
        for requester in [AGENT, OPERATOR] {
            let out = submit(&at_scope(json!([edge.clone()])), requester);
            assert_eq!(
                refused(&out),
                "model_push_form",
                "{edge} by {requester}: {out:?}"
            );
            assert!(
                !out.iter().any(|m| m["header"]["route"] == "ask"),
                "a malformed road is refused without asking the broker: {out:?}"
            );
        }
    }
}

#[test]
fn a_bridge_from_a_tool_emitter_onto_the_announcement_lane_is_refused_unasked() {
    if !shipped() {
        return;
    }
    for edge in [
        // the review's bridge: a talky's tool output, restamped
        json!({"from": format!("{OTHER}/talky"), "to": ".",
               "condition": "has(hop.route) && hop.route == 'tool'",
               "modifier": {"set_hop": {"route": "'model_subscribe'"}}}),
        // the rendered form for a generation this manifest does NOT grow
        json!({"from": OTHER, "to": ".",
               "condition": "has(hop.route) && hop.route == 'mutation_committed'",
               "modifier": {"set_hop": {"route": "'model_subscribe'"},
                            "set_context": {"model_generation": "'/os/orgs/beta/members/bo/assistants/helper'",
                                            "model_announced": "'[{\"cell_path\":\"/os/orgs/beta/members/bo/assistants/helper/talky/brain\",\"start_model\":\"x\"}]'"}}}),
        // an edge that only forges the two context keys
        json!({"from": format!("{OTHER}/talky"), "to": ".",
               "condition": "has(hop.route) && hop.route == 'tool'",
               "modifier": {"set_context": {"model_announcer": "'meclaw-os'",
                                            "model_generation": format!("'{GEN}'")}}}),
    ] {
        let out = submit(&at_scope(json!([edge.clone()])), AGENT);
        assert_eq!(refused(&out), "model_announcement_form", "{edge}: {out:?}");
    }

    // The rendered manifest with ONE thing changed each time: the announced
    // brains leave the generation, the generation stamp lies, the receipt
    // condition widens.
    let mutate = |f: &dyn Fn(&mut Value)| {
        let mut m = grown();
        let edges = m[1]["diff"]["add_edges"].as_array_mut().expect("edges");
        let a = edges.iter_mut().find(|e| e["to"] == ".").expect("announce");
        f(a);
        m
    };
    for (what, m) in [
        (
            "a brain outside the generation",
            mutate(&|a| {
                a["modifier"]["set_context"]["model_announced"] = json!(
                    "'[{\"cell_path\":\"/os/orgs/beta/members/bo/assistants/helper/talky/brain\",\"start_model\":\"x\"}]'"
                )
            }),
        ),
        (
            "a start value that breaks out of its CEL literal (m-1)",
            mutate(&|a| {
                a["modifier"]["set_context"]["model_announced"] = json!(format!(
                    "'[{{\"cell_path\":\"{GEN}/talky/brain\",\"start_model\":\"x' + 'y\"}}]'"
                ))
            }),
        ),
        (
            "a generation stamp that is not the source",
            mutate(&|a| {
                a["modifier"]["set_context"]["model_generation"] =
                    json!("'/os/orgs/beta/members/bo/assistants/helper'")
            }),
        ),
        (
            "a condition wider than the receipt",
            mutate(&|a| a["condition"] = json!("has(hop.route)")),
        ),
    ] {
        let out = submit(&m, AGENT);
        assert_eq!(refused(&out), "model_announcement_form", "{what}: {out:?}");
    }
}

#[test]
fn a_manifest_without_the_road_is_not_touched_by_the_form() {
    if !shipped() {
        return;
    }
    let out = submit(
        &at_scope(json!([{"from": ".", "to": "./acme",
                          "condition": "has(hop.route) && hop.route == 'in_turn'"}])),
        AGENT,
    );
    assert!(parked_and_asked(&out), "{out:?}");
}

/// Review of fix round 1, I-1 (i) and I-2: the road is addressed by hop keys,
/// and a hop key survives a hive transit. So no submitted edge may write them,
/// whatever else it names: `set_hop` of `subscriber` or `subscribe`, and
/// `delete_hop` of those two or of `route`. The builder renders none of these
/// (its announcement carries the brains in the context since fix round 2).
#[test]
fn an_edge_that_writes_the_road_keys_is_refused_whatever_it_is_named() {
    if !shipped() {
        return;
    }
    let brain = format!("'{GEN}/talky/brain'");
    let forged = format!("'[{{\"cell_path\":\"{GEN}/talky/brain\",\"start_model\":\"x\"}}]'");
    for (what, edges) in [
        (
            "the review's pair: a detour through an own hive, and a way back \
             that rewrites the brains of a real announcement",
            json!([
                {"from": ".", "to": "./acme/x",
                 "condition": "has(hop.subscribe) && !has(hop.again)"},
                {"from": "./acme/x", "to": ".", "condition": "has(hop.subscribe)",
                 "modifier": {"set_hop": {"subscribe": forged, "again": "'1'"}}}
            ]),
        ),
        (
            "a push readdressed at somebody's brain",
            json!([{"from": format!("{OTHER}/talky"), "to": ".",
                    "condition": "has(hop.route) && hop.route == 'tool'",
                    "modifier": {"set_hop": {"subscriber": brain}}}]),
        ),
        (
            "an address removed",
            json!([{"from": ".", "to": "./acme",
                    "modifier": {"delete_hop": ["subscriber"]}}]),
        ),
        (
            "an announcement removed",
            json!([{"from": ".", "to": "./acme",
                    "modifier": {"delete_hop": ["subscribe"]}}]),
        ),
        (
            "a lane removed",
            json!([{"from": ".", "to": "./acme",
                    "modifier": {"delete_hop": ["route"]}}]),
        ),
    ] {
        for requester in [AGENT, OPERATOR] {
            let out = submit(&at_scope(edges.clone()), requester);
            assert_eq!(
                refused(&out),
                "model_road_key",
                "{what} by {requester}: {out:?}"
            );
            assert!(
                !out.iter().any(|m| m["header"]["route"] == "ask"),
                "refused without asking the broker: {out:?}"
            );
        }
    }
    // The rendered announcement with the brains on the hop again.
    let mut m = grown();
    let edges = m[1]["diff"]["add_edges"].as_array_mut().expect("edges");
    let a = edges.iter_mut().find(|e| e["to"] == ".").expect("announce");
    a["modifier"]["set_hop"]["subscribe"] = json!(forged);
    assert_eq!(refused(&submit(&m, AGENT)), "model_road_key");
}

/// Review of fix round 1, I-1 (i): `set_hop` is CEL, so `'in_' + 'model'`
/// stamps the lane without spelling it. The gate cannot tell a brain door by
/// its name, so it asks the question of EVERY edge: a computed route passes
/// only when every value it can take is a literal written in it -- string
/// literals, `hop.route`, comparisons and the ternary -- and none of them is a
/// road lane. The one computed route the builder renders (`grow_screen`'s
/// view/withdraw ternary) is such a route.
#[test]
fn a_route_the_gate_cannot_enumerate_is_refused() {
    if !shipped() {
        return;
    }
    for (to, route) in [
        (format!("{OTHER}/talky"), "'in_' + 'model'"),
        (".".to_string(), "'model_' + 'subscribe'"),
        (format!("{OTHER}/cogny"), "hop.tool_name"),
        (format!("{OTHER}/cogny"), "has(hop.lane) ? hop.lane : 'x'"),
        (format!("{OTHER}/cogny"), "'in_\\x6dodel'"),
        (format!("{OTHER}/cogny"), "r'x'"),
    ] {
        let edge = json!({"from": format!("{OTHER}/talky"), "to": to,
                          "condition": "has(hop.route) && hop.route == 'tool'",
                          "modifier": {"set_hop": {"route": route}}});
        let out = submit(&at_scope(json!([edge.clone()])), AGENT);
        assert_eq!(refused(&out), "model_route_computed", "{edge}: {out:?}");
    }
    for route in [
        "hop.route == 'withdraw' ? 'in_withdraw' : 'in_view'",
        "hop.route",
        "'in_turn'",
    ] {
        let edge = json!({"from": format!("{OTHER}/talky"), "to": format!("{OTHER}/cogny"),
                          "condition": "has(hop.route)",
                          "modifier": {"set_hop": {"route": route}}});
        let out = submit(&at_scope(json!([edge.clone()])), AGENT);
        assert!(parked_and_asked(&out), "{edge}: {out:?}");
    }
}

/// Review of fix round 1, m-1: the push form is compared byte for byte, and a
/// path is written into its condition as a CEL literal -- so a `to` that is no
/// literal-safe string would let `' || true || '` through as "the form".
#[test]
fn a_push_edge_whose_path_breaks_its_literal_is_refused() {
    if !shipped() {
        return;
    }
    let to = "./x' || true || 'y/talky";
    let edge = json!({"from": ".", "to": to,
                      "condition": format!("{PUSH_COND}/os/orgs/x' || true || 'y/talky/brain'"),
                      "modifier": {"set_hop": {"route": "'in_model'"}}});
    let out = submit(&at_scope(json!([edge.clone()])), AGENT);
    assert_eq!(refused(&out), "model_push_form", "{edge}: {out:?}");
}

/// OR-SN.L2a.14 -- the residue, pinned so the README sentence stays measured:
/// an edge that leaves the road's keys and its route as they are names nothing
/// the gate reads, so it is a broker question like any other edge -- with no
/// modifier, with one that writes other keys, or with `set_hop route
/// "hop.route"`, which keeps the lane it found. Under the shipped default the
/// broker permits each at `/os/orgs`, exactly as it permits a `swap_nodes` on a
/// foreign brain.
#[test]
fn a_bare_forward_stays_a_broker_question() {
    if !shipped() {
        return;
    }
    for modifier in [
        None,
        Some(json!({"set_context": {"note": "'x'"}, "set_hop": {"lane": "'x'"}})),
        Some(json!({"set_hop": {"route": "hop.route"}})),
    ] {
        let mut edge = json!({"from": ".", "to": format!("{OTHER}/talky"),
                              "condition": "has(hop.subscriber)"});
        if let Some(m) = modifier {
            edge["modifier"] = m;
        }
        let out = submit(&at_scope(json!([edge.clone()])), AGENT);
        assert!(parked_and_asked(&out), "{edge}: {out:?}");
    }
}

/// Review of fix round 2, I-1: the closed-route check stripped `hop.route`,
/// `true`, `false` and its literal placeholder without a word boundary, so
/// `hop.routetrue` -- an ordinary hop key in CEL -- counted as closed. Two
/// edges then stamped `in_model` computed: one writes the lane into a key of its
/// own choosing, the next copies it into `route`. A key that only BEGINS like
/// `route` is a computed value like any other.
#[test]
fn a_route_read_from_a_key_that_only_begins_like_route_is_refused() {
    if !shipped() {
        return;
    }
    let pair = json!([
        {"from": ".", "to": "./acme/x",
         "modifier": {"set_hop": {"routetrue": "'in_' + 'model'"}}},
        {"from": "./acme/x", "to": format!("{OTHER}/talky"),
         "modifier": {"set_hop": {"route": "hop.routetrue"}}}
    ]);
    for requester in [AGENT, OPERATOR] {
        let out = submit(&at_scope(pair.clone()), requester);
        assert_eq!(
            refused(&out),
            "model_route_computed",
            "{requester}: {out:?}"
        );
        assert!(
            !out.iter().any(|m| m["header"]["route"] == "ask"),
            "refused without asking the broker: {out:?}"
        );
    }
    for route in [
        "hop.routetrue",
        "hop.routefalse",
        "hop.routeL",
        "hop.routeLtrue",
        "hop.route_x",
        "hop.route.x",
        "hop.route == 'x' ? hop.routeL : 'y'",
        "has(hop.routetrue) ? hop.routetrue : 'in_turn'",
        "truehop",
        "hop.route\u{0}",
    ] {
        let edge = json!({"from": format!("{OTHER}/talky"), "to": format!("{OTHER}/cogny"),
                          "condition": "has(hop.route)",
                          "modifier": {"set_hop": {"route": route}}});
        let out = submit(&at_scope(json!([edge.clone()])), AGENT);
        assert_eq!(refused(&out), "model_route_computed", "{edge}: {out:?}");
    }
    for route in [
        "hop.route == 'withdraw' ? 'in_withdraw' : 'in_view'",
        "has(hop.route) && hop.route == 'x' ? 'in_x' : hop.route",
        "!has(hop.route) ? 'in_turn' : (hop.route)",
    ] {
        let edge = json!({"from": format!("{OTHER}/talky"), "to": format!("{OTHER}/cogny"),
                          "condition": "has(hop.route)",
                          "modifier": {"set_hop": {"route": route}}});
        let out = submit(&at_scope(json!([edge.clone()])), AGENT);
        assert!(parked_and_asked(&out), "{edge}: {out:?}");
    }
}

/// Review of fix round 2, I-2: the recipe wrote the announced brains with
/// `ensure_ascii`, so a name beyond ASCII became a `\uXXXX` escape -- a
/// backslash in a CEL literal -- and the gate refused the WHOLE growth, not
/// just its road, naming the wrong cause. A node name is not limited to ASCII.
#[test]
fn a_generation_named_beyond_ascii_is_grown_with_its_road() {
    if !shipped() {
        return;
    }
    let (m, stderr) = render_assistant("bücher", &example_ctx());
    let manifest = m["manifest"].clone();
    assert_eq!(
        manifest.as_array().map(Vec::len),
        Some(2),
        "the level and the road: {manifest}"
    );
    let announce = manifest[1]["diff"]["add_edges"]
        .as_array()
        .and_then(|e| e.iter().find(|e| e["to"] == "."))
        .expect("an announcement edge");
    let announced = announce["modifier"]["set_context"]["model_announced"]
        .as_str()
        .expect("a literal");
    assert!(
        announced.contains("/assistants/bücher/talky/brain") && !announced.contains('\\'),
        "written as it is, no escape: {announced}"
    );
    assert!(stderr.is_empty(), "nothing left out: {stderr}");
    for requester in [OPERATOR, AGENT] {
        let out = submit(&manifest, requester);
        assert!(
            parked_and_asked(&out),
            "{requester}: the rendered road is the legal form: {out:?}"
        );
    }
}

/// Review of fix round 2, I-2, second half: a brain whose announcement entry
/// is still no plain CEL literal once written as JSON (a `"` in its start value
/// becomes `\"`) is left out of the road -- and said so, on stderr and in the
/// recipe's answer, never silently. The rest of the road and the growth pass.
#[test]
fn a_brain_whose_announcement_is_no_literal_is_left_out_and_said() {
    if !shipped() {
        return;
    }
    let ctx = json!({"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}",
                     "model_surface": "vendor/\"quoted\""});
    let (m, stderr) = render_assistant("scribe", &ctx);
    let manifest = m["manifest"].clone();
    let road = manifest[1]["diff"]["add_edges"]
        .as_array()
        .cloned()
        .expect("the road still stands for the brain that is a literal");
    let pushes: Vec<&Value> = road.iter().filter(|e| e["to"] != ".").collect();
    assert_eq!(pushes.len(), 1, "only cogny's push edge: {road:?}");
    assert_eq!(
        pushes[0]["to"],
        "./acme/members/alex/assistants/scribe/cogny"
    );
    let announced = road
        .iter()
        .find(|e| e["to"] == ".")
        .and_then(|e| e["modifier"]["set_context"]["model_announced"].as_str())
        .expect("an announcement")
        .to_string();
    assert!(
        announced.contains("/cogny/brain") && !announced.contains("/talky/brain"),
        "{announced}"
    );
    for skipped in [
        format!("{GEN}/talky/brain"),
        format!("{GEN}/talky-chat/brain"),
    ] {
        assert!(
            stderr.contains(&skipped),
            "stderr names {skipped}: {stderr}"
        );
        let text = m["messages"][0]["text"].as_str().unwrap_or_default();
        assert!(
            text.contains(&skipped),
            "the answer names {skipped}: {text}"
        );
    }
    for requester in [OPERATOR, AGENT] {
        let out = submit(&manifest, requester);
        assert!(parked_and_asked(&out), "{requester}: {out:?}");
    }
}

/// Review of L2a round 4, m-2: a CEL string literal cannot hold a raw line
/// break, and the substrate reserves only `~` in a node name -- so a path with
/// `\n` or `\r` passed the literal check and failed only when the colony
/// compiled the push condition. The gate refuses it as the form it is not.
#[test]
fn a_push_edge_whose_path_holds_a_raw_line_break_is_refused() {
    if !shipped() {
        return;
    }
    for brk in ["\n", "\r"] {
        let to = format!("./x{brk}y/talky");
        let edge = json!({"from": ".", "to": to,
                          "condition": format!("{PUSH_COND}/os/orgs/x{brk}y/talky/brain'"),
                          "modifier": {"set_hop": {"route": "'in_model'"}}});
        let out = submit(&at_scope(json!([edge.clone()])), AGENT);
        assert_eq!(refused(&out), "model_push_form", "{edge}: {out:?}");
    }
}

/// Review of L2a round 4, m-1: the recipe says WHY it leaves a brain out, and
/// the reason has to be the cause -- a start value that is not a string is not
/// "no start value", and a `'` is not a JSON escape.
#[test]
fn a_brain_left_out_is_named_with_its_real_cause() {
    if !shipped() {
        return;
    }
    let ctx = json!({"model": 5, "model_fast": "${MODEL_CORE_FAST}",
                     "model_surface": "vendor/it's"});
    let (_m, stderr) = render_assistant("scribe", &ctx);
    assert!(
        stderr.contains(&format!(
            "{GEN}/cogny/brain (its start value in ctx.model is not a string)"
        )),
        "a number is a start value that is not a string: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "{GEN}/talky/brain (its start value holds a quote, a backslash or a line break)"
        )),
        "a `'` is named as what it is: {stderr}"
    );
    assert!(!stderr.contains("no start value"), "{stderr}");

    // Review of the fix strand, m-1: since round 4 m-2 the same check refuses a
    // raw line break, so the reason names it too.
    for brk in ["\n", "\r"] {
        let ctx = json!({"model": "vendor/deep", "model_fast": "${MODEL_CORE_FAST}",
                         "model_surface": format!("vendor/a{brk}b")});
        let (_m, stderr) = render_assistant("scribe", &ctx);
        assert!(
            stderr.contains(&format!(
                "{GEN}/talky/brain (its start value holds a quote, a backslash or a line break)"
            )),
            "a raw line break is named with its cause: {stderr:?}"
        );
    }
}
