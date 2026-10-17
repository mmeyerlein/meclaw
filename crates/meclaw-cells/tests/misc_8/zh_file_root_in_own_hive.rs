//! GH #1234 (T6-12) — `file` and `edit` take a relative `base_path` as ONE root
//! in the hive that encloses them, created at the spawn.
//!
//! The shipped tool surface rooted both cells at `/tmp`: every assistant on the
//! machine read and wrote the same directory, and an operator who never
//! repointed the pair never learned it. A relative root names a directory of
//! the cells' own hive instead; two cells in one hive that name the same
//! relative root share it, two hives never do.

use meclaw_cells::edit::EditCellFactory;
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
        ("edit".to_string(), Arc::new(EditCellFactory)),
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

fn tool(kind: &str) -> Value {
    json!({
        "cell": {"type": kind},
        "params": {"base_path": "workspace", "max_concurrency": 1},
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
async fn file_and_edit_share_one_relative_root_in_their_hive() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    write_json(&root.join("main/files/config.json"), &tool("file"));
    write_json(&root.join("main/edits/config.json"), &tool("edit"));
    write_json(
        &root.join("main/config.json"),
        &json!({"cell": {"type": "hive"},
                "params": {"graph": {"edges": [{"from": "./files", "to": "./edits"}]}}}),
    );

    let h = ColonyHandle::new_with_factories_at(&td, factories());
    bootstrap_from_filesystem(root, &registry(), &h.runtime())
        .await
        .expect("a relative file root must boot");
    let workspace = root.join("main/workspace");
    assert!(
        workspace.is_dir(),
        "the root is created at the spawn, inside the enclosing hive"
    );

    h.send(call(
        "/files",
        json!({"op": "write", "path": "note.txt", "content": "foo"}),
    ))
    .await;
    h.quiesce().await;
    h.send(call(
        "/edits",
        json!({"op": "find_replace", "path": "note.txt", "find": "foo", "replace": "bar"}),
    ))
    .await;
    h.quiesce().await;

    assert_eq!(
        std::fs::read_to_string(workspace.join("note.txt")).expect("file wrote into the hive"),
        "bar",
        "edit must see what file wrote: one root for both"
    );
    h.shutdown().await;
}
