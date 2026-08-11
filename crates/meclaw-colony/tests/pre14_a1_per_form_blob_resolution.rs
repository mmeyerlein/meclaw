//! Pre-14 Pass-2 backstop A1 — per-form blob-resolution at the delivery boundary.
//!
//! Substrate contract (CONTRIBUTING.md harte Regel 14 + spec Z.1363): the ONLY
//! `Body::Blob` a cell can be routed is the substrate's whole-body offload of an
//! oversized inline body (`offload_oversized` on the way out, serialized length
//! `>= blob_inline_max_bytes`). Every `cell_task` resolves that `Body::Blob` back
//! to `Body::Inline` via `resolve_blob_for_delivery` BEFORE `handle()` — so a
//! receiving cell NEVER sees a `Body::Blob` ref, only the full inline content.
//!
//! `phase_13_5_a7_a8_demo.rs::demo_b_c` already pins the `messages[]` form. This
//! backstop widens the guarantee to EVERY UBF body form that can carry the
//! whole-body offload: `messages[]`, `system`, and `attachments[]`. For each
//! form we push a body above a tiny inline threshold straight at a `CaptureCell`
//! sink and assert the consumer's `handle()` receives the FULL resolved content,
//! byte-identical to what was sent, and NEVER a `Body::Blob`.
//!
//! Anti-Cascade (Phase-6.5 lesson): `/sink` is registered before any probe.

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::time::Duration;
use tokio::sync::mpsc;

/// Tiny inline threshold so any of the probe bodies below crosses it and is
/// offloaded to a whole-body `Body::Blob` on the wire.
const TINY_THRESHOLD: usize = 16;

fn write_colony_json(dir: &std::path::Path, blob_inline_max_bytes: usize) {
    std::fs::write(
        dir.join("colony.json"),
        format!("{{\"blob_inline_max_bytes\": {blob_inline_max_bytes}}}"),
    )
    .unwrap();
}

fn probe(target: &str, body: Value) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(body))
        .ttl(8)
        .build()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Poll `message_log` until a row for `to_path` exists, then return its
/// `body_kind` ("inline" | "blob"). Proves the wire hop ACTUALLY offloaded —
/// without this, an inline-stays-inline body would pass the resolution asserts
/// trivially (a hollow backstop). Generous 30 s failure marker.
async fn await_body_kind(db_dir: &std::path::Path, to_path: &str) -> String {
    let db = db_dir.join("colony.db");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let conn =
            rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let row: Option<String> = conn
            .query_row(
                "SELECT body_kind FROM message_log WHERE to_path = ?1 LIMIT 1",
                [to_path],
                |r| r.get(0),
            )
            .ok();
        if let Some(kind) = row {
            return kind;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no message_log row for {to_path}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Round-trip a single body FORM: send it (over the offload threshold) at a
/// `CaptureCell` and assert the consumer sees the FULL inline content,
/// byte-identical, and NEVER a `Body::Blob`. The `sent` body is the exact
/// inline `Value` the probe carries; the resolved body must equal it.
async fn assert_form_roundtrips_inline(form_label: &str, sent: Value) {
    let td = tempfile::TempDir::new().unwrap();
    write_colony_json(td.path(), TINY_THRESHOLD);
    let h = ColonyHandle::new_with_blobs_at(&td, vec![]);

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // Sanity: the serialized body is genuinely over the threshold, so the
    // substrate offloads it to a whole-body Body::Blob on the wire (otherwise
    // this test would prove nothing — it would stay inline trivially).
    let serialized_len = meclaw_core::serde_json::to_string(&sent).unwrap().len();
    assert!(
        serialized_len >= TINY_THRESHOLD,
        "{form_label}: probe body must exceed the inline threshold to force offload \
         (len={serialized_len}, threshold={TINY_THRESHOLD})"
    );

    h.send(probe("/sink", sent.clone())).await;

    let got = recv_bounded(&mut sink_rx)
        .await
        .unwrap_or_else(|| panic!("{form_label}: /sink must receive"));

    match got.body {
        Body::Inline(resolved) => {
            assert_eq!(
                resolved, sent,
                "{form_label}: consumer must see the FULL resolved body, byte-identical"
            );
        }
        Body::Blob(id) => {
            panic!("{form_label}: NO Body::Blob may leak to handle() — got an unresolved ref {id}")
        }
    }

    // The wire hop genuinely offloaded (body_kind="blob") — so the inline body
    // the consumer saw is the RESULT of resolution, not an offload that never
    // happened. Without this the resolution asserts above would be hollow.
    assert_eq!(
        await_body_kind(td.path(), "/sink").await,
        "blob",
        "{form_label}: the over-threshold body must offload on the wire"
    );

    h.shutdown().await;
}

// ── messages[] form — the conversational slot carries the offload ────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messages_form_offload_resolves_to_full_inline_at_consumer() {
    let body = json!({
        "messages": [{"origin": "user", "type": "text", "text": "m".repeat(500)}]
    });
    assert_form_roundtrips_inline("messages[]", body).await;
}

// ── system form — the system slot carries the offload ────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn system_form_offload_resolves_to_full_inline_at_consumer() {
    let body = json!({
        "system": {"header": {"finish_reason": "stop"}, "prompt": "s".repeat(500)}
    });
    assert_form_roundtrips_inline("system", body).await;
}

// ── attachments[] form — the file-upload slot carries the offload ────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attachments_form_offload_resolves_to_full_inline_at_consumer() {
    // A single schema-valid Attachment serializes well over the tiny threshold
    // (uuid + mime + size + filename), so the whole body offloads.
    let body = json!({
        "attachments": [{
            "blob_id": Uuid::now_v7().to_string(),
            "mime_type": "application/pdf",
            "filename": "report.pdf",
            "size_bytes": 4096
        }]
    });
    assert_form_roundtrips_inline("attachments[]", body).await;
}
