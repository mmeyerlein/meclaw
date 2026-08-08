//! Phase-12-X T19: end-to-end proof that a multipart `POST /messages` request
//! with a file attachment runs through the entire stack —
//! upload → DiskBlobStore (file + sidecar on disk) → UBF body with an
//! `attachments[]` slot → colony routing → `colony.db.message_log` →
//! `GET /colony/trace` returns the entry including the serialized
//! `attachments[]` slot.
//!
//! Routing detail: `route_with_log` writes the `message_log` row only when
//! `pre_routable = registry.contains_key(resolved_target)`. So that the probe
//! lands in the trace, the bootstrap registers a real `bash` cell. Since
//! `plan_bootstrap` expects exactly one top-level root directory and maps it to
//! meclaw `/` (see `fs_to_meclaw_path`), the cell lives under `/cell` — hence
//! the routing target `/cell` and the trace filter `path_prefix=/cell`.
//!
//! NOT in the test scope: the `/ui/trace` HTML render (phase 12-D).

use meclaw_cli::{Cli, run_with_hooks};
use std::net::SocketAddr;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multipart_attachment_appears_in_colony_trace_entry() {
    // Bootstrap tree: the root directory `<tmp>/root` is the only top-level
    // cell hive (the bootstrap expects exactly one), and below it a real `bash`
    // cell at `<tmp>/root/cell` → meclaw path `/cell`.
    // The cell's existence in the registry is mandatory so that `route_with_log`
    // writes the `message_log` row (pre_routable check).
    let td = tempfile::TempDir::new().unwrap();
    let root_dir = td.path().join("root");
    let cell_dir = root_dir.join("cell");
    std::fs::create_dir_all(&cell_dir).unwrap();
    std::fs::write(root_dir.join("config.json"), br#"{"cell":{"type":"hive"}}"#).unwrap();
    std::fs::write(
        cell_dir.join("config.json"),
        br#"{"cell":{"type":"bash"},"params":{},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let blob_td = tempfile::TempDir::new().unwrap();
    let blob_root = blob_td.path().to_path_buf();

    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let cli = Cli {
        root: td.path().into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some(bind),
        daemon: false,
        validate: false,
        strict: false,
        blobs: Some(blob_root.clone()),
        tokio_console: false,
        tokio_console_port: 6669,
    };

    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });

    let actual_addr = addr_rx.await.unwrap();
    let client = reqwest::Client::new();

    // Hand-rolled multipart body (`reqwest` is configured here without the
    // `multipart` feature; same as T18 in `phase_12_x_multipart_upload.rs`).
    let boundary = "----E2EBoundary";
    let pdf_content = b"%PDF-1.4 e2e attachment payload";
    let prelude = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"target\"\r\n\r\n\
         /cell\r\n\
         --{boundary}\r\n\
         Content-Disposition: form-data; name=\"attachment\"; filename=\"report.pdf\"\r\n\
         Content-Type: application/pdf\r\n\r\n",
        boundary = boundary
    );
    let mut body_bytes = prelude.into_bytes();
    body_bytes.extend_from_slice(pdf_content);
    body_bytes.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let resp = client
        .post(format!("http://{actual_addr}/messages"))
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    let post_json: serde_json::Value = resp.json().await.unwrap();
    let attachments = post_json["attachments"]
        .as_array()
        .expect("attachments array in POST response");
    assert_eq!(attachments.len(), 1);
    let blob_id = attachments[0]["blob_id"]
        .as_str()
        .expect("blob_id string")
        .to_string();

    // Filesystem-Check: Blob + Sidecar liegen on disk.
    let blob_path = blob_root.join(format!("{blob_id}.pdf"));
    let sidecar_path = blob_root.join(format!("{blob_id}.pdf.meta.json"));
    assert!(blob_path.exists(), "missing blob file: {blob_path:?}");
    assert!(
        sidecar_path.exists(),
        "missing sidecar file: {sidecar_path:?}"
    );
    let on_disk = std::fs::read(&blob_path).unwrap();
    assert_eq!(on_disk, pdf_content);

    // Routing is async; wait briefly on the writer path until the
    // `message_log` row is written by `route_with_log`.
    let trace_entries = poll_for_trace_entry(&client, actual_addr, "/cell").await;
    assert!(
        !trace_entries.is_empty(),
        "expected at least one /colony/trace entry for path_prefix=/cell within 2s"
    );

    // DTO shape (`MessageLogDto`): `body_payload` is a JSON string
    // (Option<String>), not nested JSON. Parse it, then check `attachments[]`.
    // `body_kind` must be "inline" (no Body::Blob auto-offload).
    let our_entry = trace_entries
        .iter()
        .find(|e| e["to_path"].as_str() == Some("/cell"))
        .expect("entry with to_path=/cell");
    assert_eq!(our_entry["body_kind"], "inline");
    let body_payload_str = our_entry["body_payload"]
        .as_str()
        .expect("body_payload string");
    let body_json: serde_json::Value =
        serde_json::from_str(body_payload_str).expect("body_payload is valid JSON");
    let atts = body_json["attachments"]
        .as_array()
        .expect("attachments array in trace body");
    assert_eq!(atts.len(), 1);
    assert_eq!(atts[0]["mime_type"], "application/pdf");
    assert_eq!(atts[0]["filename"], "report.pdf");
    assert_eq!(atts[0]["blob_id"], blob_id);
    assert_eq!(atts[0]["size_bytes"], pdf_content.len() as u64);

    // Shutdown.
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), join)
        .await
        .expect("run_with_hooks must finish within 30s after shutdown")
        .expect("join handle must not panic")
        .expect("run_with_hooks must return Ok");
}

/// Polls `GET /colony/trace?path_prefix=<prefix>&limit=10` for up to ~2s for at
/// least one entry; returns the entries on a hit or at the deadline. Polling
/// instead of a fixed sleep, same as the phase-12-B demo.
async fn poll_for_trace_entry(
    client: &reqwest::Client,
    addr: SocketAddr,
    path_prefix: &str,
) -> Vec<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let resp = client
            .get(format!(
                "http://{addr}/colony/trace?path_prefix={path_prefix}&limit=10"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let json: serde_json::Value = resp.json().await.unwrap();
        let entries: Vec<serde_json::Value> = json["trace"].as_array().cloned().unwrap_or_default();
        if !entries.is_empty() {
            return entries;
        }
        if tokio::time::Instant::now() >= deadline {
            return entries;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
