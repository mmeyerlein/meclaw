//! GH #1234 (T6-15) — `bash` takes a relative `params.cwd` as a directory of
//! the hive that encloses it, created at the spawn, and runs every command
//! there.
//!
//! The coder pipeline's shell knew no root of its own: it ran in the colony's
//! process directory, so `./dispatch` put `cd /tmp/meclaw-coder &&` in front of
//! every command, and every pipeline on the machine worked in that one
//! directory. With a hive-relative `cwd` the shell starts where its sibling
//! `file` cell writes, the same anchor `file` and `edit` use.

use meclaw_cells::bash::BashCellFactory;
use meclaw_cells::file::FileCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json, to_string_pretty};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "file".to_string(),
            Arc::new(FileCellFactory) as Arc<dyn CellFactory>,
        ),
        ("bash".to_string(), Arc::new(BashCellFactory)),
    ]
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

fn cell(kind: &str, params: Value) -> Value {
    json!({
        "cell": {"type": kind},
        "params": params,
        "contract": {"version": "1.0.0", "settings": {}, "emits": {}, "consumes": {}}
    })
}

fn call(target: &str, args: Value) -> meclaw_core::Message {
    MessageBuilder::new(Path::new(target))
        .reply_to(Path::new("/nobody"))
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant", "type": "tool_call",
            "text": args.to_string(), "id": "call-1"
        }]})))
        .build()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relative_shell_root_runs_the_command_in_the_enclosing_hive() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    write_json(
        &root.join("main/files/config.json"),
        &cell(
            "file",
            json!({"base_path": "workspace", "max_concurrency": 1}),
        ),
    );
    write_json(
        &root.join("main/runner/config.json"),
        &cell("bash", json!({"cwd": "workspace", "max_concurrency": 1})),
    );
    write_json(
        &root.join("main/config.json"),
        &json!({"cell": {"type": "hive"},
                "params": {"graph": {"edges": [{"from": "./files", "to": "./runner"}]}}}),
    );

    let h = ColonyHandle::new_with_factories_at(&td, factories());
    bootstrap_from_filesystem(root, &registry(), &h.runtime())
        .await
        .expect("a relative shell root must boot");
    let workspace = root.join("main/workspace");

    h.send(call(
        "/files",
        json!({"op": "write", "path": "note.txt", "content": "foo"}),
    ))
    .await;
    h.quiesce().await;
    // Writes only where `note.txt` is: a shell in the colony's process
    // directory finds none and leaves that directory untouched.
    h.send(call(
        "/runner",
        json!({"command": "test -f note.txt && cp note.txt copy.txt"}),
    ))
    .await;
    h.quiesce().await;

    assert_eq!(
        std::fs::read_to_string(workspace.join("copy.txt"))
            .expect("the shell ran in the hive's workspace, not in the colony's directory"),
        "foo",
        "the shell must see what the sibling file cell wrote: one root for both"
    );
    // Restliste H4: the spawn marks the root, or the next boot walks into it.
    assert!(
        workspace.join(meclaw_colony::CELL_ROOT_MARKER).is_file(),
        "the spawn must leave the cell-root marker in the root it creates"
    );
    h.shutdown().await;
}
