//! GH #1310 (plan A5) — the builder's `simple_agent` block, rendered into the
//! example colony, passes the real `meclaw --validate --validate-strict`.
//!
//! `examples/simple-agent-colony/grow.json` is the recipe's rendering of
//! `wish.json` (the drift lock is `sa2_builder_block_renders_one_agent_the_door_accepts`
//! in `meclaw-cells`). This lock lays that one declaration into the example's
//! seed the way the mutation door stages it, and asks the binary:
//!
//! * the level is the example's `seed/main` -- the colony's `access`,
//!   `affinity` and `llm-registry`, grown from the `ref` markers the example
//!   ships as its first boot grows them (they need no ctx);
//! * the agent is instantiated at `./agent` with the declaration's `ctx`
//!   (every `${ctx.<key>}` of the template substituted, nested `ref` markers
//!   resolved to the template they name), and the declaration's
//!   `override_params` laid onto the cells they address. The agent is not a
//!   bare `ref` marker in the seed: a boot grows a marker with an empty ctx, and
//!   `simple-agent@2` requires `ctx.model`;
//! * the level's graph is the declaration's `add_edges`, verbatim.
//!
//! `seed_rows` (the grants into `./access/store`) are data of a store the
//! validator does not open; the boot of the same rendering in
//! `sa2_example_colony_answers_a_turn_with_the_stub` reads them.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! shipped example and library is skipped.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Map, Value};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write_json(p: &Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, serde_json::to_string_pretty(v).expect("json")).expect("write");
}

/// Every `${ctx.<key>}` of `raw` replaced by the declaration's `ctx`.
fn with_ctx(raw: &str, ctx: &Map<String, Value>) -> String {
    let mut out = raw.to_string();
    for (k, v) in ctx {
        if let Some(s) = v.as_str() {
            out = out.replace(&format!("${{ctx.{k}}}"), s);
        }
    }
    out
}

/// The template a `ref` marker names: its pinned copy, else its live line.
fn template_dir(reference: &str) -> PathBuf {
    let pinned = repo("templates").join(reference);
    if pinned.join("config.json").is_file() {
        return pinned;
    }
    let name = reference.split('@').next().unwrap_or_default();
    repo("templates").join(name)
}

/// Lay `params` over the `params` of the cell at `cell`.
fn override_on_disk(cell: &Path, params: &Value) {
    let f = cell.join("config.json");
    let mut cfg = read_json(&f);
    for (k, v) in params.as_object().cloned().unwrap_or_default() {
        cfg["params"][k] = v;
    }
    write_json(&f, &cfg);
}

/// `src` instantiated at `dst` with `ctx`: every file copied, every `ref`
/// marker below resolved to its template (with its own `override_params`),
/// as the door's staging lays a subtree down.
fn instantiate(src: &Path, dst: &Path, ctx: &Map<String, Value>, depth: usize) {
    assert!(depth < 8, "a ref chain without end at {}", src.display());
    let marker = src.join("config.json");
    if marker.is_file() {
        let cfg = read_json(&marker);
        if cfg["cell"]["type"] == "ref" {
            let reference = cfg["cell"]["template"]
                .as_str()
                .expect("a ref names a template");
            let target = template_dir(reference);
            assert!(
                target.join("config.json").is_file(),
                "{}: `{reference}` resolves to no template",
                marker.display()
            );
            instantiate(&target, dst, ctx, depth + 1);
            for (cell, params) in cfg["override_params"]
                .as_object()
                .cloned()
                .unwrap_or_default()
            {
                override_on_disk(&dst.join(cell), &params);
            }
            return;
        }
    }
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            instantiate(&from, &to, ctx, depth);
        } else if from.extension().is_some_and(|e| e == "json") {
            let raw = std::fs::read_to_string(&from).expect("utf-8 json");
            std::fs::write(&to, with_ctx(&raw, ctx)).expect("write");
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

/// Copy `src` to `dst` verbatim (the example's seed).
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("readable") {
        let entry = entry.expect("entry");
        let to = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("copy");
        }
    }
}

/// Every bare `${NAME}` token under `dir` (a token with a default keeps it).
fn env_tokens(dir: &Path, out: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).expect("readable").flatten() {
        let p = entry.path();
        if p.is_dir() {
            env_tokens(&p, out);
            continue;
        }
        let raw = std::fs::read_to_string(&p).unwrap_or_default();
        let mut rest = raw.as_str();
        while let Some(start) = rest.find("${") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find('}') else { break };
            let name = &rest[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                out.insert(name.to_string());
            }
            rest = &rest[end + 1..];
        }
    }
}

