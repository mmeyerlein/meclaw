//! GH #858 — every shipped `llm` cell states in prose what it needs, and every
//! sealed composite that runs one opens the model door for it.
//!
//! The model registry chooses a model by translating what a cell needs against
//! its catalogue. That only works where the need is written down, next to the
//! cell, in words the catalogue can change under without the cell changing:
//! `params.requirement` (an immutable llm param since 0.46.0, never a model
//! name). And a choice only reaches a cell inside a sealed composite through
//! that composite's `in_model` door -- the seal refuses every edge onto the
//! cell itself. What is claimed, read off the tree:
//!
//! 1. every `llm` cell under `templates/` and `examples/` carries a
//!    `requirement` -- except `templates/display/**`, whose one authoritative
//!    description lives elsewhere (OR-SN-49) -- non-empty, within the 2 KiB the
//!    llm cell holds it to, and naming no model: no `vendor/model` shape, no
//!    model id of the shipped catalogue, not the cell's own start value;
//! 2. every sealed template (`params.ports == []`) with an `llm` cell directly
//!    in it -- except `display`, and `llm-registry`, whose translator is never
//!    resolved by the registry itself -- accepts `in_model`, draws exactly one door
//!    edge per llm cell from `.`, and no other door from `.` takes the lane;
//!    each such llm cell consumes a params-only body;
//! 3. the builder's recipe announces a grown brain's need as a byte copy of the
//!    template cell's `requirement` (it reads nothing but its stdin), and so do
//!    the shell's own announcement of its judge and its composer;
//! 4. the start tokens the recipe announces for the member's memory cells are
//!    the tokens those cells are born on, and the shell announces its judge
//!    and not its composer;
//! 5. every subscriber the shipped tree announces can take a catalogue push:
//!    in the default environment its start `base_url` is the catalogue's, or
//!    the catalogue's origin is in its `base_url_allow` -- the llm cell refuses
//!    any other endpoint at run time (`check_run_time_update`), and a refused
//!    push is not reported back to the registry (review rev-L2b2 I-1); nor
//!    does a catalogue package call for an `external_timeout_ms` a
//!    subscriber's backstop does not clear. No shipped row carries one today,
//!    so that half is made to bite on test rows (review rev-F2, m3).
//!
//! Static: no colony, no provider. R2b / GH #49: a tree without the templates
//! skips.

use meclaw_core::serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every template this file reads by name (the announced cells live in talky,
/// cogny, memory-hive and argus); the sweeps below read whatever the tree
/// carries. All of them ship in the public tree as well.
fn shipped() -> bool {
    [
        "talky",
        "cogny",
        "memory-hive",
        "argus",
        "llm-registry",
        "builder",
        "meclaw-os",
    ]
    .iter()
    .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

fn read_json(p: &Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Every `config.json` under `root` whose cell is an `llm`, keyed by the path
/// of its directory relative to the repository.
fn llm_cells(root: &str) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let mut stack = vec![repo(root)];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "config.json") {
                let Ok(raw) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
                    continue;
                };
                if v["cell"]["type"] == "llm" {
                    let rel = p
                        .parent()
                        .unwrap()
                        .strip_prefix(repo(""))
                        .unwrap_or(p.parent().unwrap())
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.insert(rel.trim_start_matches("./").to_string(), v);
                }
            }
        }
    }
    out
}

/// Every model id the shipped catalogue lists.
fn catalogue_ids() -> BTreeSet<String> {
    let seed = repo("templates/llm-registry/store/seed/models.jsonl");
    let Ok(raw) = std::fs::read_to_string(&seed) else {
        return BTreeSet::new();
    };
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| meclaw_core::serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| {
            let row = if v["row"].is_object() { &v["row"] } else { &v };
            row["model_id"].as_str().map(str::to_string)
        })
        .filter(|id| !id.is_empty())
        .collect()
}

/// A word shaped like `vendor/model` -- the form every provider id takes.
fn model_shaped(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || ",;:()".contains(c))
        .find(|w| {
            let mut parts = w.split('/');
            let (a, b) = (parts.next(), parts.next());
            a.is_some_and(|a| !a.is_empty())
                && b.is_some_and(|b| !b.is_empty() && !b.ends_with('.'))
        })
        .map(str::to_string)
}

