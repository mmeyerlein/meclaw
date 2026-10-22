//! GH #1292 -- `templates/simple-agent`, the file: a plain agent in hive form
//! (contract V-T, the turn door; V-A, the identity in its own affinity).
//!
//! What the file alone decides, without a colony: the rim lanes are exactly the
//! turn door's, the hive is sealed and drains what its memory insists on, one
//! brain thinks and every model cell names the app, the tools hive ships six
//! families and no build pair, the file tools are rooted per instance, and the
//! grants name this template only. The loop itself is pinned in
//! `simple_agent_loop.rs`.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not
//! carry the template or one of its refs is skipped, never judged.

use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable};
use meclaw_colony::mutation::required_drains::{check_required_drains, collect_required_drains};
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::TemplatesRegistry;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Path, Uuid};
use meclaw_testing::override_params_on_disk;

const AGENT: &str = "/agent";

/// The model cells of the tree.
const MODEL_CELLS: [&str; 7] = [
    "brain",
    "curator/summarizer",
    "memory-hive/closer",
    "memory-hive/dialectic",
    "memory-hive/dreamer",
    "memory-hive/judge",
    "memory-hive/embed",
];

/// The six tool families the agent owns (R-SA-7).
const OWN_TOOLS: [&str; 6] = ["bash", "file", "edit", "web_fetch", "web_search", "weather"];

// ─────────────────────────────────────────────────────────────── the tree

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "simple-agent",
        "collector",
        "curator",
        "dispatcher",
        "memory-hive",
        "access",
        "tools",
        "affinity",
    ]
    .iter()
    .all(|t| repo(&format!("templates/{t}/config.json")).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

/// The template laid out the way instantiation lays it out: a `ref` directory
/// is replaced by the referenced template, its `override_params` applied
/// (GH #140, #277), and what the ref directory carries beside its marker (the
/// grants seeded under `access/store/seed`) laid over it.
fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(depth < 8, "ref chain does not end at {}", src.display());
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .expect("a ref names a template");
            let name = reference.split('@').next().unwrap_or_default();
            let target = repo("templates").join(name);
            assert!(
                target.join("config.json").is_file(),
                "{}: `{reference}` resolves to no template in this tree",
                marker.display()
            );
            copy_resolved(&target, dst, depth + 1);
            if let Some(over) = cfg["override_params"].as_object() {
                for (cell, params) in over {
                    override_params_on_disk(&dst.join(cell), params);
                }
            }
            overlay_seed(src, dst);
            return;
        }
    }
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let name = entry.file_name();
        if from.is_dir() {
            copy_resolved(&from, &dst.join(name), depth);
        } else if name == "config.json" || is_seed_row_file(src, &from) {
            std::fs::copy(&from, dst.join(name)).expect("copy");
        }
    }
}

fn is_seed_row_file(dir: &std::path::Path, file: &std::path::Path) -> bool {
    dir.file_name().is_some_and(|d| d == "seed") && file.extension().is_some_and(|e| e == "jsonl")
}

fn overlay_seed(src: &std::path::Path, dst: &std::path::Path) {
    for entry in std::fs::read_dir(src).expect("readable") {
        let from = entry.expect("entry").path();
        if from.is_dir() {
            let to = dst.join(from.file_name().expect("a name"));
            std::fs::create_dir_all(&to).expect("mkdir");
            overlay_seed(&from, &to);
        } else if is_seed_row_file(src, &from) {
            std::fs::copy(&from, dst.join(from.file_name().expect("a name"))).expect("copy");
        }
    }
}

fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "seed") {
                configs_under(&p, out);
            }
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// `(path relative to the agent, config)` of every cell of the instantiated tree.
fn cells_of(agent: &std::path::Path) -> Vec<(String, Value)> {
    let mut files = Vec::new();
    configs_under(agent, &mut files);
    let mut out: Vec<(String, Value)> = files
        .into_iter()
        .map(|f| {
            let rel = f
                .parent()
                .expect("a cell directory")
                .strip_prefix(agent)
                .expect("under the agent")
                .to_string_lossy()
                .replace('\\', "/");
            (rel, read_json(&f))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn agent_config() -> Value {
    read_json(&repo("templates/simple-agent/config.json"))
}

fn instantiated() -> (tempfile::TempDir, std::path::PathBuf) {
    let td = tempfile::TempDir::new().expect("tempdir");
    let agent = td.path().join("main/agent");
    copy_resolved(&repo("templates/simple-agent"), &agent, 0);
    (td, agent)
}

fn routes(lanes: &Value) -> Vec<String> {
    lanes
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

/// The single-quoted routes a condition names in `hop.route == '<r>'`.
fn named_routes(condition: &str) -> Vec<String> {
    condition
        .split("hop.route == '")
        .skip(1)
        .filter_map(|s| s.split('\'').next().map(str::to_string))
        .collect()
}

fn abs(base: &str, endpoint: &str) -> String {
    match endpoint {
        "." => base.to_string(),
        other => format!("{base}/{}", other.trim_start_matches("./")),
    }
}

fn table_of(base: &str, specs: &[EdgeSpec]) -> EdgeTable {
    let mut t = EdgeTable::new();
    for spec in specs {
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(base, &spec.from)),
            to: Path::new(&abs(base, &spec.to)),
            condition: spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
            }),
            modifier: spec.modifier.as_ref().map(|m| {
                meclaw_colony::cel_eval::parse_modifier(m)
                    .unwrap_or_else(|(k, e)| panic!("modifier {k}: {e}"))
            }),
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        });
    }
    t
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

// ═══════════════════════════════════════════════════════ the file

#[test]
fn simple_agent_is_sealed_and_speaks_the_turn_door() {
    if !shipped() {
        return;
    }
    let cfg = agent_config();
    let params = &cfg["params"];
    assert_eq!(params["ports"], json!([]), "the agent is sealed");
    let accepts = routes(&params["contract"]["accepts"]);
    let emits = routes(&params["contract"]["emits"]);
    assert_eq!(
        sorted(accepts.clone()),
        sorted(
            [
                "turn",
                "tool_result",
                "close",
                "in_pack",
                "in_model",
                "mutation_committed"
            ]
            .map(str::to_string)
            .to_vec()
        ),
        "accepts exactly the turn door (V-T)"
    );
    assert_eq!(
        sorted(emits.clone()),
        sorted(
            ["answer", "calls", "close_report", "model_refused", "error"]
                .map(str::to_string)
                .to_vec()
        ),
        "emits exactly the turn door (V-T)"
    );
    let hp: HiveParams =
        meclaw_core::serde_json::from_value(params.clone()).expect("the agent's params parse");
    for e in &hp.graph.edges {
        let named = named_routes(e.condition.as_deref().unwrap_or(""));
        if e.from == "." {
            for r in &named {
                assert!(
                    accepts.contains(r),
                    "the door {} -> {} opens the undeclared lane {r}",
                    e.from,
                    e.to
                );
            }
        }
        if e.to == "." {
            let leaves: Vec<String> = e
                .modifier
                .as_ref()
                .and_then(|m| m.set_hop.get("route"))
                .map(|r| vec![r.trim_matches('\'').to_string()])
                .unwrap_or(named);
            for r in &leaves {
                assert!(
                    emits.contains(r),
                    "the exit {} -> . carries the undeclared lane {r}",
                    e.from
                );
            }
        }
    }
    // Every accepts lane names a drain, and every lane the agent draws into
    // its memory hive and its affinity is drained.
    let drained: Vec<String> = params["required_drains"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|d| d["accepts"].as_str().map(str::to_string))
        .collect();
    for lane in ["turn", "tool_result", "close", "in_model"] {
        assert!(drained.contains(&lane.to_string()), "a drain for {lane}");
    }
    let (td, _agent) = instantiated();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{"type":"hive"}}"#,
    )
    .expect("root");
    let hives = [
        Path::new("/agent"),
        Path::new("/agent/memory-hive"),
        Path::new("/agent/affinity"),
        Path::new("/agent/tools"),
    ];
    let reqs = collect_required_drains(td.path(), hives.iter());
    assert!(!reqs.is_empty(), "the hives declare their drains");
    let table = table_of(AGENT, &hp.graph.edges);
    if let Err(e) = check_required_drains(&reqs, &table) {
        panic!("the agent leaves a lane undrained: {e:?}");
    }
}