/// The example colony with the block rendered into it; the root directory.
fn example_with_the_block(td: &Path) {
    let grow = read_json(&repo("examples/simple-agent-colony/grow.json"));
    let manifest = grow["manifest"].as_array().cloned().unwrap_or_default();
    assert_eq!(manifest.len(), 1, "the block is one declaration: {grow}");
    let decl = &manifest[0];
    assert_eq!(decl["scope"], "/", "the example grows at its level");
    let ctx = decl["ctx"].as_object().cloned().unwrap_or_default();
    let main = td.join("main");
    copy_tree(&repo("examples/simple-agent-colony/seed/main"), &main);
    // The level's three neighbours, grown from their markers as the first
    // boot grows them (`--validate` only plans a growth, and the block's
    // edges end at the cells the growth lays down).
    for neighbour in ["access", "affinity", "llm-registry"] {
        let dir = main.join(neighbour);
        let marker = read_json(&dir.join("config.json"));
        let reference = marker["cell"]["template"].as_str().expect("a ref marker");
        instantiate(&template_dir(reference), &dir, &Map::new(), 0);
    }
    let node = &decl["diff"]["add_nodes"][0];
    assert_eq!(node["name"], "agent");
    let reference = node["template"].as_str().expect("a template");
    let agent = main.join("agent");
    instantiate(&template_dir(reference), &agent, &ctx, 0);
    for (cell, params) in node["override_params"]
        .as_object()
        .cloned()
        .unwrap_or_default()
    {
        assert!(
            agent.join(&cell).join("config.json").is_file(),
            "the block overrides no cell of the agent: {cell}"
        );
        override_on_disk(&agent.join(&cell), &params);
    }
    let mut level = read_json(&main.join("config.json"));
    level["params"]["graph"]["edges"] = decl["diff"]["add_edges"].clone();
    write_json(&main.join("config.json"), &level);
    let mut names = BTreeSet::new();
    env_tokens(&main, &mut names);
    let body: String = names.iter().map(|k| format!("{k}=dummy-{k}\n")).collect();
    std::fs::write(td.join(".env"), body).expect("the env file");
}

