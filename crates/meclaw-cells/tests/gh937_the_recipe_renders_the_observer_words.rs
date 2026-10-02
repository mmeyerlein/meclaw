//! GH #937 — two observer words an app declaration may say:
//! `observes_tool_calls: {at, tools, route?}` (new) and the object form of
//! `observes_tool_results` (beside its original string form).
//!
//! An observation is a FAN-OUT, never an interception. `observes_tool_calls`
//! draws the offer's own edge — from each surface of the generation, guarded on
//! the tool names, with the same context stamp — onto the observer's cell, so
//! the offering app still gets the call and the observer gets a copy, restamped
//! `in_tool_call`. The object form of `observes_tool_results` guards the two
//! producers on the tool names and adds a third, the other apps (`./apps`, the
//! container, so the install order does not matter; `context.tool_answerer`
//! keeps an app from hearing its own results). The string form must keep
//! drawing byte for byte what it drew, because installed apps carry those
//! edges.
//!
//! The lane on every edge is the producer's lane (`tool`, `tool_result`): the
//! mutation door matches that against the observer's contract, while
//! `set_hop.route` is only what the observing cell hears. And no observer edge
//! touches `audience_set` — who may hear a turn is not an app's to rewrite.
//!
//! Pure script tests: the SHIPPED `classify` and `recipes` are run over stdin,
//! nothing is booted. Guarded like every other template-reading test (GH #49):
//! a builder that did not travel into this tree is skipped, not judged.

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, emit_one, shipped_script};
use std::path::Path;

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const CLASSIFY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/classify/config.json"
);

const MEMBER: &str = "/os/orgs/acme/members/alex";
const APP: &str = "probe-observer";
const GEN: &str = "sam";

fn skip() -> bool {
    if !Path::new(RECIPES).exists() || !Path::new(CLASSIFY).exists() {
        eprintln!("the builder did not travel into this tree -- skipped (GH #49)");
        return true;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("no python3 -- skipped");
        return true;
    }
    false
}

fn params(declaration: Value) -> Value {
    json!({"scope": MEMBER, "app": APP, "template": "probe-observer@1.0.0",
           "screen": "display", "generation": GEN, "declaration": declaration})
}

fn run_recipes(params: &Value) -> Vec<Value> {
    emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": params}).to_string()}],
        }),
    )
}