/// The literal a start value names, when it names one: `x`, or the default of
/// `${VAR:-x}`. An environment token without a default names nothing.
fn start_literal(model: &str) -> Option<String> {
    if let Some(inner) = model.strip_prefix("${").and_then(|m| m.strip_suffix('}')) {
        return inner
            .split_once(":-")
            .map(|(_, d)| d.to_string())
            .filter(|d| !d.is_empty());
    }
    (!model.is_empty() && !model.contains("${")).then(|| model.to_string())
}

#[test]
fn every_llm_cell_states_what_it_needs_and_names_no_model() {
    if !shipped() {
        return;
    }
    let catalogue = catalogue_ids();
    let mut cells = llm_cells("templates");
    cells.extend(llm_cells("examples"));
    // Floors for the SMALLER tree: the public export leaves out the templates
    // it does not publish, and with them six llm cells. Counted 2026-09-26:
    // 22 walked / 21 checked in the full tree, 16 / 15 in the public one.
    assert!(
        cells.len() >= 15,
        "the walk found {} llm cells -- wrong root",
        cells.len()
    );
    let mut checked = 0;
    for (path, cell) in &cells {
        if path.starts_with("templates/display/") {
            // OR-SN-49: `display-hive.md` is the one authoritative description
            // of the display, and its judge is not described a second time here.
            continue;
        }
        let need = cell["params"]["requirement"].as_str().unwrap_or_default();
        assert!(
            !need.trim().is_empty(),
            "{path} states no `params.requirement`: the registry cannot choose a model for a \
             need nobody wrote down"
        );
        assert!(
            need.len() <= 2048,
            "{path}: the requirement is {} bytes, over the 2 KiB the llm cell holds it to",
            need.len()
        );
        assert_eq!(
            model_shaped(need),
            None,
            "{path}: the requirement names something shaped like a model id: {need}"
        );
        for id in &catalogue {
            let short = id.rsplit('/').next().unwrap_or(id);
            assert!(
                !need.contains(id.as_str()) && !need.contains(short),
                "{path}: the requirement names the catalogue's `{id}` -- a need is prose, the \
                 choice is the registry's: {need}"
            );
        }
        if let Some(start) = start_literal(cell["params"]["model"].as_str().unwrap_or_default()) {
            let short = start.rsplit('/').next().unwrap_or(&start).to_string();
            assert!(
                !need.contains(&start) && !need.contains(&short),
                "{path}: the requirement names the cell's own start value `{start}`"
            );
        }
        checked += 1;
    }
    assert!(checked >= 15, "only {checked} cells checked");
}

/// Hop of a push addressed to `cell` inside a composite.
fn push_hop(cell: &str) -> Map<String, Value> {
    json!({"route": "in_model", "subscriber": format!("/x/composite/{cell}")})
        .as_object()
        .cloned()
        .unwrap()
}

fn takes(condition: Option<&str>, hop: &Map<String, Value>) -> bool {
    let Some(src) = condition else {
        return true;
    };
    let cond = meclaw_colony::cel_eval::parse_condition(src)
        .unwrap_or_else(|e| panic!("condition {src:?}: {e}"));
    matches!(
        meclaw_colony::cel_eval::evaluate_condition(&cond, &Map::new(), hop),
        Ok(true)
    )
}