#[test]
fn the_simple_agent_block_validates_strict_in_the_example_colony() {
    if !repo("examples/simple-agent-colony/grow.json").is_file()
        || !repo("templates/simple-agent/config.json").is_file()
        || !repo("templates/llm-registry/config.json").is_file()
    {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    example_with_the_block(td.path());
    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .arg("--templates")
        .arg(repo("templates"))
        .arg("--validate")
        .arg("--validate-strict")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run meclaw --validate --validate-strict");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the block in the example colony fails --validate-strict:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// The example's README command, verbatim: `meclaw --root <colony>
/// --templates templates --apply examples/simple-agent-colony/grow.json` on a
/// colony grown from the example's seed. The block goes through the real
/// mutation door (staging, birth checks, `seed_rows`), not through a copy of
/// its staging, and the door commits it.
#[test]
fn the_example_readme_apply_commits_the_block() {
    if !repo("examples/simple-agent-colony/grow.json").is_file()
        || !repo("templates/simple-agent/config.json").is_file()
        || !repo("templates/llm-registry/config.json").is_file()
    {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    copy_tree(
        &repo("examples/simple-agent-colony/seed/main"),
        &td.path().join("main"),
    );
    let mut names = BTreeSet::new();
    // The agent's ref markers reach further into the library than its own
    // directory: every bare token of the library gets a dummy.
    env_tokens(&repo("templates"), &mut names);
    let body: String = names.iter().map(|k| format!("{k}=dummy-{k}\n")).collect();
    std::fs::write(td.path().join(".env"), body).expect("the env file");
    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td.path())
        .arg("--templates")
        .arg(repo("templates"))
        .arg("--apply")
        .arg(repo("examples/simple-agent-colony/grow.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run meclaw --apply");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the README's --apply does not commit the block:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        td.path().join("main/agent/enrol/config.json").is_file(),
        "the door laid the agent down: {stdout}"
    );
}

/// A hive directory with `edges` as its graph.
fn hive(dir: &Path, edges: Value) {
    write_json(
        &dir.join("config.json"),
        &serde_json::json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
}

/// A `ref` marker of the example's seed (`access`, `affinity`, `llm-registry`)
/// at `dir`.
fn marker(dir: &Path, name: &str) {
    let m = read_json(
        &repo("examples/simple-agent-colony/seed/main")
            .join(name)
            .join("config.json"),
    );
    write_json(&dir.join("config.json"), &m);
}

/// A member colony under the shell, laid down as files: the registry under
/// `/os`, its pushes arriving at the container `/os/orgs` (the shell's road
/// of `meclaw-os`, verbatim, with its operator door onto the registry --
/// `in_hand` requires `context.actor`; the header-contract check finds that
/// setter upstream of `/os/orgs`, over the registry's push), and the member
/// `/os/orgs/o/members/m` with its `access`, `affinity` and an empty `apps`.
/// `/os` is connected the way the shell's is: its only external edges lead to
/// `/colony/*` endpoints (a lab shell, 25.10.: `/os/builder/eyes ->
/// /colony/graph` and its siblings). Without one the unit `/os` has no external
/// edge, the boot derives its whole subtree inactive, and the member's
/// `access/store` can never answer a seed order (#1315, OR-agent-2.N2.6).
/// Returns the member's directory.
fn member_colony(td: &Path) -> PathBuf {
    let main = td.join("main");
    hive(&main, serde_json::json!([]));
    let os = main.join("os");
    hive(
        &os,
        serde_json::json!([
            {"from": ".", "to": "/colony/graph",
             "condition": "has(hop.route) && hop.route == 'graph'"},
            {"from": ".", "to": "./llm-registry",
             "condition": "has(hop.route) && hop.route == 'in_hand'",
             "modifier": {"set_context": {"actor": "'operator'"}}},
            {"from": "./llm-registry", "to": "./orgs",
             "condition": "has(hop.route) && hop.route == 'update' && has(hop.subscriber) && hop.subscriber.startsWith('/os/orgs/')",
             "modifier": {"set_hop": {"route": "'in_model'"}}},
            {"from": "./orgs", "to": "./llm-registry",
             "condition": "has(hop.route) && hop.route == 'model_refused' && has(hop.refused_subscriber) && hop.refused_subscriber.startsWith('/os/orgs/')",
             "modifier": {"set_hop": {"route": "'in_refused'"}, "delete_context": ["actor"]}},
            {"from": "./orgs", "to": "./llm-registry",
             "condition": "has(hop.route) && hop.route == 'model_subscribe'",
             "modifier": {"set_hop": {"route": "'in_hand'"},
                          "set_context": {"model_announcer": "'meclaw-os'"},
                          "delete_context": ["actor"]}}
        ]),
    );
    marker(&os.join("llm-registry"), "llm-registry");
    let member = os.join("orgs/o/members/m");
    // Each level serves the next one down, and the member's `access` answers
    // up to the member -- the shell's own roads (lab slot-3, 25.10.), without
    // which every hive below `/os/orgs` has no external edge and stays inactive.
    for (dir, child, key, name) in [
        (os.join("orgs"), "./o", "org", "o"),
        (os.join("orgs/o"), "./members", "", ""),
        (os.join("orgs/o/members"), "./m", "member", "m"),
    ] {
        let condition = if key.is_empty() {
            "has(hop.route) && hop.route == 'in_turn'".to_string()
        } else {
            format!(
                "has(hop.route) && hop.route == 'in_turn' && (!has(context.{key}) || context.{key} == '{name}')"
            )
        };
        hive(
            &dir,
            serde_json::json!([{"from": ".", "to": child, "condition": condition}]),
        );
    }
    hive(
        &member,
        serde_json::json!([{"from": "./access", "to": ".",
                            "condition": "has(hop.route) && hop.route == 'error'"}]),
    );
    hive(&member.join("apps"), serde_json::json!([]));
    marker(&member.join("access"), "access");
    marker(&member.join("affinity"), "affinity");
    let mut names = BTreeSet::new();
    // The agent's ref markers reach further into the library than its own
    // directory: every bare token of the library gets a dummy.
    env_tokens(&repo("templates"), &mut names);
    let body: String = names.iter().map(|k| format!("{k}=dummy-{k}\n")).collect();
    std::fs::write(td.join(".env"), body).expect("the env file");
    member
}

/// The shipped `recipes` script's manifest for `recipe` on `params`, with the
/// builder's registry container `/os/orgs` (as `meclaw-os` sets it).
fn render(recipe: &str, params: &Value) -> Value {
    let answer = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("a utf-8 path"),
        ),
        &serde_json::json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "params": {"model_registry_scope": "/os/orgs"},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": serde_json::json!({"recipe": recipe,
                                                     "request": "grow it",
                                                     "params": params}).to_string()}],
        }),
    )
    .into_iter()
    .next()
    .expect("the recipe answered");
    assert!(
        answer["header"]["error_code"].is_null(),
        "refused: {answer}"
    );
    answer["manifest"].clone()
}

