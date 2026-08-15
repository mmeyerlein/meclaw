//! GH #20 -- instantiation must not materialize secrets into `config.json`.
//!
//! Two placeholder classes, one rule each (see
//! `meclaw_colony::mutation::substitute` for the table):
//!
//! - **environment** (`${VAR}`, `${VAR:-default}`) -- owned by the environment,
//!   never written to disk, resolved in memory at every read (boot AND
//!   instantiation).
//! - **instance** (`${ctx.<key>}`, `${uuid7:<label>}`) -- owned by the instance,
//!   resolved once, at instantiation, and written out resolved.
//!
//! The sweep runs against the staging surface every instantiation path shares
//! (`build_staging_tree_from_templates` → `patch_and_substitute_config`, used by
//! `add_nodes`, the `swap_nodes` with-side, `adopt`, and the subtree/rebirth
//! staging), plus the boot surface (`plan_bootstrap_with_env`) that binds late.
//!
//! `SENTINEL` is the value an unfixed instantiation would leave on disk. Every
//! test that writes a tree ends with `assert_no_sentinel_under`, a literal
//! grep over every file of the instance tree -- the negative proof the issue
//! actually asks for.

use meclaw_colony::mutation::stage::build_staging_tree_from_templates;
use meclaw_colony::mutation::substitute::substitute_mutation_diff;
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry};
use meclaw_core::serde_json::{Value as JsonValue, json};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

/// The secret value that must never reach the filesystem.
const SENTINEL: &str = "sk-do-not-materialize-me";
/// A second secret, carried by `contract.settings.*.default` (the path a sweep
/// that only inspects `params` misses).
const SENTINEL_TOKEN: &str = "bot-token-do-not-materialize-me";

const CONTRACT_TAIL: &str = r#""version":"0.1.0","settings":{},"consumes":{}"#;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// `.env` map carrying both sentinels.
fn env_with_secrets() -> HashMap<String, String> {
    [
        ("SECRET_API_KEY".to_string(), SENTINEL.to_string()),
        ("SECRET_BOT_TOKEN".to_string(), SENTINEL_TOKEN.to_string()),
        ("BASE_URL".to_string(), "https://real.example".to_string()),
    ]
    .into()
}

/// Register a single-cell template directory under `<root>/templates/<name>`.
fn register_template(root: &std::path::Path, name: &str, config: &str) -> TemplatesRegistry {
    let dir = root.join("templates").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("template.json"), format!(r#"{{"name":"{name}"}}"#)).unwrap();
    std::fs::write(dir.join("config.json"), config).unwrap();
    TemplatesRegistry::from_entries(vec![TemplateEntry {
        template_id: format!("t-{name}"),
        name: name.to_string(),
        version: None,
        filesystem_path: dir,
    }])
}

/// A template config that exercises BOTH materialization paths and all four
/// placeholder forms: `params.*` and `contract.settings.*.default`.
fn secret_template_config() -> String {
    format!(
        r#"{{
          "cell": {{"type": "echo"}},
          "params": {{
            "echo_to": "/sink",
            "api_key": "${{SECRET_API_KEY}}",
            "base_url": "${{BASE_URL:-https://fallback.example}}",
            "owner": "${{ctx.user_id}}",
            "session": "${{uuid7:sess}}"
          }},
          "contract": {{
            {CONTRACT_TAIL},
            "settings": {{
              "bot_token": {{"type": "string", "default": "${{SECRET_BOT_TOKEN}}"}},
              "api_key": {{"type": "string", "default": "${{SECRET_API_KEY}}"}}
            }}
          }}
        }}"#
    )
}

/// Read a staged/instantiated `config.json` as JSON.
fn read_config(dir: &std::path::Path) -> JsonValue {
    let raw = std::fs::read_to_string(dir.join("config.json")).unwrap();
    meclaw_core::serde_json::from_str(&raw).unwrap()
}