#[test]
fn every_sealed_composite_with_a_model_opens_the_model_door() {
    if !shipped() {
        return;
    }
    let mut sealed_with_llm = 0;
    for entry in std::fs::read_dir(repo("templates")).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let cfg_path = entry.path().join("config.json");
        // `display`: OR-SN-49, as above. `llm-registry`: its translator runs on a
        // start value the registry never resolves -- a registry that needed a
        // model to pick a model could not repair one -- so it has no door.
        if name == "display" || name == "llm-registry" || !cfg_path.is_file() {
            continue;
        }
        let cfg = read_json(&cfg_path);
        if cfg["params"]["ports"] != json!([]) {
            continue;
        }
        let mut llms: Vec<String> = std::fs::read_dir(entry.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().join("config.json").is_file())
            .filter(|e| read_json(&e.path().join("config.json"))["cell"]["type"] == "llm")
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        if llms.is_empty() {
            continue;
        }
        llms.sort();
        sealed_with_llm += 1;
        let accepts = cfg["params"]["contract"]["accepts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            accepts.iter().any(|l| l["route"] == "in_model"),
            "{name} is sealed and runs a model, and does not accept `in_model`: the registry's \
             choice has no way in"
        );
        let edges = cfg["params"]["graph"]["edges"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for llm in &llms {
            let hop = push_hop(llm);
            let reached: Vec<String> = edges
                .iter()
                .filter(|e| e["from"] == ".")
                .filter(|e| takes(e["condition"].as_str(), &hop))
                .map(|e| e["to"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                reached,
                vec![format!("./{llm}")],
                "{name}: a push addressed to `{llm}` must reach that cell and nothing else -- \
                 not another llm cell, not a door that takes every `in_` lane"
            );
            let body = &read_json(&entry.path().join(llm).join("config.json"))["contract"]["consumes"]
                ["body"];
            assert!(
                body["messages"]["required"] == false && body["params"]["type"] == "object",
                "{name}/{llm} does not consume a params-only body (no `messages`, a `params` \
                 slot): {body}"
            );
        }
    }
    // Floor for the SMALLER tree, as above: 11 in the full tree, 7 in the
    // public one (four sealed composites with a model are not published).
    assert!(
        sealed_with_llm >= 7,
        "only {sealed_with_llm} sealed composites with a model found"
    );
}

/// The recipe's copy of each need it announces, as the script spells it.
fn recipe_requirements() -> BTreeMap<String, String> {
    let recipes = read_json(&repo("templates/builder/recipes/config.json"));
    let script = recipes["params"]["script_inline"]
        .as_str()
        .unwrap_or_default();
    let start = script
        .find("REQUIREMENTS = {")
        .expect("the recipe spells the needs it announces once");
    let block = &script[start..start + script[start..].find("\n}").expect("a closed dict")];
    let mut out = BTreeMap::new();
    for line in block.lines().skip(1) {
        let line = line.trim().trim_end_matches(',');
        if let Some((k, v)) = line.split_once(": ") {
            let k: String = meclaw_core::serde_json::from_str(k).expect("a key");
            let v: String = meclaw_core::serde_json::from_str(v).expect("a value");
            out.insert(k, v);
        }
    }
    out
}

#[test]
fn what_the_recipe_announces_is_what_the_cell_states() {
    if !shipped() {
        return;
    }
    let copies = recipe_requirements();
    assert_eq!(
        copies.keys().cloned().collect::<Vec<_>>(),
        vec![
            "cogny/brain",
            "memory-hive/closer",
            "memory-hive/dialectic",
            "memory-hive/dreamer",
            "memory-hive/judge",
            "talky/brain"
        ],
        "the recipe announces the assistant's brains and the member's memory cells"
    );
    for (cell, copy) in &copies {
        let cfg = read_json(&repo(&format!("templates/{cell}/config.json")));
        assert_eq!(
            Some(copy.as_str()),
            cfg["params"]["requirement"].as_str(),
            "the recipe's copy of `{cell}`'s need drifted from the template: the registry \
             would choose for a need the cell no longer states"
        );
        assert!(
            !copy.contains('\'') && !copy.contains('\\') && !copy.contains('"'),
            "`{cell}`'s need cannot stand in the announcement's CEL literal: {copy}"
        );
    }
}

/// The shell's announcement edge and the entries it carries.
fn shell_announcement() -> (Vec<Value>, Value) {
    let shell = read_json(&repo("templates/meclaw-os/config.json"));
    let edges = shell["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let announce = edges
        .iter()
        .find(|e| {
            e["from"] == "."
                && e["to"] == "./llm-registry"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'mutation_committed'"))
        })
        .cloned()
        .expect("the shell announces its own model cells on the mutation receipt");
    (edges, announce)
}

fn announced_entries(announce: &Value) -> Value {
    let lit = announce["modifier"]["set_context"]["model_announced"]
        .as_str()
        .unwrap_or_default();
    meclaw_core::serde_json::from_str(lit.trim_matches('\'')).expect("a JSON literal")
}

/// The one road from the registry a push addressed to `path` takes.
fn roads_for<'a>(edges: &'a [Value], path: &str) -> Vec<&'a Value> {
    let hop = json!({"route": "update", "subscriber": path})
        .as_object()
        .cloned()
        .unwrap();
    edges
        .iter()
        .filter(|e| e["from"] == "./llm-registry")
        .filter(|e| takes(e["condition"].as_str(), &hop))
        .collect()
}

