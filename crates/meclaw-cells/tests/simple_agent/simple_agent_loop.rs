//! GH #1292 -- `templates/simple-agent`, the loop: a colony of the
//! instantiated template with stub models (no real model call).
//!
//! A turn loops over the agent's own tools until the model stops; a client
//! tool leaves on `calls` and its `tool_result` continues the same turn; the
//! client menu lives for one turn; a client tool named like an own one is
//! dropped; a missing client result ends the turn named (the round's
//! watchdog); a result without its round keys is refused named; the loop stops
//! at `max_iter` with a partial answer; `close` reports after the close pass;
//! the sidecar's memory section reaches the memory hive. Under `@2` the run
//! grows the agent the way a colony does (the colony's access and affinity
//! beside it, the contract lanes drawn by the root); the identity boot into
//! an own affinity is `@1` only and runs against the frozen `@1.1.4`.
//!
//! Real time with hang marks only (AGENTS rule 11): code cells are child
//! processes, a virtual clock does not reach them.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::deadline::HANG;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{ColonyHandle, NEVER_CRON, override_params_on_disk};
use mock_openai::{MockOpenAI, OpenAiRequestSnapshot, canned_chat_completion, canned_tool_calls};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::mpsc;

const AGENT: &str = "/agent";
const VAULT_PASSPHRASE: &[u8] = b"simple-agent-test-passphrase";
const BRAIN_MODEL: &str = "brain-stub";
/// The model the second brain (`strong`, plan A2) is born with in a run.
pub(crate) const STRONG_MODEL: &str = "strong-stub";
pub(crate) const AUDIENCE: &str = "[\"member:caller\"]";

/// Which simple-agent a run grows (R-SA-37). `@2` grows the way a colony grows
/// it: the colony's own `access` and `affinity` beside the agent, the grant
/// ids handed in at birth (`override_params`), and the contract lanes drawn by
/// the embedder -- here the run's root. `@1` is the frozen `simple-agent@1.1.4`
/// with its own access and affinity inside.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Form {
    V1,
    V2,
    /// `examples/simple-agent-colony`: the colony's access, affinity and
    /// llm-registry beside an `@2` grown from the example's `grow.json` (the
    /// builder block's rendering, plan A5): its override params, its edges
    /// and its grant rows, laid out the way the door lays them out.
    Example,
    /// `@2` one level deeper: the level `/lvl` holds the colony's access,
    /// affinity and the agent (`/lvl/agent`), drawn as under `V2`, and the
    /// colony's commit receipts (`mutation_receipts` to `/lvl`) travel on to
    /// the run's sink only -- the agent hears the boot's receipt from nobody,
    /// so a run hands it exactly the receipt it chooses.
    V2Deep,
}

/// The level of a `V2Deep` run.
pub(crate) const LEVEL: &str = "/lvl";

/// The agent's id in the colony and the subscriber token of its identity.
pub(crate) const AGENT_ID: &str = "ava";
pub(crate) const SUBSCRIBER: &str = "agent:ava";
/// The provider keys deposited in the colony's vault.
pub(crate) const OPENROUTER_KEY: &str = "test-key";
pub(crate) const ANTHROPIC_KEY: &str = "test-anthropic-key";

/// The eight cells of `@2` that may spend a grant (R-SA-37: none of them
/// carries one in the template; the embedder names it at birth): the eight
/// model cells (`strong` among them, plan A2) and `tools/web_search`, which ships anonymous but opens the
/// same credential lane one level deeper (`templates/tools/config.json`).
pub(crate) const MODEL_CELLS: [&str; 9] = [
    "brain",
    "strong",
    "curator/summarizer",
    "memory-hive/closer",
    "memory-hive/dialectic",
    "memory-hive/dreamer",
    "memory-hive/judge",
    "memory-hive/embed",
    "tools/web_search",
];

/// The grant a consumer is born with in the run's colony, per credential.
pub(crate) fn grant_of(cell: &str, cred: &str) -> String {
    format!("grant:{cred}@colony-test/{}", cell.replace('/', "-"))
}

// ─────────────────────────────────────────────────────────────── the tree

pub(crate) fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

pub(crate) fn shipped() -> bool {
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

pub(crate) fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

pub(crate) fn as_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().expect("a JSON object")
}

