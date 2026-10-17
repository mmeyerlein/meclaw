//! GH #1234 (T6-11) — a relative `params.transfer.base_path` names a directory
//! inside the hive that encloses the cell, never a directory every colony on
//! the machine shares.
//!
//! Until now the fence had to be absolute, and the shipped stores pointed it at
//! `/tmp/...`: one scratch directory for every member of every colony on the
//! host, a read path between cells that no edge carries. A relative fence is
//! anchored at the spawn in the cell's own hive directory and created there
//! with a marker the boot walker never descends into, so the exported seed set
//! never becomes a node.

use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json, to_string_pretty};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "store".to_string(),
        Arc::new(StoreCellFactory) as Arc<dyn CellFactory>,
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

fn store(transfer: Option<&str>) -> Value {
    let mut params = json!({"schema": {"facts": {"id": "text", "claim": "text"}}});
    if let Some(base) = transfer {
        params["transfer"] = json!({"base_path": base});
    }
    json!({
        "cell": {"type": "store"},
        "params": params,
        "contract": {"version": "1.0.0", "settings": {}, "emits": {}, "consumes": {}}
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relative_transfer_root_lands_in_the_enclosing_hive() {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    write_json(&root.join("main/one/config.json"), &store(Some("transfer")));
    write_json(&root.join("main/two/config.json"), &store(None));
    write_json(
        &root.join("main/config.json"),
        &json!({"cell": {"type": "hive"},
                "params": {"graph": {"edges": [{"from": "./one", "to": "./two"}]}}}),
    );

    let h = ColonyHandle::new_with_factories_at(&td, factories());
    bootstrap_from_filesystem(root, &registry(), &h.runtime())
        .await
        .expect("a relative transfer root must boot");

    h.send(
        MessageBuilder::new(Path::new("/one"))
            .reply_to(Path::new("/nobody"))
            .body(Body::Inline(
                json!({"transfer": {"operation": "export", "to": "gen-1"}}),
            ))
            .build(),
    )
    .await;
    h.quiesce().await;

    let seed = root.join("main/transfer/gen-1/seed");
    assert!(
        seed.join("export_final.json").is_file(),
        "the export must land in the enclosing hive's own directory, got: {:?}",
        std::fs::read_dir(root.join("main"))
            .map(|d| d
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .collect::<Vec<_>>())
            .unwrap_or_default()
    );
    assert!(seed.join("facts.jsonl").is_file());
    h.shutdown().await;
}
