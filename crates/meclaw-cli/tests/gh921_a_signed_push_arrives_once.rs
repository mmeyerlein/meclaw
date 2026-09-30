//! GH #921: a signed push arrives once, an unsigned one nowhere.
//!
//! One real colony runs in this test process through `run_with_hooks` on
//! `127.0.0.1:0`; the fixture `tests/fixtures/gh921` holds two `proxy` cells on
//! `platform: "webhook"` whose mounts sit on that one listener:
//!
//! ```text
//!   POST /probe-hook/  (hmac_sha256)  -> /hook  --hop.route 'probe'-------> /sink
//!   POST /probe-token/ (token)        -> /token --hop.route 'probe-token'-> /sink
//!   /hook, /token --hop.route 'receipt'--> /receipts
//! ```
//!
//! Measured at the receiver: rows of `GET /colony/messages` under `/sink` and
//! `/receipts`, never "the cell emitted". The secrets live only in the
//! TempDir's `.env`; every other file and every captured log line is searched
//! for them. The signature below is HMAC-SHA256 of [`SIGNED_BODY`] under
//! [`HOOK_SECRET`], computed once outside the test (Python `hmac`), so the test
//! needs no crypto crate of its own. The file is deliberately not named
//! `*_e2e` (see `gh617_a_lane_crosses_a_colony_boundary.rs`).

use meclaw_cli::{Cli, run_with_hooks};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The 30-second convention: a failure marker, never a budget.
const RECV: Duration = Duration::from_secs(30);

const HOOK_SECRET: &str = "probe-secret-921-xq";
const TOKEN_SECRET: &str = "probe-token-921-zv";
const SIGNED_BODY: &str = r#"{"action":"opened","number":7}"#;
/// `hmac.new(HOOK_SECRET, SIGNED_BODY, sha256).hexdigest()`.
const SIGNATURE: &str = "6db48652e9f90e551790e5f20f6d051dba37d2174a6a3638ddc4e6fe4790ca23";

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Copies the committed fixture tree into a TempDir -- never boot in place.
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
    td: tempfile::TempDir,
}

impl Colony {
    async fn stop(self) {
        let _ = self.shutdown.send(());
        let _ = tokio::time::timeout(RECV, self.join).await;
    }
}

/// Boots the fixture with `env` as its `.env`.
async fn boot(env: &[(&str, &str)]) -> Colony {
    let td = tempfile::TempDir::new().expect("tempdir");
    copy_dir_recursive(&fixture_path("gh921"), td.path());
    let lines: String = env.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(td.path().join(".env"), lines).expect("write env");
    let cli = cli_for(td.path());
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });
    let addr = tokio::time::timeout(RECV, addr_rx)
        .await
        .expect("the colony must bind HTTP within 30s")
        .expect("addr hook");
    Colony {
        addr,
        shutdown,
        join,
        td,
    }
}

/// One row of `GET /colony/messages`: its hop and its body.
struct Row {
    hop: Value,
    body: Value,
}

async fn rows(addr: &SocketAddr, prefix: &str) -> Vec<Row> {
    let url = format!("http://{addr}/colony/messages?to_path_prefix={prefix}&limit=1000");
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
        .map(|e| {
            let h: Value =
                serde_json::from_str(e["headers_json"].as_str().unwrap_or("{}")).expect("headers");
            let body = e["body_payload"]
                .as_str()
                .map(|s| serde_json::from_str(s).expect("body"))
                .unwrap_or(Value::Null);
            Row {
                hop: h.get("hop").cloned().unwrap_or(json!({})),
                body,
            }
        })
        .collect()
}

