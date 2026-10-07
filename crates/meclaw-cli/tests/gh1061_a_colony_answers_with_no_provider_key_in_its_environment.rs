//! GH #1061 (#801), lock T4 — a colony answers with no provider key in its
//! environment.
//!
//! #801 retires the `${…_KEY}` road: every secret a cell spends arrives SEALED
//! from the vault over a credential grant, so a colony process must work with
//! no secret in its environment and none in `{root}/.env`. This lock runs the
//! REAL `meclaw` binary as a child process with `Command::env_clear()` — only
//! `PATH`, `HOME`, `TMPDIR` and `CREDENTIALS_DIRECTORY` (the directory of the
//! vault's key file, the systemd contract) survive — and proves that every
//! consumer class still authenticates:
//!
//! | consumer | cell | what the stub provider sees |
//! |---|---|---|
//! | `/brain` | `llm` (#1058) | `Authorization: Bearer stub-secret-1` on the chat call |
//! | `/search` | `web_search` (#1060) | `Authorization: Bearer stub-secret-2` on the query |
//! | `/embed` | `code` (#1060) | `Authorization: Bearer stub-secret-3`, sent by the script from its env entry |
//! | `/tg` | `proxy` Telegram (#1059) | `stub-secret-4` in the URL path (`/bot<token>/getUpdates`) |
//! | `/voice` | `voice`, Deepgram slot (#1059) | `stub-secret-5` in the `Authorization` header of the STT upgrade |
//!
//! HOW THE TREE IS MADE. The five consumers stand on disk (copied from the
//! shipped single-cell templates where there is one, pointed at loopback stubs
//! by their `base_url`/`endpoint` params, each naming its grant). ONE manifest,
//! applied with `meclaw --apply`, grows the `access` hive (vault on
//! `key_source: "systemd-cred"`) in its first entry and, in its second, draws
//! the two v-lanes per consumer and seeds the grants through `seed_rows` — the
//! form of `templates/builder/recipes` `_credential_edges`/`_credential_rows`.
//! The secrets go in ONLY through `meclaw --vault-add` (stdin,
//! `--vault-key-source plainfile --vault-key-file`), between the one-shot apply
//! and the daemon, so no colony holds the lease.
//!
//! OR-VG.V4.D1 — the voice claim is the strongest observable short of a live
//! call: a client opens a session on the mount, and the measure is the
//! provider connection the STT slot makes with the delivered key (the stub
//! answers the upgrade with `404`, as in `gh1059_voice_grant.rs`). No audio is
//! transcribed.
//!
//! No stub value may appear in the daemon's stdout/stderr, `{root}/log.jsonl`
//! or the colony's `message_log`.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! shipped templates, or a host without `python3` (the broker's cells are
//! Python), is skipped, never judged.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Failure marker (30 s convention), never a timing claim.
const MARKER: Duration = Duration::from_secs(30);
const PASSPHRASE: &str = "stub-secret-passphrase-1061";
const VAULT: &str = "/main/access/vault";
/// The colony's subject in every grant row.
const SUBJECT: &str = "colony:t4";

/// One consumer: its cell name, the vault name of its credential, the value.
struct Consumer {
    cell: &'static str,
    cred_ref: &'static str,
    secret: &'static str,
}

const CONSUMERS: [Consumer; 5] = [
    Consumer {
        cell: "brain",
        cred_ref: "cred:t4-llm",
        secret: "stub-secret-1",
    },
    Consumer {
        cell: "search",
        cred_ref: "cred:t4-search",
        secret: "stub-secret-2",
    },
    Consumer {
        cell: "embed",
        cred_ref: "cred:t4-embed",
        secret: "stub-secret-3",
    },
    Consumer {
        cell: "tg",
        cred_ref: "cred:t4-telegram",
        secret: "stub-secret-4",
    },
    Consumer {
        cell: "voice",
        cred_ref: "cred:t4-deepgram",
        secret: "stub-secret-5",
    },
];

fn grant(cell: &str) -> String {
    format!("grant:t4-{cell}@colony-t4/{cell}")
}

fn requester(cell: &str) -> String {
    format!("agent:t4/{cell}")
}

// ─────────────────────────────────────────────────────────────── the stub

/// One request a stub saw: its request target and every header (lowercased).
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: BTreeMap<String, String>,
}

