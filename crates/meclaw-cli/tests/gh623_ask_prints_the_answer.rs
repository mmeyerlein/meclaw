//! GH #623: `meclaw ask` — one turn to a running colony, the answer on stdout.
//!
//! The file is deliberately not named `*_e2e`: `scripts/gate_plan.py` subtracts
//! `binary(/e2e/)` as the scenario class, and a strand gate would then never run
//! the test that measures the change it is gating. This is a binary-against-a-
//! fixture test like `gh17_timer_trigger_http`, and it belongs in the tier that
//! runs.
//!
//! The quickstart used to need two `curl` calls and a four-line `jq` pipeline
//! over `/colony/trace` to send a turn and read the answer. `ask` is a client of
//! exactly those two routes, so the test drives the real binary against a real
//! colony and reads what a reader would read: stdout, and the exit code.
//!
//! The colony under test is the committed fixture `tests/fixtures/gh623-ask` —
//! a door that echoes and stamps `hop.route`, and a sink that ends the lane:
//!
//! ```text
//!   (a turn over HTTP) -> /door --(every emission)--> /sink
//! ```
//!
//! Deterministic by construction: no provider, no wall clock. The door answers
//! on route `answer`, or on route `error` when the text starts in `fail`, and a
//! turn addressed straight at `/sink` is never answered at all.

use meclaw_cli::{Cli, run_with_hooks};
use std::net::SocketAddr;
use std::time::Duration;

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Copies the committed fixture tree into a TempDir -- never boot in place
/// (`colony.db`/`cell.db` are created at runtime).
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
        env: None,
        templates: None,
        rescan_templates: false,
        api: Some("127.0.0.1:0".parse().expect("bind")),
        daemon: false,
        validate: false,
        validate_strict: false,
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

/// The booted fixture: its bind address, and the handles that stop it again.
struct Colony {
    addr: SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<anyhow::Result<()>>,
    _td: tempfile::TempDir,
}

impl Colony {
    async fn boot() -> Self {
        let td = tempfile::TempDir::new().expect("tempdir");
        copy_dir_recursive(&fixture_path("gh623-ask"), td.path());
        let cli = cli_for(td.path());
        let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
        let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
        let join =
            tokio::spawn(
                async move { run_with_hooks(cli, Some(addr_tx), Some(shutdown_rx)).await },
            );
        let addr = tokio::time::timeout(Duration::from_secs(30), addr_rx)
            .await
            .expect("the colony must bind HTTP within 30s")
            .expect("addr hook");
        Self {
            addr,
            shutdown,
            join,
            _td: td,
        }
    }

    async fn stop(self) {
        let _ = self.shutdown.send(());
        let _ = tokio::time::timeout(Duration::from_secs(30), self.join).await;
    }
}

/// What the real `meclaw` binary did: exit code, stdout, stderr.
struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Runs `meclaw ask <args>` as a subprocess. The binary is the one cargo just
/// built for this test target, so the exit code is the process's own, not a
/// library return value dressed up as one.
async fn ask(args: Vec<String>) -> Run {
    ask_in(std::path::Path::new("."), args).await
}