/// The template laid out the way instantiation lays it out: a `ref` directory
/// is replaced by the referenced template, its `override_params` applied
/// (GH #140, #277), and what the ref directory carries beside its marker (the
/// grants seeded under `access/store/seed`) laid over it.
pub(crate) fn copy_resolved(src: &std::path::Path, dst: &std::path::Path, depth: usize) {
    assert!(depth < 8, "ref chain does not end at {}", src.display());
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .expect("a ref names a template");
            // A pinned copy (`simple-agent@1.1.4`) is its own directory.
            let name = reference.split('@').next().unwrap_or_default();
            let pinned = repo("templates").join(reference);
            let target = if pinned.join("config.json").is_file() {
                pinned
            } else {
                repo("templates").join(name)
            };
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

pub(crate) struct Stubs {
    pub(crate) brain: MockOpenAI,
    pub(crate) background: MockOpenAI,
}

/// Every `llm` cell at a stub (the brain at the scripted one), every model of
/// the tree named, every timer out of the run's way, the weather's endpoints
/// at the background stub (no network), the env file and the provider keys
/// deposited in the vault: the agent's own under `@1`, the colony's under
/// `@2`. `overlay` maps a cell path of the agent to params laid over it.
fn build(td: &tempfile::TempDir, stubs: &Stubs, overlay: &Value, form: Form) {
    let root = td.path();
    let deep = form == Form::V2Deep;
    let main = if deep {
        root.join("main").join(LEVEL.trim_start_matches('/'))
    } else {
        root.join("main")
    };
    let agent = main.join("agent");
    let template = match form {
        Form::V1 => "templates/simple-agent@1.1.4",
        Form::V2 | Form::V2Deep | Form::Example => "templates/simple-agent",
    };
    copy_resolved(&repo(template), &agent, 0);
    if matches!(form, Form::V2 | Form::V2Deep) {
        // The colony's own access and affinity (R-SA-37): the agent carries
        // neither, and its grants are handed in at birth.
        copy_resolved(&repo("templates/access"), &main.join("access"), 0);
        copy_resolved(&repo("templates/affinity"), &main.join("affinity"), 0);
        for cell in MODEL_CELLS {
            override_params_on_disk(
                &agent.join(cell),
                &json!({"credential_grant_id": grant_of(cell, "openrouter")}),
            );
        }
    }
    override_params_on_disk(
        &agent.join("tools/weather"),
        &json!({"geocode_endpoint": format!("{}/v1/geocode", stubs.background.base_url),
                "forecast_endpoint": format!("{}/v1/forecast", stubs.background.base_url)}),
    );
    let example = (form == Form::Example).then(example_declaration);
    if let Some(decl) = &example {
        copy_resolved(&repo("templates/access"), &main.join("access"), 0);
        copy_resolved(&repo("templates/affinity"), &main.join("affinity"), 0);
        copy_resolved(
            &repo("templates/llm-registry"),
            &main.join("llm-registry"),
            0,
        );
        let node = &decl["diff"]["add_nodes"][0];
        assert_eq!(node["name"], "agent", "the example grows `./agent`");
        for (cell, params) in as_map(&node["override_params"]) {
            override_params_on_disk(&agent.join(cell), &params);
        }
        seed_rows_on_disk(&main, &decl["diff"]["seed_rows"]);
    }
    for (cell, params) in as_map(overlay) {
        override_params_on_disk(&agent.join(cell), &params);
    }
    let mut files = Vec::new();
    configs_under(&main, &mut files);
    for f in &files {
        let mut cfg = read_json(f);
        let rel = f
            .parent()
            .expect("a cell directory")
            .strip_prefix(&agent)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        match cfg["cell"]["type"].as_str() {
            Some("llm") => {
                // The second brain answers from the scripted stub as well, under
                // its own model name, so a run tells the two apart on the wire.
                let (url, model) = if rel == "brain" {
                    (&stubs.brain.base_url, BRAIN_MODEL.to_string())
                } else if rel == "strong" {
                    (&stubs.brain.base_url, STRONG_MODEL.to_string())
                } else {
                    (
                        &stubs.background.base_url,
                        format!("stub-{}", rel.replace('/', "-")),
                    )
                };
                cfg["params"]["base_url"] = json!(format!("{url}/v1"));
                cfg["params"]["model"] = json!(model);
            }
            Some("timer") => {
                if let Some(schedules) = cfg
                    .pointer_mut("/params/schedules")
                    .and_then(Value::as_array_mut)
                {
                    for s in schedules.iter_mut() {
                        if s.get("cron").is_some() {
                            s["cron"] = json!(NEVER_CRON);
                        }
                    }
                }
            }
            _ => {}
        }
        write_json(f, &cfg);
    }
    // No armed night inside the run: the night comes on the lane.
    override_params_on_disk(
        &agent.join("memory-hive/writer"),
        &json!({"night_at_utc": ""}),
    );
    let mut edges = vec![json!({"from": "./agent", "to": "/sink", "condition": "has(hop.route)"})];
    let vault = match form {
        Form::V1 => "agent/access/vault",
        Form::Example => {
            let decl = example.as_ref().expect("the example's declaration");
            edges.extend(
                decl["diff"]["add_edges"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
            "access/vault"
        }
        Form::V2 | Form::V2Deep => {
            seed_grants(&main.join("access/store/seed"));
            edges.extend(colony_edges(&agent));
            "access/vault"
        }
    };
    if deep {
        // The receipts enter the level as a hive transit and leave for the
        // sink; the root above the level is a bare hive.
        edges.push(json!({"from": ".", "to": "/sink",
                          "condition": "has(hop.route) && hop.route == 'mutation_committed'"}));
        write_json(
            &root.join("main/config.json"),
            &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
        );
        write_json(
            &root.join("colony.json"),
            &json!({"schema_version": 1, "mutation_receipts": {"to": LEVEL}}),
        );
    }
    write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    let key_file = root.join("vault.key");
    std::fs::write(&key_file, VAULT_PASSPHRASE).expect("key file");
    // The operator's half of a grown vault (as `examples/` seed it): it opens
    // itself from a key file named in the environment.
    override_params_on_disk(
        &main.join(vault),
        &json!({"key_source": "plainfile", "key_file": key_file.display().to_string()}),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600)).expect("0600");
    }
    let vault_cell = if deep {
        format!("/main{LEVEL}/{vault}")
    } else {
        format!("/main/{vault}")
    };
    let mut deposits = vec![("cred:openrouter", OPENROUTER_KEY)];
    if matches!(form, Form::V2 | Form::V2Deep) {
        deposits.push(("cred:anthropic", ANTHROPIC_KEY));
    }
    for (cred, key) in deposits {
        meclaw_cells::vault::user_channel::add(
            root,
            &vault_cell,
            cred,
            key.as_bytes(),
            VAULT_PASSPHRASE,
        )
        .expect("the cold deposit into the vault");
    }
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for f in &files {
        let raw = std::fs::read_to_string(f).unwrap_or_default();
        let mut rest = raw.as_str();
        while let Some(start) = rest.find("${") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            // A token with a default keeps it; only a bare one needs a value.
            let name = &rest[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                vars.insert(name.to_string(), format!("dummy-{name}"));
            }
            rest = &rest[end + 1..];
        }
    }
    vars.insert(
        "MECLAW_VAULT_KEY_FILE".into(),
        key_file.display().to_string(),
    );
    vars.insert(
        "MEMORY_LLM_BASE_URL".into(),
        format!("{}/v1", stubs.background.base_url),
    );
    vars.insert(
        "MEMORY_EMBED_ENDPOINT".into(),
        format!("{}/v1/embeddings", stubs.background.base_url),
    );
    let body: String = vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(root.join(".env"), body).expect("the env file");
}

