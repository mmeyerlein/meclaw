//! GH #1353 — the road of a params-only update from the shell's contact.
//!
//! Three renderers and judges of one road, each driven as it ships:
//!
//!   * `templates/operator` takes `in_params`, hands it on as a plain hive
//!     transit and turns the answer of the cell into `params_receipt`;
//!   * `templates/builder/recipes` draws one edge per door hive of a level it
//!     grows, at the container the shell hands the lane to, and one way back;
//!   * `templates/submit/gate` lets an edge of the lane through in exactly the
//!     form that recipe renders and refuses every other one as
//!     `params_push_form`, without asking the broker.
//!
//! The renderer's output is driven into the gate, never a hand-written "legal"
//! edge that nobody emits.

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::code_wire::{code_stdin, emit_all, run_shipped_script, shipped_script};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const SCOPE: &str = "/os/orgs";
const MEMBER: &str = "/os/orgs/acme/members/alex";
const GEN: &str = "/os/orgs/acme/members/alex/assistants/scribe";
const AGENT: &str = "/os/orgs/acme/members/alex/assistants/scribe/talky/brain";
const OPERATOR: &str = "/os/operator/intake";

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read(rel: &str) -> Value {
    let path = repo(rel);
    serde_json::from_str(&std::fs::read_to_string(&path).expect(rel)).expect(rel)
}

fn script(rel: &str) -> String {
    shipped_script(repo(rel).to_str().expect("utf-8 path"))
}

fn edges(config: &Value) -> Vec<Value> {
    config["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

/// A door hive is a SEALED one that takes the lane; the open shell takes it too
/// (the rim of the road) and is no target of an edge.
fn is_a_door(config: &Value) -> bool {
    config["params"]["ports"].is_array() && accepts_the_lane(config)
}

fn accepts_the_lane(config: &Value) -> bool {
    config["params"]["contract"]["accepts"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|r| r["route"] == "in_params"))
}

/// What the recipe cell answers: its manifest.
fn render(recipe: &str, params: Value, scope: &str) -> Vec<Value> {
    let wish = json!({"recipe": recipe, "request": "grow it", "params": params}).to_string();
    let doc = code_stdin(&json!({
        "target": "/os/builder/recipes",
        "header": {"hop": {"route": "recipe"}, "context": {}},
        "ttl": 64,
        "params": {"model_registry_scope": scope},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "", "text": wish}],
    }));
    let out = run_shipped_script(
        &script("templates/builder/recipes/config.json"),
        &doc.to_string(),
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the recipe exited non-zero: {stderr}");
    let all: Value = serde_json::from_slice(&out.stdout).expect("json out");
    let answer = match &all {
        Value::Array(a) => a
            .iter()
            .find(|m| m["header"]["operation"] == "recipe")
            .cloned()
            .expect("an answer"),
        one => one.clone(),
    };
    assert!(answer["header"]["error_code"].is_null(), "{answer}");
    answer["manifest"].as_array().expect("a manifest").clone()
}

fn level(level: &str, scope: &str, name: &str, template: &str, root: &str) -> Vec<Value> {
    render(
        "grow_level",
        json!({"scope": scope, "level": level, "name": name, "template": template,
               "bind_chat": "${CHAT}",
               "ctx": {"model": "m", "model_fast": "f", "model_surface": "s",
                       "member_person": "alex"}, "assistant": "scribe", "screen": "display"}),
        root,
    )
}

/// Every edge of the manifest that names the lane, with the scope it stands at.
fn pushes(manifest: &[Value]) -> Vec<(String, Value)> {
    edges_where(manifest, |e| e.to_string().contains("'in_params'"))
}

fn ways_back(manifest: &[Value]) -> Vec<(String, Value)> {
    edges_where(manifest, |e| e.to_string().contains("'params_refused'"))
}