/// The same, run from a chosen working directory -- that is where a colony mode
/// would drop its `log.jsonl` and `colony.db`, and `ask` must not.
async fn ask_in(cwd: &std::path::Path, args: Vec<String>) -> Run {
    let cwd = cwd.to_path_buf();
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_meclaw"))
            .current_dir(&cwd)
            .arg("ask")
            .args(&args)
            .output()
            .expect("spawn meclaw ask")
    })
    .await
    .expect("join");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn ask_args(addr: &SocketAddr, target: &str, text: &str, extra: &[&str]) -> Vec<String> {
    let mut v = vec![
        "--api".to_string(),
        addr.to_string(),
        "--target".to_string(),
        target.to_string(),
    ];
    v.extend(extra.iter().map(|s| s.to_string()));
    v.push(text.to_string());
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_prints_the_answer_and_exits_zero() {
    let colony = Colony::boot().await;
    let run = ask(ask_args(&colony.addr, "/door", "hello", &[])).await;
    colony.stop().await;

    assert_eq!(
        run.code, 0,
        "an answered turn exits 0; stdout={:?} stderr={:?}",
        run.stdout, run.stderr
    );
    assert_eq!(
        run.stdout, "echo: hello\n",
        "the answer text, and nothing else, on stdout; stderr={:?}",
        run.stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_exits_one_when_the_answer_travels_on_route_error() {
    let colony = Colony::boot().await;
    let run = ask(ask_args(&colony.addr, "/door", "fail please", &[])).await;
    colony.stop().await;

    assert_eq!(
        run.code, 1,
        "a hop on route `error` exits 1; stdout={:?} stderr={:?}",
        run.stdout, run.stderr
    );
    assert_eq!(
        run.stdout, "echo: fail please\n",
        "the refusal is still printed, because it is the answer the colony gave"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_exits_two_when_no_answer_arrives_before_the_timeout() {
    let colony = Colony::boot().await;
    // `/sink` ends the lane: it emits nothing, so no hop ever carries a route.
    let run = ask(ask_args(
        &colony.addr,
        "/sink",
        "nobody answers this",
        &["--timeout", "2"],
    ))
    .await;
    colony.stop().await;

    assert_eq!(
        run.code, 2,
        "a turn nobody answers exits 2; stdout={:?} stderr={:?}",
        run.stdout, run.stderr
    );
    assert_eq!(
        run.stdout, "",
        "and it prints no answer, because there is none"
    );
    assert!(
        run.stderr.contains("2"),
        "the timeout says how long it waited: {:?}",
        run.stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_json_prints_the_trace_row_of_the_message_it_sent() {
    let colony = Colony::boot().await;
    let run = ask(ask_args(&colony.addr, "/door", "hello", &["--json"])).await;

    assert_eq!(
        run.code, 0,
        "stdout={:?} stderr={:?}",
        run.stdout, run.stderr
    );
    let row: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "--json must print one JSON object, got {:?}: {e}",
            run.stdout
        )
    });

    // The load-bearing property: `trace_id` survives the hop chain, so the
    // client can follow exactly the turn it sent instead of guessing from a
    // window of the log. The row `ask` returned belongs to the message the POST
    // acknowledged.
    let posted: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{}/colony/trace?limit=1000", colony.addr))
        .send()
        .await
        .expect("GET /colony/trace")
        .json()
        .await
        .expect("json");
    colony.stop().await;

    let source = posted["trace"]
        .as_array()
        .expect("trace")
        .iter()
        .find(|r| r["from_path"] == "@external")
        .expect("the ingress row");
    assert_eq!(
        row["trace_id"], source["id"],
        "the answer's trace_id is the id of the message that was posted"
    );
    assert_eq!(row["to_path"], "/sink", "the answer hop reached the sink");
    let headers: serde_json::Value =
        serde_json::from_str(row["headers_json"].as_str().expect("headers_json"))
            .expect("headers parse");
    assert_eq!(headers["hop"]["route"], "answer", "headers: {headers}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_exits_one_when_nothing_answers_on_that_address() {
    // Port 1 on loopback: nothing binds it, and the connection is refused
    // immediately. A transport failure is an error, not a timeout (R-0908-3).
    let cwd = tempfile::TempDir::new().expect("tempdir");
    let run = ask_in(
        cwd.path(),
        vec![
            "--api".into(),
            "127.0.0.1:1".into(),
            "--target".into(),
            "/door".into(),
            "hello".into(),
        ],
    )
    .await;

    assert_eq!(
        run.code, 1,
        "stdout={:?} stderr={:?}",
        run.stdout, run.stderr
    );
    assert_eq!(run.stdout, "", "nothing on stdout when nothing was asked");
    assert!(
        run.stderr.contains("127.0.0.1:1"),
        "the message names the address it could not reach: {:?}",
        run.stderr
    );
    let left: Vec<_> = std::fs::read_dir(cwd.path())
        .expect("read_dir")
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(
        left.is_empty(),
        "`ask` runs no colony: no log.jsonl, no colony.db, nothing. Found {left:?}"
    );
}