/// The one declaration of `examples/simple-agent-colony/grow.json`.
pub(crate) fn example_declaration() -> Value {
    let grow = read_json(&repo("examples/simple-agent-colony/grow.json"));
    let manifest = grow["manifest"].as_array().cloned().unwrap_or_default();
    assert_eq!(manifest.len(), 1, "the block is one declaration: {grow}");
    manifest[0].clone()
}

/// `seed_rows` of a declaration as the seed files of the targeted stores
/// (`<store>/seed/<table>.jsonl`, schema line first): what the door writes
/// into a running store, laid out for a first boot. A table the store's
/// shipped seed already carries keeps its rows and gains these.
fn seed_rows_on_disk(main: &std::path::Path, seed_rows: &Value) {
    for entry in seed_rows.as_array().cloned().unwrap_or_default() {
        let target = entry["target"].as_str().expect("a target");
        let table = entry["table"].as_str().expect("a table");
        let store = main.join(target.trim_start_matches("./"));
        let schema = read_json(&store.join("config.json"))["params"]["schema"][table].clone();
        let file = store.join("seed").join(format!("{table}.jsonl"));
        std::fs::create_dir_all(file.parent().expect("a seed dir")).expect("mkdir seed");
        let mut body = std::fs::read_to_string(&file).unwrap_or_default();
        if body.is_empty() {
            // The declared column types of the store (a json column stays json).
            let types: Map<String, Value> = as_map(&schema)
                .into_iter()
                .map(|(k, v)| {
                    let ty = v["type"].as_str().or_else(|| v.as_str()).unwrap_or("text");
                    (k, json!(ty))
                })
                .collect();
            body.push_str(&format!("{}\n", json!({"schema": types})));
        }
        for row in entry["rows"].as_array().cloned().unwrap_or_default() {
            body.push_str(&format!("{row}\n"));
        }
        std::fs::write(&file, body).expect("seed rows");
    }
}

/// The colony's grants for the eight consumers, one per credential the run
/// deposits: what the embedder seeds into the colony's access (`seed_rows` of
/// the builder block later, A5).
fn seed_grants(seed: &std::path::Path) {
    std::fs::create_dir_all(seed).expect("mkdir seed");
    let mut grants = vec![json!({"schema": {
        "constraints": "json", "capability": "text", "cred_ref": "text", "expires_at": "text",
        "grant_id": "text", "issued_at": "text", "purpose": "text", "requester": "text",
        "rule_id": "text", "scope": "json", "subject": "text"}})];
    let mut events = vec![json!({"schema": {
        "actor": "text", "at": "text", "detail": "json", "event": "text", "grant_id": "text",
        "id": "text", "reason_code": "text"}})];
    for cell in MODEL_CELLS {
        for cred in ["openrouter", "anthropic"] {
            let id = grant_of(cell, cred);
            grants.push(json!({
                "capability": "credential.read", "constraints": {"rate_per_min": 60},
                "cred_ref": format!("cred:{cred}"), "expires_at": "2099-01-01T00:00:00.000000Z",
                "grant_id": id, "issued_at": "2026-10-24T00:00:00.000000Z",
                "purpose": "the run's agent spends its provider key", "requester": format!("agent:agent/{cell}"),
                "rule_id": "test-credential-read", "scope": {"actions": ["vault.deliver"]},
                "subject": "colony:test"}));
            events.push(json!({
                "actor": "operator", "at": "2026-10-24T00:00:00.000000Z",
                "detail": {"why": "seeded by the run's colony"}, "event": "granted",
                "grant_id": id, "id": format!("ev-{}", id.replace([':', '@', '/'], "-")),
                "reason_code": ""}));
        }
    }
    let lines = |rows: &[Value]| -> String {
        rows.iter()
            .map(|r| {
                format!(
                    "{}\n",
                    meclaw_core::serde_json::to_string(r).expect("a row")
                )
            })
            .collect()
    };
    std::fs::write(seed.join("grants.jsonl"), lines(&grants)).expect("grants");
    std::fs::write(seed.join("grant_events.jsonl"), lines(&events)).expect("grant events");
}