fn edges_where(manifest: &[Value], keep: impl Fn(&Value) -> bool) -> Vec<(String, Value)> {
    manifest
        .iter()
        .flat_map(|d| {
            let scope = d["scope"].as_str().expect("scope").to_string();
            d["diff"]["add_edges"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |e| (scope.clone(), e))
        })
        .filter(|(_, e)| keep(e))
        .collect()
}

/// The pair the recipe draws for the door hive at the absolute path `door`.
fn the_pair(door: &str) -> (Value, Value) {
    let rel = format!(
        ".{}",
        door.strip_prefix(SCOPE).expect("under the container")
    );
    (
        json!({"from": ".", "to": rel,
               "condition": format!("has(hop.route) && hop.route == 'in_params' && \
                                     has(hop.subscriber) && hop.subscriber.startsWith('{door}/')"),
               "modifier": {"set_hop": {"route": "'in_params'"}}}),
        json!({"from": rel, "to": ".",
               "condition": "has(hop.route) && (hop.route == 'params_applied' || \
                             hop.route == 'params_refused')"}),
    )
}

fn assert_doors(manifest: &[Value], doors: &[String]) {
    let want: BTreeSet<String> = doors
        .iter()
        .flat_map(|d| {
            let (push, back) = the_pair(d);
            [format!("{SCOPE} {push}"), format!("{SCOPE} {back}")]
        })
        .collect();
    let mut drawn = pushes(manifest);
    drawn.extend(ways_back(manifest));
    let got: BTreeSet<String> = drawn.iter().map(|(s, e)| format!("{s} {e}")).collect();
    assert_eq!(got, want, "one push and one way back per door hive");
    assert_eq!(drawn.len(), want.len(), "and no edge of them twice");
}

// ── the contact ──────────────────────────────────────────────────────────────

#[test]
fn gh1353_the_contact_turns_the_answer_of_the_cell_into_a_receipt() {
    let hive = read("templates/operator/config.json");
    let es = edges(&hive);
    assert!(accepts_the_lane(&hive));
    let contract = &hive["params"]["contract"];
    assert!(
        contract["accepts"]
            .as_array()
            .expect("accepts")
            .iter()
            .any(|r| r["route"] == "in_params_answer"),
        "the answer comes back on a lane of its own: on the cell's word \
         (`params_applied`) it would take the edges of the contact's own cells"
    );
    assert!(
        contract["emits"]
            .as_array()
            .expect("emits")
            .iter()
            .any(|r| r["route"] == "params_receipt")
    );
    assert!(es.iter().any(|e| e["from"] == "."
        && e["to"] == "./params"
        && e["condition"] == "has(hop.route) && hop.route == 'in_params_answer'"));
    assert!(es.iter().any(|e| e["from"] == "./params"
        && e["to"] == "."
        && e["condition"] == "has(hop.route) && hop.route == 'params_receipt'"));

    for (word, key) in [
        ("params_applied", "applied_subscriber"),
        ("params_refused", "refused_subscriber"),
    ] {
        let mut hop = json!({"route": "in_params_answer", "params_answer": word,
                             "operation": "params_update"});
        hop[key] = json!(AGENT);
        if word == "params_refused" {
            hop["error_code"] = json!("invalid_input");
        }
        let out = emit_all(
            &script("templates/operator/params/config.json"),
            &json!({"target": "/os/operator/params", "header": {"hop": hop, "context": {}},
                    "ttl": 64, "params": {},
                    "messages": [], "meta": {"detail": "the detail, verbatim"}}),
        );
        assert_eq!(out.len(), 1, "{out:?}");
        let h = &out[0]["header"];
        assert_eq!(h["route"], "params_receipt");
        assert_eq!(h["params_answer"], word);
        assert_eq!(h[key], AGENT, "whose answer it is: {h}");
        if word == "params_refused" {
            assert_eq!(h["error_code"], "invalid_input", "the cell's own code: {h}");
        } else {
            assert!(h["error_code"].is_null(), "{h}");
        }
        assert_eq!(
            out[0]["meta"]["detail"], "the detail, verbatim",
            "a cell says its detail in `meta` and sends no turn: {out:?}"
        );
    }
}