/// Reads `rows` every 50 ms until at least `n` are there. The test is a client
/// outside the substrate, so polling is its to do.
async fn wait_for_rows(addr: &SocketAddr, prefix: &str, n: usize) -> Vec<Row> {
    let deadline = tokio::time::Instant::now() + RECV;
    loop {
        let r = rows(addr, prefix).await;
        if r.len() >= n {
            return r;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{n} row(s) under {prefix} within 30s -- saw {}",
            r.len()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// One POST to a mount; the status code.
async fn post(addr: &SocketAddr, path: &str, headers: &[(&str, &str)], body: &str) -> u16 {
    let mut req = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .body(body.to_string());
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.send().await.expect("POST").status().as_u16()
}

/// Every `tracing` event this process emits at `debug` and above; see the same
/// helper in `gh617_a_lane_crosses_a_colony_boundary.rs` for what it misses.
fn captured_logs() -> Arc<Mutex<Vec<u8>>> {
    static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    LOGS.get_or_init(|| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&buf);
        let sub = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("debug"))
            .with_ansi(false)
            .with_writer(move || LogWriter(Arc::clone(&sink)))
            .finish();
        let _ = tracing::subscriber::set_global_default(sub);
        buf
    })
    .clone()
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut v) = self.0.lock() {
            v.extend_from_slice(b);
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn find(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Every file under the colony's root except the `.env` that holds the secrets
/// on purpose, and every captured log line: none contains a needle.
fn assert_nowhere(c: &Colony, needles: &[&str]) {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).expect("read_dir").flatten() {
            let p = e.path();
            if e.file_type().expect("file_type").is_dir() {
                walk(&p, out);
            } else if p.file_name().is_some_and(|n| n != ".env") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(c.td.path(), &mut files);
    assert!(
        files.iter().any(|f| f.ends_with("colony.db")),
        "the search covers the message log"
    );
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        for n in needles {
            assert!(
                !find(&bytes, n.as_bytes()),
                "a secret stands in {}",
                f.display()
            );
        }
    }
    let logs = captured_logs().lock().expect("logs").clone();
    assert!(!logs.is_empty(), "the log capture saw lines");
    for n in needles {
        assert!(!find(&logs, n.as_bytes()), "a secret stands in a log line");
    }
}

/// The receipts under `/receipts` with `peer_event == event` and, for a
/// refusal, `error_code == code`, on lane `lane`.
fn receipts<'a>(all: &'a [Row], lane: &str, event: &str) -> Vec<&'a Row> {
    all.iter()
        .filter(|r| r.hop["lane"] == json!(lane) && r.hop["peer_event"] == json!(event))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signed_push_arrives_once_and_an_unsigned_one_nowhere() {
    let _ = captured_logs();
    let c = boot(&[
        ("PROBE_HOOK_SECRET", HOOK_SECRET),
        ("PROBE_TOKEN_SECRET", TOKEN_SECRET),
    ])
    .await;
    let a = c.addr;
    let sig = format!("sha256={SIGNATURE}");
    let json_ct = ("content-type", "application/json");

    // The HMAC mount: one signed POST with three headers, two of them allowed.
    let signed = [
        json_ct,
        ("X-Probe-Signature", sig.as_str()),
        ("X-Probe-Event", "push"),
        ("X-Probe-Delivery", "d-921-1"),
        ("X-Probe-Other", "not-allowed"),
    ];
    assert_eq!(post(&a, "/probe-hook/", &signed, SIGNED_BODY).await, 202);
    // Unsigned, wrongly signed, signed over other bytes: 401 each.
    assert_eq!(post(&a, "/probe-hook/", &[json_ct], SIGNED_BODY).await, 401);
    let wrong = format!("sha256={}", "0".repeat(64));
    let bad_sig = [json_ct, ("X-Probe-Signature", wrong.as_str())];
    assert_eq!(post(&a, "/probe-hook/", &bad_sig, SIGNED_BODY).await, 401);
    let tampered = r#"{"action":"closed","number":7}"#;
    assert_eq!(post(&a, "/probe-hook/", &signed, tampered).await, 401);
    // Another method reads no body and gets no verdict.
    let get = reqwest::get(format!("http://{a}/probe-hook/"))
        .await
        .expect("GET");
    assert_eq!(get.status().as_u16(), 405);
    // Past the 1 KiB limit: 413, before any verification.
    let big = "x".repeat(2048);
    assert_eq!(post(&a, "/probe-hook/", &signed, &big).await, 413);

    // The token mount behaves the same.
    let tok = [("X-Probe-Token", TOKEN_SECRET), ("X-Probe-Event", "sync")];
    assert_eq!(post(&a, "/probe-token/sub/path", &tok, "ping").await, 202);
    let wrong_tok = [("X-Probe-Token", "probe-token-921-zw")];
    assert_eq!(post(&a, "/probe-token/", &wrong_tok, "ping").await, 401);
    assert_eq!(post(&a, "/probe-token/", &[], "ping").await, 401);

    // Receipts: one crossed and three refused on `probe`, one crossed and two
    // refused on `probe-token`. They are emitted after the arrivals of the
    // same requests, so once all are there, every arrival has been routed.
    let all = wait_for_rows(&a, "/receipts", 7).await;
    assert_eq!(receipts(&all, "probe", "crossed").len(), 1);
    assert_eq!(receipts(&all, "probe-token", "crossed").len(), 1);
    for (lane, n) in [("probe", 3), ("probe-token", 2)] {
        let refused = receipts(&all, lane, "refused");
        assert_eq!(refused.len(), n, "{lane}");
        for r in refused {
            assert_eq!(r.hop["error_code"], json!("webhook_unverified"));
            assert_eq!(r.hop["route"], json!("receipt"));
        }
    }

    // The arrivals: exactly one per lane, and nothing of a refused request.
    let sink = rows(&a, "/sink").await;
    assert_eq!(sink.len(), 2, "one arrival per verified request, no more");
    let hook: Vec<&Row> = sink
        .iter()
        .filter(|r| r.hop["route"] == json!("probe"))
        .collect();
    assert_eq!(hook.len(), 1, "the signed push arrives exactly once");
    let w = &hook[0].body["webhook"];
    assert_eq!(hook[0].body["messages"], json!([]), "no conversation turn");
    assert_eq!(w["raw"], json!(SIGNED_BODY), "the raw body, byte for byte");
    assert_eq!(w["json"], json!({"action": "opened", "number": 7}));
    assert_eq!(w["content_type"], json!("application/json"));
    assert_eq!(w["path"], json!(""));
    assert_eq!(
        w["headers"],
        json!({"x-probe-event": "push", "x-probe-delivery": "d-921-1"}),
        "only the allowed headers, lower-case"
    );

    let token: Vec<&Row> = sink
        .iter()
        .filter(|r| r.hop["route"] == json!("probe-token"))
        .collect();
    assert_eq!(token.len(), 1);
    let w = &token[0].body["webhook"];
    assert_eq!(w["raw"], json!("ping"));
    assert_eq!(w.get("json"), None, "not application/json: no json slot");
    assert_eq!(w["path"], json!("sub/path"));
    assert_eq!(w["headers"], json!({"x-probe-event": "sync"}));

    assert_nowhere(&c, &[HOOK_SECRET, TOKEN_SECRET]);
    c.stop().await;
}

/// A secret bound blank never opens the mount: every POST is `503`, and the
/// cell books `secret_missing` once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blank_secret_answers_503_and_is_booked_once() {
    let c = boot(&[
        ("PROBE_HOOK_SECRET", HOOK_SECRET),
        ("PROBE_TOKEN_SECRET", ""),
    ])
    .await;
    let a = c.addr;
    assert_eq!(post(&a, "/probe-token/", &[], "ping").await, 503);
    assert_eq!(
        post(&a, "/probe-token/", &[("X-Probe-Token", "")], "ping").await,
        503
    );
    let all = wait_for_rows(&a, "/receipts", 1).await;
    let missing: Vec<&Row> = all
        .iter()
        .filter(|r| r.hop["error_code"] == json!("secret_missing"))
        .collect();
    assert_eq!(missing.len(), 1, "booked once per life, not per request");
    assert_eq!(missing[0].hop["lane"], json!("probe-token"));
    assert!(
        rows(&a, "/sink").await.is_empty(),
        "nothing arrives through a mount without a secret"
    );
    c.stop().await;
}