#[test]
fn the_shell_announces_its_judge_and_not_its_composer() {
    if !shipped() {
        return;
    }
    let (edges, announce) = shell_announcement();
    let set = &announce["modifier"]["set_context"];
    assert_eq!(set["model_announcer"], "'meclaw-os'");
    assert_eq!(set["model_generation"], "'/os'");
    let entries = announced_entries(&announce);
    let paths: Vec<&str> = entries
        .as_array()
        .map(|a| a.iter().filter_map(|e| e["cell_path"].as_str()).collect())
        .unwrap_or_default();
    // Review rev-L2b2 I-1: `builder/compose` is born on `LOCAL_LLM_BASE_URL`
    // and no allow list, so no catalogue push can land in it -- it is not
    // announced. Its door and its need stay: an operator whose local endpoint
    // is a catalogue endpoint subscribes it by hand.
    assert_eq!(
        paths,
        vec!["/os/argus/judge"],
        "the shell announces its judge and only its judge: {entries}"
    );
    let entry = &entries[0];
    let judge = read_json(&repo("templates/argus/judge/config.json"));
    // The shell substitutes nothing in its own values (gh302), and a token
    // written there is not bound when the shell is grown by a mutation --
    // measured in fix round 1 of L2b2 (review m-2): the registry row read the
    // judge's `${ARGUS_JUDGE_MODEL:-…}` literally. So the judge is announced
    // with an empty start value, which the registry takes from an entry that
    // states a need, and which never clears a start value an operator stated
    // (`llm_registry_template`
    // `an_announcement_with_no_start_value_keeps_the_one_the_registry_holds`).
    assert_eq!(
        entry["start_model"], "",
        "/os/argus/judge: the shell announces no start value it would have to substitute"
    );
    assert_eq!(
        entry["requirement"], judge["params"]["requirement"],
        "/os/argus/judge's announced need drifted from the template"
    );
    // A push addressed to either occupant has exactly one road: the registry's
    // `update`, restamped onto the occupant's door. The composer keeps its road
    // for a hand-made subscription.
    for (path, template) in [
        ("/os/argus/judge", "argus"),
        ("/os/builder/compose", "builder"),
    ] {
        let roads = roads_for(&edges, path);
        assert_eq!(roads.len(), 1, "{path}: {roads:?}");
        assert_eq!(roads[0]["to"], format!("./{template}"));
        assert_eq!(
            roads[0]["modifier"]["set_hop"]["route"], "'in_model'",
            "{path}: {}",
            roads[0]
        );
    }
    // A push for a brain under the organisations still takes the old road and
    // no other.
    let roads: Vec<String> = roads_for(&edges, "/os/orgs/acme/members/alex/memory-hive/judge")
        .iter()
        .map(|e| e["to"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(roads, vec!["./orgs".to_string()]);
}

/// A value as the default environment binds it: `${VAR:-d}` is `d`, a bare
/// `${VAR}` is empty.
fn default_env(v: &str) -> String {
    meclaw_testing::code_wire::resolve_script_vars(v)
}

/// `scheme://host[:port]` of a URL -- the unit `base_url_allow` is matched in.
fn origin(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    format!("{scheme}://{}", rest.split('/').next().unwrap_or_default())
}

/// Every place `templates/<template>` is instantiated -- a `ref` cell naming
/// `<template>@…` -- with the `override_params` it gives `cell` there (an
/// empty object where it gives none). Review rev2-L2b2, m-4: a subscriber
/// runs the params of its INSTANCE, and an instance may override what its
/// template cell is born on.
fn instances_of(template: &str, cell: &str) -> Vec<(String, Map<String, Value>)> {
    let prefix = format!("{template}@");
    let mut out = Vec::new();
    let mut stack = vec![repo("templates")];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "config.json") {
                let Ok(raw) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
                    continue;
                };
                if v["cell"]["type"] == "ref"
                    && v["cell"]["template"]
                        .as_str()
                        .is_some_and(|t| t.starts_with(&prefix))
                {
                    let over = v["override_params"][cell]
                        .as_object()
                        .cloned()
                        .unwrap_or_default();
                    out.push((p.to_string_lossy().to_string(), over));
                }
            }
        }
    }
    out
}

