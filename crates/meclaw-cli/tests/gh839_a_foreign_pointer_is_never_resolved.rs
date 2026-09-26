//! GH #839: a blob reference from another colony is never resolved against
//! this colony's store — measured at a real receiving colony.
//!
//! The south fixture (`tests/fixtures/peer-south`) boots in this process via
//! `run_with_hooks`, with its accept lane patched to NAME the two pointer keys
//! `messages[].text_id` and `messages[].messages_id`: the declaration is
//! exactly what must not unlock them. A blob sits in south's own store. A
//! frame whose turn points at that blob is posted straight at the peer mount
//! from loopback (the default `trusted_proxies`), carrying the identity header
//! the way the proxy in front would. Before the fix the frame crossed and the
//! delivery boundary of south (`cell_task.rs`, `resolve_blob_for_delivery`)
//! expanded the pointer into south's own text for the next cell. Now the
//! receipt is `refused` / `lane_body_unsupported`, nothing arrives under
//! `/sink`, and no row of the trace carries the blob's text.
//!
//! Helpers are the ones of `gh617_a_lane_crosses_a_colony_boundary.rs`, cut
//! to what one colony needs.

use meclaw_cli::{Cli, run_with_hooks};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;

/// The 30-second convention: a failure marker, never a budget.
const RECV: Duration = Duration::from_secs(30);

/// Test-only text, deliberately unlike anything else, so a hit is a leak.
const SOUTH_SECRET: &str = "south-own-text-839-glorp";

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("read_dir").flatten() {
        let to = dst.join(entry.file_name());
        if entry.file_type().expect("file_type").is_dir() {
            copy_dir_recursive(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("copy");
        }
    }
}

fn cli_for(root: &std::path::Path) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        log_stderr: meclaw_cli::LogSink::Auto,
        log_file: meclaw_cli::LogSink::Auto,
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some("127.0.0.1:0".parse().expect("bind")),
        daemon: false,
        validate: false,
        validate_strict: false,
        env_report: false,
        apply: None,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6688,
        sandbox_probe: false,
        vault: None,
        vault_add: None,
        vault_status: false,
        vault_revoke: None,
        vault_key_source: "auto".to_string(),
        vault_key_file: None,
        stdio_format: meclaw_cli::StdioFormat::Text,
        command: None,
    }
}

struct Colony {
    addr: SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<anyhow::Result<()>>,
    _td: tempfile::TempDir,
}

impl Colony {
    async fn stop(self) {
        let _ = self.shutdown.send(());
        let _ = tokio::time::timeout(Duration::from_secs(30), self.join).await;
    }
}

/// South with its accept lane naming the pointer keys, and one blob in its
/// own store (the store the colony reads, `<root>/blobs`). Returns the colony
/// and the blob's id.
async fn south_with_a_blob() -> (Colony, String) {
    let td = tempfile::TempDir::new().expect("tempdir");
    copy_dir_recursive(&fixture_path("peer-south"), td.path());
    let cfg_path = td.path().join("main/friend/config.json");
    let mut cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).expect("read")).expect("json");
    cfg["params"]["lanes"]["accepts"][0]["fields"] = json!([
        "topic",
        "messages[].text_id",
        "messages[].messages_id",
        "messages[].origin",
        "messages[].type",
        "messages[].text"
    ]);
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).expect("ser")).expect("write");
    let store = meclaw_colony::DiskBlobStore::new(td.path().join("blobs")).expect("store");
    let doc = json!({"messages": [{"origin": "user", "type": "text", "text": SOUTH_SECRET}]});
    let blob_id = store
        .write_streaming(
            std::io::Cursor::new(serde_json::to_vec(&doc).expect("bytes")),
            "application/json",
            None,
        )
        .await
        .expect("blob")
        .blob_id
        .to_string();
    let cli = cli_for(td.path());
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });
    let addr = tokio::time::timeout(RECV, addr_rx)
        .await
        .expect("the colony must bind HTTP within 30s")
        .expect("addr hook");
    (
        Colony {
            addr,
            shutdown,
            join,
            _td: td,
        },
        blob_id,
    )
}