/// A target without a door must end as `hive_no_route`, so the contact's
/// default door may not take the lane, and it may not take an answer either.
#[test]
fn gh1353_the_contacts_default_door_takes_neither_the_update_nor_an_answer() {
    let hive = read("templates/operator/config.json");
    let default = edges(&hive)
        .into_iter()
        .find(|e| e["default"] == json!(true))
        .expect("a default door");
    let guard = default["condition"].as_str().expect("a guard");
    assert!(guard.contains("!(has(hop.route) && hop.route == 'in_params')"));
    for own in ["params_applied", "params_refused", "params_receipt"] {
        assert!(
            guard.contains(&format!("hop.route != '{own}'")),
            "{own}: {guard}"
        );
    }
}

// ── the recipe ───────────────────────────────────────────────────────────────

#[test]
fn gh1353_a_grown_level_gets_one_edge_and_one_way_back_per_door_hive() {
    let grown = level("assistant", MEMBER, "scribe", "assistant", SCOPE);
    let doors: Vec<String> = ["talky", "talky-chat", "cogny"]
        .iter()
        .map(|rim| format!("{GEN}/{rim}"))
        .collect();
    assert_doors(&grown, &doors);
    assert_eq!(
        grown.len(),
        2,
        "the edges stand in the declaration the registry's road stands in"
    );

    let member = level("member", "/os/orgs/acme", "alex", "member", SCOPE);
    let mut doors: Vec<String> = [
        "access",
        "affinity",
        "file-space",
        "firewall",
        "graph-space",
        "librarian",
        "memory-hive",
        "objects",
    ]
    .iter()
    .map(|hive| format!("{MEMBER}/{hive}"))
    .collect();
    doors.push(format!("{MEMBER}/channels/display"));
    assert_doors(&member, &doors);

    // The node itself is the door hive where the template is one.
    let phone = level("channel", MEMBER, "freeswitch", "freeswitch@1.0.0", SCOPE);
    assert_doors(&phone, &[format!("{MEMBER}/channels/freeswitch")]);
    // A template without a door gets none, and a tree with no container neither.
    assert_doors(
        &level("app", MEMBER, "colony-view", "colony-view", SCOPE),
        &[],
    );
    assert_doors(&level("assistant", MEMBER, "scribe", "assistant", ""), &[]);
    // A level outside the container is not on the road.
    assert_doors(
        &level("assistant", "/elsewhere/alex", "scribe", "assistant", SCOPE),
        &[],
    );
}

