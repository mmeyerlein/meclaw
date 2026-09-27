//! GH #863 — every composite with a model door has a way back, and the road
//! back is a form the submit gate checks.
//!
//! Measured before the fix (plan R-registry § 1.7-8): a push the brain behind
//! an `in_model` door refused left it on the brain's ordinary out-edges -- in
//! `talky` onto `./errors` (a conversation's error), in `cogny` and `builder`
//! as `route error`, in `coder-pipeline`, `research-assistant` and
//! `summarizer` onto their error edges, and in the memory hive, `argus` and
//! `steward` on edges with NO condition at all, where the refusal arrived as
//! the cell's verdict.
//! What this file holds, read off the shipped tree:
//!
//! 1. **Every composite that accepts `in_model`**, for every llm cell its door
//!    reaches: exactly one way back (`./<cell> -> .` on
//!    `has(hop.refused_subscriber)`, stamping `model_refused`), and every other
//!    out-edge of that cell either excludes `hop.refused_subscriber` or takes
//!    only a successful `finish_reason` (`stop`, `tool_calls`, `length`) -- and
//!    the composite declares `model_refused` in its `emits`.
//! 2. **The shell** mirrors every push edge with a way back: for each
//!    `./llm-registry -> ./Y` edge on `update`, one `./Y -> ./llm-registry` edge
//!    on `model_refused` with the same address test on `refused_subscriber`,
//!    restamped `in_refused`; the registry accepts `in_refused`, and its door
//!    stamps `registry_origin 'refusal'` and nothing else can.
//! 3. **The recipe** draws one way back per composite -- three for an assistant
//!    (talky, talky-chat, cogny), one for a member (its memory hive) -- and the
//!    manifest passes the gate: parked and asked.
//! 4. **The gate** takes that form and no other: a modifier, another
//!    condition, a `to` that is not the container, a composite outside it, or
//!    the registry's lane named on any edge is `model_refusal_form`; writing
//!    or deleting `refused_subscriber`/`refused_model` is `model_road_key`.
//!    The code order is fixed: an edge that names `in_model` is still
//!    `model_push_form`.
//!
//! R2b / GH #49: every read is guarded by [`shipped`], so a tree without the
//! templates skips instead of failing on a dead reference.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{code_stdin, emit_all, run_shipped_script, shipped_script};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const RECIPES: &str = "templates/builder/recipes/config.json";
const GATE: &str = "templates/submit/gate/config.json";
const SHELL: &str = "templates/meclaw-os/config.json";
const REGISTRY: &str = "templates/llm-registry/config.json";
const GROW_ASSISTANT: &str = "examples/organism/grow-assistant.json";
const GROW_MEMBER: &str = "examples/organism/grow-member.json";

const SCOPE: &str = "/os/orgs";
const ORG: &str = "/os/orgs/acme";
const MEMBER: &str = "/os/orgs/acme/members/alex";
const OPERATOR: &str = "/os/operator/submit";
/// An agent inside `/os/orgs` -- the identity a model-driven submission carries.
const AGENT: &str = "/os/orgs/acme/members/alex/assistants/scribe/talky/brain";
/// Somebody else's generation, already in the tree.
const OTHER: &str = "./beta/members/bo/assistants/helper";

const REFUSAL_CONDITION: &str = "has(hop.route) && hop.route == 'model_refused'";

/// The composites of the library that carry a model door, as the fix found
/// them: the five the registry's shipped road reaches (the brains a generation
/// and a member bring, and the shell's `argus` and `builder`) and the six that
/// opened the door with GH #858 for a road somebody draws by hand. The test
/// reads the door off every template; this list is only the floor, so a
/// composite that loses its door does not pass by vanishing.
const DOORS_AT_LEAST: [&str; 11] = [
    "argus",
    "builder",
    "coder-pipeline",
    "cogny",
    "egon",
    "memory-hive",
    "research-assistant",
    "slack-agent",
    "steward",
    "summarizer",
    "talky",
];

