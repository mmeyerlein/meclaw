//! GH #963 / PE-DP-11 -- installing the presenter makes its decider a
//! subscriber of the model registry.
//!
//! The presenter's `decide` is an llm cell on the `decisions` protocol: it is
//! born on its own `model` param and the registry is what keeps it current. A
//! level's brains are announced by `grow_level` (GH #855); an app is installed
//! by `install_app`, which announced nothing, so the decider never heard of the
//! registry. With the registry's container scope set on the builder
//! (`model_registry_scope`), `install_app` of the presenter now draws, after the
//! app's own entry, one entry at that scope with three edges:
//!
//! - the push: `in_model` addressed to `<member>/apps/presenter/decide`, into
//!   the presenter;
//! - the announcement off the newborn node on `mutation_committed`, stamping
//!   the node as `model_generation` and the one entry as `model_announced`:
//!   `{cell_path, start_model: "", requirement, protocol: "decisions"}` -- the
//!   need a byte copy of the cell's own `params.requirement`;
//! - the way back of a refusal (`model_refused`).
//!
//! All three are forms the submit gate already lets through; the lock submits
//! the rendered manifest and sees it parked and asked, not refused. Without the
//! scope, or for any other app, nothing is added.
//!
//! Pure script tests: the SHIPPED `recipes` and `submit/gate` over stdin.

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::code_wire::{code_stdin, emit_all, run_shipped_script, shipped_script};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const GATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);
const PRESENTER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/presenter/template.json"
);
const DECIDE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/presenter/decide/config.json"
);

const MEMBER: &str = "/os/orgs/acme/members/alex";
const SCOPE: &str = "/os/orgs";
const NODE: &str = "/os/orgs/acme/members/alex/apps/presenter";
const DECIDER: &str = "/os/orgs/acme/members/alex/apps/presenter/decide";
const AGENT: &str = "/os/orgs/acme/members/alex/assistants/sam/talky/brain";
const OPERATOR: &str = "/os/operator/submit";

fn read(p: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p}: {e}")))
        .unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn shipped() -> bool {
    [RECIPES, GATE, PRESENTER, DECIDE]
        .iter()
        .all(|p| std::path::Path::new(p).is_file())
}

/// The manifest `install_app` renders for `app` with the presenter's own
/// declaration, the registry scope set to `scope` ("" = none).
fn install(app: &str, scope: &str) -> Value {
    let declaration = read(PRESENTER)["app"].clone();
    let wish = json!({"recipe": "install_app", "request": "install the presenter",
                      "params": {"scope": MEMBER, "app": app,
                                 "template": format!("{app}@1.0.0"),
                                 "screen": "display", "generation": "sam",
                                 "ctx": {"member_person": "alex"},
                                 "declaration": declaration}})
    .to_string();
    let doc = code_stdin(&json!({
        "target": "/os/builder/recipes",
        "header": {"hop": {"route": "recipe"}, "context": {}},
        "ttl": 64,
        "params": {"model_registry_scope": scope},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "", "text": wish}],
    }));
    let out = run_shipped_script(&shipped_script(RECIPES), &doc.to_string());
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the recipe exited non-zero: {stderr}");
    let all: Value = serde_json::from_slice(&out.stdout).expect("json out");
    let m = all
        .as_array()
        .and_then(|a| a.iter().find(|m| m["header"]["operation"] == "recipe"))
        .cloned()
        .expect("the recipe answered");
    assert!(m["header"]["error_code"].is_null(), "{m}");
    m["manifest"].clone()
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

/// Phase A of the submit gate: a fresh submission from `requester`.
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

