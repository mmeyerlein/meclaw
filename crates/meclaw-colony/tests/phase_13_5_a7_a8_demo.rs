//! Phase-13.5 A8 demos (a)–(g): blob auto-offload (producer) + delivery-boundary
//! resolution (consumer), plus the negative `blob_unavailable` path.
//!
//! All positive-deep via a `CaptureCell` sink (Anti-Cascade: `/sink` is registered
//! BEFORE any probe goes out — Phase-6.5 lesson). UBF probes use the `origin`
//! format. Gate runs `tee` their logs at the workspace level.
//!
//! Producer hook (T7): `route_with_log::offload_oversized` rewrites an inline body
//! whose serialized length is `>= blob_inline_max_bytes` to `Body::Blob(uuid)`
//! BEFORE the message-log row is built (so `body_kind="blob"` is logged) and before
//! delivery. Consumer (T6): every `cell_task` resolves `Body::Blob` back to inline
//! at the delivery boundary (spec Z.1363) — so the cell sees transparent inline.
//!
//! The `== threshold` boundary case (A1 canonical `>=`) is pinned as a unit test in
//! `colony.rs` (T7). The `ColonyMsg::DeadLetterMessage` inbox-arm in isolation is
//! pinned in `phase_13_5_a8_dlq_arm.rs` (T6); the negative demo here closes the
//! end-to-end consumer path (cell receives an unresolvable blob → DLQ).

use meclaw_colony::bootstrap_from_filesystem;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::time::Duration;
use tokio::sync::mpsc;

// ── helpers ──────────────────────────────────────────────────────────────────

/// A UBF body whose `text` leaf is `n` bytes — sized to cross (or stay under) a
/// test threshold. `origin` format so the validator + EchoMockCell accept it.
fn ubf_body(text_len: usize) -> Body {
    Body::Inline(json!({
        "messages": [{"origin": "user", "type": "text", "text": "x".repeat(text_len)}]
    }))
}

fn probe(target: &str, body: Body) -> Message {
    MessageBuilder::new(Path::new(target))
        .body(body)
        .ttl(8)
        .build()
}

/// Write a `colony.json` with a custom `blob_inline_max_bytes` into the test root.
fn write_colony_json(dir: &std::path::Path, blob_inline_max_bytes: usize) {
    std::fs::write(
        dir.join("colony.json"),
        format!("{{\"blob_inline_max_bytes\": {blob_inline_max_bytes}}}"),
    )
    .unwrap();
}

/// Count `(blob, sidecar)` files under `<root>/blobs`. A committed blob has both a
/// `<uuid>.json` content file and a `<uuid>.json.meta.json` sidecar.
fn blob_disk_counts(blobs_dir: &std::path::Path) -> (usize, usize) {
    let Ok(rd) = std::fs::read_dir(blobs_dir) else {
        return (0, 0);
    };
    let (mut blobs, mut sidecars) = (0, 0);
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".meta.json") {
            sidecars += 1;
        } else if name.ends_with(".json") {
            blobs += 1;
        }
    }
    (blobs, sidecars)
}