fn shipped() -> bool {
    [RECIPES, GATE, SHELL, REGISTRY, GROW_ASSISTANT, GROW_MEMBER]
        .iter()
        .all(|p| repo(p).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn edges_of(cfg: &Value) -> Vec<Value> {
    cfg["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn cond(e: &Value) -> &str {
    e["condition"].as_str().unwrap_or_default()
}

/// Whether an out-edge of a model cell can never take a refusal: it excludes
/// the refusal by its key, or it takes a successful finish reason only.
fn cannot_take_a_refusal(e: &Value) -> bool {
    let c = cond(e);
    if c.contains("!has(hop.refused_subscriber)") {
        return true;
    }
    if !c.contains("has(hop.finish_reason)") || c.contains("hop.finish_reason !=") {
        return false;
    }
    let named: Vec<&str> = c
        .split("hop.finish_reason == '")
        .skip(1)
        .map(|rest| rest.split('\'').next().unwrap_or_default())
        .collect();
    !named.is_empty()
        && named
            .iter()
            .all(|v| ["stop", "tool_calls", "length"].contains(v))
}

/// Every shipped composite (a hive whose `accepts` names `in_model`), with the
/// cells its door edges reach.
fn model_doors() -> Vec<(String, Value, Vec<String>)> {
    let mut out = Vec::new();
    let mut names: Vec<String> = std::fs::read_dir(repo("templates"))
        .unwrap()
        .filter_map(|e| {
            let e = e.ok()?;
            e.path()
                .join("config.json")
                .is_file()
                .then(|| e.file_name().to_string_lossy().into_owned())
        })
        .collect();
    names.sort();
    for name in names {
        let cfg = read_json(&repo(&format!("templates/{name}/config.json")));
        let accepts = cfg["params"]["contract"]["accepts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if !accepts.iter().any(|l| l["route"] == "in_model") {
            continue;
        }
        let cells: Vec<String> = edges_of(&cfg)
            .iter()
            .filter(|e| e["from"] == "." && cond(e).contains("hop.route == 'in_model'"))
            .filter_map(|e| e["to"].as_str().map(str::to_string))
            .collect();
        out.push((name, cfg, cells));
    }
    out
}

#[test]
fn every_composite_with_a_model_door_sends_refusals_back() {
    if !shipped() {
        return;
    }
    let doors = model_doors();
    let found: Vec<&str> = doors.iter().map(|(n, _, _)| n.as_str()).collect();
    // A tree that ships a subset of the library (the public export) carries
    // only some of them; one that is on disk must still have its door.
    for name in DOORS_AT_LEAST
        .iter()
        .filter(|n| repo(&format!("templates/{n}/config.json")).is_file())
    {
        assert!(
            found.contains(name),
            "{name} no longer carries a model door: {found:?}"
        );
    }
    let mut missing = Vec::new();
    for (name, cfg, cells) in &doors {
        assert!(
            !cells.is_empty(),
            "{name}: a model door that reaches no cell"
        );
        let edges = edges_of(cfg);
        for cell in cells {
            let back: Vec<&Value> = edges
                .iter()
                .filter(|e| {
                    e["from"] == cell.as_str()
                        && e["to"] == "."
                        && cond(e) == "has(hop.refused_subscriber)"
                        && e["modifier"]["set_hop"] == json!({"route": "'model_refused'"})
                })
                .collect();
            if back.len() != 1 {
                missing.push(format!("{name}: {cell} has {} ways back", back.len()));
            }
            for e in edges.iter().filter(|e| e["from"] == cell.as_str()) {
                if back.contains(&e) {
                    continue;
                }
                if !cannot_take_a_refusal(e) {
                    missing.push(format!(
                        "{name}: {cell} -> {} takes a refusal: {}",
                        e["to"],
                        cond(e)
                    ));
                }
            }
        }
        let emits = cfg["params"]["contract"]["emits"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if !emits.iter().any(|l| l["route"] == "model_refused") {
            missing.push(format!("{name}: emits no model_refused"));
        }
    }
    assert!(
        missing.is_empty(),
        "a refused push must leave its composite on its own lane, never on a conversation's \
         error or as a verdict:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn the_shell_mirrors_every_push_edge_with_a_way_back() {
    if !shipped() {
        return;
    }
    let shell = read_json(&repo(SHELL));
    let edges = edges_of(&shell);
    let pushes: Vec<&Value> = edges
        .iter()
        .filter(|e| e["from"] == "./llm-registry" && cond(e).contains("hop.route == 'update'"))
        .collect();
    assert_eq!(pushes.len(), 3, "orgs, argus, builder: {pushes:?}");
    for p in pushes {
        let to = p["to"].as_str().unwrap_or_default();
        // The address test of the push, on the refusal's own key.
        let address = cond(p)
            .split("has(hop.subscriber) && ")
            .nth(1)
            .unwrap_or_default()
            .replace("hop.subscriber", "hop.refused_subscriber");
        assert!(!address.is_empty(), "{p}");
        let want = format!(
            "has(hop.route) && hop.route == 'model_refused' && has(hop.refused_subscriber) && \
             {address}"
        );
        let back: Vec<&Value> = edges
            .iter()
            .filter(|e| e["from"] == to && e["to"] == "./llm-registry" && cond(e) == want)
            .collect();
        assert_eq!(
            back.len(),
            1,
            "no mirror of the push into {to}: want `{want}`"
        );
        assert_eq!(
            back[0]["modifier"],
            json!({"set_hop": {"route": "'in_refused'"}, "delete_context": ["actor"]}),
            "{}",
            back[0]
        );
    }

    let registry = read_json(&repo(REGISTRY));
    let accepts = registry["params"]["contract"]["accepts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        accepts.iter().any(|l| l["route"] == "in_refused"),
        "the registry accepts the way back"
    );
    let redges = edges_of(&registry);
    let doors: Vec<&Value> = redges
        .iter()
        .filter(|e| e["from"] == "." && cond(e).contains("'in_refused'"))
        .collect();
    assert_eq!(doors.len(), 1, "{doors:?}");
    assert_eq!(doors[0]["to"], "./hand");
    assert_eq!(
        doors[0]["modifier"]["set_context"],
        json!({"registry_origin": "'refusal'"}),
        "the door stamps its own origin: {}",
        doors[0]
    );
    // No other edge of the registry stamps it, and the command door clears it.
    let stamps: Vec<&Value> = redges
        .iter()
        .filter(|e| e.to_string().contains("'refusal'"))
        .collect();
    assert_eq!(stamps.len(), 1, "{stamps:?}");
    let hand_door = redges
        .iter()
        .find(|e| e["from"] == "." && cond(e).contains("'in_hand'"))
        .expect("the command door");
    assert!(
        hand_door["modifier"]["delete_context"]
            .as_array()
            .is_some_and(|d| d.iter().any(|k| k == "registry_origin")),
        "a command can never pose as a refusal: {hand_door}"
    );
}

// ───────────────────────────────────────────────── the recipe and the gate

fn digest_of(decls: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    String::from_utf8(run_shipped_script(program, &decls.to_string()).stdout).expect("hex")
}

fn submit(decls: &Value, requester: &str) -> Vec<Value> {
    emit_all(
        &shipped_script(repo(GATE).to_str().unwrap()),
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

fn template_of(example: &str) -> String {
    read_json(&repo(example))["diff"]["add_nodes"][0]["template"]
        .as_str()
        .expect("the example names its template")
        .to_string()
}

/// What `grow_level` answers for one wish in a tree with a registry.
fn render(wish: Value) -> Vec<Value> {
    let doc = code_stdin(&json!({
        "target": "/os/builder/recipes",
        "header": {"hop": {"route": "recipe"}, "context": {}},
        "ttl": 64,
        "params": {"model_registry_scope": SCOPE},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                      "text": json!({"recipe": "grow_level", "request": "…",
                                     "params": wish}).to_string()}],
    }));
    let out = run_shipped_script(
        &shipped_script(repo(RECIPES).to_str().unwrap()),
        &doc.to_string(),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let all: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("json");
    let m = all
        .as_array()
        .and_then(|a| a.iter().find(|m| m["header"]["operation"] == "recipe"))
        .cloned()
        .expect("the recipe answered");
    assert!(m["header"]["error_code"].is_null(), "{m}");
    m["manifest"].as_array().cloned().expect("a manifest")
}

fn grown_assistant() -> Vec<Value> {
    render(
        json!({"scope": MEMBER, "level": "assistant", "name": "scribe",
                  "template": template_of(GROW_ASSISTANT),
                  "ctx": {"model": "${MODEL_CORE}", "model_fast": "${MODEL_CORE_FAST}",
                          "model_surface": "${MODEL_SURFACE}"}}),
    )
}

fn grown_member() -> Vec<Value> {
    render(json!({"scope": ORG, "level": "member", "name": "alex",
                  "template": template_of(GROW_MEMBER)}))
}

fn ways_back(manifest: &[Value]) -> Vec<Value> {
    manifest
        .iter()
        .filter(|d| d["scope"] == SCOPE)
        .flat_map(|d| {
            d["diff"]["add_edges"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|e| cond(e).contains("'model_refused'"))
        .collect()
}

#[test]
fn the_recipe_draws_one_way_back_per_rim_and_the_gate_takes_it() {
    if !shipped() {
        return;
    }
    let generation = "./acme/members/alex/assistants/scribe";
    let assistant = grown_assistant();
    let back = ways_back(&assistant);
    let want: Vec<Value> = ["cogny", "talky", "talky-chat"]
        .iter()
        .map(|rim| {
            json!({"from": format!("{generation}/{rim}"), "to": ".",
                          "condition": REFUSAL_CONDITION})
        })
        .collect();
    assert_eq!(back, want, "one way back per composite of the assistant");

    let member = grown_member();
    let back = ways_back(&member);
    assert_eq!(
        back,
        vec![json!({"from": "./acme/members/alex/memory-hive", "to": ".",
                    "condition": REFUSAL_CONDITION})],
        "one way back for the four memory cells: they share one composite"
    );

    for (what, manifest) in [("assistant", assistant), ("member", member)] {
        for requester in [OPERATOR, AGENT] {
            let out = submit(&Value::Array(manifest.clone()), requester);
            assert!(
                parked_and_asked(&out),
                "{what} by {requester}: the rendered road with its way back is the legal \
                 form: {out:?}"
            );
        }
    }
}

fn at_scope(edges: Value) -> Value {
    json!([{"scope": SCOPE, "diff": {"add_edges": edges}}])
}

#[test]
fn a_refusal_edge_in_any_other_form_is_refused() {
    if !shipped() {
        return;
    }
    let rim = format!("{OTHER}/talky");
    // The form itself, standing alone -- it is not bound to a generation this
    // manifest brings (the upgrade road for an older generation).
    let legal = json!({"from": rim, "to": ".", "condition": REFUSAL_CONDITION});
    let out = submit(&at_scope(json!([legal.clone()])), AGENT);
    assert!(parked_and_asked(&out), "{legal}: {out:?}");

    for (what, edge, code) in [
        (
            "a modifier on the way back",
            json!({"from": rim, "to": ".", "condition": REFUSAL_CONDITION,
                   "modifier": {"set_hop": {"route": "'in_refused'"}}}),
            "model_refusal_form",
        ),
        (
            "a wider condition",
            json!({"from": rim, "to": ".",
                   "condition": format!("{REFUSAL_CONDITION} || true")}),
            "model_refusal_form",
        ),
        (
            "into a foreign composite instead of the container",
            json!({"from": rim, "to": format!("{OTHER}/cogny"),
                   "condition": REFUSAL_CONDITION}),
            "model_refusal_form",
        ),
        (
            "from the container itself",
            json!({"from": ".", "to": ".", "condition": REFUSAL_CONDITION}),
            "model_refusal_form",
        ),
        // A composite outside the container: a way back carries only the
        // refusals of a composite under it -- relative past the container,
        // absolute elsewhere, or a sibling that shares only a string prefix
        // (`under` compares path segments, never characters).
        (
            "from a composite outside the container, relative",
            json!({"from": "../argus", "to": ".", "condition": REFUSAL_CONDITION}),
            "model_refusal_form",
        ),
        (
            "from a composite outside the container, absolute",
            json!({"from": "/os/builder", "to": ".", "condition": REFUSAL_CONDITION}),
            "model_refusal_form",
        ),
        (
            "from a sibling that shares only a string prefix",
            json!({"from": "../orgs-beta/talky", "to": ".",
                   "condition": REFUSAL_CONDITION}),
            "model_refusal_form",
        ),
        (
            "the registry's lane stamped by a submitted edge",
            json!({"from": rim, "to": ".",
                   "condition": "has(hop.route) && hop.route == 'tool'",
                   "modifier": {"set_hop": {"route": "'in_refused'"}}}),
            "model_refusal_form",
        ),
        (
            "the refusal lane stamped onto a tool call",
            json!({"from": rim, "to": ".",
                   "condition": "has(hop.route) && hop.route == 'tool'",
                   "modifier": {"set_hop": {"route": "'model_refused'"}}}),
            "model_refusal_form",
        ),
        (
            "a refusal forged by address",
            json!({"from": rim, "to": ".",
                   "condition": "has(hop.route) && hop.route == 'tool'",
                   "modifier": {"set_hop": {"refused_subscriber": format!("'{AGENT}'")}}}),
            "model_road_key",
        ),
        (
            "a refused model removed",
            json!({"from": rim, "to": ".",
                   "modifier": {"delete_hop": ["refused_model"]}}),
            "model_road_key",
        ),
        (
            "an edge naming both lanes is read as a push first",
            json!({"from": ".", "to": rim,
                   "condition": REFUSAL_CONDITION,
                   "modifier": {"set_hop": {"route": "'in_model'"}}}),
            "model_push_form",
        ),
    ] {
        for requester in [AGENT, OPERATOR] {
            let out = submit(&at_scope(json!([edge.clone()])), requester);
            assert_eq!(refused(&out), code, "{what} by {requester}: {out:?}");
            assert!(
                !out.iter().any(|m| m["header"]["route"] == "ask"),
                "{what}: a malformed road is refused without asking the broker: {out:?}"
            );
        }
    }
}