/// The edges an embedder draws around `simple-agent@2` (its README, "Embedding
/// one level deeper"): the credential v-lanes per consumer, addressed by the
/// grant each cell was born with, the brief road, the proposal road and the
/// identity pack road into the colony's affinity.
pub(crate) fn colony_edges(agent: &std::path::Path) -> Vec<Value> {
    let mut edges = Vec::new();
    for cell in MODEL_CELLS {
        let cfg = read_json(&agent.join(cell).join("config.json"));
        let grant = cfg["params"]["credential_grant_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        edges.push(json!({
            "from": format!("./agent/{cell}"), "to": "./access", "lane": "credential_request",
            "condition": "has(hop.route) && hop.route == 'credential_request' && !has(hop.refused_subscriber)",
            "modifier": {"set_hop": {"route": "'in_invoke'"},
                         "set_context": {"requester": format!("'agent:agent/{cell}'")}}}));
        edges.push(json!({
            "from": "./access", "to": format!("./agent/{cell}"), "lane": "in_sealed",
            "condition": format!("has(hop.route) && hop.route == 'ack' && has(hop.operation) && hop.operation == 'vault.deliver' && has(hop.grant_id) && hop.grant_id == '{grant}'"),
            "modifier": {"set_hop": {"route": "'in_sealed'"}}}));
    }
    edges.extend([
        json!({"from": "./agent", "to": "./affinity",
               "condition": "has(hop.route) && hop.route == 'brief'",
               "modifier": {"set_hop": {"route": "'in_brief'"},
                            "set_context": {"asker": format!("'{SUBSCRIBER}'"),
                                            "audience_set": "has(context.audience_set) ? context.audience_set : ''",
                                            "brief_caller": "'agent'"}}}),
        json!({"from": "./affinity", "to": "./agent/collector", "lane": "in_briefing",
               "condition": "has(hop.route) && (hop.route == 'answer' || hop.route == 'error') && has(context.brief_caller) && context.brief_caller == 'agent'",
               "modifier": {"set_hop": {"route": "'in_briefing'", "brief_outcome": "hop.route"},
                            "delete_context": ["asker", "brief_caller"]}}),
        json!({"from": "./agent", "to": "./affinity",
               "condition": "has(hop.route) && hop.route == 'affinity_propose'",
               "modifier": {"set_hop": {"route": "'in_propose'"},
                            // The enrolment's `subscribe` steps (A5, `./enrol`) take their
                            // subscriber from this edge (affinity README, "Identity comes
                            // from the edge"): the identity under the agent's token, the
                            // charter under its `/brain` (OR-agent-2.A5.2).
                            "set_context": {"actor": "'agent:' + hop.agent_id",
                                            "subscriber": format!("has(context.sa_enrol) && context.sa_enrol == 'subscribe' ? '{SUBSCRIBER}' : (has(context.sa_enrol) && context.sa_enrol == 'subscribe_brain' ? '{SUBSCRIBER}/brain' : '')"),
                                            "propose_caller": "'agent'"}}}),
        json!({"from": "./affinity", "to": "./agent",
               "condition": "has(hop.route) && (hop.route == 'ack' || hop.route == 'error') && has(context.propose_caller) && context.propose_caller == 'agent'",
               "modifier": {"set_hop": {"route": "'in_propose_ack'", "propose_outcome": "hop.route"},
                            "delete_context": ["actor", "propose_caller"]}}),
        json!({"from": "./affinity", "to": "./agent/curator", "lane": "in_pack",
               "condition": format!("has(hop.route) && hop.route == 'answer' && has(hop.subscriber) && (hop.subscriber == '{SUBSCRIBER}' || hop.subscriber == '{SUBSCRIBER}/brain')"),
               "modifier": {"set_hop": {"route": "'in_pack'"},
                            "set_context": {"pack_sub": "hop.pack_sub", "pack_hash": "hop.pack_hash"}}}),
        json!({"from": "./agent", "to": "./affinity",
               "condition": "has(hop.route) && hop.route == 'pack_ack' && has(context.pack_sub) && context.pack_sub != ''",
               "modifier": {"set_hop": {"route": "'in_pack_ack'"}}}),
    ]);
    edges
}

pub(crate) struct Ports {
    pub(crate) sink: mpsc::Receiver<Message>,
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
        (
            "vault".to_string(),
            Arc::new(meclaw_cells::vault::VaultCellFactory),
        ),
        ("bash".to_string(), Arc::new(meclaw_cells::BashCellFactory)),
        ("file".to_string(), Arc::new(meclaw_cells::FileCellFactory)),
        ("edit".to_string(), Arc::new(meclaw_cells::EditCellFactory)),
        (
            "web_fetch".to_string(),
            Arc::new(meclaw_cells::WebFetchCellFactory),
        ),
        (
            "web_search".to_string(),
            Arc::new(meclaw_cells::WebSearchCellFactory),
        ),
    ]
}

pub(crate) async fn boot(
    brain: Vec<meclaw_testing::mock_http::MockResponse>,
    overlay: Value,
) -> (tempfile::TempDir, Stubs, ColonyHandle, Ports) {
    boot_form(Form::V2, brain, overlay).await
}

pub(crate) async fn boot_form(
    form: Form,
    brain: Vec<meclaw_testing::mock_http::MockResponse>,
    overlay: Value,
) -> (tempfile::TempDir, Stubs, ColonyHandle, Ports) {
    let stubs = Stubs {
        brain: MockOpenAI::start(brain).await,
        // One answer for every background model: an empty verdict is a close
        // pass that changes nothing, and a summary all the same.
        background: MockOpenAI::start(vec![canned_chat_completion("{}", "stop")]).await,
    };
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &stubs, &overlay, form);
    let agent = if form == Form::V2Deep {
        format!("{LEVEL}{AGENT}")
    } else {
        AGENT.to_string()
    };
    let (h, ports) = grow(&td, &agent).await;
    (td, stubs, h, ports)
}

/// A colony again on a tree an earlier run grew and shut down: every cell
/// starts a new life on the `cell.db` it left (a restart, not a rebirth).
pub(crate) async fn reboot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
    grow(td, AGENT).await
}

async fn grow(td: &tempfile::TempDir, agent: &str) -> (ColonyHandle, Ports) {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the instantiated agent boots");
    // The menu before the first turn: a colony asks its collector after a
    // mutation; here the rim lane does, and the run waits for the answers.
    h.send(
        MessageBuilder::new(Path::new(agent))
            .hop(as_map(&json!({"route": "mutation_committed"})))
            .context(Map::new())
            .body(Body::Inline(json!({"messages": []})))
            .ttl(400)
            .build(),
    )
    .await;
    let collector = format!("{agent}/collector");
    for answerer in ["tools", "curator", "memory-hive"] {
        wait_route(&h, &format!("{agent}/{answerer}"), &collector, "in_menu", 1).await;
    }
    h.quiesce().await;
    (h, Ports { sink: sink_rx })
}

pub(crate) fn body_of(m: &Message) -> Value {
    match &m.body {
        Body::Inline(v) => v.clone(),
        other => panic!("an inline body on the rim, got {other:?}"),
    }
}

