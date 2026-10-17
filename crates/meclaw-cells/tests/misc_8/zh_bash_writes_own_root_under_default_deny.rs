//! GH #1235 (T6-16, OR-programm-tm.H5.orch-1) — under the default-deny block a
//! `bash` cell may write the root it owns: its relative `params.cwd`, resolved
//! into the enclosing hive at the spawn.
//!
//! The coder pipeline's `runner` declares `"cwd": "workspace"` and no
//! `sandbox`, so an instantiation writes the default block next to it
//! (`mutation/stage.rs`), and with T6-16 a hand-written tree boots under the
//! same block. That block keeps only the runtime set reachable, so the shell
//! started in its workspace and could write nothing there (H4 report, open
//! points). The cell's own root is its own right; everything else stays denied.

use meclaw_cells::bash::BashCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json, to_string_pretty};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::deadline::hang;
use std::sync::Arc;

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "bash".to_string(),
        Arc::new(BashCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factories() {
        r.insert(name, f);
    }
    r
}

fn write_json(path: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, to_string_pretty(v).unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_template_shell_writes_its_own_workspace_under_the_default_deny_block() {
    if meclaw_cells::sandbox::landlock_abi().is_none()
        || !meclaw_cells::sandbox::network_isolation_supported()
    {
        eprintln!("SKIPPED: no Landlock or no unprivileged network namespace on this host");
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    // The runner's params as `templates/coder-pipeline/runner` declares them,
    // plus the block an instantiation writes when a template declares none.
    write_json(
        &root.join("main/runner/config.json"),
        &json!({
            "cell": {"type": "bash"},
            "params": {
                "max_concurrency": 1,
                "cwd": "workspace",
                "sandbox": {"trust": "restricted", "network": "deny",
                            "filesystem": {"runtime": true}}
            },
            "contract": {"version": "1.0.0", "settings": {}, "emits": {}, "consumes": {}}
        }),
    );
    write_json(
        &root.join("main/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );

    let h = ColonyHandle::new_with_factories_at(&td, factories());
    hang(
        "the boot",
        bootstrap_from_filesystem(root, &registry(), &h.runtime()),
    )
    .await
    .expect("a sandboxed shell with a relative root must boot");
    h.send(
        MessageBuilder::new(Path::new("/runner"))
            .reply_to(Path::new("/nobody"))
            .body(Body::Inline(json!({"messages": [{
                "origin": "assistant", "type": "tool_call",
                "text": json!({"command": "echo built > out.txt"}).to_string(),
                "id": "call-1"
            }]})))
            .build(),
    )
    .await;
    hang("the colony quiesces", h.quiesce()).await;

    assert_eq!(
        std::fs::read_to_string(root.join("main/workspace/out.txt"))
            .expect("the shell must be able to write its own workspace under default-deny"),
        "built\n"
    );
    hang("the colony shuts down", h.shutdown()).await;
}