/// `meclaw --apply` of `manifest` on the colony under `td`: (success, output).
fn apply(td: &Path, manifest: &Value) -> (bool, String) {
    let grow = td.join("grow.json");
    write_json(&grow, &serde_json::json!({"manifest": manifest}));
    let out = Command::new(env!("CARGO_BIN_EXE_meclaw"))
        .arg("--root")
        .arg(td)
        .arg("--templates")
        .arg(repo("templates"))
        .arg("--apply")
        .arg(&grow)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run meclaw --apply");
    let text = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// GH #1312 (K-1, review C1) -- the block in a MEMBER colony goes through the
/// real mutation door. The registry stands under `/os`, its pushes arrive at
/// the container `/os/orgs` (the shell's three edges of `meclaw-os`), and the
/// builder names that container as `model_registry_scope`, as for the
/// assistant. The door judges each manifest entry on its own: the entry that
/// gives the agent birth must wire every required lane (`in_model`
/// included), and the container's road follows as the second entry. Before
/// the fix the block drew `./llm-registry` beside the agent, where none stands.
#[test]
fn the_simple_agent_block_commits_in_a_member_colony() {
    if !repo("templates/simple-agent/config.json").is_file()
        || !repo("templates/builder/recipes/config.json").is_file()
        || !repo("templates/llm-registry/config.json").is_file()
    {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let member = member_colony(td.path());
    let mut wish = read_json(&repo("examples/simple-agent-colony/wish.json"))["params"].clone();
    wish["scope"] = serde_json::json!("/os/orgs/o/members/m");
    let manifest = render("simple_agent", &wish);
    assert_eq!(manifest.as_array().map(Vec::len), Some(2), "{manifest}");
    let (ok, out) = apply(td.path(), &manifest);
    assert!(ok, "the door refuses the block in a member colony:\n{out}");
    assert!(
        member.join("agent/enrol/config.json").is_file(),
        "the door laid the agent down at the member: {out}"
    );
    the_store_answered_every_seed_order(&member);
}

/// #1315 (OR-agent-2.N2.6): the seed order reached the member's running
/// `access/store` and the store wrote it on its own connection. The rows in
/// its `grants` table are the store's receipt: the deadline path
/// (`seed_receipt_timeout`) and an inactive store (`seed_target_inactive`)
/// both leave the table empty, because the door writes into `cell.db` only for
/// a store the same mutation grows. Read after `--apply` has exited.
fn the_store_answered_every_seed_order(member: &Path) {
    let path = member.join("access/store/cell.db");
    let db =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|e| panic!("the store's own database {}: {e}", path.display()));
    let rows: i64 = db
        .query_row("SELECT count(*) FROM grants", [], |r| r.get(0))
        .expect("the store's grants");
    assert!(
        rows > 0,
        "the block seeds ./access/store, and the store wrote no grant: the door answered without it"
    );
}

/// GH #1312 (K-4, OR-AG-76/-77) -- `install_app` of an app that embeds
/// `simple-agent@2` (orga's coordinator; here `figure@2`, whose rim carries
/// the agent's lanes one level down) goes through the real mutation door in a
/// member colony. The app's rim declares `in_sealed`, `in_pack`,
/// `in_briefing` and `in_model` as required, and the door wants each of them
/// wired in the entry that instantiates the app. Before the fix `install_app`
/// drew only the credential pair (`in_sealed`) and the door refused the birth
/// (`hive_contract`: "declares the lane ... as required ... but the mutation
/// that instantiates it draws no edge", orga-hive O1 report line 541). The
/// rim's other lanes come from the block's own function (`_sa_rim`).
#[test]
fn install_app_wires_the_rim_of_an_embedded_agent_in_a_member_colony() {
    if !repo("templates/figure/config.json").is_file()
        || !repo("templates/builder/recipes/config.json").is_file()
        || !repo("templates/llm-registry/config.json").is_file()
    {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let member = member_colony(td.path());
    let example = read_json(&repo("examples/simple-agent-colony/wish.json"))["params"].clone();
    let figure = read_json(&repo("templates/figure/config.json"));
    let version = read_json(&repo("templates/figure/template.json"))["version"].clone();
    let wish = serde_json::json!({
        "scope": "/os/orgs/o/members/m",
        "app": "fig",
        "template": format!("figure@{}", version.as_str().unwrap_or_default()),
        "screen": "display",
        "ctx": {"model": "example-provider/brain-model"},
        "declaration": {},
        "override_params": {"world": {"figure_id": "fig", "world_id": "w", "role": "npc"}},
        "credential": example["credential"],
        "contract": figure["params"]["contract"],
    });
    let manifest = render("install_app", &wish);
    let (ok, out) = apply(td.path(), &manifest);
    assert!(
        ok,
        "the door refuses an app that embeds simple-agent@2:\n{out}\nmanifest: {manifest:#}"
    );
    assert!(
        member
            .join("apps/fig/simple-agent/enrol/config.json")
            .is_file(),
        "the door laid the app's agent down: {out}"
    );
    the_store_answered_every_seed_order(&member);
}