/// Every row of the trace under `prefix`, raw: the whole entry as text, so a
/// search for the secret covers every column the log keeps.
async fn raw_rows(addr: &SocketAddr, trace: &str, prefix: &str) -> Vec<String> {
    let url = format!(
        "http://{addr}/colony/messages?trace_id={trace}&to_path_prefix={prefix}&limit=1000"
    );
    let page: Value = reqwest::get(&url)
        .await
        .expect("GET")
        .json()
        .await
        .expect("json");
    assert_eq!(
        page["scan_truncated"],
        json!(false),
        "a truncated scan reads as 'nothing matched' -- {url}"
    );
    page["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(Value::to_string)
        .collect()
}

/// One frame straight at south's peer mount, as the proxy in front would
/// forward it; the answer IS the receipt.
///
/// The HTTP bind is reported before the peer cell has registered its mount, so
/// the first POST can meet a `404` (no such mount yet): measured twice under a
/// loaded test run, never with the binary alone. A `404` is retried until the
/// mount is there; once it is, every verdict is a `200`, refused or not.
async fn post(addr: &SocketAddr, frame: &Value) -> Value {
    let deadline = tokio::time::Instant::now() + RECV;
    let resp = loop {
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/peer/"))
            .header("Content-Type", "application/json")
            .header("X-Meclaw-Peer", "north")
            .body(frame.to_string())
            .send()
            .await
            .expect("POST /peer/");
        if resp.status().as_u16() != 404 || tokio::time::Instant::now() >= deadline {
            break resp;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        resp.status().as_u16(),
        200,
        "a verdict is an answer, refused or not"
    );
    resp.json().await.expect("receipt json")
}

fn frame(trace: &str, turn: Value) -> Value {
    json!({"v": 1, "type": "message", "lane": "topic", "trace_id": trace, "ttl": 8,
        "context": {}, "body": {"messages": [turn], "topic": "gardening"}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pointer_into_the_receivers_store_is_refused_and_never_resolved() {
    let (south, blob_id) = south_with_a_blob().await;
    for turn in [
        // The two pointer forms exactly as the UBF schema defines them
        // (`TurnPointer`, `BulkPointer`): a mixed turn would already fail the
        // mount's own body check (`invalid_frame`) and prove nothing here.
        json!({"text_id": blob_id}),
        json!({"messages_id": blob_id}),
    ] {
        let trace = uuid::Uuid::now_v7().to_string();
        let r = post(&south.addr, &frame(&trace, turn.clone())).await;
        assert_eq!(
            (r["result"].clone(), r["error_code"].clone()),
            (json!("refused"), json!("lane_body_unsupported")),
            "{turn}: {r}"
        );
        // Booked on south's side under the same code, next to which nothing arrived.
        let deadline = tokio::time::Instant::now() + RECV;
        let receipts = loop {
            let r = raw_rows(&south.addr, &trace, "/receipts").await;
            if !r.is_empty() {
                break r;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "south books its refusal within 30s"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert!(
            receipts[0].contains("lane_body_unsupported"),
            "{}",
            receipts[0]
        );
        assert!(
            raw_rows(&south.addr, &trace, "/sink").await.is_empty(),
            "nothing arrived -- read NEXT TO the receipt above"
        );
        for row in raw_rows(&south.addr, &trace, "/").await {
            assert!(
                !row.contains(SOUTH_SECRET),
                "no row of the trace carries south's own text: {row}"
            );
        }
    }
    // The same mount still takes an ordinary turn on the same lane.
    let trace = uuid::Uuid::now_v7().to_string();
    let ok = post(
        &south.addr,
        &frame(
            &trace,
            json!({"origin": "user", "type": "text", "text": "plain"}),
        ),
    )
    .await;
    assert_eq!(ok["result"], json!("crossed"), "{ok}");
    south.stop().await;
}
