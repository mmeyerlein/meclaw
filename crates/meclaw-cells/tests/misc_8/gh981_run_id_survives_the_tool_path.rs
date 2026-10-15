//! GH #981 -- the static half of a run's road: what the shipped templates
//! promise about `context.run_id` and the edges a run takes. The booted half
//! (a real run, measured at the app) is
//! `gh981_a_run_reaches_the_brain_and_ends_at_the_app.rs`.
//!
//! 1. `run_id` survives the tool path: no `delete_context` list of any shipped
//!    template names `run_id` or `run_app` (exactly or by prefix), so the keys
//!    a run is told apart by ride every hop of its tool rounds.
//! 2. A run never reaches a surface: every edge that carries the core's
//!    `answer` or `ask` to a surface excludes a non-empty `context.run_id`, and
//!    this level draws no door for a run (the recipe's door enters the core's
//!    rim from the app and needs a `run_id`).
//! 3. Every tool result of a run reaches the app: the holders the recipe taps
//!    are exactly the holders the member hands a `tool_result` down to its
//!    generations from (`./<holder> -> ./assistants`). A holder the member
//!    gains is a holder a run hears, or this is red.
//! 4. Only the wiring writes the run keys (review of GH #981): `run_id`,
//!    `run_app` and `run_chain` are stamped keys (`STAMPED_CONTEXT_KEYS`), so
//!    an app whose own edge writes one never boots and the mutation door
//!    refuses it in a node's params -- an app cannot hand another app a
//!    forged `run_tool_result`/`run_answer`, nor take a person's consult off
//!    its surface. `run_chain` bounds the door's restored budget, so a config
//!    edge may not delete it either (a deleted counter restarts the bound).
//! 5. Nor parks them (re-review of GH #981): a config edge that sets a
//!    `ctx_<stamped key>` hop key per `set_hop` is refused at boot (a real
//!    tree: an app whose inner hive restores the key from its rim) and at the
//!    door -- for the run keys and for `turn_round`/`speaker` alike.
//!
//! No colony boots. Guarded like every template-reading test (GH #49).

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{emit_all, shipped_script};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/assistant/config.json",
        "templates/member/config.json",
        "templates/builder/recipes/config.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p:?}: {e}")))
        .unwrap_or_else(|e| panic!("{p:?}: {e}"))
}

fn configs(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a directory").flatten() {
        let p = entry.path();
        if p.is_dir() {
            configs(&p, out);
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// Every `delete_context` array anywhere in `v`.
fn delete_lists<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "delete_context" {
                    out.push(x);
                } else {
                    delete_lists(x, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| delete_lists(x, out)),
        _ => {}
    }
}

fn covers(entry: &str, key: &str) -> bool {
    match entry.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => entry == key,
    }
}

#[test]
fn no_template_deletes_the_run_keys() {
    if !shipped() {
        return;
    }
    let mut files = Vec::new();
    configs(&repo("templates"), &mut files);
    assert!(files.len() > 50, "the walk found the tree: {}", files.len());
    let mut lists = 0;
    for f in &files {
        let v = read_json(f);
        let mut found = Vec::new();
        delete_lists(&v, &mut found);
        for list in found {
            lists += 1;
            for entry in list
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                for key in ["run_id", "run_app"] {
                    assert!(
                        !covers(entry, key),
                        "{} deletes `{key}` (entry `{entry}`): a run would lose what it is \
                         told apart by on that edge",
                        f.display()
                    );
                }
            }
        }
    }
    assert!(lists > 100, "the walk read the delete lists: {lists}");
}