/// The road draws no inner edge: every edge ends on a hive path the level was
/// given, and the lane's inner edges are the template's own.
#[test]
fn gh1353_the_recipes_door_table_is_the_templates_own_word() {
    let source = script("templates/builder/recipes/config.json");
    let program = format!(
        "{}\nimport json, sys\nsys.stdout.write(json.dumps({{'levels': \
         PARAMS_LEVEL_DOORS, 'nodes': sorted(PARAMS_NODE_DOORS), 'frozen': \
         sorted(PARAMS_FROZEN)}}))\n",
        source
            .split("doc = json.load(sys.stdin)")
            .next()
            .expect("the definitions stand in front of the run")
    );
    let out = run_shipped_script(&program, "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let table: Value = serde_json::from_slice(&out.stdout).expect("json");

    let named = |key: &str| -> BTreeSet<String> {
        table[key]
            .as_array()
            .expect("a list")
            .iter()
            .map(|n| n.as_str().expect("name").to_string())
            .collect()
    };
    let (nodes, frozen) = (named("nodes"), named("frozen"));
    // Not every template of the table ships in every tree, so the judges of the
    // table are the templates that are here: each one, and in both directions.
    let mut doors = BTreeSet::new();
    let mut here = BTreeSet::new();
    for entry in std::fs::read_dir(repo("templates")).expect("templates") {
        let dir = entry.expect("entry");
        let name = dir.file_name().to_string_lossy().to_string();
        if !dir.path().join("config.json").is_file() {
            continue;
        }
        let door = is_a_door(&read(&format!("templates/{name}/config.json")));
        if let Some((class, _)) = name.split_once('@') {
            if nodes.contains(class) {
                assert_eq!(
                    frozen.contains(&name),
                    !door,
                    "{name}: a frozen version of a door class is listed when it has none"
                );
            }
            continue;
        }
        if door {
            doors.insert(name.clone());
        }
        here.insert(name);
    }
    for must in ["talky", "access", "display"] {
        assert!(doors.contains(must), "{must} ships, and has the door");
    }
    let judged: BTreeSet<String> = nodes.intersection(&here).cloned().collect();
    assert_eq!(judged, doors, "every template with the door, and no other");

    // Every open template is a level somebody may grow; the shell, which takes
    // the lane itself, draws its own edges.
    let mut levels = BTreeSet::new();
    for level in &here {
        let root = repo("templates").join(level);
        let config = read(&format!("templates/{level}/config.json"));
        if config["params"]["ports"].is_array() || accepts_the_lane(&config) {
            continue;
        }
        let mut inside = BTreeSet::new();
        hives_of(&root, &root, &doors, &mut inside);
        let listed: BTreeSet<String> = table["levels"][level]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|n| n.as_str().expect("name").to_string())
            .collect();
        assert_eq!(
            listed, inside,
            "{level}: every hive with the door, and no other"
        );
        if !inside.is_empty() {
            levels.insert(level.clone());
        }
    }
    let known: BTreeSet<String> = table["levels"]
        .as_object()
        .expect("levels")
        .keys()
        .cloned()
        .collect();
    assert_eq!(levels, known, "and every level of the table ships");
}

/// The nodes under `dir` whose template class is one of `doors`, by their path
/// from `root`.
fn hives_of(root: &Path, dir: &Path, doors: &BTreeSet<String>, out: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).expect("a directory") {
        let path = entry.expect("entry").path();
        if !path.is_dir() {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(path.join("config.json")) {
            let cell: Value = serde_json::from_str(&text).expect("json");
            let class = cell["cell"]["template"].as_str().unwrap_or_default();
            if doors.contains(class.split('@').next().unwrap_or_default()) {
                let rel = path.strip_prefix(root).expect("under the root");
                out.insert(rel.to_string_lossy().to_string());
            }
        }
        hives_of(root, &path, doors, out);
    }
}

/// An edge onto a hive that does not take the lane is refused as
/// `hive_contract`, and the whole wish with it. So the table speaks for the
/// shipped template of that name alone, and a contract in the wish decides.
#[test]
fn gh1353_no_road_is_drawn_to_a_hive_without_the_door() {
    let grow = |level: &str, template: &str, contract: Option<Value>| {
        let mut p = json!({"scope": MEMBER, "level": level, "name": "x", "template": template,
            "ctx": {"model": "m", "model_fast": "f", "model_surface": "s",
                    "member_person": "alex"}, "assistant": "scribe", "screen": "display"});
        if let Some(c) = contract {
            p["contract"] = c;
        }
        render("grow_level", p, SCOPE)
    };
    let says = |routes: &[&str]| {
        let rows: Vec<Value> = routes.iter().map(|r| json!({"route": r})).collect();
        Some(json!({"accepts": rows, "emits": []}))
    };
    let app = vec![format!("{MEMBER}/apps/x")];
    for (what, level, template, contract, doors) in [
        (
            "a contract without the lane",
            "app",
            "presenter@1.0.0",
            says(&["in_turn"]),
            vec![],
        ),
        (
            "an empty contract",
            "app",
            "presenter",
            Some(json!({})),
            vec![],
        ),
        (
            "a derivation's contract without the lane",
            "app",
            "objects-catalog",
            says(&[]),
            vec![],
        ),
        (
            "a derivation that says nothing",
            "app",
            "lead-gen-thirdparty",
            None,
            vec![],
        ),
        (
            "a frozen version",
            "app",
            "simple-agent@1.1.4",
            None,
            vec![],
        ),
        (
            "a derived level that says nothing",
            "assistant",
            "assistant-acme",
            None,
            vec![],
        ),
        (
            "a contract with the lane",
            "app",
            "lead-gen-thirdparty",
            says(&["in_turn", "in_params"]),
            app.clone(),
        ),
        (
            "a frozen version whose contract has the lane",
            "app",
            "simple-agent@1.1.4",
            says(&["in_params"]),
            app.clone(),
        ),
        (
            "the shipped template, pinned",
            "app",
            "presenter@1.0.0",
            None,
            app.clone(),
        ),
        (
            "a derived level whose contract has the lane",
            "assistant",
            "assistant-acme",
            says(&["in_params"]),
            ["talky", "talky-chat", "cogny"]
                .iter()
                .map(|rim| format!("{MEMBER}/assistants/x/{rim}"))
                .collect(),
        ),
    ] {
        println!("{what}");
        assert_doors(&grow(level, template, contract), &doors);
    }
}