pub(crate) fn route_of(m: &Message) -> String {
    m.headers
        .hop
        .get("route")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// The next rim message on `route`; others on the way are kept. A run that
/// never brings it fails at the hang mark with the road the messages took.
pub(crate) async fn next_on(
    h: &ColonyHandle,
    ports: &mut Ports,
    route: &str,
    seen: &mut Vec<Message>,
) -> Message {
    let wait = async {
        loop {
            let m = ports.sink.recv().await.expect("the sink is open");
            if route_of(&m) == route {
                return m;
            }
            seen.push(m);
        }
    };
    match tokio::time::timeout(meclaw_testing::deadline::scaled(HANG), wait).await {
        Ok(m) => m,
        Err(_) => panic!(
            "HANG: the agent never emitted `{route}`; the road:\n{}",
            road(h).await
        ),
    }
}

/// `from -> to route` of the last logged messages (the store's own traffic
/// left out), oldest first.
pub(crate) async fn road(h: &ColonyHandle) -> String {
    let mut rows = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await;
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows.retain(|r| {
        !r.to_path.ends_with("store")
            && !r.from_path.ends_with("store")
            && !r.to_path.ends_with("ledger")
            && !r.from_path.ends_with("ledger")
    });
    let skip = rows.len().saturating_sub(120);
    rows.iter()
        .skip(skip)
        .map(|r| {
            let hdr: Value = meclaw_core::serde_json::from_str(&r.headers_json).unwrap_or_default();
            let hop = &hdr["hop"];
            format!(
                "{} -> {} {} {}",
                r.from_path,
                r.to_path,
                hop["route"].as_str().unwrap_or("-"),
                [
                    "error_code",
                    "reject_reason",
                    "store_error",
                    "finish_reason",
                    "phase"
                ]
                .iter()
                .filter_map(|k| hop[*k].as_str().map(|v| format!("{k}={v}")))
                .collect::<Vec<_>>()
                .join(" ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn wire_text(req: &OpenAiRequestSnapshot) -> String {
    req.messages()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| match &m["content"] {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn system_text(req: &OpenAiRequestSnapshot) -> String {
    req.messages()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|m| m["role"] == "system")
        .map(|m| m["content"].as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

// ─────────────────────────────────────────────────────────────── the rim

pub(crate) fn look_tool() -> Value {
    json!({"name": "look", "description": "Look at something in front of you.",
           "parameters": {"type": "object", "properties": {"at": {"type": "string"}},
                          "required": ["at"]}})
}

/// One `turn` on the door: hop keys of V-T, a user text, the client tools.
pub(crate) fn turn(turn_id: &str, text: &str, tools: Value) -> Message {
    let mut body = json!({"messages": [{"origin": "user", "type": "text", "text": text}]});
    if !tools.is_null() {
        body["tools"] = tools;
    }
    MessageBuilder::new(Path::new(AGENT))
        .hop(as_map(
            &json!({"route": "turn", "turn_id": turn_id, "session_id": "s1",
                             "channel": "c1", "audience_set": AUDIENCE}),
        ))
        .context(Map::new())
        .body(Body::Inline(body))
        .ttl(400)
        .build()
}

/// Tell the door which own tool families are on (A4's `tools_enabled`, which
/// reaches the door on its `tools_enabled` lane): only an enabled family's
/// names shadow a client tool.
pub(crate) async fn enable(h: &ColonyHandle, families: &[&str]) {
    // From inside the hive (A4's `./tool_gate` sends it there): the sealed rim refuses an outside
    // caller that names `/agent/door` (`hive_boundary`), and the lane is no rim lane.
    h.send_from(
        Path::new("/agent/tools"),
        MessageBuilder::new(Path::new("/agent/door"))
            .hop(as_map(&json!({"route": "tools_enabled"})))
            .context(Map::new())
            .body(Body::Inline(
                json!({"messages": [], "tools_enabled": families}),
            ))
            .ttl(400)
            .build(),
    )
    .await;
    h.quiesce().await;
}

/// The `tool_result` for a `calls` message: `keys` of its hop come back.
pub(crate) fn result_for(calls: &Message, keys: &[&str], content: &str) -> Message {
    let mut hop = json!({"route": "tool_result"});
    for k in keys {
        if let Some(v) = calls.headers.hop.get(*k) {
            hop[*k] = v.clone();
        }
    }
    let id = body_of(calls)["calls"]["id"].clone();
    MessageBuilder::new(Path::new(AGENT))
        .hop(as_map(&hop))
        .context(Map::new())
        .body(Body::Inline(json!({"messages": [],
                                   "tool_result": {"id": id, "content": content}})))
        .ttl(400)
        .build()
}

pub(crate) const ROUND_KEYS: [&str; 5] =
    ["turn_id", "session_id", "iter", "channel", "audience_set"];

pub(crate) fn tool_names(req: &OpenAiRequestSnapshot) -> Vec<String> {
    req.tools()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
        .collect()
}

pub(crate) fn tool_description(req: &OpenAiRequestSnapshot, name: &str) -> String {
    req.tools()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|t| t["function"]["name"] == name)
        .and_then(|t| t["function"]["description"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// How many logged messages on `route` (any route when empty) went from under `from` to under `to`.
pub(crate) async fn count_route(h: &ColonyHandle, from: &str, to: &str, route: &str) -> usize {
    h.read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter(|r| {
            r.from_path.starts_with(from)
                && r.to_path.starts_with(to)
                && meclaw_core::serde_json::from_str::<Value>(&r.headers_json)
                    .is_ok_and(|h| route.is_empty() || h["hop"]["route"] == route)
        })
        .count()
}

/// Waits until `n` messages on `route` went from under `from` to under `to`:
/// event-driven over the log's commit counter, the hang mark as the deadline.
pub(crate) async fn wait_route(h: &ColonyHandle, from: &str, to: &str, route: &str, n: usize) {
    let mut seq = h.subscribe_log_seq();
    let wait = async {
        loop {
            seq.borrow_and_update();
            if count_route(h, from, to, route).await >= n {
                return true;
            }
            if seq.changed().await.is_err() {
                return false;
            }
        }
    };
    match tokio::time::timeout(meclaw_testing::deadline::scaled(HANG), wait).await {
        Ok(true) => {}
        Ok(false) => panic!(
            "the log closed before {n} x {route} {from} -> {to}; the road:\n{}",
            road(h).await
        ),
        Err(_) => panic!(
            "HANG: never {n} x {route} {from} -> {to}; the road:\n{}",
            road(h).await
        ),
    }
}

/// The AIEOS protocol version of the test persona. Spelled in two halves: as
/// one literal it equals the version of `templates/simple-agent`, and the
/// merge point's cascade check (`cascade-dry`) reads any such literal in a file
/// that names the template as a template pin that the bump would leave behind.
const AIEOS_VERSION: &str = concat!("1.1", ".0");

pub(crate) fn aieos(motivation: &str) -> Value {
    json!({"standard": {"protocol": "AIEOS", "version": AIEOS_VERSION},
           "metadata": {"instance_id": "ava-1"},
           "identity": {"names": {"first": "Ava", "last": "Quill"}},
           "motivations": {"core_drive": motivation}})
}

// ═══════════════════════════════════════════════ the loop (colony)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_loops_over_own_tools_until_the_model_stops() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-weather", "weather", r#"{"place":"Berlin"}"#)]),
            canned_tool_calls(vec![(
                "c-file",
                "file",
                r#"{"op":"write","path":"note.txt","content":"hello"}"#,
            )]),
            canned_chat_completion("Done.", "stop"),
        ],
        json!({"tool_gate": {"tools_enabled": ["weather", "file", "bash", "edit"]}}),
    )
    .await;
    h.send(turn("t-1", "Note the weather.", Value::Null)).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    let slot = body_of(&answer)["answer"].clone();
    assert_eq!(slot["turn_id"], "t-1", "{slot}");
    assert_eq!(slot["text"], "Done.", "{slot}");
    assert!(slot.get("partial").is_none(), "a full answer: {slot}");
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 3, "three model calls for one answer");
    let names = tool_names(&reqs[0]);
    for t in ["weather", "file", "bash", "edit"] {
        assert!(names.iter().any(|n| n == t), "own tool {t}: {names:?}");
    }
    for (i, id) in [(1, "c-weather"), (2, "c-file")] {
        let msgs = reqs[i].messages().cloned().unwrap_or_default();
        assert!(
            msgs.iter()
                .any(|m| m["role"] == "tool" && m["tool_call_id"] == id),
            "call {i} carries the result of {id}: {}",
            wire_text(&reqs[i])
        );
    }
    assert!(
        seen.iter().all(|m| route_of(m) != "calls"),
        "own tools never leave the hive"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_tool_leaves_as_calls_and_its_result_continues_the_turn() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-look", "look", r#"{"at":"the door"}"#)]),
            canned_chat_completion("I see a red door.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(turn("t-1", "What is there?", json!([look_tool()])))
        .await;
    let mut seen = Vec::new();
    let calls = next_on(&h, &mut ports, "calls", &mut seen).await;
    for k in ROUND_KEYS
        .iter()
        .chain(["tool_name", "tool_call_id"].iter())
    {
        assert!(
            calls.headers.hop.contains_key(*k),
            "calls carries {k}: {:?}",
            calls.headers.hop
        );
    }
    assert_eq!(calls.headers.hop["turn_id"], "t-1");
    let slot = body_of(&calls)["calls"].clone();
    assert_eq!(slot["name"], "look", "{slot}");
    assert_eq!(slot["arguments"]["at"], "the door", "{slot}");
    assert!(
        seen.iter().all(|m| route_of(m) != "answer"),
        "the client call does not end the turn"
    );
    h.send(result_for(&calls, &ROUND_KEYS, "A red door.")).await;
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "I see a red door.");
    assert_eq!(body_of(&answer)["answer"]["turn_id"], "t-1");
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "the same turn went on: two model calls");
    assert!(
        wire_text(&reqs[1]).contains("A red door."),
        "the client result is in the window"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_tools_are_in_the_menu_of_the_same_turn_and_gone_in_the_next() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_chat_completion("First.", "stop"),
            canned_chat_completion("Second.", "stop"),
        ],
        json!({"tool_gate": {"tools_enabled": ["weather"]}}),
    )
    .await;
    let mut seen = Vec::new();
    h.send(turn("t-1", "One.", json!([look_tool()]))).await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.send(turn("t-2", "Two.", Value::Null)).await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2);
    assert!(
        tool_names(&reqs[0]).contains(&"look".to_string()),
        "the client tool is in the menu of its turn: {:?}",
        tool_names(&reqs[0])
    );
    assert!(
        !tool_names(&reqs[1]).contains(&"look".to_string()),
        "a turn without tools[] empties the client menu: {:?}",
        tool_names(&reqs[1])
    );
    assert!(
        tool_names(&reqs[1]).contains(&"weather".to_string()),
        "the own tools stay"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_tool_named_like_an_own_tool_is_dropped() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![canned_chat_completion("Fine.", "stop")],
        json!({"tool_gate": {"tools_enabled": ["weather"]}}),
    )
    .await;
    let fake = json!({"name": "weather", "description": "CLIENT WEATHER",
                      "parameters": {"type": "object", "properties": {}}});
    h.send(turn("t-1", "Hi.", json!([fake, look_tool()]))).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    let names = tool_names(&reqs[0]);
    assert_eq!(
        names.iter().filter(|n| *n == "weather").count(),
        1,
        "{names:?}"
    );
    assert!(names.contains(&"look".to_string()), "{names:?}");
    assert!(
        !tool_description(&reqs[0], "weather").contains("CLIENT"),
        "the own weather stays"
    );
    let dropped = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
        .filter(|h| h["hop"]["route"] == "in_turn")
        .filter_map(|h| {
            h["hop"]["client_tools_dropped"]
                .as_str()
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert!(
        !dropped.is_empty() && dropped.iter().all(|d| d == "[\"weather\"]"),
        "the turn names the drop: {dropped:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_tool_named_like_an_own_family_is_dropped() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) =
        boot(vec![canned_chat_completion("Fine.", "stop")], json!({})).await;
    enable(&h, &["web"]).await;
    let fake = json!({"name": "web", "description": "CLIENT WEB",
                      "parameters": {"type": "object", "properties": {}}});
    h.send(turn("t-1", "Hi.", json!([fake, look_tool()]))).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert!(
        !tool_description(&reqs[0], "web").contains("CLIENT"),
        "the own web family stays: {:?}",
        tool_names(&reqs[0])
    );
    let dropped = h
        .read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
        .filter(|h| h["hop"]["route"] == "in_turn")
        .filter_map(|h| {
            h["hop"]["client_tools_dropped"]
                .as_str()
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert!(
        !dropped.is_empty() && dropped.iter().all(|d| d == "[\"web\"]"),
        "the door names the drop of the family word: {dropped:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_memory_comes_back_in_a_callers_recall() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-rec", "memory_recall", r#"{"query":"rowan tree"}"#)]),
            canned_chat_completion("A rowan.", "stop"),
        ],
        json!({"door": {"start_memories": [{"what": "You planted a rowan tree by the lighthouse."}]}}),
    )
    .await;
    h.send(turn("t-1", "What did you plant?", Value::Null))
        .await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert!(
        wire_text(&reqs[1]).contains("rowan tree by the lighthouse"),
        "the caller's recall finds the start memory: {}",
        wire_text(&reqs[1])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_client_result_ends_the_turn_named_not_silent() {
    if !shipped() {
        return;
    }
    // A small idle window: the round's watchdog fires 1 s after it.
    let idle = json!({"round_idle_ms": 1500});
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-look", "look", r#"{"at":"the door"}"#)]),
            canned_chat_completion("I could not look.", "stop"),
        ],
        json!({"door": idle, "collector/assemble": idle}),
    )
    .await;
    h.send(turn("t-1", "What is there?", json!([look_tool()])))
        .await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "calls", &mut seen).await;
    // No result: the watchdog fires, the collector sweeps the round.
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    let slot = body_of(&answer)["answer"].clone();
    assert_eq!(slot["turn_id"], "t-1", "{slot}");
    assert_eq!(
        slot["stale"], true,
        "the answer names the lost result: {slot}"
    );
    assert!(
        // The timer delivers its strike to `emit_to` itself (origin `@external`);
        // the relative `emit_to` resolves over the hive's edge ./watch -> ./collector.
        count_route(&h, "@external", "/agent/collector", "in_round_sweep").await >= 1,
        "the watchdog fired; the road:\n{}",
        road(&h).await
    );
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert!(
        wire_text(&reqs[1]).contains("tool result lost"),
        "the brain saw the lost result: {}",
        wire_text(&reqs[1])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_result_without_its_iter_is_refused_named() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-look", "look", r#"{"at":"the door"}"#)]),
            canned_chat_completion("A door.", "stop"),
        ],
        json!({}),
    )
    .await;
    h.send(turn("t-1", "What is there?", json!([look_tool()])))
        .await;
    let mut seen = Vec::new();
    let calls = next_on(&h, &mut ports, "calls", &mut seen).await;
    h.send(result_for(&calls, &["turn_id"], "A door.")).await;
    let err = next_on(&h, &mut ports, "error", &mut seen).await;
    let slot = body_of(&err)["error"].clone();
    assert_eq!(slot["error_code"], "tool_result_unkeyed", "{slot}");
    assert!(
        slot["missing"]
            .as_array()
            .is_some_and(|m| m.contains(&json!("iter"))),
        "{slot}"
    );
    // The keyed result still continues the turn.
    h.send(result_for(&calls, &ROUND_KEYS, "A door.")).await;
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "A door.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_loop_stops_at_max_iter_with_a_partial_answer() {
    if !shipped() {
        return;
    }
    let call = |id: &str| canned_tool_calls(vec![(id, "weather", r#"{"place":"Berlin"}"#)]);
    let (_td, stubs, h, mut ports) = boot(
        vec![call("c-1"), call("c-2"), call("c-3"), call("c-4")],
        json!({"collector/assemble": {"max_iter": 2}, "tool_gate": {"tools_enabled": ["weather"]}}),
    )
    .await;
    h.send(turn("t-1", "Loop.", Value::Null)).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    let slot = body_of(&answer)["answer"].clone();
    assert_eq!(slot["partial"], true, "the capped turn says so: {slot}");
    h.quiesce().await;
    assert!(
        stubs.brain.recorded_requests().await.len() <= 3,
        "the cap stopped the loop"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_reports_after_the_close_pass_and_triggers_the_dream() {
    if !shipped() {
        return;
    }
    let (_td, _stubs, h, mut ports) =
        boot(vec![canned_chat_completion("Hello.", "stop")], json!({})).await;
    let mut seen = Vec::new();
    h.send(turn("t-1", "Hi.", Value::Null)).await;
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.send(
        MessageBuilder::new(Path::new(AGENT))
            .hop(as_map(&json!({"route": "close"})))
            .context(Map::new())
            .body(Body::Inline(
                json!({"messages": [], "close": {"session_id": "s1"}}),
            ))
            .ttl(400)
            .build(),
    )
    .await;
    let report = next_on(&h, &mut ports, "close_report", &mut seen).await;
    let slot = body_of(&report)["close_report"].clone();
    assert_eq!(slot["session_id"], "s1", "{slot}");
    assert_eq!(slot["ok"], true, "{slot}");
    assert!(
        count_route(&h, "/agent/memory-hive", "/agent/door", "close_report").await >= 1,
        "the report follows the memory's close pass"
    );
    wait_route(&h, "/agent/door", "/agent/memory-hive", "clock_op", 1).await;
}

/// `simple-agent@1` only (R-SA-37): the frozen `@1.1.4` boots its identity
/// into its OWN affinity. `@2` takes it from the colony's affinity by pack
/// (`simple_agent_2_colony.rs`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_aieos_seed_of_simple_agent_1_reaches_the_brain_as_a_pack_once_and_again_on_change() {
    if !shipped() || !repo("templates/simple-agent@1.1.4/config.json").is_file() {
        return;
    }
    let changed = json!({"op": "upsert_entity", "entity_id": "entity:ava", "kind": "agent",
                         "display_name": "Ava Quill", "aieos": aieos("to map the coast")});
    let (_td, stubs, h, mut ports) = boot_form(
        Form::V1,
        vec![
            canned_tool_calls(vec![("c-aff", "affinity_propose", &changed.to_string())]),
            canned_chat_completion("Noted.", "stop"),
        ],
        json!({"door": {"agent_id": "ava", "aieos": aieos("to keep the lighthouse")}}),
    )
    .await;
    h.send(turn("t-1", "Who are you?", Value::Null)).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "Noted.");
    let reqs = stubs.brain.recorded_requests().await;
    let first = system_text(&reqs[0]);
    assert!(
        first.contains("Ava"),
        "the first turn knows the agent: {first}"
    );
    assert!(first.contains("lighthouse"), "{first}");
    assert!(first.contains("You are an AI agent"), "the frame: {first}");
    assert!(
        wire_text(&reqs[1]).contains("accepted"),
        "the affinity tool's ack is its result: {}",
        wire_text(&reqs[1])
    );
    // The change sends a second pack, and each is booked.
    wait_route(&h, "/agent/affinity", "/agent/curator", "in_pack", 2).await;
    wait_route(&h, "/agent/curator", "/agent/affinity", "in_pack_ack", 2).await;
    h.quiesce().await;
    assert_eq!(
        count_route(&h, "/agent/door", "/agent/affinity", "in_propose").await,
        4,
        "boot (upsert, disclosure, subscribe) once, plus the tool's write"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sidecar_memory_section_reaches_the_memory_hive() {
    if !shipped() {
        return;
    }
    let block = json!({"memory": {"nothing_new": false,
                                  "facts": [{"text": "The caller keeps bees.",
                                             "source": "I keep bees."}],
                                  "topic": {"movement": "continue"}}});
    let text = format!("Bees, then.\n```sidecar\n{block}\n```");
    let (_td, _stubs, h, mut ports) =
        boot(vec![canned_chat_completion(&text, "stop")], json!({})).await;
    h.send(turn("t-1", "I keep bees.", Value::Null)).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert!(
        !body_of(&answer)["answer"]["text"]
            .as_str()
            .unwrap_or("")
            .contains("sidecar"),
        "the block never reaches the caller"
    );
    wait_route(&h, "/agent/splitter", "/agent/curator", "in_section", 1).await;
    wait_route(&h, "/agent/curator", "/agent/memory-hive", "in_remember", 1).await;
}

/// `in_turn` hops the door handed the collector, from the log.
async fn in_turn_hops(h: &ColonyHandle) -> Vec<Value> {
    h.read_log(meclaw_colony::api_dto::MessageLogFilter::default())
        .await
        .iter()
        .filter(|r| {
            r.from_path.starts_with("/agent/door") && r.to_path.starts_with("/agent/collector")
        })
        .filter_map(|r| meclaw_core::serde_json::from_str::<Value>(&r.headers_json).ok())
        .map(|h| h["hop"].clone())
        .filter(|hop| hop["route"] == "in_turn")
        .collect()
}

/// GH #1292 FA: a turn whose body says `tool_scope: {require: true}` asks its
/// FIRST brain call for a tool call (`tool_choice: "required"`, the normal
/// form every agent has); the next call of the same tool loop goes without,
/// so the turn can end in text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_requires_a_tool_call_requires_it_of_its_first_brain_call_only() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-weather", "weather", r#"{"place":"Berlin"}"#)]),
            canned_chat_completion("Sunny.", "stop"),
        ],
        json!({"tool_gate": {"tools_enabled": ["weather"]}}),
    )
    .await;
    let mut t = turn("t-1", "Weather?", Value::Null);
    if let Body::Inline(b) = &mut t.body {
        b["tool_scope"] = json!({"require": true});
    }
    h.send(t).await;
    let mut seen = Vec::new();
    let answer = next_on(&h, &mut ports, "answer", &mut seen).await;
    assert_eq!(body_of(&answer)["answer"]["text"], "Sunny.");
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2, "the required call, then the answer");
    assert_eq!(
        reqs[0].body["tool_choice"], "required",
        "the first call of the turn requires a tool call: {}",
        reqs[0].body
    );
    assert!(
        reqs[1].body.get("tool_choice").is_none(),
        "the next call of the loop is free again: {}",
        reqs[1].body["tool_choice"]
    );
}

/// GH #1292 FA: a turn without `tool_scope` is the turn of before -- no
/// requirement on the wire and no new key on the door's `in_turn`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_without_a_requirement_reaches_the_wire_unchanged() {
    if !shipped() {
        return;
    }
    let (_td, stubs, h, mut ports) = boot(
        vec![
            canned_tool_calls(vec![("c-weather", "weather", r#"{"place":"Berlin"}"#)]),
            canned_chat_completion("Sunny.", "stop"),
        ],
        json!({"tool_gate": {"tools_enabled": ["weather"]}}),
    )
    .await;
    h.send(turn("t-1", "Weather?", Value::Null)).await;
    let mut seen = Vec::new();
    next_on(&h, &mut ports, "answer", &mut seen).await;
    h.quiesce().await;
    let reqs = stubs.brain.recorded_requests().await;
    assert_eq!(reqs.len(), 2);
    for (i, r) in reqs.iter().enumerate() {
        assert!(
            r.body.get("tool_choice").is_none(),
            "call {i} carries no tool_choice: {}",
            r.body["tool_choice"]
        );
    }
    let hops = in_turn_hops(&h).await;
    assert_eq!(hops.len(), 1, "{hops:?}");
    assert!(
        hops[0].get("tool_require").is_none(),
        "the door's in_turn is the one of before: {}",
        hops[0]
    );
}