/// Literal grep over every file under `dir`: no sentinel may appear anywhere.
///
/// This is deliberately dumber than a JSON inspection -- a secret baked into a
/// nested script string, a seed file or a settings default is still a leak, and
/// only a byte-level sweep of the whole instance tree catches all of them.
fn assert_no_sentinel_under(dir: &std::path::Path) {
    let mut stack = vec![dir.to_path_buf()];
    let mut checked = 0usize;
    while let Some(p) = stack.pop() {
        for entry in std::fs::read_dir(&p).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            for secret in [SENTINEL, SENTINEL_TOKEN] {
                assert!(
                    !text.contains(secret),
                    "secret materialized into {}: {text}",
                    path.display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "sweep found no files under {}", dir.display());
}

// ── instantiation: the disk view ────────────────────────────────────────────

/// The core pin: after instantiation the instance config carries the ENV forms
/// literally and the INSTANCE forms resolved -- on both materialization paths.
#[test]
fn instantiation_keeps_env_forms_literal_on_params_and_settings_defaults() {
    let td = TempDir::new().unwrap();
    let registry = register_template(td.path(), "leaky", &secret_template_config());
    let ctx: HashMap<String, String> = [("user_id".to_string(), "u-7".to_string())].into();
    let diff = json!({"add_nodes": [{"name": "n1", "template": "leaky"}]});

    let (staged, subtrees) = build_staging_tree_from_templates(
        td.path(),
        "mid-1",
        "/",
        &diff,
        &registry,
        &env_with_secrets(),
        &ctx,
    )
    .unwrap();
    assert_eq!(staged.len(), 1);
    assert!(subtrees.is_empty());

    let cfg = read_config(&staged[0].staging_path);

    // Path 1 -- `params.*`.
    assert_eq!(
        cfg["params"]["api_key"], "${SECRET_API_KEY}",
        "a plain env token stays a token on disk"
    );
    assert_eq!(
        cfg["params"]["base_url"], "${BASE_URL:-https://fallback.example}",
        "the POSIX-default form stays intact, fallback included"
    );

    // Path 2 -- `contract.settings.*.default` (the second, easily missed path).
    assert_eq!(
        cfg["contract"]["settings"]["bot_token"]["default"], "${SECRET_BOT_TOKEN}",
        "a settings default is instance config too, and holds the same rule"
    );
    assert_eq!(
        cfg["contract"]["settings"]["api_key"]["default"], "${SECRET_API_KEY}",
        "the same variable on the second path behaves identically"
    );

    // Instance class -- resolved, exactly as before this fix.
    assert_eq!(
        cfg["params"]["owner"], "u-7",
        "the ctx form belongs to the instance and resolves at instantiation"
    );
    let session = cfg["params"]["session"].as_str().unwrap();
    assert_eq!(
        session.len(),
        36,
        "${{uuid7:*}} resolves to a UUID: {session}"
    );
    assert!(
        cfg["cell"]["id"].as_str().is_some(),
        "cell.id is still minted at instantiation"
    );

    assert_no_sentinel_under(&staged[0].staging_path);
}

/// The runtime view a mutation hands the factory is boot-equivalent: the env
/// class IS resolved in memory, so a freshly instantiated cell behaves exactly
/// like the same cell after a reboot -- while the file holds none of it.
#[test]
fn instantiation_runtime_view_resolves_what_the_disk_view_withholds() {
    let td = TempDir::new().unwrap();
    let registry = register_template(td.path(), "leaky", &secret_template_config());
    let ctx: HashMap<String, String> = [("user_id".to_string(), "u-7".to_string())].into();
    let diff = json!({"add_nodes": [{"name": "n1", "template": "leaky"}]});

    let (staged, _) = build_staging_tree_from_templates(
        td.path(),
        "mid-2",
        "/",
        &diff,
        &registry,
        &env_with_secrets(),
        &ctx,
    )
    .unwrap();

    assert_eq!(
        staged[0].params["api_key"], SENTINEL,
        "the spawned cell receives the resolved secret"
    );
    assert_eq!(
        staged[0].params["base_url"], "https://real.example",
        "the POSIX default yields the real value when the variable is set"
    );
    assert_no_sentinel_under(&staged[0].staging_path);
}

/// A `${VAR}` with neither value nor default still rejects the mutation at
/// staging -- pre-destructively, before the atomic rename. Late binding must not
/// turn an authoring error into a silent empty string.
#[test]
fn unresolvable_env_var_still_rejects_the_instantiation() {
    let td = TempDir::new().unwrap();
    let registry = register_template(
        td.path(),
        "needy",
        &format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink","key":"${{UNSET_XYZ}}"}},"contract":{{{CONTRACT_TAIL}}}}}"#
        ),
    );
    let diff = json!({"add_nodes": [{"name": "n1", "template": "needy"}]});
    let err = build_staging_tree_from_templates(
        td.path(),
        "mid-3",
        "/",
        &diff,
        &registry,
        &HashMap::new(),
        &HashMap::new(),
    )
    .unwrap_err();
    assert_eq!(err.error_code(), "env_var_missing");
    assert!(
        format!("{err:?}").contains("UNSET_XYZ"),
        "the error names the variable: {err:?}"
    );
}

/// `override_params` handed in by the mutation itself is the path the first
/// live incident took (an `add_nodes` rollout). The diff-level class split keeps
/// it a token on disk while the cell still spawns with the resolved value.
#[test]
fn override_params_from_the_diff_stay_tokens_on_disk() {
    let td = TempDir::new().unwrap();
    let registry = register_template(
        td.path(),
        "plain",
        &format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink"}},"contract":{{{CONTRACT_TAIL}}}}}"#
        ),
    );
    let env = env_with_secrets();
    let raw_diff = json!({
        "add_nodes": [{
            "name": "n1",
            "template": "plain",
            "override_params": {"api_key": "${SECRET_API_KEY}"}
        }]
    });
    // Exactly what `handle_mutation` does before staging.
    let diff = substitute_mutation_diff(&raw_diff, &env, &HashMap::new()).unwrap();

    let (staged, _) = build_staging_tree_from_templates(
        td.path(),
        "mid-4",
        "/",
        &diff,
        &registry,
        &env,
        &HashMap::new(),
    )
    .unwrap();

    let cfg = read_config(&staged[0].staging_path);
    assert_eq!(
        cfg["params"]["api_key"], "${SECRET_API_KEY}",
        "an override carries the token to disk, not the value"
    );
    assert_eq!(
        staged[0].params["api_key"], SENTINEL,
        "the cell still spawns with the resolved value"
    );
    assert_no_sentinel_under(&staged[0].staging_path);
}