#[test]
fn simple_agent_has_one_thinking_llm_and_app_id_on_every_model_cell() {
    if !shipped() {
        return;
    }
    let (_td, agent) = instantiated();
    let cells = cells_of(&agent);
    let thinking: Vec<String> = cells
        .iter()
        .filter(|(rel, cfg)| {
            cfg["cell"]["type"] == "llm"
                && !rel.starts_with("curator/")
                && !rel.starts_with("memory-hive/")
        })
        .map(|(rel, _)| rel.clone())
        .collect();
    assert_eq!(
        thinking,
        vec!["brain".to_string()],
        "one brain, nothing else thinks"
    );
    let mut wrong = Vec::new();
    let llms: Vec<&(String, Value)> = cells
        .iter()
        .filter(|(_, c)| c["cell"]["type"] == "llm")
        .collect();
    assert!(
        llms.len() >= 6,
        "the brain, the summarizer, four memory cells"
    );
    for (rel, cfg) in &llms {
        for key in ["x_title", "http_referer"] {
            if !cfg["params"][key].as_str().is_some_and(|v| !v.is_empty()) {
                wrong.push(format!("{rel}: {key} = {}", cfg["params"][key]));
            }
        }
    }
    let embed = cells
        .iter()
        .find(|(rel, _)| rel == "memory-hive/embed")
        .expect("the memory's embedder");
    let script = embed.1["params"]["script_inline"].as_str().unwrap_or("");
    for header in ["\"X-Title\"", "\"HTTP-Referer\""] {
        if !script.contains(header) {
            wrong.push(format!("memory-hive/embed sends no {header}"));
        }
    }
    assert!(wrong.is_empty(), "attribution:\n{}", wrong.join("\n"));
}

