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
//! or the colony's message log (`GET /colony/messages`, read by a second boot
//! after the orderly stop, so the log is whole).
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! shipped templates, or a host without `python3` (the broker's cells are
//! Python), is skipped, never judged.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meclaw_testing::{deadline, wait};
use serde_json::{Value, json};

const PASSPHRASE: &str = "stub-secret-passphrase-1061";
/// GH #1209: the credential round of the three one-turn consumers (`llm`,
/// `web_search`, `code`), far beyond the hang mark (VORGABEN M4, TV-Z9). Each
/// of them gets ONE turn; a round that outlasts `credential_wait_ms` answers
/// that turn `credential_pending`, and nothing asks again. The 10 s product
/// default is an operation timeout, not the subject of this lock, so it must
/// not judge the colony's speed: sitting the round out is a hang (30 s mark).
/// The reds under load had another cause (GH #1216): the vault derived its key
/// on a runtime worker and the colony watchdog ended the process at boot.
/// Scaled like the mark itself, so no `MECLAW_TEST_TIME_FACTOR` brings the
/// round under the hang mark again.
/// One source since OR-T12-13 (tv-br M10m): `deadline::product_wait_ms`, for
/// every granted cell of this colony (brain, embed, Telegram, voice).
fn round_budget_ms() -> u64 {
    deadline::product_wait_ms()
}
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
    // Re-check fix3 H2: the hang mark, not a literal of its own.
    let _ = conn.set_read_timeout(Some(deadline::scaled(deadline::HANG)));
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
            "cell": {"type": "llm", "message_timeout": meclaw_testing::deadline::product_backstop_ms()},
            "params": {
                "provider": "openai", "model": "gpt-4o-mini", "api_key": "",
                "base_url": format!("http://{}/v1", stubs.llm.addr),
                "credential_grant_id": grant("brain"),
                "credential_wait_ms": round_budget_ms(),
                "external_timeout_ms": meclaw_testing::deadline::product_wait_ms()
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
                   "api_key": "", "credential_grant_id": grant("search"),
                   "credential_wait_ms": round_budget_ms()}),
        ),
    );

    write_json(
        &main.join("embed/config.json"),
        &json!({
            "cell": {"type": "code", "message_timeout": meclaw_testing::deadline::product_backstop_ms()},
            "params": {
                "runner": "python3",
                "script_inline": embed_script(&format!("http://{}/embed", stubs.embed.addr)),
                "credential_grant_id": grant("embed"),
                "credential_wait_ms": round_budget_ms(),
                "credential_env": "T4_EMBED_KEY",
                "external_timeout_ms": meclaw_testing::deadline::product_wait_ms()
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
                   "credential_wait_ms": round_budget_ms(), "emit_to": "/brain",
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
                           "credential_wait_ms": round_budget_ms()},
                   // A voice cell must be able to speak unless it echoes
                   // (`tts: required unless stt.provider is "echo"`). The tts
                   // slot shares the stt grant: one box opens both, and the
                   // handler asks once per round (review V2 M2).
                   "tts": {"provider": "openai",
                           "base_url": format!("http://{}", stubs.deepgram.addr),
                           "credential_grant_id": grant("voice"),
                           "credential_wait_ms": round_budget_ms()},
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

/// Run to completion; return (success, stdout + stderr). The exit is awaited
/// under the boot mark, no sleep loop (re-check fix3 M3, TV-S2/TV-S3); a hang
/// names the process state.
///
/// The boot mark, not the ordinary one (GH #1216, tv-br M10k): every call is a
/// whole run of the binary -- a boot that opens the root, applies or seals,
/// and exits. Under an 80 % CPU quota with 44 tests beside it (build04) the
/// `--apply` run took 37.5 s and was off the CPU for 20.2 s of it: a run that
/// finished, cut by the 30-s mark of an ordinary wait.
async fn run_async(mut cmd: Command, stdin: Option<String>) -> (bool, String) {
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
    let pid = child.id();
    let mut owned = Owned(Some(child));
    let running = owned.0.as_mut().unwrap();
    wait::boot_until_with(
        &format!("meclaw finishes: {cmd:?}"),
        || std::future::ready(running.try_wait().ok().flatten().is_some()),
        || proc_state(pid),
    )
    .await;
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

/// A process's state from `/proc` (the child is reaped only at drop, so an
/// exited one shows as a zombie).
fn proc_state(pid: u32) -> String {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let st = stat
        .rsplit_once(") ")
        .and_then(|(_, rest)| rest.split(' ').next())
        .unwrap_or("gone");
    let alive = if st == "Z" || st == "gone" {
        "EXITED"
    } else {
        "alive"
    };
    format!("pid {pid} state {st} ({alive})")
}

/// Waits for an event of the running daemon under the HANG mark (TV-Z3, TV-S3);
/// a hang names the daemon's last observed state (GH #1209, #1216): whether the
/// process still lives, and its lines about credential rounds, the watchdog and
/// every WARN/ERROR, so a red run says which side went quiet. One mark only
/// (`wait::until_with`): an inner mark firing first would drop the state.
async fn until(what: &str, d: &Daemon<'_>, mut f: impl FnMut() -> bool) {
    wait::until_with(
        what,
        || {
            d.alive(what);
            std::future::ready(f())
        },
        || d.state(),
    )
    .await;
}

/// [`until`] for the daemon's boot: under the BOOT_HANG mark (TV-Z3).
async fn boot_until(what: &str, d: &Daemon<'_>, mut f: impl FnMut() -> bool) {
    wait::boot_until_with(
        what,
        || {
            d.alive(what);
            std::future::ready(f())
        },
        || d.state(),
    )
    .await;
}

/// The running daemon as a hang diagnosis sees it: the process, its pid, its
/// output dir and the stem of its `<stem>.out`/`<stem>.err` there.
struct Daemon<'a> {
    owned: RefCell<Owned>,
    pid: u32,
    out: &'a Path,
    stem: &'static str,
}

impl Daemon<'_> {
    /// The daemon's exit status, once it has exited (`try_wait` reaps it).
    fn exit(&self) -> Option<std::process::ExitStatus> {
        self.owned
            .borrow_mut()
            .0
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten())
    }

    /// Fail at once when the daemon has exited (tv-br M10g, TV-Z3): an
    /// exited daemon answers nothing, and waiting out a hang mark on it hid
    /// WHY. An exit is an event, not a time verdict.
    fn alive(&self, what: &str) {
        if self.exit().is_some() {
            panic!(
                "the daemon exited while waiting for: {what}; last state: {:#?}",
                self.state()
            );
        }
    }

    /// Running or its exit status (`try_wait`; while it runs, the process
    /// state from `/proc`), then the last 40 telling lines, then the last 15
    /// lines of its stderr as they are (tv-br M10g: an exit's own reason
    /// need not match a filter).
    fn state(&self) -> Vec<String> {
        let head = match self.exit() {
            Some(status) => format!("daemon pid {} EXITED: {status}", self.pid),
            None => format!("daemon {}", proc_state(self.pid)),
        };
        let mut lines = vec![head];
        let mut tail = Vec::new();
        for ext in ["err", "out"] {
            let f = format!("{}.{ext}", self.stem);
            let text = std::fs::read_to_string(self.out.join(f)).unwrap_or_default();
            tail.extend(
                text.lines()
                    .filter(|l| {
                        [
                            "credential",
                            "vault",
                            "grant",
                            "sealed",
                            "denied",
                            "attest",
                            "WARN",
                            "ERROR",
                            "watchdog",
                            "panic",
                            "brain",
                        ]
                        .iter()
                        .any(|k| l.contains(k))
                    })
                    .map(|l| l.chars().take(400).collect::<String>()),
            );
        }
        lines.extend(tail.split_off(tail.len().saturating_sub(40)));
        let err = std::fs::read_to_string(self.out.join(format!("{}.err", self.stem)))
            .unwrap_or_default();
        let err: Vec<&str> = err.lines().collect();
        lines.push("-- stderr tail --".into());
        lines.extend(
            err[err.len().saturating_sub(15)..]
                .iter()
                .map(|l| l.chars().take(400).collect::<String>()),
        );
        lines
    }
}

/// One turn to `target`, retried until the API accepts it, under the hang
/// mark; a hang names the last refusal (re-check fix3 M3, TV-Z3) and the
/// daemon's state, an exited daemon ends the wait at once (tv-br M10g).
async fn post_turn(api: SocketAddr, target: &str, body: Value, d: &Daemon<'_>) {
    let client = reqwest::Client::new();
    let turn = json!({"target": target, "body": body, "ttl": 400});
    let last = RefCell::new(String::from("no attempt yet"));
    let (client, turn, last_seen) = (&client, &turn, &last);
    let what = format!("POST /messages to {target} is accepted");
    wait::until_with(
        &what,
        || {
            d.alive(&what);
            async move {
                match client
                    .post(format!("http://{api}/messages"))
                    .json(turn)
                    .send()
                    .await
                {
                    Ok(r) if r.status() == reqwest::StatusCode::ACCEPTED => true,
                    other => {
                        *last_seen.borrow_mut() = format!("{other:?}");
                        false
                    }
                }
            }
        },
        || {
            let mut state = vec![format!("last refusal: {}", last.borrow())];
            state.extend(d.state());
            state
        },
    )
    .await;
}

/// Rows per page of the message-log reads (re-check legacy-b fix4 M1, TV-A3):
/// `GET /colony/messages` resolves at most `RESOLVE_BLOB_MAX` = 25 blob bodies
/// per page (`meclaw-api` `handlers/message_log.rs`) and keeps only the blob id
/// of every further one, so a page of 1000 rows left an absence check blind
/// from its 26th blob on. 25 rows hold at most 25 blobs; [`assert_whole_page`]
/// asserts it anyway, should the cap ever change.
const LOG_PAGE: usize = 25;

/// The server's ceiling on `?scan_budget` (`MAX_SCAN_BUDGET`): with it a page
/// reports `scan_truncated` only past 50 000 rows of its window.
const LOG_SCAN_BUDGET: usize = 50_000;

/// A page of `GET /colony/messages` is whole, or the test fails here
/// (re-check legacy-b fix4 M1, tv-br M6 M1, TV-A3 fail-closed): the page says
/// whether it is whole (both flags are booleans -- a missing flag is no
/// `false`), the server resolved every blob body it was asked for
/// (`blob_resolution_truncated == false`), the scan saw its whole window
/// (`scan_truncated == false`), and no blob row of the page lacks its body: a
/// read the server could not do (`blob_error`, `handlers/message_log.rs`
/// `attach_blob_body`) leaves the flags `false` and the row without a body.
fn assert_whole_page(page: &Value) {
    let flags = format!(
        "blob_resolution_truncated={} scan_truncated={} scan_budget={} rows={}",
        page["blob_resolution_truncated"],
        page["scan_truncated"],
        page["scan_budget"],
        page["messages"].as_array().map_or(0, Vec::len)
    );
    assert!(
        page["blob_resolution_truncated"].is_boolean() && page["scan_truncated"].is_boolean(),
        "a message-log page does not say whether it is whole (no truncation flags), so it is \
         no record of the log: {flags}"
    );
    let unread: Vec<String> = page["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| {
            r["body_kind"] == "blob" && (r["blob_body"].is_null() || r.get("blob_error").is_some())
        })
        .map(|r| format!("{} blob_error={}", r["body_payload"], r["blob_error"]))
        .collect();
    assert!(
        page["blob_resolution_truncated"] == Value::Bool(false),
        "a message-log page left blob bodies unresolved (the server resolves 25 per page), \
         so the rows read are not the bodies logged: {flags}"
    );
    assert!(
        page["scan_truncated"] == Value::Bool(false),
        "a message-log page scanned only part of its window, so the rows read are not the \
         whole log: {flags}"
    );
    assert!(
        unread.is_empty(),
        "a message-log page holds blob rows whose body was not read, so the rows read are not \
         the bodies logged: {unread:?} ({flags})"
    );
}