fn edges(declaration: Value) -> Vec<Value> {
    let all = run_recipes(&params(declaration));
    let first = all.first().expect("an emission");
    assert!(first["header"]["error_code"].is_null(), "refused: {first}");
    first["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .expect("edges")
        .clone()
}

/// The refusal of a render, as `(error_code, payload)`.
fn refusal(declaration: Value) -> (String, Value) {
    let all = run_recipes(&params(declaration));
    let first = all.first().expect("an emission");
    let code = first["header"]["error_code"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a refusal, got a render: {first}"))
        .to_string();
    let payload: Value =
        serde_json::from_str(first["messages"][0]["text"].as_str().expect("a payload"))
            .expect("the refusal payload is json");
    (code, payload)
}

/// No modifier of an observer edge may set or delete `audience_set`.
fn assert_audience_untouched(e: &Value) {
    let m = &e["modifier"];
    assert!(m["set_context"].get("audience_set").is_none(), "{e}");
    let deleted = m["delete_context"].as_array().cloned().unwrap_or_default();
    assert!(!deleted.contains(&json!("audience_set")), "{e}");
}

/// **A call is observed on the offer's own edge, with another target.** One
/// `tool` v-lane per surface, guarded on the names, stamped like the offer,
/// restamped to the declared route — or to `in_tool_call` without one.
#[test]
fn an_observed_call_is_one_tool_lane_per_surface() {
    if skip() {
        return;
    }
    for (route, want) in [
        (Some("in_probe_call"), "in_probe_call"),
        (None, "in_tool_call"),
    ] {
        let mut o = json!({"at": "./ear", "tools": ["probe_timer"]});
        if let Some(r) = route {
            o["route"] = json!(r);
        }
        let got = edges(json!({"observes_tool_calls": o}));
        assert_eq!(got.len(), 2, "{got:?}");
        let mut froms: Vec<&str> = got.iter().map(|e| e["from"].as_str().unwrap()).collect();
        froms.sort();
        assert_eq!(
            froms,
            [
                format!("./assistants/{GEN}/talky"),
                format!("./assistants/{GEN}/talky-chat")
            ]
        );
        for e in &got {
            assert_eq!(e["to"], json!(format!("./apps/{APP}/ear")), "{e}");
            assert_eq!(e["lane"], json!("tool"), "{e}");
            // Ruling Important 2 (b): an observer edge is PASSIVE -- it does
            // not count when the edge table drops a sender's defaults, so the
            // surface's own `./talky -> ./tools` still runs beside it.
            assert_eq!(e["tap"], json!(true), "{e}");
            let cond = e["condition"].as_str().expect("a condition");
            assert!(cond.contains("hop.route == 'tool'"), "{e}");
            assert!(cond.contains("hop.tool_name == 'probe_timer'"), "{e}");
            assert_eq!(
                e["modifier"]["set_hop"]["route"],
                json!(format!("'{want}'")),
                "{e}"
            );
            assert_eq!(
                e["modifier"]["set_context"]["assistant"],
                json!(format!("'{GEN}'"))
            );
            assert_audience_untouched(e);
        }
    }
}

/// **The object form guards the results and hears the other apps.** Three
/// producers, all on the `tool_result` lane, all guarded on the names,
/// restamped `in_tool_result`; the `./apps` edge excludes the observer itself.
#[test]
fn an_observed_result_object_draws_three_guarded_lanes() {
    if skip() {
        return;
    }
    let got = edges(json!({"observes_tool_results":
                           {"at": "./ear", "tools": ["probe_timer", "probe_alarm"]}}));
    assert_eq!(got.len(), 3, "{got:?}");
    let mut froms: Vec<&str> = got.iter().map(|e| e["from"].as_str().unwrap()).collect();
    froms.sort();
    let tools = format!("./assistants/{GEN}/tools");
    assert_eq!(froms, ["./apps", tools.as_str(), "./memory-hive"]);
    for e in &got {
        assert_eq!(e["to"], json!(format!("./apps/{APP}/ear")), "{e}");
        assert_eq!(e["lane"], json!("tool_result"), "{e}");
        assert_eq!(e["tap"], json!(true), "{e}");
        let cond = e["condition"].as_str().expect("a condition");
        assert!(cond.contains("hop.route == 'tool_result'"), "{e}");
        assert!(cond.contains("hop.tool_name == 'probe_timer'"), "{e}");
        assert!(cond.contains("hop.tool_name == 'probe_alarm'"), "{e}");
        // Review Important 1: the two producers that serve EVERY generation of
        // the member are bounded to the one the observer was installed for, on
        // the stamp the generation's exit and the offer's call edge set. The
        // generation's own `./tools` is bounded by its path already.
        let gen_guard = format!("has(context.assistant) && context.assistant == '{GEN}'");
        assert_eq!(
            e["from"] != json!(format!("./assistants/{GEN}/tools")),
            cond.contains(&gen_guard),
            "{e}"
        );
        assert_eq!(
            e["modifier"]["set_hop"]["route"],
            json!("'in_tool_result'"),
            "{e}"
        );
        assert_audience_untouched(e);
        let own = format!("context.tool_answerer != '{APP}'");
        assert_eq!(e["from"] == json!("./apps"), cond.contains(&own), "{e}");
    }
}

/// **The string form draws what it always drew**, byte for byte: installed
/// apps carry these two edges, and a new word must not move them.
#[test]
fn the_string_form_draws_the_same_two_edges() {
    if skip() {
        return;
    }
    // Bytes first: the recipe's own serialisation of the edge list and the
    // manifest hash over it, as `builder@1.19.0` rendered them. A comparison of
    // parsed values alone would not see a reordered key or a new `"tap"`.
    let raw = meclaw_testing::run_shipped_script(
        &shipped_script(RECIPES),
        &meclaw_testing::code_stdin(&json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": params(json!({"observes_tool_results": "./stage"}))})
                                      .to_string()}],
        }))
        .to_string(),
    );
    let stdout = String::from_utf8(raw.stdout).expect("utf-8");
    let bytes = format!(
        "\"add_edges\": [\
         {{\"from\": \"./assistants/{GEN}/tools\", \"to\": \"./apps/{APP}/stage\", \
         \"lane\": \"tool_result\", \"condition\": \"has(hop.route) && hop.route == 'tool_result'\"}}, \
         {{\"from\": \"./memory-hive\", \"to\": \"./apps/{APP}/stage\", \
         \"lane\": \"tool_result\", \"condition\": \"has(hop.route) && hop.route == 'tool_result'\"}}]"
    );
    assert!(
        stdout.contains(&bytes),
        "byte for byte {bytes}\nin {stdout}"
    );
    // `manifest_sha256` of this very request under builder@1.19.0 (master
    // e3f9f0e2c), measured before #937 touched the recipe.
    assert!(
        stdout.contains(
            "\"manifest_sha256\": \"d92fd1c043ee782912991310c86ed391086534c15343cfca53604899418a94da\""
        ),
        "the manifest hash moved: {stdout}"
    );

    let got = edges(json!({"observes_tool_results": "./stage"}));
    let want = [
        json!({"from": format!("./assistants/{GEN}/tools"), "to": format!("./apps/{APP}/stage"),
               "lane": "tool_result",
               "condition": "has(hop.route) && hop.route == 'tool_result'"}),
        json!({"from": "./memory-hive", "to": format!("./apps/{APP}/stage"),
               "lane": "tool_result",
               "condition": "has(hop.route) && hop.route == 'tool_result'"}),
    ];
    assert_eq!(got, want);
}

