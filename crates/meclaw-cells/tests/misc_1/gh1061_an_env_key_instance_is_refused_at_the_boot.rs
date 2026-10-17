//! GH #1061 (#801), lock T5 (A8), ended by GH #1235 (T6-22) — an instance
//! that carries its key as an environment token is refused at the boot.
//!
//! 0.62 shipped no `${…_KEY}` in any template, and A8 let the instances grown
//! before it -- an `llm` cell with `api_key: "${OPENROUTER_API_KEY}"`, no
//! grant, the value in `{root}/.env` -- run for one release. That window is
//! closed: a secret param takes no `${VAR}`, the key comes sealed under
//! `credential_grant_id`. The boot names the key and echoes no value.

use meclaw_cells::LlmCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::deadline::hang;
use std::sync::Arc;

const SECRET: &str = "stub-secret-5";

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, v.to_string()).unwrap();
}

/// The instance as 0.61 grew it: the key as a late-bound token, no grant.
fn instance(root: &std::path::Path) {
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    write(
        root,
        "main/brain/config.json",
        &json!({
            "cell": {"type": "llm"},
            "params": {
                "provider": "openai", "model": "gpt-4o-mini",
                "api_key": "${OPENROUTER_API_KEY}",
                "base_url": "http://127.0.0.1:9/v1", "external_timeout_ms": 10_000,
                "x_title": "meclaw-lab gh1061", "http_referer": "https://meclaw.ai"
            },
            "contract": {
                "version": "1.0.0", "settings": {},
                "emits": {"body": {"messages": {"type": "array", "required": false}}},
                "consumes": {"body": {"messages": {"type": "array", "required": false}}},
                "capabilities": ["network:llm", "db:own"]
            }
        }),
    );
    std::fs::write(root.join(".env"), format!("OPENROUTER_API_KEY={SECRET}\n")).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1061_an_env_key_instance_is_refused_at_the_boot() {
    let td = tempfile::TempDir::new().unwrap();
    instance(td.path());
    let fs: Vec<(String, Arc<dyn CellFactory>)> = vec![(
        "llm".to_string(),
        Arc::new(LlmCellFactory) as Arc<dyn CellFactory>,
    )];
    let h = ColonyHandle::new_with_factories_at(&td, fs.clone());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in fs {
        registry.insert(name, f);
    }
    let refused = match hang(
        "the boot",
        bootstrap_from_filesystem(td.path(), &registry, &h.runtime()),
    )
    .await
    {
        Ok(_) => panic!("an env-token key must not boot"),
        Err(e) => format!("{e:?}"),
    };
    assert!(
        refused.contains("api_key"),
        "the refusal names the key: {refused}"
    );
    assert!(
        !refused.contains(SECRET),
        "the refusal echoes no value: {refused}"
    );
    hang("the colony shuts down", h.shutdown()).await;
}