/// The cursor of the page after `page`, or `None` at the log's end (re-check
/// tv-br M6 M1, TV-A3 fail-closed): the server names `next` only on a full page
/// (`colony_dispatch.rs` `handle_read_messages`), so `next == null` is the
/// end. A page without a `next` field, or one that names a next page without
/// a usable cursor (or on no rows), says nothing about the end and fails here
/// -- it never ends the walk as if the log were read.
fn next_cursor(page: &Value) -> Option<String> {
    let next = page
        .get("next")
        .unwrap_or_else(|| panic!("a message-log page does not say where the log goes on: {page}"));
    if next.is_null() {
        return None;
    }
    let rows = page["messages"].as_array().map_or(0, Vec::len);
    match (next["created_at"].as_i64(), next["id"].as_str()) {
        (Some(at), Some(id)) if rows > 0 => Some(format!("&before_created_at={at}&before_id={id}")),
        _ => panic!(
            "a message-log page names a next page without a usable cursor ({rows} rows, \
             next={next}), so its end is not the log's end"
        ),
    }
}

/// The colony's own lines for a message-log read it failed: on an SQL or
/// `spawn_blocking` error `handle_read_messages` logs one of them and answers
/// 200 with an empty page, no `next` and `scan_truncated == false`
/// (`colony_dispatch.rs`), which no page flag tells from the log's end.
const LOG_READ_ERRORS: [&str; 2] = [
    "ReadMessages SQL failed",
    "ReadMessages spawn_blocking failed",
];