/// The second live incident: a whole-tree rebirth. Every node of a subtree
/// template goes through the same staging helper, so the rule holds per node --
/// one mutation no longer leaks one secret, and a rebirth no longer leaks all.
#[test]
fn subtree_rebirth_keeps_every_node_free_of_secrets() {
    let td = TempDir::new().unwrap();
    let tpl = td.path().join("templates").join("tree");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), r#"{"name":"tree"}"#).unwrap();
    // Root hive + two leaf cells, each carrying a secret on a different path.
    std::fs::write(
        tpl.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    write(&tpl, "worker/config.json", &secret_template_config());
    write(
        &tpl,
        "notifier/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink"}},"contract":{{{CONTRACT_TAIL},"settings":{{"bot_token":{{"type":"string","default":"${{SECRET_BOT_TOKEN}}"}}}}}}}}"#
        ),
    );

    let registry = TemplatesRegistry::from_entries(vec![TemplateEntry {
        template_id: "t-tree".into(),
        name: "tree".into(),
        version: None,
        filesystem_path: tpl,
    }]);
    let ctx: HashMap<String, String> = [("user_id".to_string(), "u-7".to_string())].into();
    let diff = json!({"add_nodes": [{"name": "colony", "template": "tree"}]});

    let (staged, subtrees) = build_staging_tree_from_templates(
        td.path(),
        "mid-5",
        "/",
        &diff,
        &registry,
        &env_with_secrets(),
        &ctx,
    )
    .unwrap();
    assert!(
        staged.is_empty(),
        "a multi-cell template stages as a subtree"
    );
    assert_eq!(subtrees.len(), 1);

    assert_eq!(subtrees[0].rename_roots.len(), 1, "whole-fresh subtree");
    let root = &subtrees[0].rename_roots[0].root_staging_path;
    let worker = read_config(&root.join("worker"));
    assert_eq!(worker["params"]["api_key"], "${SECRET_API_KEY}");
    assert_eq!(
        worker["contract"]["settings"]["bot_token"]["default"],
        "${SECRET_BOT_TOKEN}"
    );
    let notifier = read_config(&root.join("notifier"));
    assert_eq!(
        notifier["contract"]["settings"]["bot_token"]["default"],
        "${SECRET_BOT_TOKEN}"
    );

    // The whole reborn tree, byte for byte.
    assert_no_sentinel_under(root);
}