/// Poll `message_log` until a row with the given `to_path` exists, then return its
/// `body_kind` ("inline" | "blob"). The log write is fire-and-forget to the async
/// writer, so a CaptureCell receipt does not guarantee the row is committed yet.
async fn await_body_kind(db_dir: &std::path::Path, to_path: &str) -> String {
    let db = db_dir.join("colony.db");
    let deadline = Duration::from_secs(30);
    let start = std::time::Instant::now();
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
            start.elapsed() < deadline,
            "no message_log row for {to_path}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Read the `body_payload` (the blob uuid for a blob row) of a `to_path` row.
fn read_body_payload(db_dir: &std::path::Path, to_path: &str) -> (String, Option<String>) {
    let conn = rusqlite::Connection::open_with_flags(
        db_dir.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    conn.query_row(
        "SELECT body_kind, body_payload FROM message_log WHERE to_path = ?1 LIMIT 1",
        [to_path],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Pull the first `messages[].text` leaf out of an inline UBF body.
fn first_text(body: &Body) -> String {
    match body {
        Body::Inline(Value::Object(m)) => m
            .get("messages")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|m0| m0.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        _ => panic!("consumer must see an INLINE body at the delivery boundary, got a blob"),
    }
}

// ── (b)+(c) — emission >= threshold → blob on disk + logged; consumer sees inline ─

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_b_c_offload_then_roundtrip_to_inline() {
    let td = tempfile::TempDir::new().unwrap();
    write_colony_json(td.path(), 64); // tiny threshold so a normal UBF body offloads

    let h = ColonyHandle::new_with_blobs_at(&td, vec![]);

    // Anti-Cascade: /sink (consumer) registered before the probe.
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    // Producer: /echo re-emits the (resolved) body to /sink.
    h.spawn(Path::new("/echo"), move || {
        EchoMockCell::new(Path::new("/echo")).emitted_target(Path::new("/sink"))
    })
    .await;
    // W2b (Ruling A1): /echo's emission to /sink needs a wired catch-all out-edge
    // (identity-fallback gone). Both cells are runtime-spawned and live.
    h.add_edge(
        meclaw_core::Uuid::now_v7(),
        Path::new("/echo"),
        Path::new("/sink"),
    )
    .await;

    let marker = "y".repeat(500);
    let body = Body::Inline(json!({
        "messages": [{"origin": "user", "type": "text", "text": marker.clone()}]
    }));
    h.send(probe("/echo", body)).await;

    // (c) W1.b roundtrip: the consumer behind two offload hops sees INLINE content.
    let got = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive");
    let text = first_text(&got.body);
    assert_eq!(
        text, marker,
        "full body content survives offload→resolution"
    );

    // (b) both routed hops logged body_kind="blob".
    assert_eq!(await_body_kind(td.path(), "/echo").await, "blob");
    assert_eq!(await_body_kind(td.path(), "/sink").await, "blob");

    // (b) fresh-probe the disk: committed blob = content file + sidecar.
    let (blobs, sidecars) = blob_disk_counts(&td.path().join("blobs"));
    assert!(
        blobs >= 1 && sidecars >= 1,
        "blob + .meta.json sidecar exist on disk"
    );
    assert_eq!(blobs, sidecars, "every committed blob has its sidecar");

    h.shutdown().await;
}

// ── (d) — body < threshold stays inline, no blob on disk ─────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_d_below_threshold_stays_inline() {
    // Default threshold (no colony.json) = 64 KiB → a small body stays inline.
    let td = tempfile::TempDir::new().unwrap();
    let h = ColonyHandle::new_with_blobs_at(&td, vec![]);

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    h.send(probe("/sink", ubf_body(8))).await;
    let got = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive");
    assert!(matches!(got.body, Body::Inline(_)));

    assert_eq!(await_body_kind(td.path(), "/sink").await, "inline");
    let (blobs, sidecars) = blob_disk_counts(&td.path().join("blobs"));
    assert_eq!(
        (blobs, sidecars),
        (0, 0),
        "sub-threshold body writes no blob"
    );

    h.shutdown().await;
}

// ── (a) — custom blob_inline_max_bytes shifts the threshold ──────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_a_custom_threshold_shifts_offload_boundary() {
    // A body that is ~80 bytes serialized: inline at the 64 KiB default (demo d),
    // but a colony.json threshold of 16 forces it to offload — the threshold moved.
    let td = tempfile::TempDir::new().unwrap();
    write_colony_json(td.path(), 16);
    let h = ColonyHandle::new_with_blobs_at(&td, vec![]);
    assert_eq!(h.blob_inline_max_bytes(), 16, "custom threshold parsed");

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    h.send(probe("/sink", ubf_body(40))).await;
    let got = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive");
    // Consumer still sees inline (resolution) — but the wire hop offloaded.
    assert!(matches!(got.body, Body::Inline(_)));
    assert_eq!(
        await_body_kind(td.path(), "/sink").await,
        "blob",
        "a body below the 64 KiB default offloads under the custom threshold"
    );

    h.shutdown().await;
}

// ── (e) — blob body through hive transit + modifier; ref intact, resolved at sink ─

fn write_hive_modifier_topology(td: &std::path::Path) {
    std::fs::create_dir_all(td.join("main")).unwrap();
    std::fs::write(
        td.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./sink","modifier":{"set_hop":{"transited":"'yes'"}}}
        ]}}}"#,
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_e_blob_through_hive_transit_with_modifier() {
    let td = tempfile::TempDir::new().unwrap();
    write_colony_json(td.path(), 64);
    write_hive_modifier_topology(td.path());

    let h = ColonyHandle::new_with_blobs_at(&td, vec![]);

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    bootstrap_from_filesystem(
        td.path(),
        &meclaw_colony::CellFactoryRegistry::new(),
        &h.runtime(),
    )
    .await
    .expect("bootstrap must succeed");

    // Big body addressed at the hive root `/` → offloaded at the hive hop, then
    // transited (Body::Blob ref intact through transit) to /sink.
    let marker = "z".repeat(400);
    let body = Body::Inline(json!({
        "messages": [{"origin": "user", "type": "text", "text": marker.clone()}]
    }));
    h.send(probe("/", body)).await;

    let got = recv_bounded(&mut sink_rx)
        .await
        .expect("/sink must receive via transit");
    // Resolved at the delivery boundary → inline content intact.
    assert_eq!(
        first_text(&got.body),
        marker,
        "content intact across transit + resolution"
    );
    // CEL modifier applied on the transit edge.
    assert_eq!(
        got.headers.hop.get("transited").and_then(|v| v.as_str()),
        Some("yes"),
        "hive out-edge modifier set the header"
    );
    // The hive hop logged the body as a blob (ref rode through transit).
    assert_eq!(await_body_kind(td.path(), "/sink").await, "blob");

    h.shutdown().await;
}