/// Re-check tv-br M8 A-M2, orchestrator ruling OR-T10-7: the product source
/// [`LOG_READ_ERRORS`] is bound to, read at compile time -- a renamed line in
/// the colony turns this file red instead of letting a failed read pass as the
/// log's end.
const COLONY_DISPATCH_SRC: &str = include_str!("../../meclaw-colony/src/colony_dispatch.rs");

/// The `tracing::error!` messages of `handle_read_messages` in `src` and the
/// number of its empty fallbacks `(Vec::new(), 0)`.
fn read_error_lines_of(src: &str) -> (Vec<String>, usize) {
    let body = src
        .split_once("pub async fn handle_read_messages(")
        .map(|(_, rest)| rest.split_once("\n}\n").map_or(rest, |(b, _)| b))
        .expect("colony_dispatch.rs defines `pub async fn handle_read_messages(`");
    let messages = body
        .split("tracing::error!(")
        .skip(1)
        .map(|call| {
            let call = call.split_once(");").map_or(call, |(c, _)| c);
            let end = call.rfind('"').expect("a tracing::error! with a message");
            let start = call[..end].rfind('"').expect("a quoted message") + 1;
            call[start..end].to_string()
        })
        .collect();
    (messages, body.matches("(Vec::new(), 0)").count())
}

