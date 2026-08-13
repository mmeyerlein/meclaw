//! GH #19 (D-025) — in-message blob pointers resolve at the delivery boundary.
//!
//! Until now the substrate resolved only the whole-body `Body::Blob` offload;
//! `text_id` and `messages_id` entries inside `messages[]` fell through
//! untouched, and the rule that kept the system honest was a PROHIBITION: no
//! cell may emit them (CLAUDE.md rule 14, class 1). A prohibition is not a
//! guarantee — it holds exactly as long as everyone remembers it, and the
//! `code` cell was about to forget.
//!
//! These tests replace the prohibition. A cell that receives a message with
//! in-message pointers sees a fully expanded conversation; a chain that does not
//! terminate is dead-lettered loudly with the canonical `blob_recursion_too_deep`
//! instead of reaching `handle()` as an unusable UUID.
//!
//! The unit coverage of the resolver semantics lives in
//! `meclaw_colony::blob::pointers`; this file pins the WIRING: that the boundary
//! calls it, that it runs before the cell, and that failures dead-letter.
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
///
/// Uses the SAME store root the colony reads from (`<root>/blobs`), so the
/// pointer a probe carries names a blob the delivery boundary can actually find.
async fn put(root: &std::path::Path, doc: Value) -> Uuid {
    let store = DiskBlobStore::new(root.join("blobs")).unwrap();
    let bytes = meclaw_core::serde_json::to_vec(&doc).unwrap();
    store
        .write_streaming(std::io::Cursor::new(bytes), "application/json", None)
        .await
        .unwrap()
        .blob_id
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
/// `cell_task::tests::resolve_*_dead_letters_*` (unit, with a colony inbox
/// wired) plus `phase_13_5_a8_dlq_arm.rs` for the colony arm. The test harness'
/// plain `cell_task` deliberately carries no `colony_inbox_tx`, so what this
/// end-to-end level can prove is the load-bearing half: the cell is not called.
async fn assert_cell_is_not_called(rx: &mut mpsc::Receiver<Message>, why: &str) {
    assert!(
        tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .is_err(),
        "{why}"
    );
}

// ── the guarantee that replaces the prohibition ─────────────────────────────

/// A `messages_id` pointer reaches the cell as the conversation it names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bulk_pointer_is_expanded_before_the_cell_sees_the_body() {
    let td = tempfile::TempDir::new().unwrap();
    let id = put(td.path(), json!({"messages": [turn("from the blob")]})).await;
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    h.send(probe(json!({
        "messages": [turn("inline"), {"messages_id": id.to_string()}]
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
        json!({"messages": [turn("inline"), turn("from the blob")]}),
        "the cell must see the expanded conversation, not the pointer"
    );
    h.shutdown().await;
}

/// A `text_id` pointer reaches the cell as the single turn it names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_pointer_is_expanded_before_the_cell_sees_the_body() {
    let td = tempfile::TempDir::new().unwrap();
    let id = put(td.path(), json!({"messages": [turn("the long one")]})).await;
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    h.send(probe(json!({"messages": [{"text_id": id.to_string()}]})))
        .await;

    let got = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive");
    let Body::Inline(body) = got.body else {
        panic!("no Body::Blob may reach handle()")
    };
    assert_eq!(body, json!({"messages": [turn("the long one")]}));
    h.shutdown().await;
}

/// Recursion: a pointer inside a resolved blob is resolved too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nested_pointers_are_resolved_all_the_way_down() {
    let td = tempfile::TempDir::new().unwrap();
    let leaf = put(td.path(), json!({"messages": [turn("leaf")]})).await;
    let mid = put(
        td.path(),
        json!({"messages": [turn("mid"), {"messages_id": leaf.to_string()}]}),
    )
    .await;
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    h.send(probe(
        json!({"messages": [{"messages_id": mid.to_string()}]}),
    ))
    .await;

    let got = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive");
    let Body::Inline(body) = got.body else {
        panic!("no Body::Blob may reach handle()")
    };
    assert_eq!(body, json!({"messages": [turn("mid"), turn("leaf")]}));
    h.shutdown().await;
}

/// The loud failure mode the issue asks for: a chain past the configured limit
/// is dead-lettered under the canonical code, and the cell is never called.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chain_past_the_configured_limit_dead_letters_and_never_reaches_the_cell() {
    let td = tempfile::TempDir::new().unwrap();
    // The colony reads the limit from colony.json — 1 allows exactly one hop.
    std::fs::write(
        td.path().join("colony.json"),
        r#"{"blob_max_recursion_depth": 1}"#,
    )
    .unwrap();
    let leaf = put(td.path(), json!({"messages": [turn("leaf")]})).await;
    let top = put(
        td.path(),
        json!({"messages": [{"messages_id": leaf.to_string()}]}),
    )
    .await;
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    h.send(probe(
        json!({"messages": [{"messages_id": top.to_string()}]}),
    ))
    .await;

    assert_cell_is_not_called(
        &mut sink_rx,
        "the cell must NOT be called with a half-expanded conversation",
    )
    .await;
    h.shutdown().await;
}

/// An unresolvable pointer is the other loud failure (`blob_unavailable`, the
/// same code a missing whole-body blob reports).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unresolvable_pointer_dead_letters_as_blob_unavailable() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut sink_rx) = colony_with_sink(&td).await;

    h.send(probe(
        json!({"messages": [{"messages_id": Uuid::now_v7().to_string()}]}),
    ))
    .await;

    assert_cell_is_not_called(
        &mut sink_rx,
        "an unresolvable pointer must never reach handle()",
    )
    .await;
    h.shutdown().await;
}

/// The other classes are deliberately untouched: `attachments[]` refs belong to
/// the consuming cell, and `system` `{text_id}` leaves are a separate site.
/// Both must arrive verbatim, not silently mangled by this resolver.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attachments_and_system_pointers_arrive_untouched() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut sink_rx) = colony_with_sink(&td).await;
    let sent = json!({
        "system": {"identity": {"body": {"text_id": Uuid::now_v7().to_string()}}},
        "attachments": [{
            "blob_id": Uuid::now_v7().to_string(),
            "mime_type": "application/pdf",
            "filename": "report.pdf",
            "size_bytes": 4096
        }]
    });

    h.send(probe(sent.clone())).await;

    let got = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive");
    let Body::Inline(body) = got.body else {
        panic!("no Body::Blob may reach handle()")
    };
    assert_eq!(
        body, sent,
        "attachments[] refs and system text_id leaves are other pointer classes \
         and pass through byte-identical"
    );
    h.shutdown().await;
}
