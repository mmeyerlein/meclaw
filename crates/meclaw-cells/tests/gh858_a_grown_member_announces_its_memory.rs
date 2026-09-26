//! GH #858 — a member grown by the builder in a tree with a model registry
//! makes the four llm cells of its memory hive subscribers, the way GH #855
//! made the brains of an assistant generation subscribers.
//!
//! `grow_level member` renders a SECOND declaration at the builder's
//! `model_registry_scope`: one push edge per memory cell onto the hive's
//! `in_model` door, addressed by the cell's path, and one announcement edge
//! from the member on its mutation receipt, carrying each cell's start token
//! and its prose need. What is claimed, measured by running the shipped recipe
//! over stdin and driving its manifest into the shipped submit gate (as in
//! `gh855_the_model_road_is_a_form_at_the_gate`):
//!
//! 1. with the setting, the member's manifest carries the road; without it, it
//!    carries none;
//! 2. the push edges end at `<member>/memory-hive` and each carries only the
//!    pushes addressed to one cell directly in it;
//! 3. the announcement names the four cells, each on the token it is born on,
//!    with the need its template cell states;
//! 4. the gate lets that manifest through for an agent and for the operator
//!    and asks the broker about it; a push edge two segments deep, one onto
//!    somebody else's memory, an announced key it does not know and a need
//!    over the bound are refused by name.

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
const GROW_MEMBER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/organism/grow-member.json"
);

const ORG: &str = "/os/orgs/acme";
const SCOPE: &str = "/os/orgs";
const MEMBER: &str = "/os/orgs/acme/members/alex";
const OPERATOR: &str = "/os/operator/submit";
const AGENT: &str = "/os/orgs/acme/members/alex/assistants/scribe/talky/brain";
const CELLS: [&str; 4] = ["closer", "dialectic", "dreamer", "judge"];

fn shipped() -> bool {
    [RECIPES, GATE, GROW_MEMBER]
        .iter()
        .all(|p| std::path::Path::new(p).is_file())
}

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The member template the shipped example grows.
fn member_template() -> String {
    read_json(std::path::Path::new(GROW_MEMBER))["diff"]["add_nodes"][0]["template"]
        .as_str()
        .expect("the example names its member")
        .to_string()
}