/// [`LOG_READ_ERRORS`] are exactly the error lines of `handle_read_messages`
/// in `src`, one per empty fallback: a renamed line, a new line or a new
/// silent fallback is a drift of the oracle (OR-T10-7).
fn assert_log_read_errors_bound(src: &str) {
    let (messages, fallbacks) = read_error_lines_of(src);
    assert_eq!(
        messages, LOG_READ_ERRORS,
        "the oracle's log-read errors are not the colony's own lines \
         (colony_dispatch.rs handle_read_messages)"
    );
    assert_eq!(
        fallbacks,
        LOG_READ_ERRORS.len(),
        "handle_read_messages answers an empty page on a path the oracle has no line for"
    );
}

/// Re-check tv-br M6 M1 (b), orchestrator ruling OR-T10-3: a failed read is
/// closed on the test side, the HTTP contract stays -- the stderr of the boot
/// that served the log reads holds none of [`LOG_READ_ERRORS`], or the rows
/// read are not the whole log.
fn assert_no_log_read_error(stderr: &str) {
    assert_log_read_errors_bound(COLONY_DISPATCH_SRC);
    let failed: Vec<&str> = stderr
        .lines()
        .filter(|l| LOG_READ_ERRORS.iter().any(|e| l.contains(e)))
        .collect();
    assert!(
        failed.is_empty(),
        "the colony failed a message-log read and served it as an empty page (200, no next), \
         so the rows read are not the whole log: {failed:?}"
    );
}