/// **A malformed observation is refused by field**, like every other word of
/// the declaration — a guard the recipe cannot write is not written at all.
#[test]
fn a_malformed_observation_is_refused_by_field() {
    if skip() {
        return;
    }
    let many: Vec<String> = (0..17).map(|i| format!("probe_{i}")).collect();
    let cases = [
        (
            json!({"observes_tool_calls": {"at": "./ear", "tools": []}}),
            "observes_tool_calls.tools",
        ),
        (
            json!({"observes_tool_calls": {"at": "./ear", "tools": many}}),
            "observes_tool_calls.tools",
        ),
        (
            json!({"observes_tool_calls": {"at": "./ear", "tools": ["Probe_timer"]}}),
            "observes_tool_calls.tools",
        ),
        (
            json!({"observes_tool_results": {"at": "./ear", "tools": ["probe_timer", "probe_timer"]}}),
            "observes_tool_results.tools",
        ),
        (
            json!({"observes_tool_calls": {"at": "./ear", "tools": ["probe_timer"], "route": "In-x"}}),
            "observes_tool_calls.route",
        ),
        (
            json!({"observes_tool_calls": {"at": "./ear", "tools": ["probe_timer"], "via": "x"}}),
            "observes_tool_calls",
        ),
        (
            json!({"observes_tool_results": {"at": "ear", "tools": ["probe_timer"]}}),
            "observes_tool_results.at",
        ),
        (
            json!({"observes_tool_calls": {"at": "./ear", "tools": {"probe_timer": true}}}),
            "observes_tool_calls.tools",
        ),
    ];
    for (declaration, field) in cases {
        let (code, payload) = refusal(declaration.clone());
        assert_eq!(code, "app_declaration_invalid", "{declaration}: {payload}");
        assert_eq!(payload["field"], json!(field), "{declaration}: {payload}");
        assert!(
            payload["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "{declaration}: and say why: {payload}"
        );
    }
}

/// **An observed call needs the generation it is observed in**, and the switch
/// says so before the renderer is reached.
#[test]
fn an_observed_call_needs_a_generation() {
    if skip() {
        return;
    }
    let mut p = params(json!({"observes_tool_calls": {"at": "./ear", "tools": ["probe_timer"]}}));
    p.as_object_mut().expect("an object").remove("generation");
    let out = emit_one(
        &shipped_script(CLASSIFY),
        &json!({
            "target": "/os/builder/classify",
            "header": {"hop": {"route": "in_build"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_call", "id": "c1",
                          "text": json!({"request": "install an app",
                                         "recipe": "install_app",
                                         "params": p}).to_string()}],
        }),
    );
    assert_eq!(
        out["header"]["error_code"],
        json!("recipe_params_incomplete"),
        "{out}"
    );
    let payload: Value =
        serde_json::from_str(out["messages"][0]["text"].as_str().expect("a payload"))
            .expect("json");
    assert_eq!(payload["missing"], json!(["generation"]), "{payload}");
}