/// A loopback provider on std threads (it needs nothing from any runtime) that
/// answers every request with one canned response and records what came in.
struct Stub {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Stub {
    fn start(status: &'static str, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let addr = listener.local_addr().expect("stub address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::clone(&seen);
        let body = Arc::new(body);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let seen = Arc::clone(&shared);
                let body = Arc::clone(&body);
                std::thread::spawn(move || serve(conn, &seen, status, &body));
            }
        });
        Self { addr, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn header_values(&self, name: &str) -> Vec<String> {
        self.seen()
            .into_iter()
            .filter_map(|s| s.headers.get(name).cloned())
            .collect()
    }
}

fn serve(mut conn: TcpStream, seen: &Mutex<Vec<Seen>>, status: &str, body: &str) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let path = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or_default()
        .to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while buf.len() < head_end + length {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    seen.lock().unwrap().push(Seen { path, headers });
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = conn.write_all(reply.as_bytes());
}

// ─────────────────────────────────────────────────────────── the material

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// GH #49: the shipped templates this lock copies, and the interpreter the
/// broker's cells run on.
fn shipped() -> bool {
    let files = [
        "templates/access/template.json",
        "templates/access/vault/config.json",
        "templates/telegram-connector/config.json",
        "templates/voice/config.json",
        "templates/_cell-types/web_search-min/config.json",
    ];
    files.iter().all(|f| repo(f).is_file())
        && Command::new("python3")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write_json(p: &Path, v: &Value) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// A shipped single-cell template's `config.json`, its params patched.
fn shipped_cell(rel: &str, patch: Value) -> Value {
    let mut cfg = read_json(&repo(rel));
    for (k, v) in patch.as_object().unwrap() {
        cfg["params"][k] = v.clone();
    }
    cfg
}

/// The python the `/embed` code cell runs: it reads its credential from the
/// environment entry the substrate sets for THIS run and presents it as a
/// bearer — the shape of the shipped embed scripts, without their store work.
fn embed_script(url: &str) -> String {
    format!(
        r#"
import json, os, sys, urllib.request
sys.stdin.read()
key = os.environ.get("T4_EMBED_KEY", "")
req = urllib.request.Request("{url}", data=b"{{}}", method="POST",
    headers={{"Authorization": "Bearer " + key, "Content-Type": "application/json"}})
status = urllib.request.urlopen(req, timeout=10).status
print(json.dumps({{"header": {{}}, "messages": [{{"origin": "tool", "type": "tool_result",
    "id": "", "text": json.dumps({{"status": status}})}}]}}))
"#
    )
}

struct Stubs {
    llm: Stub,
    search: Stub,
    embed: Stub,
    telegram: Stub,
    deepgram: Stub,
}

impl Stubs {
    fn start() -> Self {
        let chat = json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1,
            "model": "gpt-4o-mini",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": "pong"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        });
        Self {
            llm: Stub::start("200 OK", chat.to_string()),
            search: Stub::start(
                "200 OK",
                r#"{"results":[{"title":"A","url":"u1","snippet":"s1"}]}"#.into(),
            ),
            embed: Stub::start("200 OK", "{}".into()),
            telegram: Stub::start("200 OK", r#"{"ok":true,"result":[]}"#.into()),
            deepgram: Stub::start("404 Not Found", String::new()),
        }
    }
}

/// The colony root: a root hive, the five consumers on disk, the `access`
/// template in the library, and a `.env` that names nothing secret.
fn build_root(root: &Path, stubs: &Stubs) {
    write_json(
        &root.join("main/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": []}}}),
    );
    copy_tree(
        &repo("templates/access"),
        &root.join("templates").join("access"),
    );
    std::fs::write(root.join(".env"), "GH1061_NOTE=nothing-secret-here\n").unwrap();
    let main = root.join("main");

    write_json(
        &main.join("brain/config.json"),
        &json!({
            "cell": {"type": "llm"},
            "params": {
                "provider": "openai", "model": "gpt-4o-mini", "api_key": "",
                "base_url": format!("http://{}/v1", stubs.llm.addr),
                "credential_grant_id": grant("brain"),
                "external_timeout_ms": 10_000
            },
            "contract": {
                "version": "1.0.0", "settings": {},
                "emits": {
                    "body": {"messages": {"type": "array", "required": false},
                             "meta": {"type": "object", "required": false}},
                    "hop": {"route": {"type": "string", "values": ["credential_request"],
                                      "required": false},
                            "grant_id": {"type": "string", "required": false},
                            "error_code": {"type": "string", "required": false},
                            "finish_reason": {"type": "string", "required": false}}
                },
                "consumes": {"body": {"messages": {"type": "array", "required": false},
                                      "sealed": {"type": "object", "required": false}}},
                "capabilities": ["network:llm", "db:own"]
            }
        }),
    );

    write_json(
        &main.join("search/config.json"),
        &shipped_cell(
            "templates/_cell-types/web_search-min/config.json",
            json!({"endpoint": format!("http://{}/", stubs.search.addr),
                   "api_key": "", "credential_grant_id": grant("search")}),
        ),
    );

    write_json(
        &main.join("embed/config.json"),
        &json!({
            "cell": {"type": "code"},
            "params": {
                "runner": "python3",
                "script_inline": embed_script(&format!("http://{}/embed", stubs.embed.addr)),
                "credential_grant_id": grant("embed"),
                "credential_env": "T4_EMBED_KEY",
                "external_timeout_ms": 15_000
            },
            "contract": {
                "version": "1.0.0", "settings": {},
                "consumes": {"body": {"messages": {"type": "array", "required": false},
                                      "sealed": {"type": "object", "required": false}}}
            }
        }),
    );

    write_json(
        &main.join("tg/config.json"),
        &shipped_cell(
            "templates/telegram-connector/config.json",
            json!({"base_url": format!("http://{}", stubs.telegram.addr),
                   "bot_token": "", "bot_token_grant_id": grant("tg"),
                   "credential_wait_ms": 2_000, "emit_to": "/brain",
                   "long_poll_request_secs": 1, "long_poll_timeout_ms": 2_000}),
        ),
    );

    write_json(
        &main.join("voice/config.json"),
        &shipped_cell(
            "templates/voice/config.json",
            json!({"mount": "t4voice",
                   "stt": {"provider": "deepgram",
                           "base_url": format!("ws://{}", stubs.deepgram.addr),
                           "credential_grant_id": grant("voice"),
                           "credential_wait_ms": 2_000},
                   // A voice cell must be able to speak unless it echoes
                   // (`tts: required unless stt.provider is "echo"`). The tts
                   // slot shares the stt grant: one box opens both, and the
                   // handler asks once per round (review V2 M2).
                   "tts": {"provider": "openai",
                           "base_url": format!("http://{}", stubs.deepgram.addr),
                           "credential_grant_id": grant("voice"),
                           "credential_wait_ms": 2_000},
                   "duplex": null}),
        ),
    );
}

