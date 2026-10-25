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