/// What `grow_level member` answers with the builder's `settings`: the manifest
/// and what the recipe wrote to stderr.
fn render(settings: Value) -> (Vec<Value>, String) {
    let wish = json!({"recipe": "grow_level", "request": "grow a member",
                      "params": {"scope": ORG, "level": "member", "name": "alex",
                                 "template": member_template()}})
    .to_string();
    let doc = code_stdin(&json!({
        "target": "/os/builder/recipes",
        "header": {"hop": {"route": "recipe"}, "context": {}},
        "ttl": 64,
        "params": settings,
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
    let manifest = m["manifest"].as_array().cloned().expect("a manifest");
    (manifest, stderr)
}

fn road_of(manifest: &[Value]) -> Option<Value> {
    manifest.iter().find(|d| d["scope"] == SCOPE).cloned()
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

#[test]
fn a_grown_member_brings_the_road_of_its_memory_only_where_a_registry_is() {
    if !shipped() {
        return;
    }
    let (plain, _) = render(json!({}));
    assert!(road_of(&plain).is_none(), "no registry, no road: {plain:?}");

    let (with, stderr) = render(json!({"model_registry_scope": SCOPE}));
    assert!(
        stderr.trim().is_empty(),
        "the recipe left a cell out: {stderr}"
    );
    assert_eq!(
        with.len(),
        plain.len() + 1,
        "the member's own declarations do not move, and one road is added: {with:?}"
    );
    let road = road_of(&with).expect("a declaration at the registry's scope");
    assert!(
        road["diff"].get("seed_rows").is_none() && road["diff"].get("add_nodes").is_none(),
        "the road draws edges and writes nothing"
    );
    let edges = road["diff"]["add_edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let pushes: Vec<&Value> = edges.iter().filter(|e| e["from"] == ".").collect();
    assert_eq!(pushes.len(), 4, "one push edge per memory cell: {pushes:?}");
    for (cell, edge) in CELLS.iter().zip(&pushes) {
        assert_eq!(edge["to"], "./acme/members/alex/memory-hive");
        assert_eq!(
            edge["condition"],
            format!(
                "has(hop.route) && hop.route == 'in_model' && has(hop.subscriber) && \
                 hop.subscriber == '{MEMBER}/memory-hive/{cell}'"
            )
        );
        assert_eq!(
            edge["modifier"],
            json!({"set_hop": {"route": "'in_model'"}})
        );
    }

    let announce: Vec<&Value> = edges.iter().filter(|e| e["to"] == ".").collect();
    assert_eq!(announce.len(), 1, "{edges:?}");
    assert_eq!(announce[0]["from"], "./acme/members/alex");
    assert_eq!(
        announce[0]["condition"],
        "has(hop.route) && hop.route == 'mutation_committed'"
    );
    let set = &announce[0]["modifier"]["set_context"];
    assert_eq!(set["model_generation"], format!("'{MEMBER}'"));
    let lit = set["model_announced"].as_str().unwrap_or_default();
    let entries: Value =
        meclaw_core::serde_json::from_str(lit.trim_matches('\'')).expect("a JSON literal");
    let expected: Vec<Value> = CELLS
        .iter()
        .map(|cell| {
            let cfg = read_json(&repo(&format!("templates/memory-hive/{cell}/config.json")));
            let born = cfg["params"]["model"].as_str().unwrap_or_default();
            // A plain `${VAR}` is announced `${VAR:-}`: an unset key must not
            // fail the whole growth on an edge.
            let start = if born.contains(":-") {
                born.to_string()
            } else {
                format!("{}:-}}", born.trim_end_matches('}'))
            };
            json!({"cell_path": format!("{MEMBER}/memory-hive/{cell}"),
                   "start_model": start,
                   "requirement": cfg["params"]["requirement"]})
        })
        .collect();
    assert_eq!(entries, Value::Array(expected));
}

#[test]
fn the_gate_takes_the_members_road_and_asks_about_it() {
    if !shipped() {
        return;
    }
    let (with, _) = render(json!({"model_registry_scope": SCOPE}));
    let manifest = Value::Array(with);
    for requester in [AGENT, OPERATOR] {
        let out = submit(&manifest, requester);
        assert!(
            parked_and_asked(&out),
            "the road the builder renders for a member must pass the gate's form check and \
             reach the broker, by {requester}: {out:?}"
        );
    }
}

/// Mutate the rendered road, keep everything else as rendered.
fn with_road(f: impl FnOnce(&mut Vec<Value>)) -> Value {
    let (mut decls, _) = render(json!({"model_registry_scope": SCOPE}));
    let road = decls
        .iter_mut()
        .find(|d| d["scope"] == SCOPE)
        .expect("a road");
    let mut edges = road["diff"]["add_edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    f(&mut edges);
    road["diff"]["add_edges"] = Value::Array(edges);
    Value::Array(decls)
}

fn rewrite_announced(edges: &mut [Value], f: impl FnOnce(&mut Vec<Value>)) {
    let a = edges
        .iter_mut()
        .find(|e| e["to"] == ".")
        .expect("an announcement");
    let lit = a["modifier"]["set_context"]["model_announced"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mut entries: Vec<Value> =
        meclaw_core::serde_json::from_str(lit.trim_matches('\'')).expect("entries");
    f(&mut entries);
    let written = meclaw_core::serde_json::to_string(&entries).unwrap();
    a["modifier"]["set_context"]["model_announced"] = json!(format!("'{written}'"));
}

#[test]
fn what_is_not_the_members_road_is_refused_by_name() {
    if !shipped() {
        return;
    }
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a push addressed two segments deep",
            with_road(|edges| {
                let e = edges.iter_mut().find(|e| e["from"] == ".").unwrap();
                e["condition"] = json!(
                    e["condition"]
                        .as_str()
                        .unwrap()
                        .replace("/memory-hive/closer'", "/memory-hive/x/closer'")
                );
            }),
            "model_push_form",
        ),
        (
            "a push edge onto somebody else's memory",
            with_road(|edges| {
                let e = edges.iter_mut().find(|e| e["from"] == ".").unwrap();
                e["to"] = json!("./beta/members/bo/memory-hive");
            }),
            "model_push_form",
        ),
        (
            "an announced key the gate does not know",
            with_road(|edges| {
                rewrite_announced(edges, |entries| {
                    entries[0]["package"] = json!("x");
                })
            }),
            "model_announcement_form",
        ),
        (
            "a need over the bound",
            with_road(|edges| {
                rewrite_announced(edges, |entries| {
                    entries[0]["requirement"] = json!("x".repeat(2049));
                })
            }),
            "model_announcement_form",
        ),
        (
            "an empty need",
            with_road(|edges| {
                rewrite_announced(edges, |entries| {
                    entries[0]["requirement"] = json!("  ");
                })
            }),
            "model_announcement_form",
        ),
    ];
    for (what, manifest, code) in cases {
        for requester in [AGENT, OPERATOR] {
            let out = submit(&manifest, requester);
            assert_eq!(refused(&out), code, "{what}, by {requester}: {out:?}");
            assert!(
                !out.iter().any(|m| m["header"]["route"] == "ask"),
                "{what} was asked about instead of refused: {out:?}"
            );
        }
    }
}