// ── the gate ─────────────────────────────────────────────────────────────────

/// Phase A of the gate: a fresh submission from `requester`.
fn submit(decls: &[Value], requester: &str) -> Vec<Value> {
    let decls = Value::Array(decls.to_vec());
    let digest = String::from_utf8(
        run_shipped_script(
            concat!(
                "import sys, json, hashlib\n",
                "d = json.load(sys.stdin)\n",
                "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
                "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
            ),
            &decls.to_string(),
        )
        .stdout,
    )
    .expect("hex");
    emit_all(
        &script("templates/submit/gate/config.json"),
        &json!({"target": "/os/operator/submit", "reply_to": requester,
                "header": {"hop": {"manifest_sha256": digest}, "context": {}},
                "ttl": 64, "manifest": decls, "params": {},
                "messages": [{"origin": "assistant", "type": "tool_call", "id": "op:c1",
                              "text": "{}"}]}),
    )
}

fn refused(out: &[Value]) -> String {
    out.iter()
        .find(|m| m["header"]["route"] == "receipt")
        .and_then(|m| m["header"]["error_code"].as_str())
        .unwrap_or_default()
        .to_string()
}

fn asked(out: &[Value]) -> bool {
    out.iter().any(|m| m["header"]["route"] == "ask")
}

fn at(scope: &str, edge: Value) -> Vec<Value> {
    vec![json!({"scope": scope, "diff": {"add_edges": [edge]}})]
}

#[test]
fn gh1353_the_road_the_recipe_renders_passes_the_gate_and_is_asked_about() {
    for manifest in [
        level("assistant", MEMBER, "scribe", "assistant", SCOPE),
        level("member", "/os/orgs/acme", "alex", "member", SCOPE),
    ] {
        assert!(!pushes(&manifest).is_empty(), "the road is in it");
        for requester in [OPERATOR, AGENT] {
            let out = submit(&manifest, requester);
            assert_eq!(refused(&out), "", "{requester}: {out:?}");
            assert!(asked(&out), "{requester}: {out:?}");
        }
    }
}

