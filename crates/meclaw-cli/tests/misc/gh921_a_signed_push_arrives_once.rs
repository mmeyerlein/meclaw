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
//! `/receipts`, never "the cell emitted". The secrets come sealed from a real
//! vault in the TempDir's tree under the cells' grants (GH #1235: a secret
//! param takes no `${VAR}`, `support/vault_grant.rs`); every file and every
//! captured log line is searched for them. The signature below is HMAC-SHA256 of [`SIGNED_BODY`] under
//! [`HOOK_SECRET`], computed once outside the test (Python `hmac`), so the test
//! needs no crypto crate of its own. The file is deliberately not named
//! `*_e2e` (see `gh617_a_lane_crosses_a_colony_boundary.rs`).

// harness_fold: #[path = "support/vault_grant.rs"]
#[allow(unused_imports)]
use crate::vault_grant; // harness_fold: mod vault_grant;

use meclaw_cli::{Cli, run_with_hooks};
use meclaw_testing::deadline;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};

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
        let _ = deadline::hang("the colony stops", self.join).await;
    }
}

const HOOK: vault_grant::Granted = vault_grant::Granted {
    cell: "hook",
    grant: "grant:probe-hook@colony-test/hook",
    cred_ref: "cred:probe-hook",
    secret: HOOK_SECRET,
};
const TOKEN: vault_grant::Granted = vault_grant::Granted {
    cell: "token",
    grant: "grant:probe-token@colony-test/token",
    cred_ref: "cred:probe-token",
    secret: TOKEN_SECRET,
};

/// Boots the fixture with a vault holding the secrets of `granted`, once
/// every one of their boxes opened.
async fn boot(granted: &[vault_grant::Granted]) -> Colony {
    let _ = captured_logs();
    let td = tempfile::TempDir::new().expect("tempdir");
    copy_dir_recursive(&fixture_path("gh921"), td.path());
    vault_grant::grow_vault(td.path(), cli_for(td.path()), granted).await;
    let cli = cli_for(td.path());
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
    let join =
        tokio::spawn(async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await });
    let addr = deadline::boot_hang("the colony binds HTTP", addr_rx)
        .await
        .expect("addr hook");
    vault_grant::wait_opened(&captured_logs(), granted.len()).await;
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

/// The rows under `prefix` from `GET /colony/trace`, and the log's commit
/// counter as it stood before the read. With `after_seq` the colony holds the
/// read until its log committed past it (GH #1099), so a wait is an event,
/// never a clock.
async fn trace(addr: &SocketAddr, prefix: &str, after_seq: Option<u64>) -> (Vec<Row>, u64) {
    let wait = after_seq
        .map(|s| format!("&after_seq={s}&wait_ms={}", deadline::HANG_MS))
        .unwrap_or_default();
    let url = format!("http://{addr}/colony/trace?path_prefix={prefix}&limit=1000{wait}");
    let page: Value = reqwest::get(&url)
        .await
        .expect("GET")
        .json()
        .await
        .expect("json");
    let entries = page["trace"].as_array().expect("trace");
    assert!(
        entries.len() < 1000,
        "a capped read read as the whole -- {url}"
    );
    let rows = entries
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
        .collect();
    (rows, page["log_seq"].as_u64().expect("log_seq"))
}

/// The rows under `prefix` as they stand now.
async fn rows(addr: &SocketAddr, prefix: &str) -> Vec<Row> {
    trace(addr, prefix, None).await.0
}

/// Waits until at least `n` rows stand under `prefix`: every read after the
/// first is held until the log commits again. The deadline is the hang mark,
/// never a verdict.
async fn wait_for_rows(addr: &SocketAddr, prefix: &str, n: usize) -> Vec<Row> {
    let desc = format!("{n} row(s) under {prefix}");
    deadline::hang(&desc, async {
        let mut seq = None;
        loop {
            let (r, s) = trace(addr, prefix, seq).await;
            if r.len() >= n {
                return r;
            }
            seq = Some(s);
        }
    })
    .await
}

/// GH #1501: the order the test needs, held by the log capture. When the
/// `probe-hook` mount's ready marker is written, the writing task is held
/// until the test released it -- after its first signed POST was answered.
/// The marker therefore has to stand where the mount is already open: a
/// marker written one hop before the secret reaches the mount answers that
/// POST `503 credential pending` every time, not only under load.
fn hold_on_open() -> &'static Mutex<Option<std::sync::mpsc::Receiver<()>>> {
    static HOLD: OnceLock<Mutex<Option<std::sync::mpsc::Receiver<()>>>> = OnceLock::new();
    HOLD.get_or_init(|| Mutex::new(None))
}

/// Arms [`hold_on_open`] once; sending on (or dropping) the sender releases it.
fn arm_hold_on_open() -> std::sync::mpsc::Sender<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    *hold_on_open().lock().expect("hold") = Some(rx);
    tx
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
        vault_grant::log_written().notify_waiters();
        // GH #1501: held after the line is in the capture and the waiter woke,
        // with no lock taken, so the rest of the process logs on.
        if find(b, b"received sealed and opened in RAM") && find(b, b"probe-hook") {
            let held = hold_on_open().lock().ok().and_then(|mut h| h.take());
            if let Some(release) = held {
                // Blocks the tokio worker that logs the line: the test runs
                // on four workers and the mount serves on its own tasks, so
                // the POST still gets through; the wait is only a hang mark.
                let _ = release.recv_timeout(deadline::scaled(deadline::HANG));
            }
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

/// Every file under the colony's root, `.env` included (GH #1235: it holds no
/// secret any more)
/// on purpose, and every captured log line: none contains a needle.
fn assert_nowhere(c: &Colony, needles: &[&str]) {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).expect("read_dir").flatten() {
            let p = e.path();
            if e.file_type().expect("file_type").is_dir() {
                walk(&p, out);
            } else {
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
    let release = arm_hold_on_open();
    let c = boot(&[HOOK, TOKEN]).await;
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
    let first = post(&a, "/probe-hook/", &signed, SIGNED_BODY).await;
    drop(release);
    assert_eq!(
        first, 202,
        "the ready marker stands where the mount is open"
    );
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
    let sink = wait_for_rows(&a, "/sink", 2).await;
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

/// GH #1235 (T6-22): a blank `${VAR}` no longer boots and the vault stores
/// no empty secret, so `secret_missing` has no road into a colony any more
/// (the cell-level locks keep it). What a mount without its secret is in a
/// colony now: a grant whose box never comes. It never opens -- every POST is
/// `503` with a `credential_pending` receipt on its lane -- and nothing
/// arrives (GH #1106).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grant_whose_box_never_comes_answers_503_and_delivers_nothing() {
    let c = boot(&[HOOK]).await;
    let a = c.addr;
    assert_eq!(post(&a, "/probe-token/", &[], "ping").await, 503);
    assert_eq!(
        post(&a, "/probe-token/", &[("X-Probe-Token", "")], "ping").await,
        503
    );
    let all = wait_for_rows(&a, "/receipts", 1).await;
    let pending: Vec<&Row> = all
        .iter()
        .filter(|r| r.hop["error_code"] == json!("credential_pending"))
        .collect();
    assert!(!pending.is_empty(), "the closed mount books its refusal");
    assert!(
        pending
            .iter()
            .all(|r| r.hop["lane"] == json!("probe-token")),
        "on its own lane"
    );
    assert!(
        rows(&a, "/sink").await.is_empty(),
        "nothing arrives through a mount without a secret"
    );
    c.stop().await;
}