/// The one manifest: grow `access` (vault self-unlocking from the systemd
/// credential), then — scoped at the same hive, one entry later — the ask and
/// answer lane of every consumer and its grant through the mutation door.
fn manifest(root: &Path) -> PathBuf {
    let mut edges = Vec::new();
    let mut grants = Vec::new();
    let mut events = Vec::new();
    for c in &CONSUMERS {
        let at = format!("./{}", c.cell);
        edges.push(json!({
            "from": at, "to": "./access",
            "condition": "has(hop.route) && hop.route == 'credential_request'",
            "modifier": {"set_hop": {"route": "'in_invoke'"},
                         "set_context": {"requester": format!("'{}'", requester(c.cell))}}
        }));
        edges.push(json!({
            "from": "./access", "to": at,
            "condition": format!(
                "has(hop.route) && hop.route == 'ack' && has(hop.operation) && \
                 hop.operation == 'vault.deliver' && has(hop.grant_id) && \
                 hop.grant_id == '{}'", grant(c.cell)),
            "modifier": {"set_hop": {"route": "'in_sealed'"}}
        }));
        grants.push(json!({
            "grant_id": grant(c.cell), "requester": requester(c.cell),
            "capability": "credential.read", "subject": SUBJECT,
            "scope": {"actions": ["vault.deliver"]}, "cred_ref": c.cred_ref,
            "purpose": format!("authenticate /{} against its stub provider", c.cell),
            "issued_at": "2026-10-07T00:00:00.000000Z",
            "expires_at": "2099-01-01T00:00:00.000000Z",
            "rule_id": "credential-read", "constraints": {"rate_per_min": 60}
        }));
        events.push(json!({
            "id": format!("ev-t4-{}", c.cell), "grant_id": grant(c.cell),
            "event": "granted", "at": "2026-10-07T00:00:00.000000Z",
            "actor": "operator", "reason_code": "",
            "detail": {"why": "seeded with the lab colony (#1061)"}
        }));
    }
    let m = json!({"manifest": [
        {"scope": "/", "diff": {"add_nodes": [{
            "name": "access", "template": "access",
            "override_params": {"vault": {"key_source": "systemd-cred"}}
        }]}},
        {"scope": "/", "diff": {
            "add_edges": edges,
            "seed_rows": [
                {"target": "./access/store", "table": "grants", "rows": grants},
                {"target": "./access/store", "table": "grant_events", "rows": events}
            ]
        }}
    ]});
    let p = root.join("grow-t4.json");
    write_json(&p, &m);
    p
}