/// The shipped catalogue's rows, as the store seeds them.
fn catalogue_rows() -> Vec<Value> {
    let seed = std::fs::read_to_string(repo("templates/llm-registry/store/seed/models.jsonl"))
        .expect("the shipped catalogue");
    seed.lines()
        .filter_map(|l| meclaw_core::serde_json::from_str::<Value>(l).ok())
        .map(|v| {
            if v["row"].is_object() {
                v["row"].clone()
            } else {
                v
            }
        })
        .filter(|row| row["model_id"].as_str().is_some())
        .collect()
}

/// Every push of a catalogue `rows` that an announced subscriber of the
/// shipped tree would refuse, one line each -- empty when every package can
/// land. The two things a cell refuses a push over (`check_run_time_update`):
/// an endpoint its start value and its `base_url_allow` do not admit, and a
/// package call timeout its backstop does not clear.
fn push_refusals(rows: &[Value]) -> Vec<String> {
    let endpoints: BTreeSet<String> = rows
        .iter()
        .filter_map(|row| row["base_url"].as_str().map(str::to_string))
        .filter(|u| !u.is_empty())
        .collect();
    let timeouts: BTreeSet<u64> = rows
        .iter()
        .filter_map(|row| row["package"]["external_timeout_ms"].as_u64())
        .collect();
    // Who the shipped tree announces: the shell's own entries, and every
    // template cell the builder's recipe announces for a grown level.
    let mut announced: BTreeMap<String, String> = BTreeMap::new();
    for entry in announced_entries(&shell_announcement().1)
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let path = entry["cell_path"].as_str().unwrap_or_default().to_string();
        let cell = path.trim_start_matches("/os/").to_string();
        announced.insert(format!("shell {path}"), cell);
    }
    for cell in recipe_requirements().keys() {
        announced.insert(format!("recipe {cell}"), cell.clone());
    }
    assert!(announced.len() >= 7, "{announced:?}");
    let default_backstop = meclaw_colony::ColonyConfig::default().message_timeout_default_ms;
    let mut refused = Vec::new();
    for (who, cell) in &announced {
        let born = read_json(&repo(&format!("templates/{cell}/config.json")));
        let backstop = born["cell"]["message_timeout"]
            .as_u64()
            .unwrap_or(default_backstop);
        let (template, inner) = cell.split_once('/').expect("<template>/<cell>");
        // The template cell as it is born, and every instance of its template
        // with the overrides it gives this cell (a shallow merge, as the colony
        // applies `override_params`).
        let mut variants = vec![(format!("templates/{cell}"), born["params"].clone())];
        for (at, over) in instances_of(template, inner) {
            let mut params = born["params"].as_object().cloned().unwrap_or_default();
            params.extend(over);
            variants.push((
                format!("{at} (override_params.{inner})"),
                Value::Object(params),
            ));
        }
        for (at, params) in &variants {
            let start = default_env(params["base_url"].as_str().unwrap_or_default());
            let allow: BTreeSet<String> = params["base_url_allow"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(|u| origin(&default_env(u)))
                        .collect()
                })
                .unwrap_or_default();
            for endpoint in &endpoints {
                if !(*endpoint == start || allow.contains(&origin(endpoint))) {
                    refused.push(format!(
                        "{who} is announced as a subscriber, and a catalogue push to `{endpoint}` \
                         cannot land in `{at}`: it runs on `base_url` {start:?} with \
                         allow list {allow:?} in the default environment, so the cell refuses the \
                         package while the registry books it as delivered"
                    ));
                }
            }
        }
        for ms in &timeouts {
            if meclaw_cells::llm::params::backstop_shortfall(*ms, Some(backstop)).is_some() {
                refused.push(format!(
                    "{who}: a catalogue package calls for `external_timeout_ms` {ms}, which the \
                     backstop of `templates/{cell}` ({backstop} ms) does not clear -- the cell \
                     refuses the push"
                ));
            }
        }
    }
    refused
}

