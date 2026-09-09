//! GH #640: a dead letter from a sibling hop is not a verdict on the turn.
//!
//! A trace is the whole conversation one turn sets off, not the turn alone. A
//! lane that emits on the side -- a memory write, a metric, a notification --
//! and finds nobody to consume it produces a `no_route` entry carrying the
//! trace id of that turn, while the answer is still on its way. `ask` read the
//! queue by trace id, so it reported the side emission as the fate of the turn
//! and exited 1 seconds before the answer arrived.
//!
//! The colony under test is written inline rather than committed as a fixture,
//! because its whole point is a timing shape, not a topology anyone would grow:
//!
//! ```text
//!                       .--(route memory)-->  nothing, a dead letter
//!   (a turn over HTTP) -> /door
//!                       `--(route work)---->  /slow --(route answer)--> /sink
//! ```
//!
//! `/slow` sleeps two seconds before it answers, so the dead letter is on the
//! record several polls before the answer is. Both halves are pinned here: the
//! sibling entry is waited out, and a target that does not exist -- the case
//! the queue is read for at all -- still ends the call at once.

use meclaw_cli::{Cli, run_with_hooks};
use std::net::SocketAddr;
use std::time::Duration;

/// How long `/slow` holds the answer back. Long enough that `ask` polls the
/// dead-letter queue several times first (the poll interval is 500 ms), short
/// enough that the test costs a fraction of its own timeout.
const SLOW_ANSWER_SECS: u64 = 2;

fn write_json(path: &std::path::Path, body: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write");
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

/// Writes the topology above into `root` and boots it.
struct Colony {
    addr: SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<anyhow::Result<()>>,
    _td: tempfile::TempDir,
}

impl Colony {
    async fn boot() -> Self {
        let td = tempfile::TempDir::new().expect("tempdir");
        let root = td.path();

        // Only the `work` lane has an out-edge. The `memory` emission the door
        // makes alongside it matches none, which is the `no_route` entry this
        // test is about.
        write_json(
            &root.join("main/config.json"),
            r#"{"cell":{"type":"hive"},
                "params":{"graph":{"edges":[
                  {"from":"./door","to":"./slow",
                   "condition":"has(hop.route) && hop.route == 'work'"},
                  {"from":"./slow","to":"./sink"}
                ]}}}"#,
        );

        // Two emissions from one turn: the side write, and the work the answer
        // comes out of.
        let door_script = r#"
import sys, json
body = json.load(sys.stdin)["body"]
said = ""
for turn in body.get("messages", []):
    if turn.get("origin") == "user":
        said = turn.get("text", "")
sys.stdout.write(json.dumps([
    {"header": {"route": "memory"},
     "messages": [{"origin": "assistant", "type": "text", "text": "remember: " + said}]},
    {"header": {"route": "work"},
     "messages": [{"origin": "user", "type": "text", "text": said}]},
]))
"#;
        write_json(
            &root.join("main/door/config.json"),
            &serde_json::json!({
                "cell": {"type": "code"},
                "params": {
                    "runner": "python3",
                    "script_inline": door_script,
                    "external_timeout_ms": 10000
                },
                "contract": {
                    "version": "1.0.0",
                    "settings": {},
                    "multi_send_capable": true,
                    "consumes": {},
                    "capabilities": ["shell:exec"]
                }
            })
            .to_string(),
        );

        let slow_script = format!(
            r#"
import sys, json, time
body = json.load(sys.stdin)["body"]
said = ""
for turn in body.get("messages", []):
    said = turn.get("text", "")
time.sleep({SLOW_ANSWER_SECS})
sys.stdout.write(json.dumps({{
    "header": {{"route": "answer"}},
    "messages": [{{"origin": "assistant", "type": "text", "text": "echo: " + said}}],
}}))
"#
        );
        write_json(
            &root.join("main/slow/config.json"),
            &serde_json::json!({
                "cell": {"type": "code"},
                "params": {
                    "runner": "python3",
                    "script_inline": slow_script,
                    "external_timeout_ms": 30000
                },
                "contract": {
                    "version": "1.0.0",
                    "settings": {},
                    "consumes": {},
                    "capabilities": ["shell:exec"]
                }
            })
            .to_string(),
        );

        write_json(
            &root.join("main/sink/config.json"),
            &serde_json::json!({
                "cell": {"type": "code"},
                "params": {
                    "runner": "python3",
                    "script_inline": "\nimport sys, json\nsys.stdin.read()\nsys.stdout.write(json.dumps([]))\n",
                    "external_timeout_ms": 10000
                },
                "contract": {
                    "version": "1.0.0",
                    "settings": {},
                    "multi_send_capable": true,
                    "consumes": {},
                    "capabilities": ["shell:exec"]
                }
            })
            .to_string(),
        );

        let cli = cli_for(root);
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

async fn ask(addr: &SocketAddr, target: &str, text: &str) -> Run {
    let args = vec![
        "--api".to_string(),
        addr.to_string(),
        "--target".to_string(),
        target.to_string(),
        "--timeout".to_string(),
        "30".to_string(),
        text.to_string(),
    ];
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_meclaw"))
            .current_dir(std::env::temp_dir())
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

async fn dead_letters(addr: &SocketAddr) -> Vec<serde_json::Value> {
    reqwest::Client::new()
        .get(format!("http://{addr}/colony/dead_letters?limit=1000"))
        .send()
        .await
        .expect("GET /colony/dead_letters")
        .json::<serde_json::Value>()
        .await
        .expect("json")["dead_letters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dead_letter_from_a_sibling_hop_does_not_end_the_wait() {
    let colony = Colony::boot().await;
    let run = ask(&colony.addr, "/door", "hello").await;
    let queue = dead_letters(&colony.addr).await;
    colony.stop().await;

    // The premise, asserted rather than assumed: the side emission really did
    // die while the turn was still being worked on.
    let sibling = queue
        .iter()
        .find(|d| d["error_code"] == "no_route")
        .unwrap_or_else(|| panic!("the memory lane must dead-letter; queue={queue:?}"));
    assert_eq!(
        sibling["sender_path"], "/door",
        "the entry belongs to the door's side emission: {sibling}"
    );

    assert_eq!(
        run.code, 0,
        "the answer decides, not a sibling's dead letter; stdout={:?} stderr={:?}",
        run.stdout, run.stderr
    );
    assert_eq!(
        run.stdout, "echo: hello\n",
        "and it is the answer that is printed; stderr={:?}",
        run.stderr
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dead_letter_on_the_posted_turn_still_ends_it_at_once() {
    let colony = Colony::boot().await;
    // The case the queue is read for at all: a target nobody has. The POST is
    // acknowledged with a 202 and the message dies in the router, so no answer
    // is ever coming and there is nothing to wait for.
    let run = ask(&colony.addr, "/dor", "hello").await;
    colony.stop().await;

    assert_eq!(
        run.code, 1,
        "a turn that was itself dead-lettered exits 1; stdout={:?} stderr={:?}",
        run.stdout, run.stderr
    );
    assert_eq!(
        run.stdout, "",
        "nothing was answered, so nothing is printed"
    );
    assert!(
        run.stderr.contains("dead-lettered") && run.stderr.contains("/dor"),
        "the message names the code and the address: {:?}",
        run.stderr
    );
}
