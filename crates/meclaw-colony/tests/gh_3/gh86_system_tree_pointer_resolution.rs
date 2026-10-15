//! GH #86 — `{text_id}` leaves in the `system` tree resolve at the delivery
//! boundary, like the `messages[]` pointer class does since GH #19.
//!
//! Same pointer name, different substitution: in `messages[]` a pointer becomes
//! a turn object, in the `system` tree a leaf becomes `{"text": …}`, a plain
//! string container, and the walk covers an arbitrarily deep object tree rather
//! than one array. Until now nothing resolved this site at all: the substrate
//! passed such a leaf through, `llm` rejected it with `BlobUnsupported`, and the
//! `system` state persisted it verbatim.
//!
//! The unit coverage of the resolver semantics lives in
//! `meclaw_colony::blob::pointers`; this file pins the WIRING: that the boundary
//! walks the `system` tree, that it runs before the cell, and that failures
//! dead-letter instead of delivering half an expanded tree.
//!
//! Anti-Cascade (Phase-6.5 lesson): `/sink` is registered before any probe.

use meclaw_colony::DiskBlobStore;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::time::Duration;
use tokio::sync::mpsc;

/// Generous failure marker (30 s convention, robust under cargo parallel load).
const MARKER: Duration = Duration::from_secs(30);

fn turn(text: &str) -> Value {
    json!({"origin": "user", "type": "text", "text": text})
}

/// Write a UBF body document into the colony's blob store, return its id.
async fn put(root: &std::path::Path, doc: Value) -> Uuid {
    let store = DiskBlobStore::new(root.join("blobs")).unwrap();
    let bytes = meclaw_core::serde_json::to_vec(&doc).unwrap();
    store
        .write_streaming(std::io::Cursor::new(bytes), "application/json", None)
        .await
        .unwrap()
        .blob_id
}

/// The one-turn document form a `text_id` names, in both pointer classes.
async fn put_text(root: &std::path::Path, text: &str) -> Uuid {
    put(root, json!({"messages": [turn(text)]})).await
}

fn probe(body: Value) -> Message {
    MessageBuilder::new(Path::new("/sink"))
        .body(Body::Inline(body))
        .ttl(8)
        .build()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(MARKER, rx.recv()).await.ok().flatten()
}

/// Boot a colony with a real blob store and a `CaptureCell` at `/sink`.
async fn colony_with_sink(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new_with_blobs_at(td, vec![]);
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    (h, sink_rx)
}

/// A failed resolution must NOT reach `handle()`.
///
/// The DLQ half of the contract is pinned where the channel actually exists:
/// `cell_task::tests::resolve_*_system_*` (unit, with a colony inbox wired).
/// The test harness' plain `cell_task` deliberately carries no
/// `colony_inbox_tx`, so what this end-to-end level can prove is the
/// load-bearing half: the cell is not called.
async fn assert_cell_is_not_called(rx: &mut mpsc::Receiver<Message>, why: &str) {
    assert!(
        tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .is_err(),
        "{why}"
    );
}

// ── the guarantee ───────────────────────────────────────────────────────────

/// A `system` leaf pointer reaches the cell as the string container it names,
/// alongside the inline leaves it shares the tree with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_system_leaf_pointer_is_expanded_before_the_cell_sees_the_body() {
    let td = tempfile::TempDir::new().unwrap();
    let id = put_text(td.path(), "the long persona").await;
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    h.send(probe(json!({
        "system": {
            "identity": {"soul": {"text": "inline"}, "body": {"text_id": id.to_string()}}
        }
    })))
    .await;

    let got = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive");
    let Body::Inline(body) = got.body else {
        panic!("no Body::Blob may reach handle()")
    };
    assert_eq!(
        body,
        json!({
            "system": {
                "identity": {"soul": {"text": "inline"}, "body": {"text": "the long persona"}}
            }
        }),
        "the cell must see a {{\"text\": …}} container, not the pointer"
    );
    h.shutdown().await;
}