#[test]
fn simple_agent_ships_the_six_tool_families_and_no_build_pair() {
    if !shipped() {
        return;
    }
    let collector = read_json(&repo("templates/simple-agent/collector/config.json"));
    let asked: Vec<String> = collector["override_params"]["assemble"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    assert!(
        !asked.contains(&"*".to_string()),
        "`*` brings the build pair: {asked:?}"
    );
    for build in ["build_topology", "apply_manifest"] {
        assert!(
            !asked.contains(&build.to_string()),
            "the menu never asks for {build}: {asked:?}"
        );
    }
    for t in OWN_TOOLS {
        assert!(asked.contains(&t.to_string()), "the menu asks for {t}");
    }
    let cfg = agent_config();
    let hp: HiveParams =
        meclaw_core::serde_json::from_value(cfg["params"].clone()).expect("params parse");
    let mut served = Vec::new();
    for e in &hp.graph.edges {
        let cond = e.condition.as_deref().unwrap_or("");
        for build in ["build_topology", "apply_manifest", "in_build_result"] {
            assert!(!cond.contains(build), "no edge for {build}: {cond}");
        }
        if e.from == "./dispatcher" && e.to == "./tools" {
            served.extend(
                cond.split("hop.tool_name == '")
                    .skip(1)
                    .filter_map(|s| s.split('\'').next().map(str::to_string)),
            );
        }
        if e.to == "./tools" || e.from == "./tools" {
            assert!(
                !e.is_default,
                "no default edge into or out of the tools hive"
            );
        }
    }
    assert_eq!(
        sorted(served),
        sorted(OWN_TOOLS.map(str::to_string).to_vec()),
        "the dispatcher sends exactly the six families to the tools hive"
    );
}

#[test]
fn simple_agent_roots_files_in_its_own_tools_workspace() {
    if !shipped() {
        return;
    }
    // Two instances: the root of each is a directory of ITS tools hive.
    let (_ta, a) = instantiated();
    let (_tb, b) = instantiated();
    let root_of = |agent: &std::path::Path| -> Vec<std::path::PathBuf> {
        ["file", "edit"]
            .iter()
            .map(|c| {
                let cfg = read_json(&agent.join("tools").join(c).join("config.json"));
                let base = cfg["params"]["base_path"]
                    .as_str()
                    .expect("a base path")
                    .to_string();
                assert!(
                    !base.is_empty()
                        && !std::path::Path::new(&base).is_absolute()
                        && !base.contains(".."),
                    "{c}: the shipped root stays inside the tools hive: {base:?}"
                );
                agent.join("tools").join(base)
            })
            .collect()
    };
    let (ra, rb) = (root_of(&a), root_of(&b));
    assert_eq!(ra[0], ra[1], "file and edit share one root");
    assert_ne!(ra[0], rb[0], "two instances, two roots");
    // An instance moves the root through `tools/file` and `tools/edit`: the
    // nested cells of the ref are addressable, the param is admitted.
    let registry = TemplatesRegistry::from_entries(Vec::new());
    let parsed = parse_subtree(&a, &registry).expect("the agent parses");
    let version = read_json(&repo("templates/simple-agent/template.json"))["version"]
        .as_str()
        .expect("a version")
        .to_string();
    for c in ["tools/file", "tools/edit"] {
        let cell = parsed
            .cells
            .iter()
            .find(|x| x.rel_path == c)
            .unwrap_or_else(|| panic!("a cell {c}"));
        check_override_params(
            cell,
            Some(c),
            &format!("simple-agent@{version}"),
            &json!({"base_path": "elsewhere"}),
        )
        .unwrap_or_else(|why| panic!("{c}.base_path is admitted: {why:?}"));
        override_params_on_disk(&a.join(c), &json!({"base_path": "elsewhere"}));
    }
    let moved = root_of(&a);
    assert_eq!(moved[0], moved[1], "both moved together");
    assert_ne!(moved[0], ra[0], "the override moved the root");
}

#[test]
fn simple_agent_grants_name_only_template_simple_agent() {
    if !shipped() {
        return;
    }
    let (_td, agent) = instantiated();
    let cells = cells_of(&agent);
    let grants = std::fs::read_to_string(agent.join("access/store/seed/grants.jsonl"))
        .expect("the agent seeds its grants");
    let mut wrong = Vec::new();
    for line in grants.lines().skip(1) {
        let row: Value = meclaw_core::serde_json::from_str(line).expect("a grant row");
        let id = row["grant_id"].as_str().unwrap_or("");
        let who = row["requester"].as_str().unwrap_or("");
        if !id.starts_with("grant:openrouter@template-simple-agent/")
            || !who.starts_with("agent:simple-agent/")
            || row["subject"] != "template:simple-agent"
        {
            wrong.push(format!("foreign grant row: {line}"));
        }
    }
    for cell in MODEL_CELLS {
        let (_, cfg) = cells
            .iter()
            .find(|(rel, _)| rel == cell)
            .unwrap_or_else(|| panic!("no cell {cell}"));
        let grant = cfg["params"]["credential_grant_id"].as_str().unwrap_or("");
        if grant.is_empty() || !grants.contains(&format!("\"grant_id\": \"{grant}\"")) {
            wrong.push(format!("{cell}: grant {grant:?} not seeded"));
        }
    }
    // The requester an edge stamps is the one the grant names.
    let cfg = agent_config();
    for e in cfg["params"]["graph"]["edges"].as_array().expect("edges") {
        if let Some(req) = e["modifier"]["set_context"]["requester"].as_str() {
            let req = req.trim_matches('\'');
            if !grants.contains(&format!("\"requester\": \"{req}\"")) {
                wrong.push(format!("edge {} stamps unknown requester {req}", e["from"]));
            }
        }
    }
    let raw = std::fs::read_to_string(repo("templates/simple-agent/config.json")).expect("raw");
    for foreign in ["template-npc", "agent:npc/"] {
        if raw.contains(foreign) {
            wrong.push(format!("the hive names {foreign}"));
        }
    }
    assert!(wrong.is_empty(), "grants:\n{}", wrong.join("\n"));
}

#[test]
fn the_door_knows_the_tools_the_collector_asks_for() {
    if !shipped() {
        return;
    }
    let names = |v: &Value| -> Vec<String> {
        let mut out: Vec<String> = v
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        out.sort();
        out
    };
    let collector = read_json(&repo("templates/simple-agent/collector/config.json"));
    let door = read_json(&repo("templates/simple-agent/door/config.json"));
    assert_eq!(
        names(&door["params"]["own_tools"]),
        names(&collector["override_params"]["assemble"]["tools"]),
        "one list: the door drops exactly the names the menu asks for"
    );
}

/// The code cells of the agent that run rarely and keep no child between
/// their messages (GH #1292, R-SA-10): the identity boot and a proposal
/// (`affinity`), the schema answers the collector asks once at boot, the
/// night's close, dream and entity passes, the operator reads and timers.
/// A warm child of each would stand 30 minutes after its last job (GH #1276);
/// the ref markers set them `cold` instead, and the README says why.
const COLD_RARE: [&str; 13] = [
    "access/policy",
    "access/sweep",
    "affinity/brief",
    "affinity/gate",
    "affinity/push",
    "curator/schemas",
    "curator/stats",
    "memory-hive/close-glue",
    "memory-hive/dream-glue",
    "memory-hive/entity-glue",
    "memory-hive/recur",
    "memory-hive/schemas",
    "tools/schemas",
];

#[test]
fn simple_agent_runs_its_rare_code_cells_cold_and_its_turn_road_as_shipped() {
    if !shipped() {
        return;
    }
    let (_td, agent) = instantiated();
    let code: Vec<(String, Value)> = cells_of(&agent)
        .into_iter()
        .filter(|(_, cfg)| cfg["cell"]["type"] == "code")
        .collect();
    let mode = |cfg: &Value| {
        cfg["params"]["runner_mode"]
            .as_str()
            .unwrap_or("cold")
            .to_string()
    };
    for cell in COLD_RARE {
        let (_, cfg) = code
            .iter()
            .find(|(rel, _)| rel == cell)
            .unwrap_or_else(|| panic!("{cell}: a code cell of the agent"));
        assert_eq!(
            mode(cfg),
            "cold",
            "{cell}: a rare cell runs cold in the agent"
        );
    }
    // Every other code cell keeps the mode its own template ships: the turn
    // road (door, splitter, collector, dispatcher, curator, recall, embed, the
    // credential door) never starts a child per message.
    for (rel, cfg) in &code {
        if COLD_RARE.contains(&rel.as_str()) {
            continue;
        }
        let shipped_cfg = if rel == "door" || rel == "splitter" {
            read_json(&repo(&format!("templates/simple-agent/{rel}/config.json")))
        } else {
            let (tpl, cell) = rel.split_once('/').unwrap_or((rel.as_str(), ""));
            read_json(&repo(&format!("templates/{tpl}/{cell}/config.json")))
        };
        assert_eq!(
            mode(cfg),
            mode(&shipped_cfg),
            "{rel}: keeps its shipped mode"
        );
        assert_ne!(mode(cfg), "cold", "{rel}: no turn-road cell runs cold");
    }
    // Each cold cell has its written reason (RUNNER-MODES.md: `cold` only with
    // a reason) in the README's runner-mode section.
    let readme = std::fs::read_to_string(repo("templates/simple-agent/README.md")).expect("README");
    let section = readme
        .split("## Runner modes")
        .nth(1)
        .and_then(|s| s.split("\n## ").next())
        .expect("a `## Runner modes` section");
    for cell in COLD_RARE {
        assert!(
            section.contains(&format!("`{cell}`")),
            "{cell}: the README's runner-mode section names it and why"
        );
    }
}
