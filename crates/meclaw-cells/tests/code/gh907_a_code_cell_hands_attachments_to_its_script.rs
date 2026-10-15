//! GH #907 (interface 2) -- a `code` cell whose contract declares
//! `consumes.body.attachments` reads every object entry of the incoming
//! `body.attachments` from the colony blob store BEFORE its script runs, and
//! the script finds the entry plus `data_b64` (or plus `error: {code,
//! message}`) on stdin. Only stdin changes: nothing the cell emits carries the
//! bytes, and a cell without the declaration hands the entry on untouched.
//!
//! Red against the code before GH #907: `CodeCell::with_attachment_reader`
//! did not exist, and without it the script saw the bare entry -- `n` was 0,
//! `sha` the hash of nothing, `err` empty where `not_found` is asserted.

use meclaw_cells::code::{CodeCell, CodeParams};
use meclaw_colony::{AttachmentReader, ContractView, DiskBlobStore, StatelessCell};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::mpsc;

/// The script decodes the first attachment and answers with what it found:
/// the decoded length, its SHA-256, the error code (or ""), whether a
/// `data_b64` was there at all, and the entry itself WITHOUT `data_b64` --
/// so the emission can be checked for the bytes it must not carry.
const SCRIPT: &str = r#"
import sys, json, base64, hashlib
doc = json.load(sys.stdin)
att = (doc["body"].get("attachments") or [{}])[0]
raw = att.get("data_b64")
data = base64.b64decode(raw, validate=True) if isinstance(raw, str) else b""
err = (att.get("error") or {}).get("code") or ""
sys.stdout.write(json.dumps({
    "messages": [],
    "n": len(data),
    "sha": hashlib.sha256(data).hexdigest(),
    "err": err,
    "had_b64": isinstance(raw, str),
    "entry": {k: v for k, v in att.items() if k != "data_b64"},
}))
"#;

fn contract(consumes: Value) -> ContractView {
    let block: meclaw_core::ConsumesBlock = meclaw_core::serde_json::from_value(consumes).unwrap();
    ContractView {
        consumes: Some(Arc::new(meclaw_core::CompiledConsumes::compile(&block))),
        ..ContractView::default()
    }
}

fn declaring() -> ContractView {
    contract(json!({"body": {"messages": {"type": "array"},
                             "attachments": {"type": "array", "required": false}}}))
}

fn not_declaring() -> ContractView {
    contract(json!({"body": {"messages": {"type": "array"}}}))
}

fn cell(contract: &ContractView, store: &Arc<DiskBlobStore>) -> CodeCell {
    let raw = json!({"runner": "python3", "script_inline": SCRIPT,
                     "external_timeout_ms": 20_000});
    let params = CodeParams::parse(&raw).unwrap();
    CodeCell::new(params, false, None, false)
        .with_stdin_params(&raw)
        .with_attachment_reader(
            AttachmentReader::for_contract(contract, Some(Arc::clone(store))),
            5_000,
        )
}

/// Drive `handle` once with `attachments` in the body; the one emission.
async fn run(cell: &CodeCell, attachments: Value) -> CellEmission {
    let (otx, mut orx) = mpsc::channel(16);
    let sink = OutputSink::new(
        otx,
        Path::new("/code"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );
    let msg = MessageBuilder::new(Path::new("/code"))
        .body(Body::Inline(
            json!({"messages": [], "attachments": attachments}),
        ))
        .reply_to(Path::new("/sink"))
        .build();
    cell.handle(msg, &sink).await;
    drop(sink);
    let mut outs = Vec::new();
    while let Some(em) = orx.recv().await {
        outs.push(em);
    }
    assert_eq!(outs.len(), 1, "exactly one emission: {outs:?}");
    outs.remove(0)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Standard padded base64 -- only to look for the text in the emission.
fn b64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= c.len() {
                A[(n >> shift) as usize & 63] as char
            } else {
                '='
            });
        }
    }
    out
}