#[test]
fn every_announced_subscriber_can_take_a_catalogue_push() {
    if !shipped() {
        return;
    }
    let rows = catalogue_rows();
    assert!(
        rows.iter()
            .any(|row| row["base_url"].as_str().is_some_and(|u| !u.is_empty())),
        "the catalogue names no endpoint"
    );
    let refused = push_refusals(&rows);
    assert!(refused.is_empty(), "{}", refused.join("\n"));
    // The walk found the instances it exists for: the assistant's brains and a
    // member's memory are instantiated by reference.
    assert!(!instances_of("talky", "brain").is_empty());
    assert!(!instances_of("memory-hive", "closer").is_empty());
}

/// Review rev-F2, m3: no shipped catalogue row carries
/// `package.external_timeout_ms` today, so over the shipped rows the timeout
/// half of `push_refusals` runs over an empty set. It is made to bite here: a
/// row whose package calls for more than any backstop clears is refused for
/// every announced subscriber, one whose package fits every backstop is not,
/// and a row on an endpoint nobody admits is refused by the endpoint half.
#[test]
fn a_catalogue_row_a_subscriber_cannot_take_is_refused() {
    if !shipped() {
        return;
    }
    let rows = catalogue_rows();
    let with_row = |row: Value| {
        let mut all = rows.clone();
        all.push(row);
        all
    };
    let mut row = rows[0].clone();
    row["model_id"] = json!("test/too-slow");
    row["package"] = json!({"external_timeout_ms": 10_000_000});
    let refused = push_refusals(&with_row(row.clone()));
    assert!(
        !refused.is_empty()
            && refused
                .iter()
                .all(|r| r.contains("`external_timeout_ms` 10000000")),
        "a package no backstop clears is refused: {refused:?}"
    );
    // Once per announced subscriber, as the backstop is checked per cell.
    assert!(refused.len() >= 7, "{refused:?}");

    row["package"] = json!({"external_timeout_ms": 1_000});
    let refused = push_refusals(&with_row(row.clone()));
    assert!(
        refused.is_empty(),
        "a package every backstop clears lands: {refused:?}"
    );

    row["package"] = json!({});
    row["base_url"] = json!("https://nobody-admits.example/v1");
    let refused = push_refusals(&with_row(row));
    assert!(
        !refused.is_empty()
            && refused
                .iter()
                .all(|r| r.contains("https://nobody-admits.example/v1")),
        "an endpoint no subscriber admits is refused: {refused:?}"
    );
}

#[test]
fn the_member_is_announced_on_the_tokens_its_memory_is_born_on() {
    if !shipped() {
        return;
    }
    let recipes = read_json(&repo("templates/builder/recipes/config.json"));
    let script = recipes["params"]["script_inline"]
        .as_str()
        .unwrap_or_default();
    for cell in ["closer", "dialectic", "dreamer", "judge"] {
        let born = read_json(&repo(&format!("templates/memory-hive/{cell}/config.json")))["params"]
            ["model"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let inner = born
            .strip_prefix("${")
            .and_then(|b| b.strip_suffix('}'))
            .unwrap_or_else(|| panic!("memory-hive/{cell} is born on {born}, no env token"));
        // A plain `${VAR}` is announced `${VAR:-}` so an unset key cannot fail
        // the growth on an edge; a token with a default is announced as it is.
        let announced = if inner.contains(":-") {
            inner.to_string()
        } else {
            format!("{inner}:-")
        };
        // The script spells it in two pieces, `ENV + "{...}"`: the colony binds
        // every whole `${...}` in a script when it reads the cell's config.
        let spelled = format!("ENV + \"{{{announced}}}\"");
        assert!(
            script.contains(&spelled),
            "the recipe does not announce memory-hive/{cell} on `{born}` (looked for {spelled})"
        );
    }
}