fn assistant_edges() -> Vec<Value> {
    read_json(&repo("templates/assistant/config.json"))["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

const NOT_A_RUN: &str = "(!has(context.run_id) || context.run_id == '')";

#[test]
fn a_run_never_reaches_a_surface_on_the_assistant_edges() {
    if !shipped() {
        return;
    }
    let edges = assistant_edges();
    let mut checked = 0;
    for e in &edges {
        let c = e["condition"].as_str().unwrap_or_default();
        let to_surface = e["from"] == json!("./cogny")
            && (e["to"] == json!("./talky") || e["to"] == json!("./talky-chat"));
        if to_surface && (c.contains("hop.route == 'answer'") || c.contains("hop.route == 'ask'")) {
            assert!(
                c.ends_with(&format!(" && {NOT_A_RUN}")),
                "the core's answer or ask reaches a surface on a run: {e:#?}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 4, "answer and ask, to each of the two surfaces");

    // The door is not this level's: the run enters the core's rim straight
    // from the app, on the edge `install_app` `runs` draws at the member
    // (`gh981_an_offer_names_its_callers.rs`). An entry lane here would have to
    // be one an occupant takes (`gh302` § the_level_declares_the_lanes_its_
    // occupants_ship), and the core takes the run as the `in_turn` it accepts.
    assert!(
        !edges.iter().any(|e| e["condition"]
            .as_str()
            .is_some_and(|c| c.contains("in_run") || c.contains("run_brain"))),
        "no edge of this level is a door for a run"
    );

    let contract = &read_json(&repo("templates/assistant/config.json"))["params"]["contract"];
    let lane = |side: &str, route: &str| -> Value {
        contract[side]
            .as_array()
            .expect("lanes")
            .iter()
            .find(|l| l["route"] == json!(route))
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert!(
        lane("accepts", "in_run").is_null(),
        "the rim declares no run lane"
    );
    assert_eq!(lane("emits", "run_answer")["at"], json!(["./cogny"]));
    assert_eq!(lane("emits", "run_tool_result")["at"], json!(["./tools"]));
}

#[test]
fn every_holder_the_member_hands_a_tool_result_down_from_is_tapped() {
    if !shipped() {
        return;
    }
    let member = read_json(&repo("templates/member/config.json"));
    let handed_down: BTreeSet<String> = member["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| e["to"] == json!("./assistants"))
        .filter(|e| {
            e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.route == 'tool_result'"))
        })
        .map(|e| e["from"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(handed_down.len() >= 5, "{handed_down:?}");

    let out = emit_all(
        &shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("utf-8"),
        ),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "install_app", "request": "…",
                                         "params": {"scope": "/m", "app": "probe-app",
                                                    "template": "probe-app@1.0.0",
                                                    "screen": "display", "generation": "sam",
                                                    "ctx": {"member_person": "alex"},
                                                    "declaration": {"runs": {"at": "./inbox",
                                                                             "brain": "cogny"}}}})
                              .to_string()}],
        }),
    );
    let tapped: BTreeSet<String> = out[0]["manifest"][0]["diff"]["add_edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| e["lane"] == json!("run_tool_result"))
        .map(|e| e["from"].as_str().unwrap_or_default().to_string())
        .filter(|f| f != "./assistants/sam/tools")
        .collect();
    assert_eq!(
        tapped, handed_down,
        "a run hears exactly the holders the member hands tool results down from"
    );
}

/// Review of GH #981 (Major 2 and OR-LP.RW.10): the run keys are the wiring's.
/// An app's own edge that writes `run_id`, `run_app` or `run_chain` is refused
/// in the boot pass and at the mutation door (`edge_schema`, naming the key);
/// the same write on an `add_edges` entry -- the recipe's door -- passes. A
/// config edge that deletes `run_chain`, exactly or by prefix, is refused too:
/// the door counts from 1 when the key is absent, so deleting it would restart
/// the bound of a chain that restores its budget at every crossing.
#[test]
fn an_app_cannot_write_the_run_keys() {
    use meclaw_colony::cel_eval::STAMPED_CONTEXT_KEYS;
    use meclaw_colony::mutation::substitute::{substitute_env_only, substitute_mutation_diff};
    use std::collections::HashMap;
    for key in ["run_id", "run_app", "run_chain"] {
        assert!(STAMPED_CONTEXT_KEYS.contains(&key), "{key} is stamped");
        let params = json!({"graph": {"edges": [
            {"from": "./tools", "to": ".", "modifier": {"set_context": {(key): "'probe-app'"}}}
        ]}});
        let err = substitute_env_only(&json!({"params": params.clone()}), &HashMap::new())
            .expect_err("an app's own edge wrote a run key and booted");
        assert_eq!(err.error_code(), "edge_schema", "{err:?}");
        assert!(err.message().contains(key), "{err:?}");
        let err = substitute_mutation_diff(
            &json!({"add_nodes": [{"name": "apps/forger", "template": "forger@1.0.0",
                                   "override_params": params}]}),
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect_err("the door laid an app edge that writes a run key");
        assert_eq!(err.error_code(), "edge_schema", "{err:?}");
        substitute_mutation_diff(
            &json!({"add_edges": [{"from": "./apps/probe-app", "to": "./assistants/sam/cogny",
                                   "modifier": {"set_context": {(key): "'probe-app'"}}}]}),
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("the wiring stamps it");
    }
    for entry in ["run_chain", "run_*", "run*"] {
        let err = substitute_env_only(
            &json!({"params": {"graph": {"edges": [
                {"from": "./inbox", "to": ".", "modifier": {"delete_context": [entry]}}
            ]}}}),
            &HashMap::new(),
        )
        .expect_err("an app's own edge deleted the chain counter");
        assert_eq!(err.error_code(), "edge_schema", "{entry}: {err:?}");
        assert!(err.message().contains("run_chain"), "{err:?}");
    }
    substitute_env_only(
        &json!({"params": {"graph": {"edges": [
            {"from": "./inbox", "to": ".", "modifier": {"delete_context": ["run_id"]}}
        ]}}}),
        &HashMap::new(),
    )
    .expect("deleting a run's id proves nothing and is not refused: the run goes unheard");
}

/// One app with an inner hive, on disk under `root/main`: the app's first edge
/// hands the inner hive the hop keys `hop` (`set_hop`, `ctx_<key>` = literal),
/// and the inner hive's edge from its rim writes each stamped key in the one
/// form a config edge may (`has(hop.ctx_<k>) ? hop.ctx_<k> : ''`). With
/// `hop` empty the first edge carries no modifier (the control).
fn app_with_an_inner_hive(root: &Path, hop: &[(&str, &str)]) -> Value {
    fn write(p: &Path, v: &Value) {
        std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
        std::fs::write(p, serde_json::to_string_pretty(v).expect("json")).expect("write");
    }
    let main = root.join("main");
    write(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}}),
    );
    write(
        &main.join("apps/config.json"),
        &json!({"cell": {"type": "hive"}}),
    );
    let mut first = json!({"from": ".", "to": "./inner"});
    if !hop.is_empty() {
        let set: serde_json::Map<String, Value> = hop
            .iter()
            .map(|(k, v)| (format!("ctx_{k}"), json!(format!("'{v}'"))))
            .collect();
        first["modifier"] = json!({"set_hop": set});
    }
    let app_params = json!({"graph": {"edges": [first]}});
    write(
        &main.join("apps/forger/config.json"),
        &json!({"cell": {"type": "hive"}, "params": app_params.clone()}),
    );
    let restore: serde_json::Map<String, Value> = hop
        .iter()
        .map(|(k, _)| {
            (
                (*k).to_string(),
                json!(format!("has(hop.ctx_{k}) ? hop.ctx_{k} : ''")),
            )
        })
        .collect();
    let mut inner_edge = json!({"from": ".", "to": "./sink"});
    if !restore.is_empty() {
        inner_edge["modifier"] = json!({"set_context": restore});
    }
    write(
        &main.join("apps/forger/inner/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [inner_edge]}}}),
    );
    write(
        &main.join("apps/forger/inner/sink/config.json"),
        &json!({"cell": {"type": "code"},
                "params": {"runner": "python3", "script_inline": "print('[]')",
                           "external_timeout_ms": 5000},
                "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                             "emits": {"body": {"messages": {"type": "array", "required": false}}},
                             "consumes": {"body": {"messages": {"type": "array", "required": false}}},
                             "capabilities": ["shell:exec"]},
                "description": {"purpose": "sink", "use_when": "Test fixture only.",
                                "not_in_scope": "Not a template."}}),
    );
    app_params
}

/// Re-review of GH #981 (Major, closes Major 2): a stamped key reaches a
/// config edge only as the restore of a parked hop key `ctx_<key>`, and that
/// hop key must come from a cell that really parked the turn (the colony drops
/// it from every other cell's emission). An app's OWN edge that sets
/// `ctx_<key>` per `set_hop` was the way around: its inner hive restores it
/// from the rim in the allowed form, and the app has written a forged
/// `run_app`/`run_id` (a `run_tool_result` at someone else's app, a consult
/// turned into a run) or a `run_chain` of its choice -- and, since GH #979,
/// `speaker`/`turn_round`. So a config edge never sets a `ctx_<stamped key>`
/// hop key: the boot (a real tree, `plan_bootstrap`) and the mutation door
/// refuse it as `edge_schema`; the same tree without that `set_hop` boots.
fn an_app_cannot_park_on_its_own_edge(hop: &[(&str, &str)]) {
    use meclaw_colony::cel_eval::STAMPED_CONTEXT_KEYS;
    use meclaw_colony::mutation::substitute::substitute_mutation_diff;
    use meclaw_colony::{BootstrapError, CellFactory, CellFactoryRegistry, RegistryOverlay};
    use std::collections::HashMap;
    use std::sync::Arc;
    for (k, _) in hop {
        assert!(STAMPED_CONTEXT_KEYS.contains(k), "{k} is stamped");
    }
    let mut factories = CellFactoryRegistry::new();
    factories.insert(
        "code".to_string(),
        Arc::new(meclaw_cells::code::CodeCellFactory) as Arc<dyn CellFactory>,
    );

    // The control: the inner hive's restore edge alone boots.
    let td = tempfile::TempDir::new().expect("tempdir");
    app_with_an_inner_hive(td.path(), &[]);
    meclaw_colony::plan_bootstrap(td.path(), &factories, &RegistryOverlay::new())
        .unwrap_or_else(|e| panic!("the app without the forging edge boots: {:?}", e.items()));
    let td = tempfile::TempDir::new().expect("tempdir");
    app_with_an_inner_hive(td.path(), hop);
    let app = td.path().join("main/apps/forger/config.json");
    std::fs::write(
        &app,
        serde_json::to_string(&json!({"cell": {"type": "hive"},
                                      "params": {"graph": {"edges": [
                                          {"from": ".", "to": "./inner"}]}}}))
        .expect("json"),
    )
    .expect("write");
    meclaw_colony::plan_bootstrap(td.path(), &factories, &RegistryOverlay::new())
        .unwrap_or_else(|e| panic!("the inner hive's restore alone boots: {:?}", e.items()));

    // The forging app: refused at boot, naming the app and the hop key.
    let td = tempfile::TempDir::new().expect("tempdir");
    let params = app_with_an_inner_hive(td.path(), hop);
    let errs = meclaw_colony::plan_bootstrap(td.path(), &factories, &RegistryOverlay::new())
        .expect_err("an app that parks a stamped key on its own edge booted");
    let refusals: Vec<String> = errs
        .items()
        .iter()
        .filter_map(|e| match e {
            BootstrapError::EnvSubstitution { path, reason } => {
                Some(format!("{}: {reason}", path.display()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(refusals.len(), 1, "{:?}", errs.items());
    assert!(
        refusals[0].contains("apps/forger:") && refusals[0].contains("EdgeSchema"),
        "{refusals:?}"
    );
    assert!(
        hop.iter()
            .any(|(k, _)| refusals[0].contains(&format!("ctx_{k}"))),
        "{refusals:?}"
    );

    // The same edges at the mutation door (a node's params).
    let err = substitute_mutation_diff(
        &json!({"add_nodes": [{"name": "apps/forger", "template": "forger@1.0.0",
                               "override_params": params}]}),
        &HashMap::new(),
        &HashMap::new(),
    )
    .expect_err("the door laid an app edge that parks a stamped key");
    assert_eq!(err.error_code(), "edge_schema", "{err:?}");
    assert!(err.message().contains("set_hop.ctx_"), "{err:?}");
}

#[test]
fn an_app_cannot_park_a_run_key_on_its_own_edge() {
    an_app_cannot_park_on_its_own_edge(&[("run_app", "victim"), ("run_id", "x")]);
    an_app_cannot_park_on_its_own_edge(&[("run_chain", "0")]);
}

#[test]
fn an_app_cannot_park_a_round_or_a_speaker_on_its_own_edge() {
    an_app_cannot_park_on_its_own_edge(&[("turn_round", "[\"*\"]"), ("speaker", "member:alex")]);
}