async fn store_with_blob() -> (tempfile::TempDir, Arc<DiskBlobStore>, Vec<u8>, Value) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DiskBlobStore::new(dir.path()).unwrap());
    // A few hundred KiB, not a repeating pattern a short window would match.
    let bytes: Vec<u8> = (0..300 * 1024u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let blob = store
        .write_streaming(bytes.as_slice(), "application/pdf", Some("a.pdf"))
        .await
        .unwrap();
    let entry = json!({"blob_id": blob.blob_id.to_string(), "mime_type": "application/pdf",
                       "filename": "a.pdf", "size_bytes": bytes.len(),
                       "sha256": sha256_hex(&bytes)});
    (dir, store, bytes, entry)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declaring_cell_hands_the_bytes_to_its_script_and_never_emits_them() {
    let (_dir, store, bytes, entry) = store_with_blob().await;
    let em = run(&cell(&declaring(), &store), json!([entry.clone()])).await;
    let c = &em.content;
    assert_eq!(c["err"], "", "{c}");
    assert_eq!(c["n"], bytes.len(), "the script decoded every byte");
    assert_eq!(c["sha"], sha256_hex(&bytes));
    assert_eq!(
        c["entry"], entry,
        "the entry itself reaches the script unchanged"
    );
    let text = c.to_string();
    assert!(!text.contains("data_b64"), "no data_b64 in an emission");
    let encoded = b64(&bytes);
    let mid = encoded.len() / 2;
    for window in [&encoded[..64], &encoded[mid..mid + 64]] {
        assert!(!text.contains(window), "no base64 text in an emission");
    }
    assert!(
        text.len() < 4096,
        "the emission stays small: {} bytes",
        text.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_blob_reaches_the_script_as_not_found() {
    let (_dir, store, _bytes, _entry) = store_with_blob().await;
    let cell = cell(&declaring(), &store);
    let unknown = json!({"blob_id": Uuid::now_v7().to_string(), "mime_type": "application/pdf"});
    let em = run(&cell, json!([unknown])).await;
    assert_eq!(em.content["err"], "not_found", "{}", em.content);
    assert_eq!(em.content["had_b64"], false);
    // A blob_id that is no UUID at all is a bad reference.
    let em = run(&cell, json!([{"blob_id": "nope"}])).await;
    assert_eq!(em.content["err"], "bad_ref", "{}", em.content);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cell_without_the_declaration_hands_the_entry_on_unchanged() {
    let (_dir, store, _bytes, entry) = store_with_blob().await;
    let em = run(&cell(&not_declaring(), &store), json!([entry.clone()])).await;
    let c = &em.content;
    assert_eq!(c["had_b64"], false, "no reader, no bytes");
    assert_eq!(c["err"], "");
    assert_eq!(c["n"], 0);
    assert_eq!(c["entry"], entry, "byte-identical entry on stdin");
}

/// Review I-FR2 m-7 (OR-FJ-82): what a producer brought along in the entry is
/// not the substrate's word. A `data_b64` and an `error` riding in on the
/// entry are dropped before the read -- the script sees the blob's bytes, or
/// the reader's own code, never the forged ones.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forged_data_b64_on_the_way_in_is_dropped() {
    let (_dir, store, bytes, entry) = store_with_blob().await;
    let cell = cell(&declaring(), &store);
    let mut forged = entry.clone();
    forged["data_b64"] = json!("Zm9yZ2Vk");
    forged["error"] = json!({"code": "forged", "message": "not the reader's"});
    let em = run(&cell, json!([forged])).await;
    assert_eq!(em.content["err"], "", "{}", em.content);
    assert_eq!(
        em.content["n"],
        bytes.len(),
        "the blob's bytes, not the forged ones"
    );
    assert_eq!(em.content["sha"], sha256_hex(&bytes));
    assert_eq!(em.content["entry"], entry, "the forged keys are gone");

    // With an unknown blob the forged bytes do not stand in for the missing ones.
    let unknown = json!({"blob_id": Uuid::now_v7().to_string(), "data_b64": "Zm9yZ2Vk"});
    let em = run(&cell, json!([unknown])).await;
    assert_eq!(em.content["err"], "not_found", "{}", em.content);
    assert_eq!(em.content["had_b64"], false);
}

/// Review I-FR2 m-7 (OR-FJ-82), AGENTS.md rule 12: a store that never answers
/// costs the cell `attachment_timeout_ms`, and the script gets `timeout` --
/// the message is not held for the cell's whole message timeout. The blob's
/// content file is a FIFO without a writer, so the read blocks in `open`.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_that_never_returns_reaches_the_script_as_timeout() {
    let (dir, store, _bytes, entry) = store_with_blob().await;
    let id = entry["blob_id"].as_str().unwrap().to_string();
    let content = dir.path().join(format!("{id}.pdf"));
    std::fs::remove_file(&content).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&content)
        .status()
        .expect("mkfifo");
    assert!(made.success(), "mkfifo failed");

    let raw = json!({"runner": "python3", "script_inline": SCRIPT,
                     "external_timeout_ms": 20_000});
    let params = CodeParams::parse(&raw).unwrap();
    let cell = CodeCell::new(params, false, None, false)
        .with_stdin_params(&raw)
        .with_attachment_reader(
            AttachmentReader::for_contract(&declaring(), Some(Arc::clone(&store))),
            300,
        );
    let started = std::time::Instant::now();
    let em = run(&cell, json!([entry])).await;
    let took = started.elapsed();
    assert_eq!(em.content["err"], "timeout", "{}", em.content);
    assert_eq!(em.content["had_b64"], false);
    assert!(
        took < std::time::Duration::from_secs(10),
        "the read was bounded by the attachment timeout, took {took:?}"
    );

    // Release the blocked reader: opening the write end completes its `open`,
    // closing it hands it EOF, so the blocking pool can wind down.
    let fifo = content.clone();
    std::thread::spawn(move || drop(std::fs::OpenOptions::new().write(true).open(fifo)))
        .join()
        .unwrap();
}
