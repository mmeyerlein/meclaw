//! Phase-16 W2d (Substrat, Marcus-Ruling 2026-06-12): a cell emission targeting
//! the READ-only `/colony/dead_letters` endpoint is HARD-REJECTED — it must NOT
//! be served as a read whose reply re-injects back to the emitting cell (the
//! pre-W2d ~13 000-message source loop). `/colony/dead_letters` (and the other
//! read endpoints) are NOT writable via EDA dispatch; only `/colony/mutations`
//! is (13.5-A6). The DLQ is read/drained exclusively via the dedicated API
//! `ColonyMsg::ReadDeadLetters`/`DrainDeadLetters` variants, never via dispatch.

use meclaw_colony::{CellFactory, CellFactoryRegistry};
use meclaw_core::Path;
use meclaw_core::serde_json::json;
use meclaw_testing::ColonyHandle;
use meclaw_testing::MessageBuilder as TestMessageBuilder;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use meclaw_testing::factories::EchoCellFactory;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn factories() -> CellFactoryRegistry {
    let mut r: CellFactoryRegistry = HashMap::new();
    let arc: Arc<dyn CellFactory> = Arc::new(EchoCellFactory);
    r.insert("echo".to_string(), arc);
    r
}

/// Count rows in the central message-log of the live colony.db.
fn message_log_count(db_path: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM message_log", [], |r| r.get(0))
        .unwrap_or(0)
}

/// Regression pin (c): a cell that emits to `/colony/dead_letters` (here an echo
/// whose `echo_to` is the READ endpoint) is hard-rejected with exactly ONE
/// `colony_endpoint_unimplemented` DLQ entry and NO self-sustaining source loop.
/// Pre-W2d this looped: the dead_letters READ-reply (reply_to = the emitting
/// cell) re-triggered the cell, which re-emitted, ~13 000×, while the DLQ stayed
/// empty. The bounded message-log count is the pin against that loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emission_to_dead_letters_endpoint_is_hard_rejected_without_loop() {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // /looper echoes whatever it receives straight to the /colony/dead_letters
    // READ endpoint — the pre-W2d loop trigger.
    write(
        td.path(),
        "main/looper/config.json",
        r#"{"cell":{"type":"echo"},"params":{"echo_to":"/colony/dead_letters"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let colony = ColonyHandle::new();
    let db_path = colony.tempdir_path().join("colony.db");
    bootstrap_from_filesystem(td.path(), &factories(), &colony.runtime())
        .await
        .expect("bootstrap must succeed");

    let src = TestMessageBuilder::new("/looper")
        .with_inline_messages(vec![json!({"origin":"user","type":"text","text":"go"})])
        .build();
    colony.send_from(Path::new("/"), src).await;

    // Bounded observation window. Pre-W2d the loop produces thousands of
    // message-log rows in this window; post-W2d the chain terminates at the
    // single hard reject.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let count = message_log_count(&db_path);
    assert!(
        count < 50,
        "message-log must stay bounded (no source loop); got {count} rows"
    );

    let dls = colony.drain_dead_letters().await;
    let rejects: Vec<_> = dls
        .iter()
        .filter(|dl| dl.resolved_target.as_str() == "/colony/dead_letters")
        .collect();
    assert_eq!(
        rejects.len(),
        1,
        "exactly one hard reject for the /colony/dead_letters write; got: {:?}",
        dls.iter()
            .map(|d| (d.resolved_target.as_str(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        rejects[0].reason.as_code(),
        "colony_endpoint_unimplemented",
        "the dead_letters write must be hard-rejected, never served as a read"
    );

    colony.shutdown().await;
}