// ─────────────────────────────────────────────────────────── the process

/// Everything a child process gets: no inherited variable at all.
struct Env {
    home: tempfile::TempDir,
    creds: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let creds = tempfile::TempDir::new().unwrap();
        let key = creds.path().join("vault_key");
        std::fs::write(&key, format!("{PASSPHRASE}\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            home: tempfile::TempDir::new().unwrap(),
            creds,
        }
    }

    fn key_file(&self) -> PathBuf {
        self.creds.path().join("vault_key")
    }

    fn command(&self, root: &Path) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_meclaw"));
        cmd.env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            )
            .env("HOME", self.home.path())
            .env("TMPDIR", std::env::temp_dir())
            .env("CREDENTIALS_DIRECTORY", self.creds.path())
            .current_dir(root)
            .arg("--root")
            .arg(root);
        cmd
    }
}

/// A child this test owns; dropping it ends the process (by its pid, never by
/// name — a live colony on this host runs the same binary).
struct Owned(Option<Child>);

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Run to completion within the marker; return (success, stdout + stderr).
/// Blocking: called through [`run_async`].
fn run(mut cmd: Command, stdin: Option<String>) -> (bool, String) {
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn meclaw");
    if let Some(s) = stdin {
        child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    }
    let mut owned = Owned(Some(child));
    let deadline = Instant::now() + MARKER;
    loop {
        let c = owned.0.as_mut().unwrap();
        if c.try_wait().ok().flatten().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "meclaw did not finish: {cmd:?}");
        std::thread::sleep(Duration::from_millis(25));
    }
    let out = owned.0.take().unwrap().wait_with_output().unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