/// The message a check panics with, or `None` when it passes (TV-Q4: a pin
/// names the reason it expects, not only that something panicked).
fn refusal(check: impl FnOnce() + std::panic::UnwindSafe) -> Option<String> {
    std::panic::catch_unwind(check).err().map(|p| {
        p.downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_else(|| "a panic without a message".into())
    })
}

/// `refused` is a refusal whose message holds `reason` (TV-Q4).
fn expect_refusal(refused: Option<String>, reason: &str, case: &str) {
    let msg = refused.unwrap_or_else(|| panic!("{case}: the oracle took it as whole"));
    assert!(
        msg.contains(reason),
        "{case}: refused, but not for `{reason}`: {msg}"
    );
}

/// Every row of the colony's message log as the colony serves it, newest
/// first (TV-A3, re-check fix3 M3): `GET /colony/messages`, the HTTP twin of
/// `ReadMessages`, paged by its keyset cursor, every page whole
/// ([`assert_whole_page`]: all blob bodies resolved, the whole window scanned). No
/// database file is opened. A failed request is the caller's to report with
/// the daemon's state (tv-br M10g).
async fn message_log(api: SocketAddr) -> Result<Vec<String>, String> {
    let client = reqwest::Client::new();
    let mut rows = Vec::new();
    let mut cursor = String::new();
    loop {
        let page: Value = client
            .get(format!(
                "http://{api}/colony/messages?limit={LOG_PAGE}&scan_budget={LOG_SCAN_BUDGET}\
                 &resolve_blob=true{cursor}"
            ))
            .send()
            .await
            .map_err(|e| format!("GET /colony/messages: {e:?}"))?
            .json()
            .await
            .map_err(|e| format!("GET /colony/messages: no message page: {e:?}"))?;
        assert_whole_page(&page);
        let messages = page["messages"].as_array().cloned().unwrap_or_default();
        rows.extend(messages.iter().map(Value::to_string));
        match next_cursor(&page) {
            Some(next) => cursor = next,
            None => return Ok(rows),
        }
    }
}

/// Stop the daemon by its pid (SIGTERM, never by name) and await the exit
/// under the hang mark; a hang names its state (re-check fix3 H1).
async fn stop_by_pid(d: &Daemon<'_>) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(d.pid.to_string())
        .status();
    wait::until_with(
        "the daemon stops on SIGTERM",
        || std::future::ready(d.exit().is_some()),
        || d.state(),
    )
    .await;
    d.owned.borrow_mut().0.take();
}