/// Red before PE-DP-11: `install_app` rendered the app's own entry and nothing
/// at the registry's scope -- the manifest had one entry.
#[test]
fn the_presenter_is_installed_with_its_deciders_road() {
    if !shipped() {
        return;
    }
    let manifest = install("presenter", SCOPE);
    let entries = manifest.as_array().expect("entries");
    assert_eq!(
        entries.len(),
        2,
        "the app, then its decider's road: {manifest:#}"
    );
    assert_eq!(
        entries[0]["diff"]["add_nodes"][0]["name"], "apps/presenter",
        "the app's own entry comes first, so the node is born before its road"
    );
    let road = &entries[1];
    assert_eq!(road["scope"], SCOPE);
    let edges = road["diff"]["add_edges"].as_array().expect("edges");
    assert_eq!(edges.len(), 3, "{edges:#?}");

    let rel = "./acme/members/alex/apps/presenter";
    assert_eq!(edges[0]["from"], ".");
    assert_eq!(edges[0]["to"], rel);
    assert_eq!(
        edges[0]["condition"],
        format!(
            "has(hop.route) && hop.route == 'in_model' && \
             has(hop.subscriber) && hop.subscriber == '{DECIDER}'"
        )
    );
    assert_eq!(
        edges[0]["modifier"]["set_hop"],
        json!({"route": "'in_model'"})
    );

    let announce = &edges[1];
    assert_eq!(
        (&announce["from"], &announce["to"]),
        (&json!(rel), &json!("."))
    );
    assert_eq!(
        announce["condition"],
        "has(hop.route) && hop.route == 'mutation_committed'"
    );
    let ctx = &announce["modifier"]["set_context"];
    assert_eq!(ctx["model_generation"], format!("'{NODE}'"));
    let lit = ctx["model_announced"].as_str().expect("a literal");
    let entries: Value =
        serde_json::from_str(lit.trim_matches('\'')).expect("the announcement is JSON");
    let need = read(DECIDE)["params"]["requirement"].clone();
    assert!(need.as_str().is_some_and(|n| !n.is_empty()), "{need}");
    assert_eq!(
        entries,
        json!([{"cell_path": DECIDER, "start_model": "", "requirement": need,
                "protocol": "decisions"}]),
        "the decider, its need byte for byte, no start value, the decisions protocol"
    );

    assert_eq!(
        (&edges[2]["from"], &edges[2]["to"]),
        (&json!(rel), &json!("."))
    );
    assert_eq!(
        edges[2]["condition"],
        "has(hop.route) && hop.route == 'model_refused'"
    );
    assert!(edges[2].get("modifier").is_none(), "{:#}", edges[2]);
}

/// The road is a form the submit gate knows: the whole rendered manifest is
/// parked and asked about, never refused, from the operator and from an agent.
#[test]
fn the_deciders_road_passes_the_gate() {
    if !shipped() {
        return;
    }
    let manifest = install("presenter", SCOPE);
    for requester in [OPERATOR, AGENT] {
        let out = submit(&manifest, requester);
        let refused = out
            .iter()
            .find(|m| m["header"]["route"] == "receipt")
            .and_then(|m| m["header"]["error_code"].as_str())
            .unwrap_or_default()
            .to_string();
        assert!(
            refused.is_empty()
                && out.iter().any(|m| m["header"]["route"] == "sstore")
                && out.iter().any(|m| m["header"]["route"] == "ask"),
            "{requester}: refused `{refused}`: {out:#?}"
        );
    }
    // Control: the gate reads the form and is no open door -- the same push
    // pointed into another member's presenter is refused unasked.
    let mut bad = manifest.clone();
    bad[1]["diff"]["add_edges"][0]["to"] = json!("./beta/members/bo/apps/presenter");
    let out = submit(&bad, AGENT);
    assert_eq!(
        out.iter()
            .find(|m| m["header"]["route"] == "receipt")
            .and_then(|m| m["header"]["error_code"].as_str()),
        Some("model_push_form"),
        "{out:#?}"
    );
}

/// No scope, no road; and only the presenter's decider is announced here.
#[test]
fn without_a_registry_or_for_another_app_nothing_is_added() {
    if !shipped() {
        return;
    }
    assert_eq!(
        install("presenter", "").as_array().map(Vec::len),
        Some(1),
        "no registry scope: the app alone"
    );
    assert_eq!(
        install("viewer", SCOPE).as_array().map(Vec::len),
        Some(1),
        "another app announces nothing"
    );
}