async fn run_async(cmd: Command, stdin: Option<String>) -> (bool, String) {
    tokio::task::spawn_blocking(move || run(cmd, stdin))
        .await
        .expect("the runner thread")
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + MARKER;
    while !f() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} never happened"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn post_turn(api: SocketAddr, target: &str, body: Value) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + MARKER;
    loop {
        let sent = client
            .post(format!("http://{api}/messages"))
            .json(&json!({"target": target, "body": body, "ttl": 400}))
            .send()
            .await;
        match sent {
            Ok(r) if r.status() == reqwest::StatusCode::ACCEPTED => return,
            other => assert!(
                tokio::time::Instant::now() < deadline,
                "POST /messages to {target} never accepted: {other:?}"
            ),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn message_log(root: &Path) -> Vec<String> {
    let Ok(conn) = rusqlite::Connection::open(root.join("colony.db")) else {
        return Vec::new();
    };
    let mut st = conn
        .prepare("SELECT headers, COALESCE(body_payload, '') FROM message_log")
        .expect("message_log");
    st.query_map([], |r| {
        Ok(format!(
            "{} {}",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?
        ))
    })
    .expect("query")
    .filter_map(Result::ok)
    .collect()
}

// ═══════════════════════════════════════════════════════════════════ the lock

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1061_a_colony_answers_with_no_provider_key_in_its_environment() {
    if !shipped() {
        eprintln!("skipped: the shipped templates or python3 are absent (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    let out = tempfile::TempDir::new().unwrap();
    let stubs = Stubs::start();
    let env = Env::new();
    build_root(root, &stubs);
    let mut transcript = String::new();

    // 1. One manifest, one-shot: the broker grows, the lanes and grants follow.
    let m = manifest(root);
    let mut apply = env.command(root);
    apply.arg("--apply").arg(&m);
    let (ok, text) = run_async(apply, None).await;
    assert!(ok, "the manifest applies: {text}");
    transcript.push_str(&text);

    // 2. The secrets, through the user channel only — no colony holds the root.
    for c in &CONSUMERS {
        let mut add = env.command(root);
        add.args(["--vault", VAULT, "--vault-add", c.cred_ref])
            .args(["--vault-key-source", "plainfile", "--vault-key-file"])
            .arg(env.key_file());
        let (ok, text) = run_async(add, Some(c.secret.to_string())).await;
        assert!(ok, "--vault-add {} stores: {text}", c.cred_ref);
        transcript.push_str(&text);
    }

    // 3. The daemon, with nothing of the secret class in its environment.
    let port = free_port();
    let api: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let stdout = std::fs::File::create(out.path().join("daemon.out")).unwrap();
    let stderr = std::fs::File::create(out.path().join("daemon.err")).unwrap();
    let mut daemon = env.command(root);
    daemon
        .arg("--api")
        .arg(api.to_string())
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    let mut owned = Owned(Some(daemon.spawn().expect("spawn the daemon")));

    {
        // Ready when the one listener accepts.
        until("the API listener accepts", || {
            std::net::TcpStream::connect(api).is_ok()
        })
        .await;

        post_turn(
            api,
            "/brain",
            json!({"messages": [{"origin": "user", "type": "text", "text": "ping"}]}),
        )
        .await;
        post_turn(
            api,
            "/search",
            json!({"messages": [{"origin": "assistant", "type": "tool_call",
                                 "text": json!({"query": "q"}).to_string(), "id": "call-1"}]}),
        )
        .await;
        post_turn(
            api,
            "/embed",
            json!({"messages": [{"origin": "user", "type": "text", "text": "go"}]}),
        )
        .await;

        until("the llm stub sees a bearer", || {
            !stubs.llm.header_values("authorization").is_empty()
        })
        .await;
        until("the search stub sees a bearer", || {
            !stubs.search.header_values("authorization").is_empty()
        })
        .await;
        until("the embed stub sees a bearer", || {
            !stubs.embed.header_values("authorization").is_empty()
        })
        .await;
        // The Telegram connector asks at its start and polls once the box opens.
        until("the Telegram stub sees a poll", || {
            stubs
                .telegram
                .seen()
                .iter()
                .any(|s| s.path.contains("/getUpdates"))
        })
        .await;

        // The voice slot connects only inside a session: open one until the
        // Deepgram stub has seen the upgrade (OR-VG.V4.D1).
        let url = format!("ws://{api}/t4voice/ws?mode=auto");
        let deadline = tokio::time::Instant::now() + MARKER;
        while stubs.deepgram.seen().is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the voice slot never connected to its provider"
            );
            let client = tokio::time::timeout(
                Duration::from_secs(5),
                meclaw_testing::voice_client::VoiceClient::connect(&url),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(1_500)).await;
            drop(client);
        }
    }

    // The deposited values, at the receivers.
    assert_eq!(
        stubs.llm.header_values("authorization"),
        ["Bearer stub-secret-1"],
        "llm"
    );
    assert!(
        stubs
            .search
            .header_values("authorization")
            .iter()
            .all(|a| a == "Bearer stub-secret-2"),
        "web_search: {:?}",
        stubs.search.seen()
    );
    assert_eq!(
        stubs.embed.header_values("authorization"),
        ["Bearer stub-secret-3"],
        "code"
    );
    let polls: Vec<String> = stubs.telegram.seen().into_iter().map(|s| s.path).collect();
    assert!(
        polls.iter().all(|p| p.contains("/botstub-secret-4/")),
        "every Telegram call carries the deposited token: {polls:?}"
    );
    let upgrades = stubs.deepgram.header_values("authorization");
    assert!(
        !upgrades.is_empty() && upgrades.iter().all(|a| a.contains("stub-secret-5")),
        "the STT slot connects with the deposited key: {:?}",
        stubs.deepgram.seen()
    );

    // Orderly stop (SIGTERM by pid), then read every log this test can read.
    let pid = owned.0.as_ref().unwrap().id();
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
    let deadline = Instant::now() + MARKER;
    while owned
        .0
        .as_mut()
        .unwrap()
        .try_wait()
        .ok()
        .flatten()
        .is_none()
        && Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drop(owned);
    for f in ["daemon.out", "daemon.err"] {
        transcript.push_str(&std::fs::read_to_string(out.path().join(f)).unwrap_or_default());
    }
    transcript.push_str(&std::fs::read_to_string(root.join("log.jsonl")).unwrap_or_default());
    let rows = message_log(root);
    assert!(!rows.is_empty(), "the colony logged its traffic");
    for c in &CONSUMERS {
        assert!(
            !transcript.contains(c.secret),
            "{} is in a process log",
            c.cred_ref
        );
        let hits: Vec<&String> = rows.iter().filter(|r| r.contains(c.secret)).collect();
        assert!(hits.is_empty(), "{} is on record: {hits:?}", c.cred_ref);
    }
    assert!(
        !transcript.contains(PASSPHRASE),
        "the passphrase is in a log"
    );
}