/// The daemon of `root` on `api`, its output in `<out>/<stem>.out|err`.
fn spawn_daemon<'a>(
    env: &Env,
    root: &Path,
    api: SocketAddr,
    out: &'a Path,
    stem: &'static str,
) -> Daemon<'a> {
    let stdout = std::fs::File::create(out.join(format!("{stem}.out"))).unwrap();
    let stderr = std::fs::File::create(out.join(format!("{stem}.err"))).unwrap();
    let mut daemon = env.command(root);
    daemon
        .arg("--api")
        .arg(api.to_string())
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    let child = daemon.spawn().expect("spawn the daemon");
    Daemon {
        pid: child.id(),
        owned: RefCell::new(Owned(Some(child))),
        out,
        stem,
    }
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
    // GH #1220: a port from the test band, not a dropped `bind(…:0)` -- while the
    // daemon (re)starts, the kernel gave such a port to another test's mock,
    // and this test's /health and /messages requests landed there.
    let port = meclaw_testing::ports::free_port();
    let api: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let d = spawn_daemon(&env, root, api, out.path(), "daemon");

    {
        // Ready when the one listener accepts.
        boot_until("the API listener accepts", &d, || {
            std::net::TcpStream::connect(api).is_ok()
        })
        .await;

        post_turn(
            api,
            "/brain",
            json!({"messages": [{"origin": "user", "type": "text", "text": "ping"}]}),
            &d,
        )
        .await;
        post_turn(
            api,
            "/search",
            json!({"messages": [{"origin": "assistant", "type": "tool_call",
                                 "text": json!({"query": "q"}).to_string(), "id": "call-1"}]}),
            &d,
        )
        .await;
        post_turn(
            api,
            "/embed",
            json!({"messages": [{"origin": "user", "type": "text", "text": "go"}]}),
            &d,
        )
        .await;

        until("the llm stub sees a bearer", &d, || {
            !stubs.llm.header_values("authorization").is_empty()
        })
        .await;
        until("the search stub sees a bearer", &d, || {
            !stubs.search.header_values("authorization").is_empty()
        })
        .await;
        until("the embed stub sees a bearer", &d, || {
            !stubs.embed.header_values("authorization").is_empty()
        })
        .await;
        // The Telegram connector asks at its start and polls once the box opens.
        until("the Telegram stub sees a poll", &d, || {
            stubs
                .telegram
                .seen()
                .iter()
                .any(|s| s.path.contains("/getUpdates"))
        })
        .await;

        // The voice slot connects only inside a session (OR-VG.V4.D1). A
        // session opened before the key waits for it at most the slot's
        // `credential_wait_ms` (`voice/vaulted.rs`) and is then refused --
        // measured under CPUQuota=80 % (TV-F7, 3 of 5 red with one held
        // session): the key came in the third round, after the refusal. So a
        // session is held until it says anything after its hello (the
        // refusal) or closes, and then the next one opens; the Deepgram
        // stub's upgrade is awaited under the hang mark (re-check fix3 M3: no
        // sleep, no deadline of its own; the last refusal is the state).
        let url = format!("ws://{api}/t4voice/ws?mode=auto");
        let session: RefCell<Option<meclaw_testing::voice_client::VoiceClient>> =
            RefCell::new(None);
        let refused = RefCell::new(String::from("no session opened yet"));
        let (url, session_held, refused_last, deepgram) =
            (&url, &session, &refused, &stubs.deepgram);
        let voice = "the voice slot connects to its provider inside a session";
        let d = &d;
        wait::until_with(
            voice,
            || {
                d.alive(voice);
                async move {
                    if !deepgram.seen().is_empty() {
                        return true;
                    }
                    let held = session_held.borrow_mut().take();
                    let held = match held {
                        // Only what is already there: a zero wait reads, never times.
                        Some(mut client) => match client.next_frame(Duration::ZERO).await {
                            Err(e) if e.starts_with("no frame") => Some(client),
                            Ok(frame) => {
                                *refused_last.borrow_mut() = format!("session said {frame:?}");
                                None
                            }
                            Err(e) => {
                                *refused_last.borrow_mut() = e;
                                None
                            }
                        },
                        None => match meclaw_testing::voice_client::VoiceClient::connect(url).await
                        {
                            Ok((client, _hello)) => Some(client),
                            Err(e) => {
                                *refused_last.borrow_mut() = e;
                                None
                            }
                        },
                    };
                    *session_held.borrow_mut() = held;
                    false
                }
            },
            || {
                let mut state = vec![format!(
                    "session held: {}; last refusal: {}",
                    session.borrow().is_some(),
                    refused.borrow()
                )];
                state.extend(d.state());
                state
            },
        )
        .await;
        drop(session);
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
    // TV-S2/TV-S3 (re-check fix2 H3): the exit is an awaited event under the
    // hang mark, no sleep loop that gives up silently.
    stop_by_pid(&d).await;
    // Re-check fix3 M3: the message log over `GET /colony/messages`, never
    // `colony.db` (TV-A3), and still AFTER the orderly stop, which empties
    // the log writer: a second boot of the same root serves the whole log.
    let r = spawn_daemon(&env, root, api, out.path(), "reader");
    wait::boot_until_with(
        "the second boot answers /health",
        || {
            r.alive("the second boot answers /health");
            async move {
                reqwest::get(format!("http://{api}/health"))
                    .await
                    .is_ok_and(|r| r.status().is_success())
            }
        },
        || r.state(),
    )
    .await;
    let rows = deadline::hang_with(
        "GET /colony/messages serves the whole log",
        message_log(api),
        || r.state(),
    )
    .await
    .unwrap_or_else(|e| panic!("{e}; last state: {:#?}", r.state()));
    stop_by_pid(&r).await;
    assert_no_log_read_error(
        &std::fs::read_to_string(out.path().join("reader.err")).expect("the reading boot's stderr"),
    );
    // GH #1216: whether the one-shot `--apply` left work open, which this
    // daemon replays in front of the turn -- its own WARN line, on the record
    // of every run.
    let replay: Vec<String> = std::fs::read_to_string(out.path().join("daemon.err"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("replaying deliveries"))
        .map(|l| l.chars().take(240).collect())
        .collect();
    eprintln!("gh1061 a_colony: replay after the one-shot --apply: {replay:?}");
    for f in ["daemon.out", "daemon.err", "reader.out", "reader.err"] {
        transcript.push_str(&std::fs::read_to_string(out.path().join(f)).unwrap_or_default());
    }
    transcript.push_str(&std::fs::read_to_string(root.join("log.jsonl")).unwrap_or_default());
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

/// A message-log page of `blobs` blob rows as `GET /colony/messages` serves it
/// (`handlers/message_log.rs`): the first 25 with their body resolved, every
/// further one with its blob id only, and the page's flag says so.
fn page_of_blobs(blobs: usize) -> Value {
    let messages: Vec<Value> = (0..blobs)
        .map(|i| {
            let mut row = json!({"body_kind": "blob", "body_payload": format!("blob-{i}")});
            if i < 25 {
                row["blob_body"] = json!({"messages": [{"text": format!("body {i}")}]});
            }
            row
        })
        .collect();
    json!({"messages": messages, "next": null, "scan_budget": LOG_SCAN_BUDGET,
           "scan_truncated": false, "blob_resolution_truncated": blobs > 25})
}

/// Re-check legacy-b fix4 M1, tv-br M6 M2 (TV-Q4): the page check of the
/// message-log oracle holds for a page whose every blob body is resolved and
/// refuses, each for its own reason, a page from its 26th blob on, a
/// truncated scan and a page without its flags.
#[test]
fn gh1061_a_colony_the_log_oracle_refuses_a_page_past_its_25th_blob() {
    let refused = |page: Value| refusal(move || assert_whole_page(&page));
    assert_eq!(
        refused(page_of_blobs(25)),
        None,
        "25 resolved blob bodies are a whole page"
    );
    expect_refusal(
        refused(page_of_blobs(26)),
        "left blob bodies unresolved",
        "the 26th blob of a page is unresolved",
    );
    let mut scanned = page_of_blobs(3);
    scanned["scan_truncated"] = json!(true);
    expect_refusal(
        refused(scanned),
        "scanned only part of its window",
        "a truncated scan",
    );
    expect_refusal(
        refused(json!({"messages": []})),
        "does not say whether it is whole",
        "a page without its flags",
    );
}

/// Re-check tv-br M6 M1 (a): a blob row whose body the server could not read
/// (`blob_error`: `missing_blob_id`, `malformed_blob_id`, `blob_unreadable`,
/// `handlers/message_log.rs` `attach_blob_body`) or did not resolve carries
/// no body, and the page flags stay `false` -- the oracle refuses the page.
#[test]
fn gh1061_a_colony_the_log_oracle_refuses_a_blob_row_without_its_body() {
    for error in ["missing_blob_id", "malformed_blob_id", "blob_unreadable"] {
        let mut page = page_of_blobs(3);
        page["messages"][1]
            .as_object_mut()
            .expect("a row")
            .remove("blob_body");
        page["messages"][1]["blob_error"] = json!(error);
        expect_refusal(
            refusal(move || assert_whole_page(&page)),
            "whose body was not read",
            error,
        );
    }
    let mut unresolved = page_of_blobs(3);
    unresolved["messages"][2]
        .as_object_mut()
        .expect("a row")
        .remove("blob_body");
    expect_refusal(
        refusal(move || assert_whole_page(&unresolved)),
        "whose body was not read",
        "a blob row without body and without error",
    );
}

/// Re-check tv-br M6 M1: the walk over the log ends only at `next == null`;
/// a page that names a next page it gives no cursor for, or a page without a
/// `next` field, is no end of the log.
#[test]
fn gh1061_a_colony_the_log_oracle_ends_only_at_a_null_next() {
    let mut full = page_of_blobs(3);
    full["next"] = json!({"created_at": 7, "id": "row-3"});
    assert_eq!(
        next_cursor(&full).as_deref(),
        Some("&before_created_at=7&before_id=row-3"),
        "a full page names its successor"
    );
    assert_eq!(
        next_cursor(&page_of_blobs(3)),
        None,
        "next == null is the end"
    );
    let mut broken = page_of_blobs(3);
    broken["next"] = json!({"created_at": 7});
    expect_refusal(
        refusal(move || drop(next_cursor(&broken))),
        "without a usable cursor",
        "a next page without an id",
    );
    let mut empty = json!({"messages": [], "next": {"created_at": 7, "id": "row-3"}});
    empty["scan_truncated"] = json!(false);
    expect_refusal(
        refusal(move || drop(next_cursor(&empty))),
        "without a usable cursor",
        "a next page named on no rows",
    );
    let mut silent = page_of_blobs(3);
    silent.as_object_mut().expect("a page").remove("next");
    expect_refusal(
        refusal(move || drop(next_cursor(&silent))),
        "does not say where the log goes on",
        "a page without a next field",
    );
}

/// Re-check tv-br M8 A-M2, OR-T10-7: the oracle's log-read errors are the
/// colony's own lines -- the shipped source passes; a renamed line and a new
/// silent fallback in a copy of it are refused.
#[test]
fn gh1061_a_colony_the_log_read_errors_are_the_colony_s_own_lines() {
    assert_eq!(
        refusal(|| assert_log_read_errors_bound(COLONY_DISPATCH_SRC)),
        None,
        "the shipped colony_dispatch.rs is the oracle's source"
    );
    let renamed = COLONY_DISPATCH_SRC.replacen(
        "\"ReadMessages SQL failed\"",
        "\"ReadMessages query failed\"",
        1,
    );
    expect_refusal(
        refusal(move || assert_log_read_errors_bound(&renamed)),
        "not the colony's own lines",
        "a renamed error line",
    );
    let silent = COLONY_DISPATCH_SRC.replacen(
        "    let next = if entries.len() == limit {",
        "    let (entries, scanned) = if scanned > 0 { (entries, scanned) } else { (Vec::new(), 0) };\n    let next = if entries.len() == limit {",
        1,
    );
    assert_ne!(silent, COLONY_DISPATCH_SRC, "the mutation applies");
    expect_refusal(
        refusal(move || assert_log_read_errors_bound(&silent)),
        "a path the oracle has no line for",
        "a silent empty fallback",
    );
}

/// Re-check tv-br M6 M1 (b), OR-T10-3: a read the colony failed is a 200 with
/// an empty page and no `next` -- the oracle refuses it by the colony's own
/// error line on the reading boot's stderr; other lines pass.
#[test]
fn gh1061_a_colony_the_log_oracle_refuses_a_log_read_the_colony_failed() {
    assert_eq!(
        refusal(|| assert_no_log_read_error(
            "2026-10-12T10:00:00Z  WARN meclaw_colony: replaying deliveries n=1\n"
        )),
        None,
        "a stderr without a failed read passes"
    );
    for line in LOG_READ_ERRORS {
        let stderr = format!(
            "2026-10-12T10:00:00Z ERROR meclaw_colony::colony_dispatch: {line} error=SqliteFailure\n"
        );
        expect_refusal(
            refusal(move || assert_no_log_read_error(&stderr)),
            "failed a message-log read",
            line,
        );
    }
}