/// The walk is over a tree, not an array: leaves at any depth are expanded, and
/// `system.tools.*` is no exception (its exemption is from prompt
/// concatenation, not from resolution).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leaves_at_every_depth_of_the_tree_are_expanded() {
    let td = tempfile::TempDir::new().unwrap();
    let deep = put_text(td.path(), "deep").await;
    let tool = put_text(td.path(), "{\"name\":\"calc\"}").await;
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    h.send(probe(json!({
        "system": {
            "a": {"b": {"c": {"d": {"text_id": deep.to_string()}}}},
            "tools": {"calculator": {"text_id": tool.to_string()}}
        }
    })))
    .await;

    let got = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive");
    let Body::Inline(body) = got.body else {
        panic!("no Body::Blob may reach handle()")
    };
    assert_eq!(
        body,
        json!({
            "system": {
                "a": {"b": {"c": {"d": {"text": "deep"}}}},
                "tools": {"calculator": {"text": "{\"name\":\"calc\"}"}}
            }
        })
    );
    h.shutdown().await;
}

/// Recursion: the referenced document goes through the `messages[]` resolver,
/// so a pointer inside it is expanded too, on one shared budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_system_leaf_resolves_through_a_nested_message_pointer() {
    let td = tempfile::TempDir::new().unwrap();
    let leaf = put_text(td.path(), "from two hops down").await;
    let mid = put(
        td.path(),
        json!({"messages": [{"text_id": leaf.to_string()}]}),
    )
    .await;
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    h.send(probe(
        json!({"system": {"identity": {"body": {"text_id": mid.to_string()}}}}),
    ))
    .await;

    let got = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive");
    let Body::Inline(body) = got.body else {
        panic!("no Body::Blob may reach handle()")
    };
    assert_eq!(
        body,
        json!({"system": {"identity": {"body": {"text": "from two hops down"}}}})
    );
    h.shutdown().await;
}

/// The loud failure mode: a chain past the configured limit is dead-lettered,
/// and the cell is never called with a half-expanded tree.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_system_chain_past_the_configured_limit_never_reaches_the_cell() {
    let td = tempfile::TempDir::new().unwrap();
    // The colony reads the limit from colony.json — 1 allows exactly one hop.
    std::fs::write(
        td.path().join("colony.json"),
        r#"{"blob_max_recursion_depth": 1}"#,
    )
    .unwrap();
    let leaf = put_text(td.path(), "leaf").await;
    let top = put(
        td.path(),
        json!({"messages": [{"text_id": leaf.to_string()}]}),
    )
    .await;
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    h.send(probe(
        json!({"system": {"identity": {"body": {"text_id": top.to_string()}}}}),
    ))
    .await;

    assert_cell_is_not_called(
        &mut sink_rx,
        "the cell must NOT be called with a half-expanded system tree",
    )
    .await;
    h.shutdown().await;
}

/// The other loud failure: an unresolvable leaf, under the same code a missing
/// whole-body blob reports.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unresolvable_system_pointer_never_reaches_the_cell() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    h.send(probe(json!({
        "system": {"identity": {"body": {"text_id": Uuid::now_v7().to_string()}}}
    })))
    .await;

    assert_cell_is_not_called(
        &mut sink_rx,
        "an unresolvable system leaf must never reach handle()",
    )
    .await;
    h.shutdown().await;
}

/// Both pointer classes in one body, resolved in one delivery: the `system`
/// tree gets its string containers, `messages[]` gets its turns.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_pointer_classes_resolve_in_the_same_delivery() {
    let td = tempfile::TempDir::new().unwrap();
    let persona = put_text(td.path(), "persona").await;
    let history = put(td.path(), json!({"messages": [turn("earlier")]})).await;
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    h.send(probe(json!({
        "system": {"identity": {"body": {"text_id": persona.to_string()}}},
        "messages": [{"messages_id": history.to_string()}, turn("now")]
    })))
    .await;

    let got = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive");
    let Body::Inline(body) = got.body else {
        panic!("no Body::Blob may reach handle()")
    };
    assert_eq!(
        body,
        json!({
            "system": {"identity": {"body": {"text": "persona"}}},
            "messages": [turn("earlier"), turn("now")]
        })
    );
    h.shutdown().await;
}