// ── boot: the late-binding view ─────────────────────────────────────────────

/// The other half of the contract: what instantiation withheld, boot supplies.
/// The instance config is the one written by the staging surface above, so this
/// is the real round-trip -- token on disk, value in the cell.
#[test]
fn boot_resolves_the_token_the_instance_carries() {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/a/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink","api_key":"${{SECRET_API_KEY}}"}},"contract":{{{CONTRACT_TAIL}}}}}"#
        ),
    );
    std::fs::write(
        td.path().join(".env"),
        format!("SECRET_API_KEY={SENTINEL}\n"),
    )
    .unwrap();

    let plan = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect("boot must plan");
    let cell = plan
        .cells
        .iter()
        .find(|c| c.path.as_str() == "/a")
        .expect("/a planned");
    assert_eq!(
        cell.params["api_key"], SENTINEL,
        "the cell receives the secret from .env at boot"
    );
    // …and the tree still holds none of it. `.env` is the one file that does,
    // by definition, so the sweep runs over the cell tree.
    assert_no_sentinel_under(&td.path().join("main"));
}

/// A token that resolves to nothing fails the boot LOUDLY and names the
/// variable. The failure mode this must never have is a silent empty string:
/// an empty API key would look like a working colony and fail at the first
/// request instead of at boot.
#[test]
fn unresolvable_token_fails_the_boot_and_names_the_variable() {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/a/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink","api_key":"${{SECRET_API_KEY}}"}},"contract":{{{CONTRACT_TAIL}}}}}"#
        ),
    );
    // No `.env` at all -- the variable has no value and no default.
    let errs = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect_err("boot must fail");
    let rendered = format!("{errs:?}");
    assert!(
        rendered.contains("env_var_missing"),
        "the boot error carries the spec token: {rendered}"
    );
    assert!(
        rendered.contains("SECRET_API_KEY"),
        "the boot error names the variable: {rendered}"
    );
}

/// `${VAR:-}` is the ONE way to get an empty value, and it is explicit: the
/// author wrote the empty default. Nothing else may produce one silently.
#[test]
fn only_an_explicit_empty_default_yields_an_empty_value() {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        td.path(),
        "main/a/config.json",
        &format!(
            r#"{{"cell":{{"type":"echo"}},"params":{{"echo_to":"/sink","api_key":"${{OPTIONAL_KEY:-}}"}},"contract":{{{CONTRACT_TAIL}}}}}"#
        ),
    );
    let plan = meclaw_colony::plan_bootstrap(td.path(), &echo_factories(), &Default::default())
        .expect("an explicit empty default is legal");
    let cell = plan.cells.iter().find(|c| c.path.as_str() == "/a").unwrap();
    assert_eq!(cell.params["api_key"], "", "the author asked for empty");
}

fn echo_factories() -> meclaw_colony::CellFactoryRegistry {
    let mut m = meclaw_colony::CellFactoryRegistry::new();
    m.insert(
        "echo".to_string(),
        Arc::new(meclaw_testing::factories::EchoCellFactory) as Arc<dyn meclaw_colony::CellFactory>,
    );
    m
}