#[test]
fn gh1353_an_in_params_edge_from_anyone_but_the_os_contact_is_refused() {
    let door = format!("{GEN}/talky");
    let (push, _) = the_pair(&door);
    let with = |key: &str, value: Value| {
        let mut e = push.clone();
        e[key] = value;
        e
    };
    let mut bare = push.clone();
    bare.as_object_mut().expect("edge").remove("modifier");
    let mut laned = push.clone();
    laned["lane"] = json!("in_params");
    let other = "./beta/members/bo/assistants/helper";
    for (what, scope, edge) in [
        (
            "another sender",
            SCOPE,
            with("from", json!(format!("{other}/cogny"))),
        ),
        (
            "the contact's own path",
            "/os",
            json!({"from": "./operator",
            "to": "./orgs/acme/members/alex/assistants/scribe/talky",
            "condition": push["condition"], "modifier": push["modifier"]}),
        ),
        (
            "a foreign door",
            SCOPE,
            with("to", json!(format!("{other}/talky"))),
        ),
        ("the container itself", SCOPE, with("to", json!("."))),
        (
            "every update",
            SCOPE,
            with(
                "condition",
                json!("has(hop.route) && hop.route == 'in_params'"),
            ),
        ),
        (
            "an escape hatch",
            SCOPE,
            with(
                "condition",
                json!(format!(
                    "{} || true",
                    push["condition"].as_str().expect("c")
                )),
            ),
        ),
        (
            "one more key written",
            SCOPE,
            with(
                "modifier",
                json!({"set_hop": {"route": "'in_params'", "x": "'1'"}}),
            ),
        ),
        (
            "a restamp of another lane",
            SCOPE,
            json!({"from": ".", "to": format!("{other}/talky"),
                "condition": "has(hop.route) && hop.route == 'tool'",
                "modifier": {"set_hop": {"route": "'in_params'"}}}),
        ),
        (
            "a route that may be the lane",
            SCOPE,
            json!({"from": ".", "to": format!("{other}/talky"),
                "modifier": {"set_hop": {"route": "has(hop.route) ? 'in_params' : 'x'"}}}),
        ),
        ("a copy of the lane as it comes", SCOPE, bare),
        ("a lane of its own", SCOPE, laned),
        (
            "a container that is not beside the contact",
            MEMBER,
            json!({"from": ".", "to": "./assistants/scribe/talky",
                "condition": push["condition"], "modifier": push["modifier"]}),
        ),
    ] {
        for requester in [AGENT, OPERATOR] {
            let out = submit(&at(scope, edge.clone()), requester);
            assert_eq!(
                refused(&out),
                "params_push_form",
                "{what} by {requester}: {out:?}"
            );
            assert!(!asked(&out), "a form is refused unasked: {out:?}");
        }
    }
    // The words of the exclusion turned around ask FOR the lane.
    for condition in [
        "!!(has(hop.route) && hop.route == 'in_params')",
        "!(hop.route != 'in_params')",
        "(hop.route != 'in_params') == false",
        "hop.route != 'in_params' ? false : true",
        "has(hop.route) && hop.route != 'in_params' ? false : true",
        "hop.route != 'in_params' || true",
        "has(hop.route) && hop.route != 'in_params' && hop.route == 'in_params'",
    ] {
        let edge = json!({"from": ".", "to": format!("{other}/talky"), "condition": condition});
        let out = submit(&at(SCOPE, edge), AGENT);
        assert_eq!(refused(&out), "params_push_form", "{condition}: {out:?}");
    }
    // The lane in two pieces is no spelling either: the gate cannot list it.
    let out = submit(
        &at(
            SCOPE,
            with(
                "modifier",
                json!({"set_hop": {"route": "'in_' + 'params'"}}),
            ),
        ),
        AGENT,
    );
    assert_ne!(refused(&out), "", "{out:?}");
}

/// An edge that only steps ASIDE for the lane is no edge of it: the doors'
/// guard stands on ordinary edges of sealed hives, and a manifest that draws
/// such an edge again is an ordinary manifest.
#[test]
fn gh1353_an_edge_that_excludes_the_lane_is_an_ordinary_edge() {
    for condition in [
        "has(hop.route) && hop.route != 'tool' && !(has(hop.route) && hop.route == 'in_params')",
        "has(hop.route) && hop.route != 'in_params'",
        "(has(hop.a) || has(hop.b)) && !(has(hop.route) && hop.route == 'in_params') && !has(hop.params_answer)",
    ] {
        let out = submit(
            &at(
                &format!("{GEN}/talky"),
                json!({"from": ".", "to": "./brain", "condition": condition}),
            ),
            AGENT,
        );
        assert_eq!(refused(&out), "", "{condition}: {out:?}");
        assert!(asked(&out), "{condition}: {out:?}");
    }
}