// ── (f) — reboot: a blob-referenced message stays readable ───────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_f_blob_survives_reboot_and_is_readable() {
    let td = tempfile::TempDir::new().unwrap();
    write_colony_json(td.path(), 64);
    let marker = "w".repeat(500);

    // Boot 1: offload a body to /sink, then shut down.
    {
        let h = ColonyHandle::new_with_blobs_at(&td, vec![]);
        let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
        h.spawn(Path::new("/sink"), move || {
            CaptureCell::new(sink_tx.clone())
        })
        .await;
        let body = Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": marker.clone()}]
        }));
        h.send(probe("/sink", body)).await;
        recv_bounded(&mut sink_rx)
            .await
            .expect("/sink must receive");
        assert_eq!(await_body_kind(td.path(), "/sink").await, "blob");
        h.shutdown().await;
    }

    // Reboot: the message_log row + the on-disk blob (No-Delete) survive. A FRESH
    // store on the same root reads the blob uuid from the persisted row.
    let (kind, payload) = read_body_payload(td.path(), "/sink");
    assert_eq!(kind, "blob");
    let blob_id = meclaw_core::Uuid::parse_str(&payload.expect("blob payload = uuid")).unwrap();
    let store = meclaw_colony::DiskBlobStore::new(td.path().join("blobs")).unwrap();
    let value = store
        .read_body(blob_id)
        .await
        .expect("blob readable after reboot");
    assert_eq!(
        first_text(&Body::Inline(value)),
        marker,
        "rebooted blob round-trips"
    );
}

// ── (g) — colony.json: absent → defaults; broken → hard-fail (boot + --validate) ─

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_g_colony_json_absent_defaults_broken_hard_fail() {
    use meclaw_colony::colony_config::read_colony_config;

    // Absent → defaults (64 KiB).
    let absent = tempfile::TempDir::new().unwrap();
    let cfg = read_colony_config(absent.path()).expect("absent colony.json → defaults");
    assert_eq!(cfg.blob_inline_max_bytes, 65_536);

    // Broken → hard boot fail (ruling 1). The CLI `--validate` exit-code
    // proof lives in `meclaw-cli/tests/phase_13_5_a7_a8_validate_colony_json.rs`.
    let broken = tempfile::TempDir::new().unwrap();
    std::fs::write(broken.path().join("colony.json"), "{ not valid json").unwrap();
    assert!(
        read_colony_config(broken.path()).is_err(),
        "invalid colony.json hard-fails the boot read"
    );
}

// ── negative — consumer receives an unresolvable blob → not delivered, no panic ─
//
// The cell-side `read_body`-fail → `ColonyMsg::DeadLetterMessage(blob_unavailable)`
// send is pinned (with a wired `colony_inbox_tx`) in the T6 unit test
// `cell_task::tests::resolve_blob_unresolvable_dead_letters_and_skips_delivery`,
// and the colony arm recording it is pinned in `phase_13_5_a8_dlq_arm.rs`. The
// test-harness `cell_task` (used by `h.spawn`) is intentionally wired with NO
// `colony_inbox_tx` (no DLQ path) — so here we prove the substrate-level guarantee
// it CAN show end-to-end: an unresolvable blob is never handed to the cell, no
// panic, and the cell task keeps serving.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demo_negative_unknown_blob_skips_delivery_without_panic() {
    let td = tempfile::TempDir::new().unwrap();
    let h = ColonyHandle::new_with_blobs_at(&td, vec![]);

    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // A blob ref to a uuid that was never written → resolution fails at delivery.
    let unknown = meclaw_core::Uuid::now_v7();
    let mut msg = MessageBuilder::new(Path::new("/sink")).ttl(8).build();
    msg.body = Body::Blob(unknown);
    h.send(msg).await;

    // The cell never sees the unresolvable blob (handle is skipped).
    let no_receipt = tokio::time::timeout(Duration::from_millis(700), sink_rx.recv()).await;
    assert!(
        no_receipt.is_err(),
        "unresolvable blob is NOT delivered to the cell"
    );

    // No panic: the colony + sink task are still alive (a follow-up send works).
    h.send(probe("/sink", ubf_body(4))).await;
    assert!(
        recv_bounded(&mut sink_rx).await.is_some(),
        "cell task survived the failed resolution"
    );

    h.shutdown().await;
}
