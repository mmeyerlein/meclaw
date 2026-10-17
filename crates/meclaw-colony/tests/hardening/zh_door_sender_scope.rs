//! GH #1231 (T6-02): the mutation door binds the sender (R-ZH-2, variant a).
//!
//! A drawn edge to `/colony/mutations` is the right to mutate, anywhere. A
//! sender WITHOUT such an edge may only mutate inside its own hive subtree —
//! the nearest hive that holds it. Anything outside is refused with
//! `sender_out_of_scope`. The HTTP door has no sender and stays the root.

use meclaw_colony::CellFactoryRegistry;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::MessageBuilder;
use meclaw_testing::bootstrap_apply::bootstrap_from_filesystem;
use tempfile::TempDir;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// One fresh colony per knock: `/org` holds the only door edge, `/org/a` and
/// `/org/b` are plain hives. Returns the `mutation` slot of the door's reply,
/// which dead-letters because no node answers at `reply_to`.
async fn knock(sender: &str, scope: &str) -> Value {
    knock_with(
        sender,
        json!({"messages": [], "scope": scope, "diff": {}}),
        "mutation",
    )
    .await
}

/// Like [`knock`], with any door body; returns the reply's `slot`
/// (`mutation` for the single form, `manifest` for a manifest).
async fn knock_with(sender: &str, body: Value, slot: &str) -> Value {
    let td = TempDir::new().unwrap();
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./org","to":"/colony/mutations","condition":"has(hop.route) && hop.route == 'mutate'"}
        ]}}}"#,
    );
    for hive in ["main/org", "main/org/a", "main/org/b"] {
        write(
            td.path(),
            &format!("{hive}/config.json"),
            r#"{"cell":{"type":"hive"}}"#,
        );
    }
    let colony = ColonyHandle::new();
    bootstrap_from_filesystem(td.path(), &CellFactoryRegistry::new(), &colony.runtime())
        .await
        .expect("bootstrap must succeed");

    let msg = MessageBuilder::new("/colony/mutations")
        .with_inline_content(body)
        .with_reply_to(sender)
        .build();
    colony.send_from(Path::new(sender), msg).await;
    colony.quiesce().await;

    let dls = colony.drain_dead_letters().await;
    let replies: Vec<Value> = dls
        .iter()
        .filter_map(|dl| match &dl.message.body {
            Body::Inline(v) => v.get(slot).cloned(),
            Body::Blob(_) => None,
        })
        .collect();
    assert_eq!(
        replies.len(),
        1,
        "exactly one door reply for {sender}; dead letters: {:?}",
        dls.iter()
            .map(|d| (d.original_target.as_str(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
    colony.shutdown().await;
    replies.into_iter().next().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sender_without_a_door_edge_mutates_only_its_own_hive_subtree() {
    // Outside the sender's hive `/org/a`: refused.
    let out = knock("/org/a/ghost", "/org/b").await;
    assert_eq!(out["outcome"], "rejected", "{out}");
    assert_eq!(out["error_code"], "sender_out_of_scope", "{out}");

    // Inside its own hive: committed.
    let own = knock("/org/a/ghost", "/org/a").await;
    assert_eq!(own["outcome"], "committed", "{own}");

    // The edge is the right: `/org` holds the door edge and may mutate the root.
    let edge = knock("/org", "/").await;
    assert_eq!(edge["outcome"], "committed", "{edge}");
}

/// Fix round 1 (review I1): the sender check runs over every entry BEFORE the
/// door, so a refused manifest applied nothing. The receipt says so:
/// `failed_at: 0`, no ids, every entry remaining — a caller who resubmits from
/// `failed_at` resubmits all of it. The refused entry's position is in
/// `details`. Before, the receipt named the out-of-scope entry's position (3)
/// with no ids, i.e. claimed entries 1 and 2 applied.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_refused_for_its_sender_names_no_applied_position() {
    let out = knock_with(
        "/org/a/ghost",
        json!({"messages": [], "manifest": [
            {"scope": "/org/a", "diff": {}},
            {"scope": "/org/a", "diff": {}},
            {"scope": "/org/b", "diff": {}}
        ]}),
        "manifest",
    )
    .await;
    assert_eq!(out["outcome"], "rejected", "{out}");
    assert_eq!(out["error_code"], "sender_out_of_scope", "{out}");
    assert_eq!(out["failed_at"], 0, "nothing was applied: {out}");
    assert_eq!(out["applied"], 0, "{out}");
    assert_eq!(out["ids"], json!([]), "{out}");
    assert_eq!(out["remaining"], 3, "every entry is still to send: {out}");
    let details = out["details"].as_str().unwrap_or_default();
    assert!(
        details.contains("entry 3"),
        "details name the refused entry: {out}"
    );
}
